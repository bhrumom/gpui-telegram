//! Domain-Partitioned ProfileState SQLite v3, Generation-Fenced `AtomicSnapshotWriter`,
//! Versioned Cache Auto-Rebuild (`open_versioned_cache_db`), and Non-Destructive
//! `ProtectedSecretPersistence` (`F16`).

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use orca_core::orchestration_protocol::{
    AtomicSnapshot, AtomicSnapshotGenerationFence, ProtectedSecret, SealedProtectedSecret,
    VaultError, open_protected_secret, seal_protected_secret,
};
use parking_lot::{Mutex, RwLock};
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{OrcaIdeRuntimeError, Result, canonical_json_sha256, now_epoch_ms};

/// Authoritative SQLite `PRAGMA user_version` for `ProfileStateAuthority`.
pub const PROFILE_STATE_SCHEMA_VERSION: u32 = 3;

const PROFILE_STATE_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS profile_state_documents (
    domain TEXT PRIMARY KEY,
    payload TEXT NOT NULL,
    domain_version INTEGER NOT NULL DEFAULT 1,
    revision INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL,
    content_hash TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS profile_state_document_parts (
    domain TEXT NOT NULL,
    part_key TEXT NOT NULL,
    ordinal INTEGER NOT NULL DEFAULT 0,
    payload TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    PRIMARY KEY (domain, part_key)
);

CREATE INDEX IF NOT EXISTS idx_profile_state_parts_domain_ord
    ON profile_state_document_parts(domain, ordinal ASC, part_key ASC);

CREATE TABLE IF NOT EXISTS profile_state_automation_runs (
    run_id TEXT PRIMARY KEY,
    automation_id TEXT NOT NULL,
    bot_id TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    summary_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_profile_state_auto_runs_started
    ON profile_state_automation_runs(automation_id, started_at DESC);
"#;

/// One persisted domain document in `profile_state_documents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileStateDocument {
    pub domain: String,
    pub payload: serde_json::Value,
    pub domain_version: u32,
    pub revision: u64,
    pub updated_at: i64,
    pub content_hash: String,
}

/// One persisted domain part in `profile_state_document_parts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileStateDocumentPart {
    pub domain: String,
    pub part_key: String,
    pub ordinal: i64,
    pub payload: serde_json::Value,
    pub content_hash: String,
}

/// One persisted automation run summary in `profile_state_automation_runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileStateAutomationRun {
    pub run_id: String,
    pub automation_id: String,
    pub bot_id: String,
    pub status: String,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    pub summary_json: serde_json::Value,
}

/// Targeted dirty-domain write with optional optimistic revision fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyDomainMutation {
    pub domain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default = "default_domain_version")]
    pub domain_version: u32,
    pub payload: serde_json::Value,
}

const fn default_domain_version() -> u32 {
    1
}

fn configure_profile_state_connection(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA auto_vacuum = INCREMENTAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA journal_size_limit = 8388608;
         PRAGMA foreign_keys = ON;",
    )?;
    conn.execute_batch(PROFILE_STATE_SCHEMA_SQL)?;
    conn.execute_batch(&format!(
        "PRAGMA user_version = {PROFILE_STATE_SCHEMA_VERSION};"
    ))?;
    Ok(())
}

fn validate_non_empty_key(label: &str, raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 256 || trimmed.chars().any(char::is_control) {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "{label} must be 1..=256 non-control characters"
        )));
    }
    Ok(trimmed.to_string())
}

/// Thread-safe (`Send + Sync`), WAL-backed SQLite v3 ProfileState authority supporting
/// targeted dirty-domain writes with monotonic optimistic revision fencing.
pub struct ProfileStateAuthority {
    conn: Mutex<Connection>,
}

impl fmt::Debug for ProfileStateAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileStateAuthority").finish()
    }
}

