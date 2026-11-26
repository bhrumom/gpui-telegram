//! AI Vault Cross-Agent Transcript Search over SQLite FTS5 (`F16`).
//!
//! Opened via [`crate::profile_state::open_versioned_cache_db`] at schema v8 so
//! corrupt or stale cache DBs auto-rebuild seamlessly. Provides `(byte_offset, prefix_hash)`
//! incremental transcript ingestion, camelCase/snake_case identifier splitting, `bm25()`
//! ranked search with `(generation, incarnation)` fenced cursors, Damerau-Levenshtein
//! typo fuzzy repair over `vault_vocab`, retention pruning, tool-output purging,
//! conversation forking, and cross-agent handoff prompt synthesis.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::profile_state::open_versioned_cache_db;
use crate::{OrcaIdeRuntimeError, Result, sha256_hex};

/// Authoritative SQLite `PRAGMA user_version` for the AI Vault FTS5 cache.
pub const VAULT_SCHEMA_VERSION: u32 = 8;

const VAULT_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS vault_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS vault_indexed_files (
    file_path TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    byte_offset INTEGER NOT NULL DEFAULT 0,
    prefix_hash TEXT NOT NULL,
    anchor_prefix_len INTEGER NOT NULL DEFAULT 0,
    anchor_prefix_hash TEXT NOT NULL DEFAULT '',
    generation INTEGER NOT NULL DEFAULT 0,
    incarnation TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS vault_messages (
    message_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_output TEXT,
    seq INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_vault_messages_session_seq
    ON vault_messages(session_id, seq ASC);

CREATE INDEX IF NOT EXISTS idx_vault_messages_created
    ON vault_messages(created_at_ms ASC);

CREATE TABLE IF NOT EXISTS vault_vocab (
    term TEXT PRIMARY KEY,
    doc_freq INTEGER NOT NULL DEFAULT 1
);

CREATE VIRTUAL TABLE IF NOT EXISTS vault_session_fts USING fts5(
    session_id UNINDEXED,
    agent_id UNINDEXED,
    workspace_id UNINDEXED,
    user_text,
    assistant_text,
    tool_text,
    identifiers,
    tokenize="unicode61 tokenchars '_.-/+'",
    detail=full
);
"#;

/// Input message for direct programmatic/RPC indexing into the AI Vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultMessageInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<i64>,
}

/// Persisted row in `vault_messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultMessageRecord {
    pub message_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<String>,
    pub seq: i64,
    pub created_at_ms: i64,
}

/// Summary returned by [`AiVaultSearchEngine::index_transcript_file`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultIndexFileReport {
    pub file_path: String,
    pub session_id: String,
    pub reindexed_from_zero: bool,
    pub incremental_append: bool,
    pub incremental: bool,
    pub bytes_indexed: usize,
    pub byte_offset: usize,
    pub new_byte_offset: usize,
    pub prefix_hash: String,
    pub messages_added: usize,
    pub generation: u64,
    pub incarnation: String,
}

/// Generation + incarnation fenced pagination cursor for ranked FTS5 search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultSearchCursor {
    pub generation: u64,
    pub incarnation: String,
    pub offset: usize,
}

/// Search parameters for [`AiVaultSearchEngine::search`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultSearchRequest {
    #[serde(alias = "q")]
    pub query: String,
    #[serde(default, alias = "workspace", alias = "workspacePath", alias = "workspace_path", skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, alias = "agent", alias = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<VaultSearchCursor>,
}

const fn default_search_limit() -> usize {
    20
}

/// Single ranked search result from `vault_session_fts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultSearchHit {
    pub session_id: String,
    pub agent_id: String,
    pub agent: String,
    pub workspace_id: String,
    pub rank: f64,
    pub snippet: String,
    pub matched_identifiers: Vec<String>,
}

/// Ranked search result envelope with fenced cursor & optional typo repair detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultSearchResponse {
    pub hits: Vec<VaultSearchHit>,
    pub generation: u64,
    pub incarnation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<VaultSearchCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuzzy_repaired_query: Option<String>,
}

/// Splits camelCase, PascalCase, snake_case, kebab-case, and dot/path code tokens
/// into normalized lowercase searchable identifier fragments plus compound stems.
pub fn split_code_identifiers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();

    let mut push_tok = |raw: &str| {
        let lower = raw.to_ascii_lowercase();
        if lower.len() >= 2 && lower.chars().any(|c| c.is_alphanumeric()) && seen.insert(lower.clone())
        {
            out.push(lower);
        }
    };

    for raw_word in text.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+'))) {
        if raw_word.is_empty() {
            continue;
        }
        push_tok(raw_word);

        for segment in raw_word.split(['_', '-', '.', '/', '+']) {
            if segment.is_empty() {
                continue;
            }
            push_tok(segment);

            // Split camelCase / PascalCase / ACRONYMWord boundaries.
            let chars: Vec<char> = segment.chars().collect();
            let mut start = 0usize;
            for idx in 1..chars.len() {
                let prev = chars[idx - 1];
                let curr = chars[idx];
                let next_is_lower = chars.get(idx + 1).is_some_and(|c| c.is_lowercase());
                let split_here = (prev.is_lowercase() && curr.is_uppercase())
                    || (prev.is_alphabetic() && curr.is_ascii_digit())
                    || (prev.is_ascii_digit() && curr.is_alphabetic())
                    || (prev.is_uppercase() && curr.is_uppercase() && next_is_lower);
                if split_here {
                    let piece: String = chars[start..idx].iter().collect();
                    push_tok(&piece);
                    start = idx;
                }
            }
            if start > 0 && start < chars.len() {
                let piece: String = chars[start..].iter().collect();
                push_tok(&piece);
            }
        }
    }

    out
}

