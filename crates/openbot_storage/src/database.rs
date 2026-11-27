//! `OpenBotDatabase` — SQLite WAL core (`openbot.db`), reentrant `BEGIN IMMEDIATE` / `SAVEPOINT`
//! transactions, deterministic SQL scalar functions (`openbot_collapse_whitespace`,
//! `openbot_lowercase`), atomic idempotent CQRS `dispatch`, bounded `thread_event_compaction`,
//! and crash-safe `file_deletion_outbox`.
//!
//! Mirrors `src/backend/database/{database-core.ts,sqlite-transaction.ts,thread-event-compaction.ts,thread-message-fold.ts,mailbox-projection.ts}`.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use parking_lot::ReentrantMutex;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, params, types::Value as SqlValue};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use uuid::Uuid;

use crate::error::{StorageError, StorageResult};
use crate::schema_v69::{
    OpenBotMigrationOptions, assert_quick_check, migrate_openbot_database, read_applied_versions,
    read_foreign_key_violations, validate_applied_versions,
};

pub const COLLAPSE_WHITESPACE_FUNCTION: &str = "openbot_collapse_whitespace";
pub const LOWERCASE_FUNCTION: &str = "openbot_lowercase";

pub const THREAD_COMPACTION_BATCH_EVENTS: usize = 256;
pub const THREAD_COMPACTION_BATCH_BYTES: usize = 4 * 1024 * 1024;
pub const THREAD_COMPACTION_MAX_RESTARTS: usize = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationEventInput {
    pub event_id: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub event_type: String,
    pub occurred_at: String,
    pub payload: JsonValue,
}

impl OrchestrationEventInput {
    pub fn new(
        aggregate_type: impl Into<String>,
        aggregate_id: impl Into<String>,
        event_type: impl Into<String>,
        payload: JsonValue,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: aggregate_type.into(),
            aggregate_id: aggregate_id.into(),
            event_type: event_type.into(),
            occurred_at: now_rfc3339_millis(),
            payload,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredOrchestrationEvent {
    pub sequence: i64,
    pub event_id: String,
    pub command_id: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub event_type: String,
    pub occurred_at: String,
    pub payload_json: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchReceipt<R = JsonValue> {
    pub command_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub accepted_at: String,
    pub first_sequence: i64,
    pub last_sequence: i64,
    pub replayed: bool,
    pub result: R,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingFileDeletion {
    pub id: String,
    pub path: String,
    pub reason: String,
    pub created_at: String,
    pub attempts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDeletionDrainReport {
    pub deleted: usize,
    pub failed: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEventCompactionBatch {
    pub done: bool,
    pub events: usize,
    pub rewritten: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
    pub completed_thread: bool,
    pub failed_thread: Option<String>,
}

pub fn now_rfc3339_millis() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn scoped_command_id(domain: &str, operation_id: &str) -> String {
    format!("{domain}:{operation_id}")
}

/// Collapses contiguous Unicode whitespace runs (including `\u{FEFF}`) into a single ASCII `' '`.
pub fn collapse_unicode_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_whitespace = false;
    for ch in input.chars() {
        if ch.is_whitespace() || ch == '\u{FEFF}' {
            if !in_whitespace {
                out.push(' ');
                in_whitespace = true;
            }
        } else {
            in_whitespace = false;
            out.push(ch);
        }
    }
    out
}

struct DbConnHolder {
    conn: Connection,
    savepoint_depth: Cell<usize>,
}

struct OpenBotDatabaseInner {
    db_path: PathBuf,
    data_dir: PathBuf,
    holder: ReentrantMutex<DbConnHolder>,
}

#[derive(Clone)]
pub struct OpenBotDatabase {
    inner: Arc<OpenBotDatabaseInner>,
}

impl fmt::Debug for OpenBotDatabase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenBotDatabase")
            .field("db_path", &self.inner.db_path)
            .field("data_dir", &self.inner.data_dir)
            .finish()
    }
}

fn resolve_openbot_db_paths(path: &Path) -> (PathBuf, PathBuf) {
    if path.is_dir() {
        return (path.join("openbot.db"), path.to_path_buf());
    }
    if path.is_file() {
        let data_dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        return (path.to_path_buf(), data_dir);
    }
    let looks_like_db_file = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("db")
                || ext.eq_ignore_ascii_case("sqlite")
                || ext.eq_ignore_ascii_case("sqlite3")
        });
    if looks_like_db_file {
        let data_dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        (path.to_path_buf(), data_dir)
    } else {
        (path.join("openbot.db"), path.to_path_buf())
    }
}

fn ensure_dir_mode_700(dir: &Path) -> StorageResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let existed = dir.exists();
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(dir)?;
        if !existed {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn set_file_mode_600(path: &Path) -> StorageResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn touch_dirty_wal(wal_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(wal_path);
    }
    #[cfg(not(unix))]
    {
        let _ = OpenOptions::new().create(true).append(true).open(wal_path);
    }
}

pub fn register_openbot_scalar_functions(conn: &Connection) -> StorageResult<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function(COLLAPSE_WHITESPACE_FUNCTION, 1, flags, |ctx| {
        let value: SqlValue = ctx.get(0)?;
        Ok(match value {
            SqlValue::Text(text) => SqlValue::Text(collapse_unicode_whitespace(&text)),
            other => other,
        })
    })?;
    conn.create_scalar_function(LOWERCASE_FUNCTION, 1, flags, |ctx| {
        let value: SqlValue = ctx.get(0)?;
        Ok(match value {
            SqlValue::Text(text) => SqlValue::Text(text.to_lowercase()),
            other => other,
        })
    })?;
    Ok(())
}