impl ProfileStateAuthority {
    /// Opens (or initializes) a durable SQLite v3 ProfileState DB at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        configure_profile_state_connection(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Opens an isolated in-memory ProfileState authority.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        configure_profile_state_connection(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Reads one persisted domain document.
    pub fn get_domain(&self, domain: &str) -> Result<Option<ProfileStateDocument>> {
        let dom = validate_non_empty_key("domain", domain)?;
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT domain, payload, domain_version, revision, updated_at, content_hash
                 FROM profile_state_documents
                 WHERE domain = ?1",
                params![dom],
                row_to_document,
            )
            .optional()?;
        Ok(row)
    }

    /// Lists all persisted domain documents ordered lexicographically by `domain`.
    pub fn list_domains(&self) -> Result<Vec<ProfileStateDocument>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT domain, payload, domain_version, revision, updated_at, content_hash
             FROM profile_state_documents
             ORDER BY domain ASC",
        )?;
        let rows = stmt.query_map([], row_to_document)?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item?);
        }
        Ok(out)
    }

    /// Writes ONLY the specified dirty domains inside a single IMMEDIATE transaction,
    /// enforcing monotonic optimistic revision fencing. On mismatch, rolls back all
    /// mutations and returns `ProfileStateRevisionConflict { domain, expected, current }`.
    pub fn write_dirty_domains(
        &self,
        mutations: &[DirtyDomainMutation],
        now_ms: i64,
    ) -> Result<Vec<ProfileStateDocument>> {
        if mutations.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut results = Vec::with_capacity(mutations.len());

        for mutation in mutations {
            let domain = validate_non_empty_key("domain", &mutation.domain)?;
            let current_rev: u64 = tx
                .query_row(
                    "SELECT revision FROM profile_state_documents WHERE domain = ?1",
                    params![&domain],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .map(|v| v.max(0) as u64)
                .unwrap_or(0);

            if let Some(expected) = mutation.expected_revision
                && expected != current_rev
            {
                return Err(OrcaIdeRuntimeError::ProfileStateRevisionConflict {
                    domain,
                    expected,
                    current: current_rev,
                });
            }

            let next_rev = current_rev.saturating_add(1);
            let payload_str = serde_json::to_string(&mutation.payload)?;
            let content_hash = canonical_json_sha256(&mutation.payload);
            let dom_ver = mutation.domain_version.max(1);

            tx.execute(
                "INSERT INTO profile_state_documents
                    (domain, payload, domain_version, revision, updated_at, content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(domain) DO UPDATE SET
                    payload = excluded.payload,
                    domain_version = excluded.domain_version,
                    revision = excluded.revision,
                    updated_at = excluded.updated_at,
                    content_hash = excluded.content_hash",
                params![
                    &domain,
                    &payload_str,
                    dom_ver as i64,
                    next_rev as i64,
                    now_ms,
                    &content_hash,
                ],
            )?;

            results.push(ProfileStateDocument {
                domain,
                payload: mutation.payload.clone(),
                domain_version: dom_ver,
                revision: next_rev,
                updated_at: now_ms,
                content_hash,
            });
        }

        tx.commit()?;
        Ok(results)
    }

    /// Upserts one domain document part (`profile_state_document_parts`).
    pub fn put_document_part(
        &self,
        domain: &str,
        part_key: &str,
        ordinal: i64,
        payload: serde_json::Value,
    ) -> Result<ProfileStateDocumentPart> {
        let dom = validate_non_empty_key("domain", domain)?;
        let key = validate_non_empty_key("part_key", part_key)?;
        let payload_str = serde_json::to_string(&payload)?;
        let content_hash = canonical_json_sha256(&payload);

        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO profile_state_document_parts
                (domain, part_key, ordinal, payload, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(domain, part_key) DO UPDATE SET
                ordinal = excluded.ordinal,
                payload = excluded.payload,
                content_hash = excluded.content_hash",
            params![&dom, &key, ordinal, &payload_str, &content_hash],
        )?;

        Ok(ProfileStateDocumentPart {
            domain: dom,
            part_key: key,
            ordinal,
            payload,
            content_hash,
        })
    }

    /// Lists all document parts for `domain` ordered by `(ordinal ASC, part_key ASC)`.
    pub fn list_document_parts(&self, domain: &str) -> Result<Vec<ProfileStateDocumentPart>> {
        let dom = validate_non_empty_key("domain", domain)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT domain, part_key, ordinal, payload, content_hash
             FROM profile_state_document_parts
             WHERE domain = ?1
             ORDER BY ordinal ASC, part_key ASC",
        )?;
        let rows = stmt.query_map(params![dom], |row| {
            let payload_raw: String = row.get(3)?;
            let payload = serde_json::from_str(&payload_raw).unwrap_or(serde_json::Value::Null);
            Ok(ProfileStateDocumentPart {
                domain: row.get(0)?,
                part_key: row.get(1)?,
                ordinal: row.get(2)?,
                payload,
                content_hash: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item?);
        }
        Ok(out)
    }

    /// Deletes a specific document part; returns `true` if a row was deleted.
    pub fn delete_document_part(&self, domain: &str, part_key: &str) -> Result<bool> {
        let dom = validate_non_empty_key("domain", domain)?;
        let key = validate_non_empty_key("part_key", part_key)?;
        let conn = self.conn.lock();
        let affected = conn.execute(
            "DELETE FROM profile_state_document_parts WHERE domain = ?1 AND part_key = ?2",
            params![dom, key],
        )?;
        Ok(affected > 0)
    }

    /// Inserts or updates an automation run entry in `profile_state_automation_runs`.
    pub fn record_automation_run(
        &self,
        run: ProfileStateAutomationRun,
    ) -> Result<ProfileStateAutomationRun> {
        let run_id = validate_non_empty_key("run_id", &run.run_id)?;
        let automation_id = validate_non_empty_key("automation_id", &run.automation_id)?;
        let bot_id = validate_non_empty_key("bot_id", &run.bot_id)?;
        let status = validate_non_empty_key("status", &run.status)?;
        let summary_str = serde_json::to_string(&run.summary_json)?;

        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO profile_state_automation_runs
                (run_id, automation_id, bot_id, status, started_at, finished_at, summary_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(run_id) DO UPDATE SET
                automation_id = excluded.automation_id,
                bot_id = excluded.bot_id,
                status = excluded.status,
                started_at = excluded.started_at,
                finished_at = excluded.finished_at,
                summary_json = excluded.summary_json",
            params![
                &run_id,
                &automation_id,
                &bot_id,
                &status,
                run.started_at,
                run.finished_at,
                &summary_str,
            ],
        )?;

        Ok(ProfileStateAutomationRun {
            run_id,
            automation_id,
            bot_id,
            status,
            started_at: run.started_at,
            finished_at: run.finished_at,
            summary_json: run.summary_json,
        })
    }

    /// Lists recorded automation runs newest-first, optionally filtered by `automation_id`.
    pub fn list_automation_runs(
        &self,
        automation_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ProfileStateAutomationRun>> {
        let conn = self.conn.lock();
        let lim = limit.clamp(1, 500) as i64;
        let mut out = Vec::new();
        if let Some(auto_id) = automation_id {
            let mut stmt = conn.prepare(
                "SELECT run_id, automation_id, bot_id, status, started_at, finished_at, summary_json
                 FROM profile_state_automation_runs
                 WHERE automation_id = ?1
                 ORDER BY started_at DESC, run_id DESC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![auto_id, lim], row_to_automation_run)?;
            for item in rows {
                out.push(item?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT run_id, automation_id, bot_id, status, started_at, finished_at, summary_json
                 FROM profile_state_automation_runs
                 ORDER BY started_at DESC, run_id DESC
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![lim], row_to_automation_run)?;
            for item in rows {
                out.push(item?);
            }
        }
        Ok(out)
    }

    /// Executes `PRAGMA incremental_vacuum(pages)` to reclaim freelist space lazily.
    pub fn run_incremental_vacuum(&self, pages: u32) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch(&format!("PRAGMA incremental_vacuum({pages});"))?;
        Ok(())
    }
}

fn row_to_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProfileStateDocument> {
    let payload_raw: String = row.get(1)?;
    let payload = serde_json::from_str(&payload_raw).unwrap_or(serde_json::Value::Null);
    Ok(ProfileStateDocument {
        domain: row.get(0)?,
        payload,
        domain_version: row.get::<_, i64>(2)?.max(1) as u32,
        revision: row.get::<_, i64>(3)?.max(0) as u64,
        updated_at: row.get(4)?,
        content_hash: row.get(5)?,
    })
}

fn row_to_automation_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProfileStateAutomationRun> {
    let summary_raw: String = row.get(6)?;
    let summary_json = serde_json::from_str(&summary_raw).unwrap_or(serde_json::Value::Null);
    Ok(ProfileStateAutomationRun {
        run_id: row.get(0)?,
        automation_id: row.get(1)?,
        bot_id: row.get(2)?,
        status: row.get(3)?,
        started_at: row.get(4)?,
        finished_at: row.get(5)?,
        summary_json,
    })
}

// ---------------------------------------------------------------------------
// Generation-Fenced AtomicSnapshotWriter
// ---------------------------------------------------------------------------

/// Handle returned by [`AtomicSnapshotWriter::begin_async_stage`] representing an
/// fsynced sibling temp file waiting for fenced atomic rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedAsyncSnapshot {
    pub generation: u64,
    pub temp_path: PathBuf,
}

#[derive(Debug, Default)]
struct AtomicWriterInner {
    fence: AtomicSnapshotGenerationFence,
    active_async_temps: BTreeMap<u64, PathBuf>,
}

/// Generation-fenced atomic snapshot writer (`Send + Sync`) preventing stale async
/// snapshot renames from overwriting a newer synchronous quit flush (`SM1-04`).
pub struct AtomicSnapshotWriter {
    target_path: PathBuf,
    inner: Mutex<AtomicWriterInner>,
}

impl fmt::Debug for AtomicSnapshotWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AtomicSnapshotWriter")
            .field("target_path", &self.target_path)
            .finish()
    }
}