/// Computes optimal string alignment / Damerau-Levenshtein edit distance (including
/// adjacent character transpositions) over Unicode scalar values.
pub fn damerau_levenshtein_distance(a: &str, b: &str) -> usize {
    let sa: Vec<char> = a.chars().collect();
    let sb: Vec<char> = b.chars().collect();
    let m = sa.len();
    let n = sb.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in dp.iter_mut().enumerate().take(m + 1) {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate().take(n + 1) {
        *cell = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            let cost = usize::from(sa[i - 1] != sb[j - 1]);
            let mut best = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
            if i > 1
                && j > 1
                && sa[i - 1] == sb[j - 2]
                && sa[i - 2] == sb[j - 1]
            {
                best = best.min(dp[i - 2][j - 2] + 1);
            }
            dp[i][j] = best;
        }
    }
    dp[m][n]
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct VaultSourceManifest {
    #[serde(default)]
    opt_in: bool,
    #[serde(default)]
    sources: BTreeMap<String, VaultSourceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultSourceRecord {
    session_id: String,
    agent_id: String,
    workspace_id: String,
    file_path: String,
}

/// Cross-agent SQLite FTS5 transcript search engine (`Send + Sync`).
pub struct AiVaultSearchEngine {
    db_path: Option<PathBuf>,
    conn: Mutex<Connection>,
    rebuilt_on_open: bool,
    rebuild_count: std::sync::atomic::AtomicU64,
}

impl fmt::Debug for AiVaultSearchEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiVaultSearchEngine")
            .field("db_path", &self.db_path)
            .field("rebuilt_on_open", &self.rebuilt_on_open)
            .field(
                "rebuild_count",
                &self.rebuild_count.load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish()
    }
}

fn ensure_vault_meta(conn: &Connection) -> Result<(u64, String)> {
    let incarnation: String = match conn
        .query_row(
            "SELECT value FROM vault_meta WHERE key = 'incarnation'",
            [],
            |r| r.get(0),
        )
        .optional()?
    {
        Some(existing) => existing,
        None => {
            let fresh = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO vault_meta (key, value) VALUES ('incarnation', ?1)",
                params![&fresh],
            )?;
            fresh
        }
    };
    let generation: u64 = match conn
        .query_row(
            "SELECT value FROM vault_meta WHERE key = 'generation'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        Some(raw) => raw.parse::<u64>().unwrap_or(0),
        None => {
            conn.execute(
                "INSERT INTO vault_meta (key, value) VALUES ('generation', '0')",
                [],
            )?;
            0
        }
    };
    Ok((generation, incarnation))
}

fn bump_generation(conn: &Connection) -> Result<(u64, String)> {
    let (current, incarnation) = ensure_vault_meta(conn)?;
    let next = current.saturating_add(1);
    conn.execute(
        "INSERT INTO vault_meta (key, value) VALUES ('generation', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![next.to_string()],
    )?;
    Ok((next, incarnation))
}

fn parse_transcript_chunk(chunk: &[u8], default_ts: i64) -> Vec<VaultMessageInput> {
    let text = String::from_utf8_lossy(chunk);
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('{')
            && let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
        {
            let role = val
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let content = val
                .get("content")
                .or_else(|| val.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let tool_output = val
                .get("tool_output")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned);
            let message_id = val
                .get("message_id")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned);
            let created_at_ms = val
                .get("created_at_ms")
                .and_then(|v| v.as_i64())
                .or(Some(default_ts));
            if !content.is_empty() || tool_output.is_some() {
                out.push(VaultMessageInput {
                    message_id,
                    role,
                    content,
                    tool_output,
                    created_at_ms,
                });
                continue;
            }
        }
        if let Some((prefix, rest)) = trimmed.split_once(':') {
            let norm = prefix.trim().to_ascii_lowercase();
            if matches!(norm.as_str(), "user" | "assistant" | "tool" | "system") {
                let body = rest.trim().to_string();
                let (content, tool_output) = if norm == "tool" {
                    (String::new(), Some(body))
                } else {
                    (body, None)
                };
                out.push(VaultMessageInput {
                    message_id: None,
                    role: norm,
                    content,
                    tool_output,
                    created_at_ms: Some(default_ts),
                });
                continue;
            }
        }
        out.push(VaultMessageInput {
            message_id: None,
            role: "user".to_string(),
            content: trimmed.to_string(),
            tool_output: None,
            created_at_ms: Some(default_ts),
        });
    }
    out
}