impl OpenBotDatabase {
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let (db_path, data_dir) = resolve_openbot_db_paths(path.as_ref());
        Self::open_resolved(db_path, data_dir)
    }

    pub fn open_in_data_dir(data_dir: impl AsRef<Path>) -> StorageResult<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let db_path = data_dir.join("openbot.db");
        Self::open_resolved(db_path, data_dir)
    }

    fn open_resolved(db_path: PathBuf, data_dir: PathBuf) -> StorageResult<Self> {
        ensure_dir_mode_700(&data_dir)?;
        if let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
            && parent != data_dir
        {
            ensure_dir_mode_700(parent)?;
        }

        let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
        let shm_path = PathBuf::from(format!("{}-shm", db_path.display()));
        let journal_path = PathBuf::from(format!("{}-journal", db_path.display()));
        let closed_cleanly = !wal_path.exists();

        if db_path.exists() {
            let ro_result = (|| -> StorageResult<()> {
                let ro_conn = if !wal_path.exists()
                    && !shm_path.exists()
                    && !journal_path.exists()
                    && let Some(raw_path) = db_path.to_str()
                {
                    let encoded = raw_path
                        .replace('%', "%25")
                        .replace('?', "%3F")
                        .replace('#', "%23");
                    let uri = format!("file:{encoded}?mode=ro&immutable=1");
                    Connection::open_with_flags(
                        uri,
                        OpenFlags::SQLITE_OPEN_READ_ONLY
                            | OpenFlags::SQLITE_OPEN_NO_MUTEX
                            | OpenFlags::SQLITE_OPEN_URI,
                    )?
                } else {
                    Connection::open_with_flags(
                        &db_path,
                        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                    )?
                };
                ro_conn.busy_timeout(Duration::from_millis(5_000))?;
                let versions = read_applied_versions(&ro_conn)?;
                validate_applied_versions(&versions)?;
                Ok(())
            })();
            match ro_result {
                Ok(()) => {}
                Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(e, _)))
                    if e.code == ErrorCode::ReadOnly =>
                {
                    // Hot rollback journal recovery requires RW access; defer validation
                    // to the RW connection below before switching to WAL mode.
                }
                Err(err) => return Err(err),
            }
        }

        let conn = Connection::open(&db_path)?;
        let mut schema_validated = false;

        let init_result = (|| -> StorageResult<()> {
            conn.busy_timeout(Duration::from_millis(5_000))?;
            conn.execute_batch(
                "PRAGMA synchronous = NORMAL;\
                 PRAGMA foreign_keys = ON;\
                 PRAGMA busy_timeout = 5000;",
            )?;
            let applied_versions = read_applied_versions(&conn)?;
            validate_applied_versions(&applied_versions)?;
            schema_validated = true;
            conn.execute_batch("PRAGMA journal_mode = WAL;")?;
            register_openbot_scalar_functions(&conn)?;
            migrate_openbot_database(
                &conn,
                &OpenBotMigrationOptions {
                    applied_at: None,
                    closed_cleanly,
                    through_version: None,
                },
            )?;
            set_file_mode_600(&db_path)?;
            Ok(())
        })();

        if let Err(err) = init_result {
            drop(conn);
            if schema_validated {
                touch_dirty_wal(&wal_path);
            }
            return Err(err);
        }

        Ok(Self {
            inner: Arc::new(OpenBotDatabaseInner {
                db_path,
                data_dir,
                holder: ReentrantMutex::new(DbConnHolder {
                    conn,
                    savepoint_depth: Cell::new(0),
                }),
            }),
        })
    }

    pub fn open_in_memory() -> StorageResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.busy_timeout(Duration::from_millis(5_000))?;
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL;\
             PRAGMA foreign_keys = ON;\
             PRAGMA busy_timeout = 5000;",
        )?;
        register_openbot_scalar_functions(&conn)?;
        migrate_openbot_database(&conn, &OpenBotMigrationOptions::default())?;
        Ok(Self {
            inner: Arc::new(OpenBotDatabaseInner {
                db_path: PathBuf::from(":memory:"),
                data_dir: std::env::temp_dir(),
                holder: ReentrantMutex::new(DbConnHolder {
                    conn,
                    savepoint_depth: Cell::new(0),
                }),
            }),
        })
    }

    #[inline]
    pub fn db_path(&self) -> &Path {
        &self.inner.db_path
    }

    #[inline]
    pub fn data_dir(&self) -> &Path {
        &self.inner.data_dir
    }

    pub fn with_conn<T, F: FnOnce(&Connection) -> StorageResult<T>>(&self, f: F) -> StorageResult<T> {
        let guard = self.inner.holder.lock();
        f(&guard.conn)
    }
}

struct TxScopeGuard<'a> {
    holder: &'a DbConnHolder,
    savepoint_name: Option<String>,
    committed: bool,
}

impl Drop for TxScopeGuard<'_> {
    fn drop(&mut self) {
        let prev = self.holder.savepoint_depth.get();
        if prev > 0 {
            self.holder.savepoint_depth.set(prev - 1);
        }
        if !self.committed && !self.holder.conn.is_autocommit() {
            if let Some(sp) = &self.savepoint_name {
                let _ = self.holder.conn.execute_batch(&format!(
                    "ROLLBACK TO SAVEPOINT {sp}; RELEASE SAVEPOINT {sp};"
                ));
            } else {
                let _ = self.holder.conn.execute_batch("ROLLBACK;");
            }
        }
    }
}