fn write_and_sync_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

impl AtomicSnapshotWriter {
    pub fn new(target_path: impl Into<PathBuf>) -> Self {
        Self {
            target_path: target_path.into(),
            inner: Mutex::new(AtomicWriterInner::default()),
        }
    }

    pub fn target_path(&self) -> &Path {
        &self.target_path
    }

    pub fn last_committed_generation(&self) -> u64 {
        self.inner.lock().fence.last_committed_generation()
    }

    fn sibling_temp_path(&self, tag: &str) -> PathBuf {
        let base_name = self
            .target_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("snapshot.json");
        self.target_path
            .with_file_name(format!(".{base_name}.{tag}-{}.tmp", Uuid::new_v4()))
    }

    /// Verifies `snapshot`, reserves a generation token, writes and fsyncs a sibling
    /// temp file, and tracks it so `flush_sync()` can unlink stale async temps.
    pub fn begin_async_stage<T: Serialize>(
        &self,
        snapshot: &AtomicSnapshot<T>,
    ) -> Result<StagedAsyncSnapshot> {
        snapshot.verify_integrity()?;
        let bytes = serde_json::to_vec_pretty(snapshot)?;
        let generation = {
            let mut inner = self.inner.lock();
            inner.fence.begin_async_write()
        };
        let temp_path = self.sibling_temp_path(&format!("async-g{generation}"));
        write_and_sync_file(&temp_path, &bytes)?;
        self.inner
            .lock()
            .active_async_temps
            .insert(generation, temp_path.clone());
        Ok(StagedAsyncSnapshot {
            generation,
            temp_path,
        })
    }