fn rebuild_session_fts_and_vocab(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM vault_session_fts WHERE session_id = ?1",
        params![session_id],
    )?;

    let mut stmt = conn.prepare(
        "SELECT agent_id, workspace_id, role, content, tool_output
         FROM vault_messages
         WHERE session_id = ?1
         ORDER BY seq ASC",
    )?;
    let mut agent_id = String::new();
    let mut workspace_id = String::new();
    let mut user_parts = Vec::new();
    let mut assistant_parts = Vec::new();
    let mut tool_parts = Vec::new();
    let mut found_any = false;

    let rows = stmt.query_map(params![session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;

    for row in rows {
        let (ag, ws, role, content, tool_output) = row?;
        found_any = true;
        agent_id = ag;
        workspace_id = ws;
        match role.to_ascii_lowercase().as_str() {
            "assistant" => {
                if !content.is_empty() {
                    assistant_parts.push(content);
                }
            }
            "tool" => {
                if !content.is_empty() {
                    tool_parts.push(content);
                }
            }
            _ => {
                if !content.is_empty() {
                    user_parts.push(content);
                }
            }
        }
        if let Some(tool) = tool_output
            && !tool.is_empty()
        {
            tool_parts.push(tool);
        }
    }

    if !found_any {
        return Ok(());
    }

    let user_text = user_parts.join("\n");
    let assistant_text = assistant_parts.join("\n");
    let tool_text = tool_parts.join("\n");
    let combined = format!("{user_text}\n{assistant_text}\n{tool_text}");
    let idents = split_code_identifiers(&combined);
    let identifiers = idents.join(" ");

    conn.execute(
        "INSERT INTO vault_session_fts
            (session_id, agent_id, workspace_id, user_text, assistant_text, tool_text, identifiers)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            session_id,
            &agent_id,
            &workspace_id,
            &user_text,
            &assistant_text,
            &tool_text,
            &identifiers,
        ],
    )?;

    // Upsert unique vocabulary terms for fuzzy typo repair.
    for term in idents {
        if term.len() <= 64 {
            conn.execute(
                "INSERT INTO vault_vocab (term, doc_freq) VALUES (?1, 1)
                 ON CONFLICT(term) DO UPDATE SET doc_freq = doc_freq + 1",
                params![term],
            )?;
        }
    }
    Ok(())
}

impl AiVaultSearchEngine {
    fn manifest_path_for(db_path: &Path) -> PathBuf {
        db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("ai_vault_sources.json")
    }