fn read_receipt_row(
    conn: &Connection,
    command_id: &str,
) -> StorageResult<Option<(String, i64, i64, String)>> {
    conn.query_row(
        "SELECT accepted_at, first_sequence, last_sequence, result_json \
         FROM orchestration_command_receipts WHERE command_id = ?1",
        params![command_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .optional()
    .map_err(StorageError::from)
}

impl OpenBotDatabase {
    pub fn with_tx<T, F: FnOnce(&Connection) -> StorageResult<T>>(&self, f: F) -> StorageResult<T> {
        let guard = self.inner.holder.lock();
        let nested = !guard.conn.is_autocommit();
        let depth = guard.savepoint_depth.get();
        let savepoint_name = if nested {
            let sp = format!("openbot_sp_{depth}");
            guard.conn.execute_batch(&format!("SAVEPOINT {sp};"))?;
            Some(sp)
        } else {
            guard.conn.execute_batch("BEGIN IMMEDIATE;")?;
            None
        };
        guard.savepoint_depth.set(depth + 1);
        let mut scope = TxScopeGuard {
            holder: &guard,
            savepoint_name,
            committed: false,
        };
        let output = f(&guard.conn)?;
        if let Some(sp) = &scope.savepoint_name {
            guard
                .conn
                .execute_batch(&format!("RELEASE SAVEPOINT {sp};"))?;
        } else {
            guard.conn.execute_batch("COMMIT;")?;
        }
        scope.committed = true;
        Ok(output)
    }

    pub fn command_result<R: DeserializeOwned>(
        &self,
        command_id: &str,
    ) -> StorageResult<Option<(i64, R)>> {
        self.with_conn(|conn| {
            let Some((_, _, last_sequence, result_json)) = read_receipt_row(conn, command_id)?
            else {
                return Ok(None);
            };
            let parsed: R = serde_json::from_str(&result_json)?;
            Ok(Some((last_sequence, parsed)))
        })
    }

    pub fn dispatch_with_projection<R, F>(
        &self,
        command_id: &str,
        operation_id: Option<&str>,
        events: &[OrchestrationEventInput],
        project: F,
    ) -> StorageResult<DispatchReceipt<R>>
    where
        R: Serialize + DeserializeOwned,
        F: FnOnce(&Connection, &[i64]) -> StorageResult<R>,
    {
        if command_id.trim().is_empty() {
            return Err(StorageError::validation("command_id must not be empty"));
        }
        if let Some((accepted_at, first_sequence, last_sequence, result_json)) =
            self.with_conn(|conn| read_receipt_row(conn, command_id))?
        {
            let result: R = serde_json::from_str(&result_json)?;
            return Ok(DispatchReceipt {
                command_id: command_id.to_string(),
                operation_id: operation_id.map(str::to_owned),
                accepted_at,
                first_sequence,
                last_sequence,
                replayed: true,
                result,
            });
        }

        self.with_tx(|conn| {
            if let Some((accepted_at, first_sequence, last_sequence, result_json)) =
                read_receipt_row(conn, command_id)?
            {
                let result: R = serde_json::from_str(&result_json)?;
                return Ok(DispatchReceipt {
                    command_id: command_id.to_string(),
                    operation_id: operation_id.map(str::to_owned),
                    accepted_at,
                    first_sequence,
                    last_sequence,
                    replayed: true,
                    result,
                });
            }

            let mut sequences = Vec::with_capacity(events.len());
            {
                let mut append_stmt = conn.prepare(
                    "INSERT INTO orchestration_events (\
                       event_id, command_id, aggregate_type, aggregate_id, event_type, occurred_at, payload_json\
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )?;
                for event in events {
                    let event_id = if event.event_id.trim().is_empty() {
                        Uuid::new_v4().to_string()
                    } else {
                        event.event_id.clone()
                    };
                    let occurred_at = if event.occurred_at.trim().is_empty() {
                        now_rfc3339_millis()
                    } else {
                        event.occurred_at.clone()
                    };
                    let payload_json = serde_json::to_string(&event.payload)?;
                    append_stmt.execute(params![
                        event_id,
                        command_id,
                        event.aggregate_type,
                        event.aggregate_id,
                        event.event_type,
                        occurred_at,
                        payload_json,
                    ])?;
                    sequences.push(conn.last_insert_rowid());
                }
            }

            let result = project(conn, &sequences)?;
            let accepted_at = now_rfc3339_millis();
            let first_sequence = sequences.first().copied().unwrap_or(0);
            let last_sequence = sequences.last().copied().unwrap_or(0);
            let keeps_event: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM orchestration_events WHERE command_id = ?1 LIMIT 1",
                    params![command_id],
                    |row| row.get(0),
                )
                .optional()?;
            if keeps_event.is_some() {
                let result_json = serde_json::to_string(&result)?;
                conn.execute(
                    "INSERT INTO orchestration_command_receipts \
                       (command_id, accepted_at, first_sequence, last_sequence, result_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        command_id,
                        accepted_at,
                        first_sequence,
                        last_sequence,
                        result_json
                    ],
                )?;
            }

            Ok(DispatchReceipt {
                command_id: command_id.to_string(),
                operation_id: operation_id.map(str::to_owned),
                accepted_at,
                first_sequence,
                last_sequence,
                replayed: false,
                result,
            })
        })
    }

    pub fn dispatch(
        &self,
        command_id: &str,
        operation_id: Option<&str>,
        events: &[OrchestrationEventInput],
    ) -> StorageResult<DispatchReceipt<JsonValue>> {
        self.dispatch_with_projection(command_id, operation_id, events, |_, _| Ok(JsonValue::Null))
    }
}

impl OpenBotDatabase {
    pub fn delete_events_and_receipt(&self, command_id: &str) -> StorageResult<()> {
        self.with_tx(|conn| {
            conn.execute(
                "DELETE FROM orchestration_events WHERE command_id = ?1",
                params![command_id],
            )?;
            conn.execute(
                "DELETE FROM orchestration_command_receipts WHERE command_id = ?1",
                params![command_id],
            )?;
            Ok(())
        })
    }

    pub fn has_aggregate_events(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
    ) -> StorageResult<bool> {
        self.with_conn(|conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM orchestration_events \
                     WHERE aggregate_type = ?1 AND aggregate_id = ?2 LIMIT 1",
                    params![aggregate_type, aggregate_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
    }

    pub fn read_aggregate_events(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        after_sequence: i64,
    ) -> StorageResult<Vec<StoredOrchestrationEvent>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT sequence, event_id, command_id, aggregate_type, aggregate_id, \
                        event_type, occurred_at, payload_json \
                 FROM orchestration_events \
                 WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND sequence > ?3 \
                 ORDER BY sequence",
            )?;
            let rows = stmt.query_map(
                params![aggregate_type, aggregate_id, after_sequence],
                |row| {
                    Ok(StoredOrchestrationEvent {
                        sequence: row.get(0)?,
                        event_id: row.get(1)?,
                        command_id: row.get(2)?,
                        aggregate_type: row.get(3)?,
                        aggregate_id: row.get(4)?,
                        event_type: row.get(5)?,
                        occurred_at: row.get(6)?,
                        payload_json: row.get(7)?,
                    })
                },
            )?;
            let mut events = Vec::new();
            for row in rows {
                events.push(row?);
            }
            Ok(events)
        })
    }

    pub fn read_events_since(
        &self,
        after_sequence: i64,
        limit: usize,
    ) -> StorageResult<Vec<StoredOrchestrationEvent>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT sequence, event_id, command_id, aggregate_type, aggregate_id, \
                        event_type, occurred_at, payload_json \
                 FROM orchestration_events \
                 WHERE sequence > ?1 \
                 ORDER BY sequence LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![after_sequence, limit as i64], |row| {
                Ok(StoredOrchestrationEvent {
                    sequence: row.get(0)?,
                    event_id: row.get(1)?,
                    command_id: row.get(2)?,
                    aggregate_type: row.get(3)?,
                    aggregate_id: row.get(4)?,
                    event_type: row.get(5)?,
                    occurred_at: row.get(6)?,
                    payload_json: row.get(7)?,
                })
            })?;
            let mut events = Vec::new();
            for row in rows {
                events.push(row?);
            }
            Ok(events)
        })
    }

    pub fn delete_receipts_of(conn: &Connection, command_ids: &[String]) -> StorageResult<()> {
        if command_ids.is_empty() {
            return Ok(());
        }
        let mut stmt = conn.prepare(
            "DELETE FROM orchestration_command_receipts \
             WHERE command_id = ?1 \
               AND NOT EXISTS (SELECT 1 FROM orchestration_events WHERE command_id = ?1)",
        )?;
        for command_id in command_ids {
            stmt.execute(params![command_id])?;
        }
        Ok(())
    }

    pub fn delete_aggregate_history(
        conn: &Connection,
        aggregate_type: &str,
        aggregate_ids: &[String],
    ) -> StorageResult<()> {
        if aggregate_ids.is_empty() {
            return Ok(());
        }
        let ids_json = serde_json::to_string(aggregate_ids)?;
        conn.execute(
            "DELETE FROM orchestration_command_receipts WHERE command_id IN (\
               SELECT DISTINCT command_id FROM orchestration_events \
               WHERE aggregate_type = ?1 AND aggregate_id IN (SELECT value FROM json_each(?2))\
             )",
            params![aggregate_type, ids_json],
        )?;
        conn.execute(
            "DELETE FROM orchestration_events \
             WHERE aggregate_type = ?1 AND aggregate_id IN (SELECT value FROM json_each(?2))",
            params![aggregate_type, ids_json],
        )?;
        Ok(())
    }

    pub fn delete_events_before(
        conn: &Connection,
        aggregate_type: &str,
        aggregate_id: &str,
        before_sequence: i64,
    ) -> StorageResult<()> {
        conn.execute(
            "DELETE FROM orchestration_command_receipts WHERE command_id IN (\
               SELECT DISTINCT command_id FROM orchestration_events \
               WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND sequence < ?3\
             )",
            params![aggregate_type, aggregate_id, before_sequence],
        )?;
        conn.execute(
            "DELETE FROM orchestration_events \
             WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND sequence < ?3",
            params![aggregate_type, aggregate_id, before_sequence],
        )?;
        Ok(())
    }

    pub fn delete_orphan_receipts(conn: &Connection) -> StorageResult<usize> {
        conn.execute(
            "DELETE FROM orchestration_command_receipts \
             WHERE NOT EXISTS (\
               SELECT 1 FROM orchestration_events \
               WHERE orchestration_events.command_id = orchestration_command_receipts.command_id\
             )",
            [],
        )
        .map_err(StorageError::from)
    }

    pub fn delete_thread_attachments(conn: &Connection, thread_id: &str) -> StorageResult<usize> {
        conn.execute(
            "DELETE FROM projection_attachments \
             WHERE owner_kind = 'thread-message' AND owner_id >= ?1 AND owner_id < ?2",
            params![format!("{thread_id}:"), format!("{thread_id};")],
        )
        .map_err(StorageError::from)
    }

    pub fn quick_check(&self) -> StorageResult<()> {
        self.with_conn(assert_quick_check)
    }

    pub fn check_foreign_keys(&self) -> StorageResult<BTreeMap<String, usize>> {
        self.with_conn(read_foreign_key_violations)
    }

    pub fn backup_legacy_file(&self, path: impl AsRef<Path>) -> StorageResult<Option<PathBuf>> {
        let source = path.as_ref();
        if !source.exists() {
            return Ok(None);
        }
        let Some(name) = source.file_name() else {
            return Ok(None);
        };
        let backup_root = self.inner.data_dir.join("legacy-backup-v1");
        ensure_dir_mode_700(&backup_root)?;
        let target = backup_root.join(name);
        if !target.exists() {
            fs::copy(source, &target)?;
        }
        set_file_mode_600(&target)?;
        Ok(Some(target))
    }
}