    /// Attempts to atomically rename a staged async temp file onto `target_path`.
    /// Fails with `VaultError::StaleGeneration` (cleaning up its temp file without
    /// touching `target_path`) if a newer commit or `flush_sync()` already won.
    pub fn commit_async(&self, staged: StagedAsyncSnapshot) -> Result<u64> {
        let mut inner = self.inner.lock();
        inner.active_async_temps.remove(&staged.generation);
        if let Err(err) = inner.fence.commit_async(staged.generation) {
            let _ = fs::remove_file(&staged.temp_path);
            return Err(OrcaIdeRuntimeError::Vault(err));
        }
        fs::rename(&staged.temp_path, &self.target_path)?;
        Ok(staged.generation)
    }

    /// Performs an authoritative synchronous flush: unlinks ALL tracked stale async
    /// temp files, writes & fsyncs a fresh sibling temp file, atomically renames it
    /// to `target_path`, and advances `commit_sync_flush()` to fence late async commits.
    pub fn flush_sync<T: Serialize>(&self, snapshot: &AtomicSnapshot<T>) -> Result<u64> {
        snapshot.verify_integrity()?;
        let bytes = serde_json::to_vec_pretty(snapshot)?;
        let mut inner = self.inner.lock();
        let stale_temps: Vec<PathBuf> = std::mem::take(&mut inner.active_async_temps)
            .into_values()
            .collect();
        for stale in stale_temps {
            let _ = fs::remove_file(stale);
        }
        let temp_path = self.sibling_temp_path("sync");
        write_and_sync_file(&temp_path, &bytes)?;
        fs::rename(&temp_path, &self.target_path)?;
        let committed = inner.fence.commit_sync_flush();
        Ok(committed)
    }

    /// Reads `target_path` if present and verifies its SHA-256 integrity envelope.
    pub fn read_verified<T: DeserializeOwned + Serialize>(
        &self,
    ) -> Result<Option<AtomicSnapshot<T>>> {
        if !self.target_path.is_file() {
            return Ok(None);
        }
        let raw = fs::read(&self.target_path)?;
        let snap: AtomicSnapshot<T> = serde_json::from_slice(&raw)?;
        snap.verify_integrity()?;
        Ok(Some(snap))
    }
}

// ---------------------------------------------------------------------------
// Versioned Cache DB Auto-Rebuild (`open_versioned_cache_db`)
// ---------------------------------------------------------------------------

fn is_corruption_sqlite_error(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
}

fn unlink_sqlite_files(path: &Path) {
    let _ = fs::remove_file(path);
    let base = path.as_os_str();
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = base.to_os_string();
        sidecar.push(suffix);
        let _ = fs::remove_file(PathBuf::from(sidecar));
    }
}

fn check_existing_cache_db(path: &Path, expected_user_version: u32) -> std::result::Result<Option<Connection>, rusqlite::Error> {
    let conn = Connection::open(path)?;
    let ver: i64 = conn.query_row("PRAGMA user_version;", [], |row| row.get(0))?;
    if ver != expected_user_version as i64 {
        return Ok(None);
    }
    let quick: String = conn.query_row("PRAGMA quick_check;", [], |row| row.get(0))?;
    if !quick.eq_ignore_ascii_case("ok") {
        return Ok(None);
    }
    Ok(Some(conn))
}

/// Opens a versioned SQLite cache database, inspecting `PRAGMA user_version` and
/// `PRAGMA quick_check`. On `SQLITE_CORRUPT`/`SQLITE_NOTADB`, failed quick-check,
/// or stale `user_version`, unlinks the DB and its `-wal`/`-shm`/`-journal` sidecars
/// (once only) and rebuilds it cleanly. Returns `(Connection, rebuilt)`.
pub fn open_versioned_cache_db(
    path: &Path,
    expected_user_version: u32,
    init_sql: &str,
) -> Result<(Connection, bool)> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let existed = path.exists();
    if existed {
        match check_existing_cache_db(path, expected_user_version) {
            Ok(Some(conn)) => {
                conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
                conn.execute_batch(init_sql)?;
                return Ok((conn, false));
            }
            Ok(None) => {
                unlink_sqlite_files(path);
            }
            Err(err) if is_corruption_sqlite_error(&err) => {
                unlink_sqlite_files(path);
            }
            Err(err) => {
                // Any corrupt header read or quick_check parse failure also triggers single rebuild.
                let msg = err.to_string().to_ascii_lowercase();
                if msg.contains("not a database")
                    || msg.contains("malformed")
                    || msg.contains("corrupt")
                {
                    unlink_sqlite_files(path);
                } else {
                    return Err(OrcaIdeRuntimeError::Sqlite(err));
                }
            }
        }
    }

    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(init_sql)?;
    conn.execute_batch(&format!("PRAGMA user_version = {expected_user_version};"))?;
    Ok((conn, existed))
}