    fn load_source_manifest(db_path: &Path) -> VaultSourceManifest {
        let p = Self::manifest_path_for(db_path);
        fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn persist_source_manifest(&self, mutate: impl FnOnce(&mut VaultSourceManifest)) {
        let Some(ref db_p) = self.db_path else {
            return;
        };
        let mut m = Self::load_source_manifest(db_p);
        mutate(&mut m);
        if let Ok(bytes) = serde_json::to_vec_pretty(&m) {
            let _ = fs::write(Self::manifest_path_for(db_p), bytes);
        }
    }

    /// Opens the AI Vault FTS5 SQLite cache via [`open_versioned_cache_db`] at schema v8.
    /// Automatically rebuilds corrupt or stale-schema DBs once and replays recorded sources (`SM1-13`).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        let (conn, rebuilt) =
            open_versioned_cache_db(&db_path, VAULT_SCHEMA_VERSION, VAULT_SCHEMA_SQL)?;
        // Safe additive migrations for pre-existing v8 DBs.
        let _ = conn.execute(
            "ALTER TABLE vault_indexed_files ADD COLUMN anchor_prefix_len INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE vault_indexed_files ADD COLUMN anchor_prefix_hash TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = ensure_vault_meta(&conn)?;
        let engine = Self {
            db_path: Some(db_path.clone()),
            conn: Mutex::new(conn),
            rebuilt_on_open: rebuilt,
            rebuild_count: std::sync::atomic::AtomicU64::new(u64::from(rebuilt)),
        };
        if rebuilt {
            let manifest = Self::load_source_manifest(&db_path);
            if manifest.opt_in {
                let _ = engine.set_opt_in(true);
            }
            let now_ms = crate::now_epoch_ms();
            for src in manifest.sources.values() {
                let fp = Path::new(&src.file_path);
                if fp.is_file() {
                    let _ = engine.index_transcript_file(
                        fp,
                        &src.session_id,
                        &src.agent_id,
                        &src.workspace_id,
                        now_ms,
                    );
                }
            }
        }
        Ok(engine)
    }

    /// Returns the on-disk cache SQLite path if durable.
    pub fn cache_db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    /// Returns how many times the corrupt FTS5 cache has been auto-rebuilt on startup.
    pub fn rebuild_count(&self) -> u64 {
        self.rebuild_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Persists user opt-in state for AI Vault indexing in `vault_meta`.
    pub fn set_opt_in(&self, enabled: bool) -> Result<bool> {
        {
            let conn = self.conn.lock();
            conn.execute(
                "INSERT INTO vault_meta (key, value) VALUES ('opt_in', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![if enabled { "true" } else { "false" }],
            )?;
        }
        self.persist_source_manifest(|m| m.opt_in = enabled);
        Ok(enabled)
    }

    /// Reads persisted opt-in state from `vault_meta` (defaults to `false`).
    pub fn is_opt_in(&self) -> Result<bool> {
        let conn = self.conn.lock();
        let val: Option<String> = conn
            .query_row(
                "SELECT value FROM vault_meta WHERE key = 'opt_in'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(val.as_deref() == Some("true"))
    }

    /// Returns `true` if opening this cache DB detected corruption or stale `user_version`
    /// and cleanly rebuilt the database.
    pub fn rebuilt_on_open(&self) -> bool {
        self.rebuilt_on_open
    }

    /// Returns current `(generation, incarnation)` fencing token pair.
    pub fn generation_and_incarnation(&self) -> Result<(u64, String)> {
        let conn = self.conn.lock();
        ensure_vault_meta(&conn)
    }

    /// Incrementally ingests a transcript file using `(byte_offset, prefix_hash)`:
    /// - Appends ONLY new bytes if `file.len() >= prev_offset` and SHA-256 of `bytes[..prev_offset]`
    ///   matches `prefix_hash`.
    /// - Re-indexes from offset 0 if the file is new, truncated, or its prefix changed.
    pub fn index_transcript_file(
        &self,
        file_path: &Path,
        session_id: &str,
        agent_id: &str,
        workspace_id: &str,
        now_ms: i64,
    ) -> Result<VaultIndexFileReport> {
        if session_id.trim().is_empty() || agent_id.trim().is_empty() || workspace_id.trim().is_empty()
        {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "session_id, agent_id, and workspace_id must be non-empty",
            ));
        }
        let bytes = fs::read(file_path)?;
        let file_key = file_path.to_string_lossy().to_string();
        self.persist_source_manifest(|m| {
            m.sources.insert(
                session_id.to_string(),
                VaultSourceRecord {
                    session_id: session_id.to_string(),
                    agent_id: agent_id.to_string(),
                    workspace_id: workspace_id.to_string(),
                    file_path: file_key.clone(),
                },
            );
        });

        let mut conn = self.conn.lock();
        let prev: Option<(usize, String, usize, String)> = conn
            .query_row(
                "SELECT byte_offset, prefix_hash, anchor_prefix_len, anchor_prefix_hash
                 FROM vault_indexed_files WHERE file_path = ?1",
                params![&file_key],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?.max(0) as usize,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?.max(0) as usize,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;

        let full_hash = sha256_hex(&bytes);
        let new_offset = bytes.len();

        let (start_offset, reindexed_from_zero, incremental_append, anchor_len, anchor_hash) =
            match prev {
                Some((prev_offset, prev_hash, prev_anchor_len, prev_anchor_hash))
                    if prev_offset > 0 && bytes.len() >= prev_offset =>
                {
                    let current_prefix_hash = sha256_hex(&bytes[..prev_offset]);
                    if current_prefix_hash == prev_hash {
                        let (eff_anchor_len, eff_anchor_hash) =
                            if prev_anchor_len > 0 && !prev_anchor_hash.is_empty() {
                                (prev_anchor_len, prev_anchor_hash)
                            } else {
                                (prev_offset, prev_hash)
                            };
                        if bytes.len() == prev_offset {
                            let (generation, inc) = ensure_vault_meta(&conn)?;
                            return Ok(VaultIndexFileReport {
                                file_path: file_key,
                                session_id: session_id.to_string(),
                                reindexed_from_zero: false,
                                incremental_append: false,
                                incremental: false,
                                bytes_indexed: 0,
                                byte_offset: prev_offset,
                                new_byte_offset: prev_offset,
                                prefix_hash: eff_anchor_hash,
                                messages_added: 0,
                                generation,
                                incarnation: inc,
                            });
                        }
                        (prev_offset, false, true, eff_anchor_len, eff_anchor_hash)
                    } else {
                        (0, true, false, new_offset, full_hash.clone())
                    }
                }
                _ => (0, true, false, new_offset, full_hash.clone()),
            };

        let parsed = parse_transcript_chunk(&bytes[start_offset..], now_ms);

        let tx = conn.transaction()?;
        if reindexed_from_zero {
            tx.execute(
                "DELETE FROM vault_messages WHERE session_id = ?1",
                params![session_id],
            )?;
        }

        let mut next_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM vault_messages WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )?;

        let mut added = 0usize;
        for msg in parsed {
            next_seq += 1;
            let msg_id = msg
                .message_id
                .unwrap_or_else(|| format!("msg-{}", Uuid::new_v4()));
            let created = msg.created_at_ms.unwrap_or(now_ms);
            tx.execute(
                "INSERT OR REPLACE INTO vault_messages
                    (message_id, session_id, agent_id, workspace_id, role, content, tool_output, seq, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    &msg_id,
                    session_id,
                    agent_id,
                    workspace_id,
                    &msg.role,
                    &msg.content,
                    &msg.tool_output,
                    next_seq,
                    created,
                ],
            )?;
            added += 1;
        }

        rebuild_session_fts_and_vocab(&tx, session_id)?;
        let (generation, incarnation) = bump_generation(&tx)?;
        tx.execute(
            "INSERT INTO vault_indexed_files
                (file_path, session_id, agent_id, workspace_id, byte_offset, prefix_hash, anchor_prefix_len, anchor_prefix_hash, generation, incarnation, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(file_path) DO UPDATE SET
                session_id = excluded.session_id,
                agent_id = excluded.agent_id,
                workspace_id = excluded.workspace_id,
                byte_offset = excluded.byte_offset,
                prefix_hash = excluded.prefix_hash,
                anchor_prefix_len = excluded.anchor_prefix_len,
                anchor_prefix_hash = excluded.anchor_prefix_hash,
                generation = excluded.generation,
                incarnation = excluded.incarnation,
                updated_at_ms = excluded.updated_at_ms",
            params![
                &file_key,
                session_id,
                agent_id,
                workspace_id,
                new_offset as i64,
                &full_hash,
                anchor_len as i64,
                &anchor_hash,
                generation as i64,
                &incarnation,
                now_ms,
            ],
        )?;
        tx.commit()?;

        Ok(VaultIndexFileReport {
            file_path: file_key,
            session_id: session_id.to_string(),
            reindexed_from_zero,
            incremental_append,
            incremental: incremental_append,
            bytes_indexed: new_offset.saturating_sub(start_offset),
            byte_offset: new_offset,
            new_byte_offset: new_offset,
            prefix_hash: anchor_hash,
            messages_added: added,
            generation,
            incarnation,
        })
    }

    /// Directly appends structured messages for a session and updates FTS5 + `vault_vocab`.
    pub fn index_messages(
        &self,
        session_id: &str,
        agent_id: &str,
        workspace_id: &str,
        messages: &[VaultMessageInput],
        now_ms: i64,
    ) -> Result<(u64, String, usize)> {
        if session_id.trim().is_empty() || agent_id.trim().is_empty() || workspace_id.trim().is_empty()
        {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "session_id, agent_id, and workspace_id must be non-empty",
            ));
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut next_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM vault_messages WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )?;
        for msg in messages {
            next_seq += 1;
            let msg_id = msg
                .message_id
                .clone()
                .unwrap_or_else(|| format!("msg-{}", Uuid::new_v4()));
            let created = msg.created_at_ms.unwrap_or(now_ms);
            tx.execute(
                "INSERT OR REPLACE INTO vault_messages
                    (message_id, session_id, agent_id, workspace_id, role, content, tool_output, seq, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    &msg_id,
                    session_id,
                    agent_id,
                    workspace_id,
                    &msg.role,
                    &msg.content,
                    &msg.tool_output,
                    next_seq,
                    created,
                ],
            )?;
        }
        rebuild_session_fts_and_vocab(&tx, session_id)?;
        let (generation, inc) = bump_generation(&tx)?;
        tx.commit()?;
        Ok((generation, inc, messages.len()))
    }
}