impl OpenBotDatabase {
    pub fn enqueue_file_deletion(
        conn: &Connection,
        id: &str,
        path: &str,
        reason: &str,
        created_at: &str,
    ) -> StorageResult<()> {
        conn.execute(
            "INSERT OR IGNORE INTO file_deletion_outbox \
               (id, path, reason, created_at, attempts, last_error) \
             VALUES (?1, ?2, ?3, ?4, 0, NULL)",
            params![id, path, reason, created_at],
        )?;
        Ok(())
    }

    pub fn list_pending_file_deletions(&self) -> StorageResult<Vec<PendingFileDeletion>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, path, reason, created_at, attempts, last_error \
                 FROM file_deletion_outbox ORDER BY created_at, id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(PendingFileDeletion {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    reason: row.get(2)?,
                    created_at: row.get(3)?,
                    attempts: row.get(4)?,
                    last_error: row.get(5)?,
                })
            })?;
            let mut pending = Vec::new();
            for row in rows {
                pending.push(row?);
            }
            Ok(pending)
        })
    }

    #[inline]
    pub fn pending_file_deletions(&self) -> StorageResult<Vec<PendingFileDeletion>> {
        self.list_pending_file_deletions()
    }

    pub fn complete_file_deletion(&self, id: &str) -> StorageResult<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM file_deletion_outbox WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn fail_file_deletion(&self, id: &str, error: &str) -> StorageResult<()> {
        let truncated: String = error.chars().take(2_000).collect();
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE file_deletion_outbox \
                 SET attempts = attempts + 1, last_error = ?1 \
                 WHERE id = ?2",
                params![truncated, id],
            )?;
            Ok(())
        })
    }

    pub fn drain_file_deletion_outbox(&self) -> StorageResult<FileDeletionDrainReport> {
        let pending = self.list_pending_file_deletions()?;
        let mut report = FileDeletionDrainReport::default();
        for entry in pending {
            let target = Path::new(&entry.path);
            let unlink = if target.is_dir() {
                fs::remove_dir_all(target)
            } else {
                fs::remove_file(target)
            };
            match unlink {
                Ok(()) => {
                    self.complete_file_deletion(&entry.id)?;
                    report.deleted += 1;
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    self.complete_file_deletion(&entry.id)?;
                    report.deleted += 1;
                }
                Err(err) => {
                    self.fail_file_deletion(&entry.id, &err.to_string())?;
                    report.values_inc_failed();
                }
            }
        }
        Ok(report)
    }
}