// ---------------------------------------------------------------------------
// Non-Destructive ProtectedSecretPersistence
// ---------------------------------------------------------------------------

/// Master key availability reported by the platform keyring backend.
pub enum KeyringState {
    Available([u8; 32]),
    UnavailableLocked,
}

impl Drop for KeyringState {
    fn drop(&mut self) {
        if let Self::Available(key) = self {
            key.zeroize();
        }
    }
}

impl fmt::Debug for KeyringState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Available(_) => f.write_str("KeyringState::Available([REDACTED])"),
            Self::UnavailableLocked => f.write_str("KeyringState::UnavailableLocked"),
        }
    }
}

/// Platform keyring abstraction supplying a 32-byte master key when unlocked.
pub trait KeyringMasterKeyProvider: Send + Sync {
    fn master_key_state(&self) -> KeyringState;
}

/// In-memory `KeyringMasterKeyProvider` supporting lock/unlock transitions with
/// automatic key zeroization on lock and drop.
pub struct InMemoryKeyringProvider {
    key: RwLock<Option<[u8; 32]>>,
}

impl Drop for InMemoryKeyringProvider {
    fn drop(&mut self) {
        if let Some(mut key) = self.key.write().take() {
            key.zeroize();
        }
    }
}

impl fmt::Debug for InMemoryKeyringProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let unlocked = self.key.read().is_some();
        f.debug_struct("InMemoryKeyringProvider")
            .field("unlocked", &unlocked)
            .finish()
    }
}

impl InMemoryKeyringProvider {
    pub fn new_available(key: [u8; 32]) -> Self {
        Self {
            key: RwLock::new(Some(key)),
        }
    }

    pub fn new_locked() -> Self {
        Self {
            key: RwLock::new(None),
        }
    }

    pub fn generate_available() -> Result<Self> {
        let mut key = [0u8; 32];
        getrandom::getrandom(&mut key)
            .map_err(|e| OrcaIdeRuntimeError::Vault(VaultError::Entropy(e.to_string())))?;
        Ok(Self::new_available(key))
    }

    pub fn set_available(&self, key: [u8; 32]) {
        let mut guard = self.key.write();
        if let Some(mut old) = guard.take() {
            old.zeroize();
        }
        *guard = Some(key);
    }

    pub fn lock(&self) {
        let mut guard = self.key.write();
        if let Some(mut old) = guard.take() {
            old.zeroize();
        }
    }
}

impl KeyringMasterKeyProvider for InMemoryKeyringProvider {
    fn master_key_state(&self) -> KeyringState {
        match *self.key.read() {
            Some(key) => KeyringState::Available(key),
            None => KeyringState::UnavailableLocked,
        }
    }
}

/// Persisted ciphertext-only secret store envelope on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSecretEnvelope {
    pub schema_version: u32,
    #[serde(default)]
    pub sealed_slots: BTreeMap<String, SealedProtectedSecret>,
    #[serde(default)]
    pub retained_blobs: BTreeMap<String, SealedProtectedSecret>,
    pub updated_at_ms: i64,
}

impl Default for PersistedSecretEnvelope {
    fn default() -> Self {
        Self {
            schema_version: 1,
            sealed_slots: BTreeMap::new(),
            retained_blobs: BTreeMap::new(),
            updated_at_ms: 0,
        }
    }
}

/// Summary returned by [`ProtectedSecretPersistence::save_non_destructive`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretPersistenceOutcome {
    pub degraded: bool,
    pub sealed_count: usize,
    pub retained_count: usize,
}

/// Non-destructive authenticated secret persistence (`Send + Sync`).
///
/// When the keyring is locked/unavailable (`degraded == true`), ALL prior
/// `sealed_slots` and `retained_blobs` ciphertext are preserved intact on disk —
/// NEVER wiped and NEVER downgraded to plaintext (`SM1-05`).
pub struct ProtectedSecretPersistence {
    path: PathBuf,
    keyring: Arc<dyn KeyringMasterKeyProvider>,
    envelope: Mutex<PersistedSecretEnvelope>,
}

impl fmt::Debug for ProtectedSecretPersistence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtectedSecretPersistence")
            .field("path", &self.path)
            .field("degraded", &self.is_degraded())
            .finish()
    }
}