fn build_fts5_match_expr(tokens: &[String]) -> Option<String> {
    let sanitized: Vec<String> = tokens
        .iter()
        .map(|t| t.replace('"', "").trim().to_string())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized.join(" "))
    }
}

fn query_fts5_hits(
    conn: &Connection,
    fts_expr: &str,
    workspace_id: Option<&str>,
    agent_id: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Vec<VaultSearchHit>> {
    let mut sql = String::from(
        "SELECT session_id, agent_id, workspace_id,
                bm25(vault_session_fts, 0.0, 0.0, 0.0, 2.0, 1.5, 0.5, 2.5) AS score,
                snippet(vault_session_fts, -1, '[', ']', '...', 18) AS snip,
                identifiers
         FROM vault_session_fts
         WHERE vault_session_fts MATCH ?1",
    );
    let mut bind_vals: Vec<rusqlite::types::Value> = vec![fts_expr.to_string().into()];
    if let Some(ws) = workspace_id {
        bind_vals.push(ws.to_string().into());
        sql.push_str(&format!(" AND workspace_id = ?{}", bind_vals.len()));
    }
    if let Some(ag) = agent_id {
        bind_vals.push(ag.to_string().into());
        sql.push_str(&format!(" AND agent_id = ?{}", bind_vals.len()));
    }
    sql.push_str(" ORDER BY score ASC, session_id ASC");
    bind_vals.push((limit as i64).into());
    sql.push_str(&format!(" LIMIT ?{}", bind_vals.len()));
    bind_vals.push((offset as i64).into());
    sql.push_str(&format!(" OFFSET ?{}", bind_vals.len()));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(bind_vals), |row| {
        let idents_raw: String = row.get(5)?;
        let matched_identifiers: Vec<String> = idents_raw
            .split_whitespace()
            .take(16)
            .map(ToOwned::to_owned)
            .collect();
        let agent_id: String = row.get(1)?;
        Ok(VaultSearchHit {
            session_id: row.get(0)?,
            agent: agent_id.clone(),
            agent_id,
            workspace_id: row.get(2)?,
            rank: row.get(3)?,
            snippet: row.get(4)?,
            matched_identifiers,
        })
    })?;
    let mut out = Vec::new();
    for item in rows {
        out.push(item?);
    }
    Ok(out)
}

fn repair_typo_tokens(conn: &Connection, query_tokens: &[String]) -> Result<Option<Vec<String>>> {
    let mut stmt = conn.prepare("SELECT term, doc_freq FROM vault_vocab")?;
    let vocab: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if vocab.is_empty() {
        return Ok(None);
    }
    let mut changed = false;
    let mut repaired = Vec::with_capacity(query_tokens.len());
    for token in query_tokens {
        let max_dist = if token.chars().count() <= 5 { 1 } else { 2 };
        let mut best: Option<(&str, usize, i64)> = None;
        for (term, freq) in &vocab {
            if term == token {
                best = Some((term.as_str(), 0, *freq));
                break;
            }
            let len_diff = term.chars().count().abs_diff(token.chars().count());
            if len_diff > max_dist {
                continue;
            }
            let dist = damerau_levenshtein_distance(token, term);
            if dist <= max_dist {
                let replace = match best {
                    None => true,
                    Some((_, best_dist, best_freq)) => {
                        dist < best_dist || (dist == best_dist && *freq > best_freq)
                    }
                };
                if replace {
                    best = Some((term.as_str(), dist, *freq));
                }
            }
        }
        if let Some((term, dist, _)) = best {
            if dist > 0 {
                changed = true;
            }
            repaired.push(term.to_string());
        } else {
            repaired.push(token.clone());
        }
    }
    if changed {
        Ok(Some(repaired))
    } else {
        Ok(None)
    }
}