impl FileDeletionDrainReport {
    fn values_inc_failed(&mut self) {
        self.failed += 1;
    }
}

#[derive(Clone, Debug)]
struct FoldSnapshotState {
    active_turn_id: Option<String>,
    messages: Vec<JsonMap<String, JsonValue>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThreadFoldBranch {
    Snapshot,
    Streamed,
    Message,
    Changes,
    None,
}

struct ParsedThreadEventMessages {
    snapshot: Option<FoldSnapshotState>,
    appended_message: Option<JsonMap<String, JsonValue>>,
    streamed_message: Option<JsonMap<String, JsonValue>>,
    imported_message: Option<JsonMap<String, JsonValue>>,
    changed_entries: Vec<Option<JsonMap<String, JsonValue>>>,
    changed_messages: Vec<JsonMap<String, JsonValue>>,
    removed_message_ids: Vec<String>,
    active_turn_id: Option<JsonValue>,
    has_active_turn_change: bool,
}

fn valid_message_map(value: &JsonValue) -> Option<JsonMap<String, JsonValue>> {
    let obj = value.as_object()?;
    let id = obj.get("id")?.as_str()?;
    let author = obj.get("author")?.as_str()?;
    let created_at = obj.get("createdAt")?.as_str()?;
    if id.is_empty() || author.is_empty() || created_at.is_empty() {
        return None;
    }
    Some(obj.clone())
}

fn parse_snapshot_value(value: Option<&JsonValue>) -> Option<FoldSnapshotState> {
    let obj = value?.as_object()?;
    let agent_id = obj
        .get("agentId")
        .or_else(|| obj.get("botId"))
        .and_then(JsonValue::as_str)?;
    if agent_id.is_empty() {
        return None;
    }
    if let Some(tid) = obj.get("threadId")
        && !tid.is_string()
        && !tid.is_null()
    {
        return None;
    }
    let active_turn_id = match obj.get("activeTurnId") {
        Some(JsonValue::String(s)) => Some(s.clone()),
        Some(JsonValue::Null) => None,
        _ => return None,
    };
    if !obj.get("revision").is_some_and(JsonValue::is_number) {
        return None;
    }
    let raw_messages = obj.get("messages")?.as_array()?;
    let mut messages = Vec::with_capacity(raw_messages.len());
    for entry in raw_messages {
        messages.push(valid_message_map(entry)?);
    }
    Some(FoldSnapshotState {
        active_turn_id,
        messages,
    })
}

fn parse_thread_event_messages(record: Option<&JsonMap<String, JsonValue>>) -> ParsedThreadEventMessages {
    let changed_entries: Vec<Option<JsonMap<String, JsonValue>>> = record
        .and_then(|r| r.get("changedMessages"))
        .and_then(JsonValue::as_array)
        .map(|arr| arr.iter().map(valid_message_map).collect())
        .unwrap_or_default();
    let changed_messages = changed_entries.iter().filter_map(Clone::clone).collect();
    let removed_message_ids = record
        .and_then(|r| r.get("removedMessageIds"))
        .and_then(JsonValue::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(JsonValue::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    ParsedThreadEventMessages {
        snapshot: parse_snapshot_value(record.and_then(|r| r.get("snapshot"))),
        appended_message: record
            .and_then(|r| r.get("appendedMessage"))
            .and_then(valid_message_map),
        streamed_message: record
            .and_then(|r| r.get("streamedMessage"))
            .and_then(valid_message_map),
        imported_message: record
            .and_then(|r| r.get("importedMessage"))
            .and_then(valid_message_map),
        changed_entries,
        changed_messages,
        removed_message_ids,
        active_turn_id: record.and_then(|r| r.get("activeTurnId")).cloned(),
        has_active_turn_change: record.is_some_and(|r| r.contains_key("activeTurnId")),
    }
}

fn message_id_of(msg: &JsonMap<String, JsonValue>) -> &str {
    msg.get("id").and_then(JsonValue::as_str).unwrap_or("")
}

pub fn merge_replayed_live_message(
    existing: &JsonMap<String, JsonValue>,
    updated: &JsonMap<String, JsonValue>,
) -> JsonMap<String, JsonValue> {
    let mut merged = existing.clone();
    for (k, v) in updated {
        merged.insert(k.clone(), v.clone());
    }
    if let Some(id) = existing.get("id") {
        merged.insert("id".to_string(), id.clone());
    }
    if let Some(author) = existing.get("author") {
        merged.insert("author".to_string(), author.clone());
    }
    let has_delivery = updated.get("delivery").is_some_and(|v| !v.is_null());
    let created_at = if has_delivery {
        updated.get("createdAt").or_else(|| existing.get("createdAt"))
    } else {
        existing.get("createdAt")
    };
    if let Some(ts) = created_at {
        merged.insert("createdAt".to_string(), ts.clone());
    }
    let turn_id = updated
        .get("turnId")
        .filter(|v| !v.is_null())
        .or_else(|| existing.get("turnId").filter(|v| !v.is_null()));
    if let Some(tid) = turn_id {
        merged.insert("turnId".to_string(), tid.clone());
    }
    merged
}

fn merge_replayed_imported_message(
    existing: &JsonMap<String, JsonValue>,
    imported: &JsonMap<String, JsonValue>,
) -> JsonMap<String, JsonValue> {
    let mut merged = existing.clone();
    for (k, v) in imported {
        merged.insert(k.clone(), v.clone());
    }
    for fixed in ["id", "author", "createdAt"] {
        if let Some(val) = existing.get(fixed) {
            merged.insert(fixed.to_string(), val.clone());
        }
    }
    let turn_id = existing
        .get("turnId")
        .filter(|v| !v.is_null())
        .or_else(|| imported.get("turnId").filter(|v| !v.is_null()));
    if let Some(tid) = turn_id {
        merged.insert("turnId".to_string(), tid.clone());
    }
    for keep_key in ["delivery", "exchange", "senderMember"] {
        if !imported.contains_key(keep_key)
            && let Some(prev) = existing.get(keep_key)
            && !prev.is_null()
        {
            merged.insert(keep_key.to_string(), prev.clone());
        }
    }
    merged
}

struct ThreadMessageFold {
    latest: Option<FoldSnapshotState>,
    latest_sequence: i64,
}

impl ThreadMessageFold {
    fn new() -> Self {
        Self {
            latest: None,
            latest_sequence: 0,
        }
    }

    fn branch_of(&self, event: &ParsedThreadEventMessages) -> ThreadFoldBranch {
        if event.snapshot.is_some() {
            ThreadFoldBranch::Snapshot
        } else if self.latest.is_some() && event.streamed_message.is_some() {
            ThreadFoldBranch::Streamed
        } else if event.appended_message.is_some() || event.imported_message.is_some() {
            ThreadFoldBranch::Message
        } else if !event.changed_messages.is_empty()
            || !event.removed_message_ids.is_empty()
            || event.has_active_turn_change
        {
            ThreadFoldBranch::Changes
        } else {
            ThreadFoldBranch::None
        }
    }

    fn ensure_latest(&mut self) -> &mut FoldSnapshotState {
        self.latest.get_or_insert_with(|| FoldSnapshotState {
            active_turn_id: None,
            messages: Vec::new(),
        })
    }

    fn apply(&mut self, sequence: i64, event: &ParsedThreadEventMessages) -> ThreadFoldBranch {
        let branch = self.branch_of(event);
        match branch {
            ThreadFoldBranch::Snapshot => {
                if let Some(snap) = &event.snapshot {
                    self.latest = Some(snap.clone());
                    self.latest_sequence = sequence;
                }
            }
            ThreadFoldBranch::Streamed => {
                if let (Some(latest), Some(streamed)) = (&mut self.latest, &event.streamed_message) {
                    let target_id = message_id_of(streamed);
                    if let Some(slot) = latest
                        .messages
                        .iter_mut()
                        .find(|m| message_id_of(m) == target_id)
                    {
                        *slot = streamed.clone();
                    } else {
                        latest.messages.push(streamed.clone());
                    }
                    apply_active_turn(&mut latest.active_turn_id, &event.active_turn_id);
                    self.latest_sequence = sequence;
                }
            }
            ThreadFoldBranch::Message => {
                let latest = self.ensure_latest();
                if let Some(msg) = event.appended_message.as_ref().or(event.imported_message.as_ref()) {
                    let target_id = message_id_of(msg);
                    if let Some(idx) = latest.messages.iter().position(|m| message_id_of(m) == target_id) {
                        if event.imported_message.is_some() {
                            latest.messages[idx] =
                                merge_replayed_imported_message(&latest.messages[idx], msg);
                        }
                    } else {
                        latest.messages.push(msg.clone());
                    }
                    apply_active_turn(&mut latest.active_turn_id, &event.active_turn_id);
                    self.latest_sequence = sequence;
                }
            }
            ThreadFoldBranch::Changes => {
                let latest = self.ensure_latest();
                let removed: HashSet<&str> =
                    event.removed_message_ids.iter().map(String::as_str).collect();
                let mut changed_by_id: HashMap<&str, &JsonMap<String, JsonValue>> = HashMap::new();
                for msg in &event.changed_messages {
                    changed_by_id.insert(message_id_of(msg), msg);
                }
                let mut next_messages = Vec::with_capacity(latest.messages.len());
                for msg in &latest.messages {
                    let mid = message_id_of(msg);
                    if removed.contains(mid) {
                        continue;
                    }
                    if let Some(changed) = changed_by_id.get(mid) {
                        next_messages.push(merge_replayed_live_message(msg, changed));
                    } else {
                        next_messages.push(msg.clone());
                    }
                }
                let mut shown: HashSet<String> = next_messages
                    .iter()
                    .map(|m| message_id_of(m).to_string())
                    .collect();
                for msg in &event.changed_messages {
                    let mid = message_id_of(msg);
                    if shown.insert(mid.to_string()) {
                        next_messages.push(msg.clone());
                    }
                }
                latest.messages = next_messages;
                apply_active_turn(&mut latest.active_turn_id, &event.active_turn_id);
                self.latest_sequence = sequence;
            }
            ThreadFoldBranch::None => {}
        }
        branch
    }
}

fn apply_active_turn(slot: &mut Option<String>, raw: &Option<JsonValue>) {
    match raw {
        Some(JsonValue::String(s)) => *slot = Some(s.clone()),
        Some(JsonValue::Null) => *slot = None,
        _ => {}
    }
}

fn kept_changed_messages(
    latest: Option<&FoldSnapshotState>,
    record: &JsonMap<String, JsonValue>,
    messages: &ParsedThreadEventMessages,
) -> Option<Vec<JsonValue>> {
    let raw = record.get("changedMessages")?.as_array()?;
    let latest = latest?;
    if messages.changed_messages.is_empty() {
        return None;
    }

    let mut shown: HashMap<&str, &JsonMap<String, JsonValue>> = HashMap::new();
    let mut repeated: HashSet<&str> = HashSet::new();
    for msg in &latest.messages {
        let mid = message_id_of(msg);
        if shown.contains_key(mid) {
            repeated.insert(mid);
        }
        shown.insert(mid, msg);
    }

    let mut last_by_id: Vec<(&str, &JsonMap<String, JsonValue>)> = Vec::new();
    for msg in &messages.changed_messages {
        let mid = message_id_of(msg);
        if let Some(slot) = last_by_id.iter_mut().find(|(id, _)| *id == mid) {
            slot.1 = msg;
        } else {
            last_by_id.push((mid, msg));
        }
    }

    let removed: HashSet<&str> = messages
        .removed_message_ids
        .iter()
        .map(String::as_str)
        .collect();
    let mut unneeded: HashSet<&str> = HashSet::new();
    for (mid, changed) in last_by_id {
        let Some(&existing) = shown.get(mid) else {
            continue;
        };
        if removed.contains(mid) || repeated.contains(mid) {
            continue;
        }
        if merge_replayed_live_message(existing, changed) == *existing {
            unneeded.insert(mid);
        }
    }
    if unneeded.is_empty() {
        return None;
    }

    let mut needed: Vec<bool> = messages
        .changed_entries
        .iter()
        .map(|entry| match entry {
            Some(msg) => !unneeded.contains(message_id_of(msg)),
            None => true,
        })
        .collect();

    let keeps_message = messages
        .changed_entries
        .iter()
        .enumerate()
        .any(|(idx, entry)| entry.is_some() && needed[idx]);
    if !keeps_message && messages.removed_message_ids.is_empty() && !messages.has_active_turn_change
        && let Some(first_valid) = messages.changed_entries.iter().position(Option::is_some)
    {
        needed[first_valid] = true;
    }

    if needed.iter().all(|&keep| keep) {
        return None;
    }
    Some(
        raw.iter()
            .enumerate()
            .filter_map(|(idx, item)| needed.get(idx).copied().unwrap_or(true).then(|| item.clone()))
            .collect(),
    )
}

struct ThreadCompactionProgress {
    thread_id: String,
    cutoff: i64,
    fold: ThreadMessageFold,
    through: i64,
    count: i64,
    sequence_sum: i64,
    last_sequence: i64,
    deleted_messages: i64,
    restarts: usize,
}

#[derive(Default)]
pub struct ThreadEventCompactor {
    current: Option<ThreadCompactionProgress>,
    after_thread_id: String,
}

impl ThreadEventCompactor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn compact_next(&mut self, db: &OpenBotDatabase) -> StorageResult<ThreadEventCompactionBatch> {
        if self.current.is_none() {
            let started = db.with_conn(|conn| start_next_thread(conn, &self.after_thread_id))?;
            match started {
                Some(progress) => {
                    self.after_thread_id = progress.thread_id.clone();
                    self.current = Some(progress);
                }
                None => {
                    self.after_thread_id.clear();
                    return Ok(ThreadEventCompactionBatch {
                        done: true,
                        ..Default::default()
                    });
                }
            }
        }

        let mut progress = self.current.take().expect("checked Some above");
        match db.with_tx(|conn| compact_batch_in_tx(conn, &mut progress)) {
            Ok((batch, keep_progress)) => {
                if keep_progress {
                    self.current = Some(progress);
                }
                Ok(batch)
            }
            Err(err) => Ok(ThreadEventCompactionBatch {
                failed_thread: Some(err.to_string()),
                ..Default::default()
            }),
        }
    }
}

fn thread_last_sequence(conn: &Connection, thread_id: &str) -> StorageResult<i64> {
    conn.query_row(
        "SELECT coalesce(max(sequence), 0) FROM orchestration_events \
         WHERE aggregate_type = 'thread' AND aggregate_id = ?1",
        params![thread_id],
        |row| row.get(0),
    )
    .map_err(StorageError::from)
}

fn deleted_message_count(conn: &Connection, thread_id: &str) -> StorageResult<i64> {
    conn.query_row(
        "SELECT count(*) FROM projection_deleted_thread_messages WHERE thread_id = ?1",
        params![thread_id],
        |row| row.get(0),
    )
    .map_err(StorageError::from)
}

fn start_next_thread(
    conn: &Connection,
    after_thread_id: &str,
) -> StorageResult<Option<ThreadCompactionProgress>> {
    let next_id: Option<String> = conn
        .query_row(
            "SELECT t.thread_id FROM projection_threads t \
             WHERE t.thread_id > ?1 \
               AND NOT EXISTS (\
                 SELECT 1 FROM thread_event_compaction c WHERE c.thread_id = t.thread_id\
               ) \
             ORDER BY t.thread_id LIMIT 1",
            params![after_thread_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(thread_id) = next_id else {
        return Ok(None);
    };
    let last_sequence = thread_last_sequence(conn, &thread_id)?;
    let deleted_messages = deleted_message_count(conn, &thread_id)?;
    Ok(Some(ThreadCompactionProgress {
        thread_id,
        cutoff: last_sequence,
        fold: ThreadMessageFold::new(),
        through: 0,
        count: 0,
        sequence_sum: 0,
        last_sequence,
        deleted_messages,
        restarts: 0,
    }))
}

fn earlier_events_unchanged(
    conn: &Connection,
    progress: &ThreadCompactionProgress,
) -> StorageResult<bool> {
    if deleted_message_count(conn, &progress.thread_id)? != progress.deleted_messages {
        return Ok(false);
    }
    if thread_last_sequence(conn, &progress.thread_id)? == progress.last_sequence {
        return Ok(true);
    }
    let (events, seq_total): (i64, f64) = conn.query_row(
        "SELECT count(*), total(sequence) FROM orchestration_events \
         WHERE aggregate_type = 'thread' AND aggregate_id = ?1 AND sequence <= ?2",
        params![progress.thread_id, progress.through],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(events == progress.count && (seq_total as i64) == progress.sequence_sum)
}

fn compact_batch_in_tx(
    conn: &Connection,
    progress: &mut ThreadCompactionProgress,
) -> StorageResult<(ThreadEventCompactionBatch, bool)> {
    let mut batch = ThreadEventCompactionBatch::default();
    let thread_exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM projection_threads WHERE thread_id = ?1",
            params![progress.thread_id],
            |row| row.get(0),
        )
        .optional()?;
    if thread_exists.is_none() {
        return Ok((batch, false));
    }

    if !earlier_events_unchanged(conn, progress)? {
        if progress.restarts >= THREAD_COMPACTION_MAX_RESTARTS {
            batch.failed_thread = Some("RestartLimit".to_string());
            return Ok((batch, false));
        }
        let last_sequence = thread_last_sequence(conn, &progress.thread_id)?;
        progress.cutoff = last_sequence;
        progress.fold = ThreadMessageFold::new();
        progress.through = 0;
        progress.count = 0;
        progress.sequence_sum = 0;
        progress.last_sequence = last_sequence;
        progress.deleted_messages = deleted_message_count(conn, &progress.thread_id)?;
        progress.restarts += 1;
    }

    let mut rewrites: Vec<(StoredOrchestrationEvent, String)> = Vec::new();
    let mut bytes = 0usize;
    let mut reached_end = true;
    {
        let mut stmt = conn.prepare(
            "SELECT sequence, event_id, command_id, aggregate_type, aggregate_id, \
                    event_type, occurred_at, payload_json \
             FROM orchestration_events \
             WHERE aggregate_type = 'thread' AND aggregate_id = ?1 \
               AND sequence > ?2 AND sequence <= ?3 \
             ORDER BY sequence",
        )?;
        let mut rows = stmt.query(params![progress.thread_id, progress.through, progress.cutoff])?;
        while let Some(row) = rows.next()? {
            if batch.events >= THREAD_COMPACTION_BATCH_EVENTS || bytes >= THREAD_COMPACTION_BATCH_BYTES {
                reached_end = false;
                break;
            }
            let stored = StoredOrchestrationEvent {
                sequence: row.get(0)?,
                event_id: row.get(1)?,
                command_id: row.get(2)?,
                aggregate_type: row.get(3)?,
                aggregate_id: row.get(4)?,
                event_type: row.get(5)?,
                occurred_at: row.get(6)?,
                payload_json: row.get(7)?,
            };
            bytes += stored.payload_json.len();
            batch.events += 1;

            let parsed_val: Option<JsonValue> = serde_json::from_str(&stored.payload_json).ok();
            let record = parsed_val.as_ref().and_then(JsonValue::as_object);
            let messages = parse_thread_event_messages(record);
            if let Some(rec) = record
                && progress.fold.branch_of(&messages) == ThreadFoldBranch::Changes
                && let Some(kept) = kept_changed_messages(progress.fold.latest.as_ref(), rec, &messages)
            {
                let mut next_rec = rec.clone();
                next_rec.insert("changedMessages".to_string(), JsonValue::Array(kept));
                let compacted = serde_json::to_string(&next_rec)?;
                batch.bytes_before += stored.payload_json.len();
                batch.bytes_after += compacted.len();
                rewrites.push((stored.clone(), compacted));
            }
            progress.fold.apply(stored.sequence, &messages);
            let _ = progress.fold.latest_sequence;
            progress.through = stored.sequence;
            progress.count += 1;
            progress.sequence_sum += stored.sequence;
        }
    }

    if !rewrites.is_empty() {
        let mut remove_stmt = conn.prepare("DELETE FROM orchestration_events WHERE sequence = ?1")?;
        let mut insert_stmt = conn.prepare(
            "INSERT INTO orchestration_events \
               (sequence, event_id, command_id, aggregate_type, aggregate_id, \
                event_type, occurred_at, payload_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (stored, compacted_payload) in &rewrites {
            remove_stmt.execute(params![stored.sequence])?;
            insert_stmt.execute(params![
                stored.sequence,
                stored.event_id,
                stored.command_id,
                stored.aggregate_type,
                stored.aggregate_id,
                stored.event_type,
                stored.occurred_at,
                compacted_payload,
            ])?;
        }
    }
    batch.rewritten = rewrites.len();

    progress.last_sequence = thread_last_sequence(conn, &progress.thread_id)?;
    progress.deleted_messages = deleted_message_count(conn, &progress.thread_id)?;
    if reached_end {
        conn.execute(
            "INSERT INTO thread_event_compaction (thread_id, through_sequence, compacted_at) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(thread_id) DO UPDATE SET \
               through_sequence = excluded.through_sequence, \
               compacted_at = excluded.compacted_at",
            params![progress.thread_id, progress.cutoff, now_rfc3339_millis()],
        )?;
        batch.completed_thread = true;
        return Ok((batch, false));
    }
    Ok((batch, true))
}

impl OpenBotDatabase {
    pub fn compact_next_thread_batch(
        &self,
        compactor: &mut ThreadEventCompactor,
    ) -> StorageResult<ThreadEventCompactionBatch> {
        compactor.compact_next(self)
    }

    pub fn compact_thread_events(&self) -> StorageResult<Vec<ThreadEventCompactionBatch>> {
        let mut compactor = ThreadEventCompactor::new();
        let mut batches = Vec::new();
        loop {
            let batch = compactor.compact_next(self)?;
            if batch.done {
                break;
            }
            batches.push(batch);
        }
        Ok(batches)
    }
}