impl ProtectedSecretPersistence {
    pub fn open(
        path: impl Into<PathBuf>,
        keyring: Arc<dyn KeyringMasterKeyProvider>,
    ) -> Result<Self> {
        let path = path.into();
        let envelope = if path.is_file() {
            let raw = fs::read_to_string(&path)?;
            serde_json::from_str::<PersistedSecretEnvelope>(&raw)?
        } else {
            PersistedSecretEnvelope::default()
        };
        Ok(Self {
            path,
            keyring,
            envelope: Mutex::new(envelope),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_degraded(&self) -> bool {
        matches!(self.keyring.master_key_state(), KeyringState::UnavailableLocked)
    }

    fn persist_envelope(&self, env: &PersistedSecretEnvelope) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(env)?;
        let base_name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("secrets.json");
        let tmp = self
            .path
            .with_file_name(format!(".{base_name}.tmp-{}", Uuid::new_v4()));
        write_and_sync_file(&tmp, &bytes)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Seals and persists a secret into `slot`. When keyring is locked, refuses to
    /// downgrade to plaintext and keeps all existing ciphertext intact.
    pub fn store_secret(
        &self,
        slot: &str,
        aad_label: &str,
        secret: &ProtectedSecret,
        now_ms: i64,
    ) -> Result<SealedProtectedSecret> {
        let slot_key = validate_non_empty_key("slot", slot)?;
        let aad = validate_non_empty_key("aad_label", aad_label)?;
        let key_state = self.keyring.master_key_state();
        let KeyringState::Available(key) = key_state else {
            return Err(OrcaIdeRuntimeError::permission_denied(
                "keyring unavailable/locked: refusing plaintext downgrade; existing sealed secrets preserved",
            ));
        };
        let sealed = seal_protected_secret(&key, &aad, secret)?;
        let mut guard = self.envelope.lock();
        guard.sealed_slots.insert(slot_key.clone(), sealed.clone());
        guard.retained_blobs.remove(&slot_key);
        guard.updated_at_ms = now_ms;
        self.persist_envelope(&guard)?;
        Ok(sealed)
    }

    /// Decrypts `slot` when the keyring is unlocked. Preserves ciphertext untouched if locked.
    pub fn read_secret(
        &self,
        slot: &str,
        expected_aad_label: &str,
    ) -> Result<Option<ProtectedSecret>> {
        let slot_key = validate_non_empty_key("slot", slot)?;
        let aad = validate_non_empty_key("aad_label", expected_aad_label)?;
        let sealed_opt = {
            let guard = self.envelope.lock();
            guard
                .sealed_slots
                .get(&slot_key)
                .or_else(|| guard.retained_blobs.get(&slot_key))
                .cloned()
        };
        let Some(sealed) = sealed_opt else {
            return Ok(None);
        };
        let key_state = self.keyring.master_key_state();
        let KeyringState::Available(key) = key_state else {
            return Err(OrcaIdeRuntimeError::permission_denied(
                "keyring unavailable/locked: cannot unseal secret; ciphertext retained intact",
            ));
        };
        let opened = open_protected_secret(&key, &aad, &sealed)?;
        Ok(Some(opened))
    }

    /// Persists updates non-destructively:
    /// - When keyring is unlocked: seals all `updates` into `sealed_slots`, restores any
    ///   matching `retained_blobs` slots, and writes atomically (`degraded = false`).
    /// - When keyring is locked: NEVER wipes `sealed_slots` or writes plaintext; preserves
    ///   every existing sealed slot in `retained_blobs` as well as `sealed_slots`, writes
    ///   the intact ciphertext envelope, and returns `degraded = true`.
    pub fn save_non_destructive(
        &self,
        updates: &[(String, String, ProtectedSecret)],
        now_ms: i64,
    ) -> Result<SecretPersistenceOutcome> {
        let key_state = self.keyring.master_key_state();
        let mut guard = self.envelope.lock();

        match key_state {
            KeyringState::Available(key) => {
                for (slot, aad, secret) in updates {
                    let slot_key = validate_non_empty_key("slot", slot)?;
                    let aad_key = validate_non_empty_key("aad_label", aad)?;
                    let sealed = seal_protected_secret(&key, &aad_key, secret)?;
                    guard.sealed_slots.insert(slot_key.clone(), sealed);
                    guard.retained_blobs.remove(&slot_key);
                }
                guard.updated_at_ms = now_ms;
                self.persist_envelope(&guard)?;
                Ok(SecretPersistenceOutcome {
                    degraded: false,
                    sealed_count: guard.sealed_slots.len(),
                    retained_count: guard.retained_blobs.len(),
                })
            }
            KeyringState::UnavailableLocked => {
                let existing: Vec<(String, SealedProtectedSecret)> = guard
                    .sealed_slots
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                for (k, v) in existing {
                    guard.retained_blobs.entry(k).or_insert(v);
                }
                guard.updated_at_ms = now_ms;
                self.persist_envelope(&guard)?;
                Ok(SecretPersistenceOutcome {
                    degraded: true,
                    sealed_count: guard.sealed_slots.len(),
                    retained_count: guard.retained_blobs.len(),
                })
            }
        }
    }

    /// Deletes a secret slot when the keyring is unlocked (never wipes while locked).
    pub fn remove_secret(&self, slot: &str, now_ms: i64) -> Result<bool> {
        let slot_key = validate_non_empty_key("slot", slot)?;
        if self.is_degraded() {
            return Err(OrcaIdeRuntimeError::permission_denied(
                "keyring unavailable/locked: refusing destructive secret deletion",
            ));
        }
        let mut guard = self.envelope.lock();
        let removed_a = guard.sealed_slots.remove(&slot_key).is_some();
        let removed_b = guard.retained_blobs.remove(&slot_key).is_some();
        if removed_a || removed_b {
            guard.updated_at_ms = now_ms;
            self.persist_envelope(&guard)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn raw_sealed_slot(&self, slot: &str) -> Option<SealedProtectedSecret> {
        let guard = self.envelope.lock();
        guard
            .sealed_slots
            .get(slot)
            .or_else(|| guard.retained_blobs.get(slot))
            .cloned()
    }

    pub fn sealed_slot_count(&self) -> usize {
        self.envelope.lock().sealed_slots.len()
    }

    pub fn retained_blob_count(&self) -> usize {
        self.envelope.lock().retained_blobs.len()
    }
}

impl ProfileStateAuthority {
    /// Convenience single-domain write using the current wall-clock timestamp.
    pub fn put_domain(
        &self,
        domain: &str,
        payload: serde_json::Value,
        expected_revision: Option<u64>,
    ) -> Result<ProfileStateDocument> {
        let mut docs = self.write_dirty_domains(
            &[DirtyDomainMutation {
                domain: domain.to_string(),
                expected_revision,
                domain_version: 1,
                payload,
            }],
            now_epoch_ms(),
        )?;
        docs.pop().ok_or_else(|| {
            OrcaIdeRuntimeError::invalid_argument("empty dirty domain mutation result")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn profile_state_dirty_writes_revision_conflict_parts_and_automation_runs() {
        assert_send_sync::<ProfileStateAuthority>();
        assert_send_sync::<AtomicSnapshotWriter>();
        assert_send_sync::<ProtectedSecretPersistence>();

        let dir = TempDir::new().unwrap();
        let auth = ProfileStateAuthority::open(dir.path().join("profile_state.db")).unwrap();

        let written = auth
            .write_dirty_domains(
                &[
                    DirtyDomainMutation {
                        domain: "worktreeMeta".to_string(),
                        expected_revision: Some(0),
                        domain_version: 1,
                        payload: json!({"wt-1": {"pinned": true}}),
                    },
                    DirtyDomainMutation {
                        domain: "ui".to_string(),
                        expected_revision: Some(0),
                        domain_version: 2,
                        payload: json!({"theme": "midnight"}),
                    },
                ],
                1_700_000_000_000,
            )
            .unwrap();
        assert_eq!(written.len(), 2);
        assert_eq!(written[0].revision, 1);
        assert_eq!(written[1].revision, 1);

        // Update only `ui`; `worktreeMeta` must stay untouched at revision 1.
        let updated = auth
            .write_dirty_domains(
                &[DirtyDomainMutation {
                    domain: "ui".to_string(),
                    expected_revision: Some(1),
                    domain_version: 2,
                    payload: json!({"theme": "solarized"}),
                }],
                1_700_000_005_000,
            )
            .unwrap();
        assert_eq!(updated[0].revision, 2);
        assert_eq!(
            auth.get_domain("worktreeMeta").unwrap().unwrap().revision,
            1
        );

        // Stale expected_revision must fail with ProfileStateRevisionConflict and roll back.
        let err = auth
            .write_dirty_domains(
                &[
                    DirtyDomainMutation {
                        domain: "worktreeMeta".to_string(),
                        expected_revision: Some(1),
                        domain_version: 1,
                        payload: json!({"wt-1": {"pinned": false}}),
                    },
                    DirtyDomainMutation {
                        domain: "ui".to_string(),
                        expected_revision: Some(1), // stale; current is 2
                        domain_version: 2,
                        payload: json!({"theme": "stale"}),
                    },
                ],
                1_700_000_010_000,
            )
            .unwrap_err();
        match err {
            OrcaIdeRuntimeError::ProfileStateRevisionConflict {
                domain,
                expected,
                current,
            } => {
                assert_eq!(domain, "ui");
                assert_eq!(expected, 1);
                assert_eq!(current, 2);
            }
            other => panic!("expected ProfileStateRevisionConflict, got {other:?}"),
        }
        // Rollback preserved worktreeMeta at revision 1.
        assert_eq!(
            auth.get_domain("worktreeMeta").unwrap().unwrap().revision,
            1
        );

        // Document parts & automation runs CRUD.
        auth.put_document_part("repos", "repo-b", 2, json!({"name": "b"}))
            .unwrap();
        auth.put_document_part("repos", "repo-a", 1, json!({"name": "a"}))
            .unwrap();
        let parts = auth.list_document_parts("repos").unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].part_key, "repo-a");
        assert!(auth.delete_document_part("repos", "repo-b").unwrap());
        assert_eq!(auth.list_document_parts("repos").unwrap().len(), 1);

        auth.record_automation_run(ProfileStateAutomationRun {
            run_id: "run-1".to_string(),
            automation_id: "auto-pr".to_string(),
            bot_id: "bot-1".to_string(),
            status: "succeeded".to_string(),
            started_at: 1000,
            finished_at: Some(2000),
            summary_json: json!({"prs_checked": 3}),
        })
        .unwrap();
        let runs = auth.list_automation_runs(Some("auto-pr"), 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "run-1");
        auth.run_incremental_vacuum(4).unwrap();
    }

    #[test]
    fn atomic_snapshot_writer_fences_stale_async_and_unlinks_temp_on_sync_flush() {
        let dir = TempDir::new().unwrap();
        let writer = AtomicSnapshotWriter::new(dir.path().join("state.snap.json"));

        let snap_v1 = AtomicSnapshot::new(json!({"step": "async-v1"}), 1000).unwrap();
        let staged_async = writer.begin_async_stage(&snap_v1).unwrap();
        assert!(staged_async.temp_path.is_file());

        // Synchronous quit flush occurs before the slow async commit completes.
        let snap_sync = snap_v1
            .next_version(1, json!({"step": "sync-quit-v2"}), 2000)
            .unwrap();
        let sync_gen = writer.flush_sync(&snap_sync).unwrap();
        assert!(sync_gen > staged_async.generation);
        // Stale async temp file was unlinked during flush_sync.
        assert!(!staged_async.temp_path.exists());

        // Late async commit must fail with VaultError::StaleGeneration and NOT overwrite v2.
        let err = writer.commit_async(staged_async).unwrap_err();
        assert!(matches!(
            err,
            OrcaIdeRuntimeError::Vault(VaultError::StaleGeneration { .. })
        ));

        let loaded: AtomicSnapshot<serde_json::Value> =
            writer.read_verified().unwrap().unwrap();
        assert_eq!(loaded.version, 2);
        assert_eq!(loaded.payload, json!({"step": "sync-quit-v2"}));
    }

    #[test]
    fn open_versioned_cache_db_rebuilds_corrupt_and_stale_user_version() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("cache.db");
        let init_sql = "CREATE TABLE IF NOT EXISTS items (k TEXT PRIMARY KEY, v TEXT);";

        // 1. Corrupt bytes on disk -> rebuilt cleanly.
        fs::write(&db_path, b"NOT_A_SQLITE_DB_CORRUPT_GARBAGE_BYTES").unwrap();
        let (conn, rebuilt) = open_versioned_cache_db(&db_path, 8, init_sql).unwrap();
        assert!(rebuilt);
        let ver: i64 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, 8);
        drop(conn);

        // 2. Healthy DB with matching version -> NOT rebuilt.
        let (conn2, rebuilt2) = open_versioned_cache_db(&db_path, 8, init_sql).unwrap();
        assert!(!rebuilt2);
        drop(conn2);

        // 3. Stale user_version (expected 9 vs on-disk 8) -> rebuilt cleanly.
        let (conn3, rebuilt3) = open_versioned_cache_db(&db_path, 9, init_sql).unwrap();
        assert!(rebuilt3);
        let ver3: i64 = conn3
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver3, 9);
    }

    #[test]
    fn protected_secret_persistence_preserves_ciphertext_when_keyring_locked() {
        let dir = TempDir::new().unwrap();
        let secret_path = dir.path().join("secrets.json");
        let key = [0x42u8; 32];
        let keyring = Arc::new(InMemoryKeyringProvider::new_available(key));

        let store = ProtectedSecretPersistence::open(&secret_path, keyring.clone()).unwrap();
        let secret = ProtectedSecret::from_string("super-secret-edit-token-99".to_string());
        let sealed = store
            .store_secret("artifact:art-1", "artifact:art-1", &secret, 1000)
            .unwrap();

        // Lock keyring -> degraded mode.
        keyring.lock();
        assert!(store.is_degraded());

        // Plain store_secret must refuse downgrade to plaintext.
        assert!(
            store
                .store_secret("artifact:art-2", "artifact:art-2", &secret, 2000)
                .is_err()
        );
        // Non-destructive save must report degraded=true and retain prior ciphertext!
        let outcome = store
            .save_non_destructive(
                &[(
                    "artifact:art-2".to_string(),
                    "artifact:art-2".to_string(),
                    ProtectedSecret::from_string("never-written-plaintext".to_string()),
                )],
                2000,
            )
            .unwrap();
        assert!(outcome.degraded);
        assert_eq!(outcome.sealed_count, 1);
        assert_eq!(outcome.retained_count, 1);

        let disk_raw = fs::read_to_string(&secret_path).unwrap();
        assert!(!disk_raw.contains("super-secret-edit-token-99"));
        assert!(!disk_raw.contains("never-written-plaintext"));
        assert!(disk_raw.contains(&sealed.ciphertext_b64));

        // Unlock keyring again and reopen from disk -> prior secret decrypts intact!
        keyring.set_available(key);
        let reopened = ProtectedSecretPersistence::open(&secret_path, keyring).unwrap();
        let opened = reopened
            .read_secret("artifact:art-1", "artifact:art-1")
            .unwrap()
            .unwrap();
        assert_eq!(opened.expose_bytes(), b"super-secret-edit-token-99");
    }
}