impl AiVaultSearchEngine {
    /// Ranked FTS5 search (`bm25()`) with `(generation, incarnation)` fenced cursors
    /// and automatic Damerau-Levenshtein typo fuzzy repair when exact search returns 0 rows.
    pub fn search(&self, req: &VaultSearchRequest) -> Result<VaultSearchResponse> {
        let conn = self.conn.lock();
        let (generation, incarnation) = ensure_vault_meta(&conn)?;
        let offset = if let Some(cursor) = &req.cursor {
            if cursor.generation != generation || cursor.incarnation != incarnation {
                return Err(OrcaIdeRuntimeError::conflict(format!(
                    "stale vault search cursor: expected ({generation}, {incarnation}), got ({}, {})",
                    cursor.generation, cursor.incarnation
                )));
            }
            cursor.offset
        } else {
            0
        };

        let limit = req.limit.clamp(1, 200);
        let raw_tokens: Vec<String> = req
            .query
            .split_whitespace()
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        let Some(exact_expr) = build_fts5_match_expr(&raw_tokens) else {
            return Ok(VaultSearchResponse {
                hits: Vec::new(),
                generation,
                incarnation,
                next_cursor: None,
                fuzzy_repaired_query: None,
            });
        };

        let mut fuzzy_repaired_query = None;
        let mut hits = query_fts5_hits(
            &conn,
            &exact_expr,
            req.workspace_id.as_deref(),
            req.agent_id.as_deref(),
            limit + 1,
            offset,
        )?;

        if hits.is_empty() && offset == 0 {
            // Try split code identifiers first (stripping unsplit compound terms that already
            // missed exact search), then Damerau-Levenshtein typo repair.
            let mut split_tokens = Vec::new();
            for raw_word in req.query.split_whitespace() {
                let w_lower = raw_word.trim().to_ascii_lowercase();
                if w_lower.is_empty() {
                    continue;
                }
                let sub: Vec<String> = split_code_identifiers(raw_word)
                    .into_iter()
                    .filter(|t| t != &w_lower)
                    .collect();
                if !sub.is_empty() {
                    split_tokens.extend(sub);
                } else {
                    split_tokens.push(w_lower);
                }
            }
            if !split_tokens.is_empty()
                && split_tokens != raw_tokens
                && let Some(split_expr) = build_fts5_match_expr(&split_tokens)
            {
                hits = query_fts5_hits(
                    &conn,
                    &split_expr,
                    req.workspace_id.as_deref(),
                    req.agent_id.as_deref(),
                    limit + 1,
                    0,
                )?;
            }
            if hits.is_empty()
                && let Some(repaired) = repair_typo_tokens(&conn, &raw_tokens)?
                && let Some(repaired_expr) = build_fts5_match_expr(&repaired)
            {
                let fuzzy_hits = query_fts5_hits(
                    &conn,
                    &repaired_expr,
                    req.workspace_id.as_deref(),
                    req.agent_id.as_deref(),
                    limit + 1,
                    0,
                )?;
                if !fuzzy_hits.is_empty() {
                    fuzzy_repaired_query = Some(repaired.join(" "));
                    hits = fuzzy_hits;
                }
            }
        }

        let has_more = hits.len() > limit;
        if has_more {
            hits.truncate(limit);
        }
        let next_cursor = if has_more {
            Some(VaultSearchCursor {
                generation,
                incarnation: incarnation.clone(),
                offset: offset + hits.len(),
            })
        } else {
            None
        };

        Ok(VaultSearchResponse {
            hits,
            generation,
            incarnation,
            next_cursor,
            fuzzy_repaired_query,
        })
    }
}

impl AiVaultSearchEngine {
    /// Lists persisted messages for `session_id` in ascending sequence order.
    pub fn list_session_messages(&self, session_id: &str) -> Result<Vec<VaultMessageRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT message_id, session_id, agent_id, workspace_id, role, content, tool_output, seq, created_at_ms
             FROM vault_messages
             WHERE session_id = ?1
             ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok(VaultMessageRecord {
                message_id: r.get(0)?,
                session_id: r.get(1)?,
                agent_id: r.get(2)?,
                workspace_id: r.get(3)?,
                role: r.get(4)?,
                content: r.get(5)?,
                tool_output: r.get(6)?,
                seq: r.get(7)?,
                created_at_ms: r.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item?);
        }
        Ok(out)
    }

    /// Retention pruning: deletes all messages older than `cutoff_ms`, rebuilds affected
    /// FTS5 rows, removes empty file index entries, and bumps vault generation.
    pub fn delete_older_than(&self, cutoff_ms: i64) -> Result<usize> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        let affected_sessions: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT session_id FROM vault_messages WHERE created_at_ms < ?1",
            )?;
            stmt.query_map(params![cutoff_ms], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        let deleted = tx.execute(
            "DELETE FROM vault_messages WHERE created_at_ms < ?1",
            params![cutoff_ms],
        )?;

        for sid in &affected_sessions {
            rebuild_session_fts_and_vocab(&tx, sid)?;
            let remaining: i64 = tx.query_row(
                "SELECT COUNT(*) FROM vault_messages WHERE session_id = ?1",
                params![sid],
                |r| r.get(0),
            )?;
            if remaining == 0 {
                tx.execute(
                    "DELETE FROM vault_indexed_files WHERE session_id = ?1",
                    params![sid],
                )?;
            }
        }
        if deleted > 0 {
            let _ = bump_generation(&tx)?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    /// Purges stored tool outputs (either for a single `session_id` or across the entire
    /// vault), rebuilds affected FTS5 documents so purged tool noise is no longer searchable,
    /// and bumps vault generation. Returns the count of purged message rows.
    pub fn purge_tool_outputs(&self, session_id: Option<&str>) -> Result<usize> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        let affected_sessions: Vec<String> = if let Some(sid) = session_id {
            vec![sid.to_string()]
        } else {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT session_id FROM vault_messages
                 WHERE tool_output IS NOT NULL OR LOWER(role) = 'tool'",
            )?;
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        let purged = if let Some(sid) = session_id {
            let a = tx.execute(
                "UPDATE vault_messages SET tool_output = NULL
                 WHERE session_id = ?1 AND tool_output IS NOT NULL",
                params![sid],
            )?;
            let b = tx.execute(
                "UPDATE vault_messages SET content = ''
                 WHERE session_id = ?1 AND LOWER(role) = 'tool' AND content != ''",
                params![sid],
            )?;
            a + b
        } else {
            let a = tx.execute(
                "UPDATE vault_messages SET tool_output = NULL WHERE tool_output IS NOT NULL",
                [],
            )?;
            let b = tx.execute(
                "UPDATE vault_messages SET content = '' WHERE LOWER(role) = 'tool' AND content != ''",
                [],
            )?;
            a + b
        };

        for sid in &affected_sessions {
            rebuild_session_fts_and_vocab(&tx, sid)?;
        }
        if purged > 0 {
            let _ = bump_generation(&tx)?;
        }
        tx.commit()?;
        Ok(purged)
    }

    /// Forks a stored conversation from `source_session_id` (optionally up to `up_to_seq`)
    /// into `new_session_id` and indexes the forked conversation in FTS5.
    pub fn fork_conversation(
        &self,
        source_session_id: &str,
        new_session_id: &str,
        up_to_seq: Option<i64>,
        now_ms: i64,
    ) -> Result<Vec<VaultMessageRecord>> {
        if new_session_id.trim().is_empty() || source_session_id == new_session_id {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "new_session_id must be non-empty and distinct from source_session_id",
            ));
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        let source_msgs: Vec<VaultMessageRecord> = {
            let mut stmt = tx.prepare(
                "SELECT message_id, session_id, agent_id, workspace_id, role, content, tool_output, seq, created_at_ms
                 FROM vault_messages
                 WHERE session_id = ?1 AND (?2 IS NULL OR seq <= ?2)
                 ORDER BY seq ASC",
            )?;
            stmt.query_map(params![source_session_id, up_to_seq], |r| {
                Ok(VaultMessageRecord {
                    message_id: r.get(0)?,
                    session_id: r.get(1)?,
                    agent_id: r.get(2)?,
                    workspace_id: r.get(3)?,
                    role: r.get(4)?,
                    content: r.get(5)?,
                    tool_output: r.get(6)?,
                    seq: r.get(7)?,
                    created_at_ms: r.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };

        if source_msgs.is_empty() {
            return Err(OrcaIdeRuntimeError::not_found(format!(
                "source vault session '{source_session_id}' has no messages to fork"
            )));
        }

        let mut forked = Vec::with_capacity(source_msgs.len());
        for (idx, src) in source_msgs.into_iter().enumerate() {
            let seq = (idx + 1) as i64;
            let new_msg = VaultMessageRecord {
                message_id: format!("fork-{}", Uuid::new_v4()),
                session_id: new_session_id.to_string(),
                agent_id: src.agent_id,
                workspace_id: src.workspace_id,
                role: src.role,
                content: src.content,
                tool_output: src.tool_output,
                seq,
                created_at_ms: now_ms,
            };
            tx.execute(
                "INSERT INTO vault_messages
                    (message_id, session_id, agent_id, workspace_id, role, content, tool_output, seq, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    &new_msg.message_id,
                    &new_msg.session_id,
                    &new_msg.agent_id,
                    &new_msg.workspace_id,
                    &new_msg.role,
                    &new_msg.content,
                    &new_msg.tool_output,
                    new_msg.seq,
                    new_msg.created_at_ms,
                ],
            )?;
            forked.push(new_msg);
        }

        rebuild_session_fts_and_vocab(&tx, new_session_id)?;
        let _ = bump_generation(&tx)?;
        tx.commit()?;
        Ok(forked)
    }

    /// Builds a structured cross-agent handoff prompt summarizing `session_id` for
    /// continuation by `target_agent_id`.
    pub fn build_cross_agent_handoff_prompt(
        &self,
        session_id: &str,
        target_agent_id: &str,
        max_messages: usize,
    ) -> Result<String> {
        let target = target_agent_id.trim();
        if target.is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "target_agent_id must be non-empty",
            ));
        }
        let msgs = self.list_session_messages(session_id)?;
        if msgs.is_empty() {
            return Err(OrcaIdeRuntimeError::not_found(format!(
                "vault session '{session_id}' not found"
            )));
        }
        let source_agent = &msgs[0].agent_id;
        let workspace_id = &msgs[0].workspace_id;
        let cap = max_messages.clamp(1, 200);
        let slice_start = msgs.len().saturating_sub(cap);

        let mut lines = vec![
            format!("# Cross-Agent Handoff -> {target}"),
            format!(
                "- Source session: `{session_id}` (agent: `{source_agent}`, workspace: `{workspace_id}`)"
            ),
            format!(
                "- Included turns: {} of {} total",
                msgs.len() - slice_start,
                msgs.len()
            ),
            String::new(),
            "## Transcript Context".to_string(),
        ];

        for msg in &msgs[slice_start..] {
            if !msg.content.trim().is_empty() {
                lines.push(format!("[#{} {}]: {}", msg.seq, msg.role, msg.content.trim()));
            }
            if let Some(tool) = &msg.tool_output
                && !tool.trim().is_empty()
            {
                let compact: String = tool.chars().take(240).collect();
                lines.push(format!("[#{} tool_output]: {compact}", msg.seq));
            }
        }
        lines.push(String::new());
        lines.push(format!(
            "Continue seamlessly as `{target}` without repeating completed steps."
        ));
        Ok(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn vault_incremental_indexing_identifier_split_fuzzy_repair_and_fenced_cursors() {
        assert_send_sync::<AiVaultSearchEngine>();
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("vault_fts5.db");
        let transcript_path = dir.path().join("session_claude.jsonl");

        // Verify corrupt DB auto-rebuild at v8.
        fs::write(&db_path, b"CORRUPTED_VAULT_BYTES").unwrap();
        let engine = AiVaultSearchEngine::open(&db_path).unwrap();
        assert!(engine.rebuilt_on_open());

        // Write initial transcript lines.
        let initial = concat!(
            "{\"role\":\"user\",\"content\":\"Fix parseOrcaYamlConfig and user_auth_token\",\"created_at_ms\":1000}\n",
            "{\"role\":\"assistant\",\"content\":\"Updated WorktreeIncludePlanner successfully\",\"tool_output\":\"VERBOSE_TOOL_TRACE_9911\",\"created_at_ms\":2000}\n"
        );
        fs::write(&transcript_path, initial).unwrap();

        let r1 = engine
            .index_transcript_file(&transcript_path, "sess-1", "claude", "ws-1", 2000)
            .unwrap();
        assert!(r1.reindexed_from_zero);
        assert!(!r1.incremental_append);
        assert_eq!(r1.messages_added, 2);

        // Append new turn -> incremental append ONLY!
        {
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(&transcript_path)
                .unwrap();
            writeln!(
                f,
                "{{\"role\":\"assistant\",\"content\":\"Added hibernationRehydrationGuard\",\"created_at_ms\":3000}}"
            )
            .unwrap();
        }
        let r2 = engine
            .index_transcript_file(&transcript_path, "sess-1", "claude", "ws-1", 3000)
            .unwrap();
        assert!(!r2.reindexed_from_zero);
        assert!(r2.incremental_append);
        assert_eq!(r2.messages_added, 1);
        assert!(r2.new_byte_offset > r1.new_byte_offset);

        // Modify prefix -> forces full re-index from offset 0.
        fs::write(
            &transcript_path,
            "{\"role\":\"user\",\"content\":\"Rewritten parseOrcaYamlConfig and hibernation_guard\",\"created_at_ms\":4000}\n",
        )
        .unwrap();
        let r3 = engine
            .index_transcript_file(&transcript_path, "sess-1", "claude", "ws-1", 4000)
            .unwrap();
        assert!(r3.reindexed_from_zero);
        assert_eq!(r3.messages_added, 1);

        // Index second session for ranking + cursor pagination.
        engine
            .index_messages(
                "sess-2",
                "codex",
                "ws-1",
                &[VaultMessageInput {
                    message_id: None,
                    role: "user".to_string(),
                    content: "Investigate parseOrcaYamlConfig schema validation".to_string(),
                    tool_output: Some("raw_stdout_blob_xyz".to_string()),
                    created_at_ms: Some(5000),
                }],
                5000,
            )
            .unwrap();

        // camelCase / snake_case identifier split search ("yaml" matches parseOrcaYamlConfig).
        let idents = split_code_identifiers("parseOrcaYamlConfig user_auth_token");
        assert!(idents.contains(&"orca".to_string()));
        assert!(idents.contains(&"yaml".to_string()));
        assert!(idents.contains(&"auth".to_string()));

        let res_page1 = engine
            .search(&VaultSearchRequest {
                query: "yaml".to_string(),
                workspace_id: Some("ws-1".to_string()),
                agent_id: None,
                limit: 1,
                cursor: None,
            })
            .unwrap();
        assert_eq!(res_page1.hits.len(), 1);
        assert!(res_page1.next_cursor.is_some());

        // Typo fuzzy repair ("hibernaton" -> "hibernation", distance 1).
        assert_eq!(damerau_levenshtein_distance("orcaymal", "orcayaml"), 1);
        let fuzzy = engine
            .search(&VaultSearchRequest {
                query: "hibernaton".to_string(),
                workspace_id: None,
                agent_id: None,
                limit: 10,
                cursor: None,
            })
            .unwrap();
        assert_eq!(fuzzy.hits.len(), 1);
        assert_eq!(fuzzy.hits[0].session_id, "sess-1");
        assert_eq!(fuzzy.fuzzy_repaired_query.as_deref(), Some("hibernation"));

        // Purge tool outputs -> removes raw_stdout_blob_xyz from FTS & bumps generation!
        let purged = engine.purge_tool_outputs(Some("sess-2")).unwrap();
        assert_eq!(purged, 1);
        let after_purge = engine
            .search(&VaultSearchRequest {
                query: "raw_stdout_blob_xyz".to_string(),
                workspace_id: None,
                agent_id: None,
                limit: 10,
                cursor: None,
            })
            .unwrap();
        assert!(after_purge.hits.is_empty());

        // Stale cursor from before purge MUST be rejected by generation fence!
        let stale_err = engine
            .search(&VaultSearchRequest {
                query: "yaml".to_string(),
                workspace_id: Some("ws-1".to_string()),
                agent_id: None,
                limit: 1,
                cursor: res_page1.next_cursor,
            })
            .unwrap_err();
        assert_eq!(stale_err.code_str(), "conflict");

        // Fork conversation & build cross-agent handoff prompt.
        let forked = engine
            .fork_conversation("sess-2", "sess-2-fork", Some(1), 6000)
            .unwrap();
        assert_eq!(forked.len(), 1);
        let handoff = engine
            .build_cross_agent_handoff_prompt("sess-2-fork", "claude", 10)
            .unwrap();
        assert!(handoff.contains("Cross-Agent Handoff -> claude"));
        assert!(handoff.contains("parseOrcaYamlConfig"));

        // Retention deletion deletes older messages cleanly.
        let deleted = engine.delete_older_than(4500).unwrap();
        assert_eq!(deleted, 1);
    }
}
