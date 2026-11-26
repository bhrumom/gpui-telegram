//! `AgentStore` & `AgentRoster` — SQLite `projection_agents` CQRS roster, zero-starter-bot host
//! invariant, `<= 100` agent cap, roster event pruning, `.openbot-stage` atomic workspace/avatar
//! duplication + `.pending` crash recovery, and full `hard_delete_agent` cascade.
//! Mirrors `src/backend/agent-store.ts` and `src/backend/database/agent-roster.ts`.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use openbot_contracts::agents::{
    AGENT_FOLDER_LIMIT, AgentAccess, AgentBackupModels, AgentDefaults, AgentMarketplaceSource,
    AgentPendingRuntime, AgentProfileDraft, AgentProviderId, AgentReasoningEffort,
    AgentSessionSettingOverrides, AgentSummary, AgentUsageLimitReturn, AvatarImageInput,
    BusyMessageMode, CreateAgentInput, DEFAULT_AGENT_ACCESS, DEFAULT_AGENT_DEFAULTS,
    DEFAULT_AGENT_PREVIEW, DEFAULT_PROVIDER_ACCOUNT_ID, UpdateAgentInput, folder_inside,
    is_agent_additional_folders, is_agent_model, is_agent_session_setting_overrides,
    is_agent_session_setting_resets, is_agent_task_limit, is_browser_profile_id,
    is_provider_account_id,
};
use openbot_contracts::avatar::{
    AvatarHue, AvatarMimeType, avatar_file_extension, is_avatar_color, is_avatar_mime_type,
    is_avatar_seed, is_valid_avatar_image, nearest_avatar_hue,
};
use openbot_contracts::limits::{
    INPUT_LIMITS, is_bounded_string, is_identifier, is_iso_timestamp, is_uuid_v4, utf16_len,
};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::database::{OpenBotDatabase, OrchestrationEventInput};
use crate::error::{StorageError, StorageResult};

pub const AGENT_PREVIEW_MAX_LENGTH: usize = 180;
pub const DEFAULT_AGENT_MODEL: &str = "gpt-6-luna";
pub const DEFAULT_AGENT_PROVIDER: AgentProviderId = AgentProviderId::Codex;
pub const DEFAULT_REASONING_EFFORT: AgentReasoningEffort = AgentReasoningEffort::Low;
pub const CLAUDE_FAST_MODE_RESET_COMMAND_ID: &str = "agent-fast-mode:claude-reset:v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelRuntime {
    pub provider: AgentProviderId,
    pub model: String,
    pub reasoning_effort: AgentReasoningEffort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelChange {
    pub initiating_agent_id: String,
    pub target_agent_id: String,
    pub previous: AgentModelRuntime,
    pub next: AgentModelRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnclaimedThread {
    pub thread_id: String,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateAgentResult {
    pub agent: AgentSummary,
    pub layout: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAgentAvatar {
    pub path: PathBuf,
    pub mime_type: AvatarMimeType,
    pub version: String,
}

#[derive(Debug, Clone, Default)]
pub struct ExtendedUpdateAgentInput {
    pub base: UpdateAgentInput,
    pub additional_folders: Option<Vec<String>>,
    pub excluded_folders: Option<Vec<String>>,
    pub own_browser_session: Option<bool>,
    pub browser_profile_id: Option<Option<String>>,
    pub usage_limit_return: Option<Option<AgentUsageLimitReturn>>,
    pub pending_runtime: Option<Option<AgentPendingRuntime>>,
    pub pinned_account_id: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDuplicationMarker {
    pub operation_id: String,
    pub source_agent_id: String,
}

/// Deterministic main conversation thread ID (`openbot-database.ts:L688-L690`).
pub fn stable_thread_id(agent_id: &str) -> String {
    format!("openbot-thread-{agent_id}")
}

/// Duplication idempotency command ID (`agent-store.ts:L1631-L1633`).
pub fn duplication_command_id(operation_id: &str) -> String {
    format!("agent-duplication:{operation_id}")
}

/// Validates `^[a-z0-9][a-z0-9-]{0,63}$` and basename equality (`agent-store.ts:L1796-L1798`).
pub fn is_valid_agent_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    if !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
        return false;
    }
    if !bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
    {
        return false;
    }
    Path::new(id).file_name().and_then(|s| s.to_str()) == Some(id)
}

pub fn is_generated_agent_id(id: &str) -> bool {
    id.strip_prefix("agent-")
        .or_else(|| id.strip_prefix("bot-"))
        .is_some_and(is_uuid_v4)
}

pub fn legacy_agent_id(id: &str) -> String {
    if let Some(rest) = id.strip_prefix("agent-")
        && is_uuid_v4(rest)
    {
        return format!("bot-{rest}");
    }
    id.to_string()
}

/// Exact UTF-16 prefix slice matching JS `text.slice(0, max)`.
pub(crate) fn slice_utf16(text: &str, max: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max {
        return text.to_string();
    }
    String::from_utf16_lossy(&units[..max])
}

/// Duplicate agent naming (`agent-store.ts:L1614-L1624`): strips trailing ` copy(?: \d+)?`
/// case-insensitively and chooses `"<base> copy"`, `"<base> copy 2"`, ... bounded to `80` chars.
pub fn duplicate_agent_name(
    source_name: &str,
    agents: &[AgentSummary],
) -> StorageResult<String> {
    let stripped = strip_copy_suffix(source_name).trim();
    let base = if stripped.is_empty() {
        source_name.trim()
    } else {
        stripped
    };
    let existing: HashSet<String> = agents.iter().map(|a| a.name.to_lowercase()).collect();
    for index in 1..=(INPUT_LIMITS.agents + 1) {
        let suffix = if index == 1 {
            " copy".to_string()
        } else {
            format!(" copy {index}")
        };
        let suffix_len = utf16_len(&suffix);
        let head_cap = INPUT_LIMITS.agent_name.saturating_sub(suffix_len).max(1);
        let head = slice_utf16(base, head_cap);
        let candidate = format!("{}{suffix}", head.trim_end());
        if !existing.contains(&candidate.to_lowercase()) {
            return Ok(candidate);
        }
    }
    Err(StorageError::Conflict(
        "Could not generate a unique copy name for the duplicated agent.".to_string(),
    ))
}

fn strip_copy_suffix(source_name: &str) -> &str {
    let bytes = source_name.as_bytes();
    let mut digit_start = bytes.len();
    while digit_start > 0 && bytes[digit_start - 1].is_ascii_digit() {
        digit_start -= 1;
    }
    if digit_start < bytes.len() && digit_start >= 6 {
        let prefix = &source_name[digit_start - 6..digit_start];
        if prefix.eq_ignore_ascii_case(" copy ") {
            return &source_name[..digit_start - 6];
        }
    }
    if source_name.len() >= 5
        && source_name.is_char_boundary(source_name.len() - 5)
        && source_name[source_name.len() - 5..].eq_ignore_ascii_case(" copy")
    {
        return &source_name[..source_name.len() - 5];
    }
    source_name
}

/// Deterministic profile signature omitting `preview`, `updatedAt`, and `threadId` (`agent-store.ts:L1626-L1629`).
pub fn duplication_profile_signature(agent: &AgentSummary) -> StorageResult<String> {
    let mut value = serde_json::to_value(agent)?;
    if let Some(obj) = value.as_object_mut() {
        obj.remove("preview");
        obj.remove("updatedAt");
        obj.remove("threadId");
    }
    Ok(serde_json::to_string(&value)?)
}

pub fn agent_avatar_url(agent_id: &str, version: &str, mime_type: AvatarMimeType) -> String {
    let mime = match mime_type {
        AvatarMimeType::Png => "image%2Fpng",
        AvatarMimeType::Jpeg => "image%2Fjpeg",
        AvatarMimeType::Webp => "image%2Fwebp",
    };
    format!("openbot-avatar://agent/{agent_id}?v={version}&type={mime}")
}

pub fn parse_agent_avatar_url(
    value: &str,
    expected_agent_id: &str,
) -> Option<(String, AvatarMimeType)> {
    let rest = value.strip_prefix("openbot-avatar://agent/")?;
    let (agent_part, query) = rest.split_once('?')?;
    let agent_id = percent_decode(agent_part.trim_matches('/'))?;
    if agent_id != expected_agent_id {
        return None;
    }
    let mut version: Option<String> = None;
    let mut mime_raw: Option<String> = None;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            match k {
                "v" => version = percent_decode(v),
                "type" => mime_raw = percent_decode(v),
                _ => {}
            }
        }
    }
    let ver = version?;
    let mime_str = mime_raw?;
    if !is_uuid_v4(&ver) || !is_avatar_mime_type(&mime_str) {
        return None;
    }
    let mime = match mime_str.as_str() {
        "image/png" => AvatarMimeType::Png,
        "image/jpeg" => AvatarMimeType::Jpeg,
        "image/webp" => AvatarMimeType::Webp,
        _ => return None,
    };
    Some((ver, mime))
}

fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            if idx + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[idx + 1..idx + 3]).ok()?;
            let byte = u8::from_str_radix(hex, 16).ok()?;
            out.push(byte);
            idx += 3;
        } else if bytes[idx] == b'+' {
            out.push(b' ');
            idx += 1;
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub(crate) fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(crate) fn delete_receipts_of(conn: &Connection, command_ids: &[String]) -> StorageResult<()> {
    if command_ids.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare_cached(
        "DELETE FROM orchestration_command_receipts
         WHERE command_id = ?1
           AND NOT EXISTS (SELECT 1 FROM orchestration_events WHERE command_id = ?1)",
    )?;
    for command_id in command_ids {
        stmt.execute(params![command_id])?;
    }
    Ok(())
}

pub(crate) fn delete_aggregate_history(
    conn: &Connection,
    aggregate_type: &str,
    aggregate_ids: &[String],
) -> StorageResult<()> {
    if aggregate_ids.is_empty() {
        return Ok(());
    }
    let placeholders = (0..aggregate_ids.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let mut args: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + aggregate_ids.len());
    args.push(&aggregate_type);
    for id in aggregate_ids {
        args.push(id);
    }
    conn.execute(
        &format!(
            "DELETE FROM orchestration_command_receipts WHERE command_id IN (
               SELECT DISTINCT command_id FROM orchestration_events
               WHERE aggregate_type = ? AND aggregate_id IN ({placeholders})
             )"
        ),
        params_from_iter(args.iter().copied()),
    )?;
    conn.execute(
        &format!(
            "DELETE FROM orchestration_events
             WHERE aggregate_type = ? AND aggregate_id IN ({placeholders})"
        ),
        params_from_iter(args.iter().copied()),
    )?;
    Ok(())
}

pub(crate) fn delete_thread_attachments(conn: &Connection, thread_id: &str) -> StorageResult<()> {
    let lower = format!("{thread_id}:");
    let upper = format!("{thread_id};");
    conn.execute(
        "DELETE FROM projection_attachments
         WHERE owner_kind = 'thread-message' AND owner_id >= ?1 AND owner_id < ?2",
        params![lower, upper],
    )?;
    Ok(())
}

pub(crate) fn ensure_thread_projection_row(
    conn: &Connection,
    agent: &AgentSummary,
    sequence: i64,
) -> StorageResult<()> {
    let Some(thread_id) = agent.thread_id.as_deref() else {
        return Ok(());
    };
    let ts = agent.updated_at.clone().unwrap_or_else(now_iso);
    conn.execute(
        "INSERT INTO projection_threads
           (thread_id, agent_id, title, active_turn_id, created_at, updated_at, last_event_sequence)
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)
         ON CONFLICT(thread_id) DO UPDATE SET
           agent_id = excluded.agent_id,
           title = excluded.title,
           updated_at = excluded.updated_at,
           last_event_sequence = MAX(projection_threads.last_event_sequence, excluded.last_event_sequence)",
        params![thread_id, agent.id, agent.name, ts, ts, sequence],
    )?;
    Ok(())
}

fn claim_thread_projection_row(
    conn: &Connection,
    agent: &AgentSummary,
    sequence: i64,
) -> StorageResult<()> {
    let Some(thread_id) = agent.thread_id.as_deref() else {
        return Ok(());
    };
    let ts = agent.updated_at.clone().unwrap_or_else(now_iso);
    conn.execute(
        "INSERT INTO projection_threads
           (thread_id, agent_id, title, active_turn_id, created_at, updated_at, last_event_sequence)
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)
         ON CONFLICT(thread_id) DO UPDATE SET
           agent_id = excluded.agent_id,
           title = excluded.title,
           updated_at = excluded.updated_at,
           last_event_sequence = MAX(projection_threads.last_event_sequence, excluded.last_event_sequence)
         WHERE projection_threads.agent_id IS NOT excluded.agent_id
            OR projection_threads.title IS NOT excluded.title",
        params![thread_id, agent.id, agent.name, ts, ts, sequence],
    )?;
    Ok(())
}

/// Prunes older `'agents'` events while retaining `'agent.model-changed'` (`agent-roster.ts:L320-L330`).
pub(crate) fn prune_roster_events(conn: &Connection, retained_sequence: i64) -> StorageResult<()> {
    let command_ids: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT command_id FROM orchestration_events
             WHERE aggregate_type = 'agents' AND aggregate_id = 'agents'
               AND sequence < ?1 AND event_type <> 'agent.model-changed'",
        )?;
        let rows = stmt.query_map(params![retained_sequence], |row| row.get(0))?;
        rows.collect::<Result<Vec<String>, _>>()?
    };
    if command_ids.is_empty() {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM orchestration_events
         WHERE aggregate_type = 'agents' AND aggregate_id = 'agents'
           AND sequence < ?1 AND event_type <> 'agent.model-changed'",
        params![retained_sequence],
    )?;
    delete_receipts_of(conn, &command_ids)
}

fn write_changed_agents(
    conn: &Connection,
    agents: &[AgentSummary],
    sequence: i64,
) -> StorageResult<()> {
    let mut stored: HashMap<String, (i64, String)> = HashMap::new();
    {
        let mut stmt =
            conn.prepare_cached("SELECT agent_id, sort_order, agent_json FROM projection_agents")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, order, json) = row?;
            stored.insert(id, (order, json));
        }
    }
    let kept: HashSet<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    for existing_id in stored.keys() {
        if !kept.contains(existing_id.as_str()) {
            conn.execute(
                "DELETE FROM projection_agents WHERE agent_id = ?1",
                params![existing_id],
            )?;
        }
    }
    for (idx, agent) in agents.iter().enumerate() {
        let sort_order = idx as i64;
        let json = serde_json::to_string(agent)?;
        let changed = match stored.get(&agent.id) {
            Some((old_order, old_json)) => *old_order != sort_order || old_json != &json,
            None => true,
        };
        if changed && stored.contains_key(&agent.id) {
            conn.execute(
                "UPDATE projection_agents
                 SET thread_id = ?1, model = ?2, updated_at = ?3, sort_order = ?4,
                     agent_json = ?5, last_event_sequence = ?6
                 WHERE agent_id = ?7",
                params![
                    agent.thread_id,
                    agent.model,
                    agent.updated_at,
                    sort_order,
                    json,
                    sequence,
                    agent.id
                ],
            )?;
        } else if changed {
            conn.execute(
                "INSERT INTO projection_agents
                   (agent_id, thread_id, model, updated_at, sort_order, agent_json, last_event_sequence)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    agent.id,
                    agent.thread_id,
                    agent.model,
                    agent.updated_at,
                    sort_order,
                    json,
                    sequence
                ],
            )?;
        }
        if agent.thread_id.is_some() {
            if changed {
                ensure_thread_projection_row(conn, agent, sequence)?;
            } else {
                claim_thread_projection_row(conn, agent, sequence)?;
            }
        }
    }
    Ok(())
}

/// Low-level SQLite roster projection facade (`src/backend/database/agent-roster.ts`).
#[derive(Clone)]
pub struct AgentRoster {
    database: OpenBotDatabase,
}

impl AgentRoster {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn list_agents(&self) -> StorageResult<Vec<AgentSummary>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT agent_json FROM projection_agents ORDER BY sort_order, agent_id",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut agents = Vec::new();
            for row in rows {
                let raw: serde_json::Value = serde_json::from_str(&row?)?;
                let (agent, _repaired) = read_stored_agent_value(&raw)?;
                agents.push(agent);
            }
            Ok(agents)
        })
    }

    pub fn get_agent(&self, agent_id: &str) -> StorageResult<Option<AgentSummary>> {
        self.database.with_conn(|conn| {
            let raw_opt: Option<String> = conn
                .prepare_cached("SELECT agent_json FROM projection_agents WHERE agent_id = ?1")?
                .query_row(params![agent_id], |row| row.get(0))
                .optional()?;
            match raw_opt {
                Some(raw_str) => {
                    let raw: serde_json::Value = serde_json::from_str(&raw_str)?;
                    let (agent, _) = read_stored_agent_value(&raw)?;
                    Ok(Some(agent))
                }
                None => Ok(None),
            }
        })
    }

    pub fn unclaimed_threads(&self) -> StorageResult<Vec<UnclaimedThread>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT thread_id, agent_id FROM projection_threads
                 WHERE thread_id NOT IN (SELECT thread_id FROM projection_agents WHERE thread_id IS NOT NULL)
                   AND thread_id NOT IN (SELECT thread_id FROM projection_channel_contexts)
                   AND thread_id NOT IN (SELECT thread_id FROM projection_messaging_threads)
                   AND thread_id NOT IN (SELECT thread_id FROM projection_agent_threads)
                 ORDER BY thread_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(UnclaimedThread {
                    thread_id: row.get(0)?,
                    agent_id: row.get(1)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn latest_roster_agents(&self) -> StorageResult<Vec<serde_json::Value>> {
        self.database.with_conn(|conn| {
            let payload_opt: Option<String> = conn
                .prepare_cached(
                    "SELECT payload_json FROM orchestration_events
                     WHERE aggregate_type = 'agents' AND aggregate_id = 'agents'
                     ORDER BY sequence DESC LIMIT 1",
                )?
                .query_row([], |row| row.get(0))
                .optional()?;
            let Some(payload_str) = payload_opt else {
                return Ok(Vec::new());
            };
            let payload: serde_json::Value = serde_json::from_str(&payload_str)?;
            Ok(payload
                .get("agents")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default())
        })
    }

    pub fn replace_agents(
        &self,
        command_id: &str,
        agents: &[AgentSummary],
        event_type: &str,
        model_change: Option<&AgentModelChange>,
    ) -> StorageResult<()> {
        let payload = match model_change {
            Some(change) => serde_json::json!({ "agents": agents, "modelChange": change }),
            None => serde_json::json!({ "agents": agents }),
        };
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agents".to_string(),
            aggregate_id: "agents".to_string(),
            event_type: event_type.to_string(),
            occurred_at: now_iso(),
            payload,
        }];
        let _ = self.database.dispatch_with_projection(
            command_id,
            None,
            &events,
            |conn, sequences| {
                let sequence = sequences.first().copied().unwrap_or(0);
                let unique_ids: HashSet<&str> = agents.iter().map(|a| a.id.as_str()).collect();
                if unique_ids.len() == agents.len() {
                    write_changed_agents(conn, agents, sequence)?;
                } else {
                    conn.execute("DELETE FROM projection_agents", [])?;
                    for (idx, agent) in agents.iter().enumerate() {
                        let json = serde_json::to_string(agent)?;
                        conn.execute(
                            "INSERT INTO projection_agents
                               (agent_id, thread_id, model, updated_at, sort_order, agent_json, last_event_sequence)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            params![
                                agent.id,
                                agent.thread_id,
                                agent.model,
                                agent.updated_at,
                                idx as i64,
                                json,
                                sequence
                            ],
                        )?;
                        ensure_thread_projection_row(conn, agent, sequence)?;
                    }
                }
                prune_roster_events(conn, sequence)?;
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(())
    }

    /// Full cascade purge (`agent-roster.ts:L105-L220` + side-thread & outbox cleanup).
    pub fn hard_delete_agent(
        &self,
        command_id: &str,
        agent_id: &str,
        thread_id: Option<&str>,
        remaining_agents: &[AgentSummary],
        file_deletions: &[String],
    ) -> StorageResult<()> {
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agents".to_string(),
            aggregate_id: "agents".to_string(),
            event_type: "agents.rebased-after-delete".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({ "agents": remaining_agents }),
        }];
        let _ = self.database.dispatch_with_projection(
            command_id,
            None,
            &events,
            |conn, sequences| {
                let sequence = sequences.first().copied().unwrap_or(0);
                for table in [
                    "agent_usage_records",
                    "agent_usage_checkpoints",
                    "agent_usage_activity",
                    "agent_usage_skill_loads",
                ] {
                    conn.execute(&format!("DELETE FROM {table} WHERE agent_id = ?1"), params![agent_id])?;
                }
                delete_aggregate_history(conn, "agent-usage", &[agent_id.to_string()])?;

                let memory_ids: Vec<String> = {
                    let mut stmt = conn.prepare(
                        "SELECT memory_id FROM projection_agent_memories WHERE agent_id = ?1",
                    )?;
                    let rows = stmt.query_map(params![agent_id], |r| r.get(0))?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                if !memory_ids.is_empty() {
                    delete_aggregate_history(conn, "agent-memory", &memory_ids)?;
                }

                let routine_ids: Vec<String> = {
                    let mut stmt = conn.prepare(
                        "SELECT routine_id FROM projection_agent_routines WHERE agent_id = ?1",
                    )?;
                    let rows = stmt.query_map(params![agent_id], |r| r.get(0))?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                if !routine_ids.is_empty() {
                    delete_aggregate_history(conn, "agent-routine", &routine_ids)?;
                    delete_aggregate_history(conn, "routine-run", &routine_ids)?;
                }

                conn.execute(
                    "DELETE FROM orchestration_command_receipts WHERE command_id IN (
                       SELECT DISTINCT command_id FROM orchestration_events
                       WHERE aggregate_type = 'hosted-site-terminal'
                         AND event_type = 'hosted-site.terminal-pending'
                         AND COALESCE(json_extract(payload_json, '$.agentId'), json_extract(payload_json, '$.botId')) = ?1
                     )",
                    params![agent_id],
                )?;
                conn.execute(
                    "DELETE FROM orchestration_events
                     WHERE aggregate_type = 'hosted-site-terminal'
                       AND event_type = 'hosted-site.terminal-pending'
                       AND COALESCE(json_extract(payload_json, '$.agentId'), json_extract(payload_json, '$.botId')) = ?1",
                    params![agent_id],
                )?;

                let side_thread_ids: Vec<String> = {
                    let mut stmt = conn.prepare(
                        "SELECT thread_id FROM projection_agent_threads WHERE agent_id = ?1",
                    )?;
                    let rows = stmt.query_map(params![agent_id], |r| r.get(0))?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                if !side_thread_ids.is_empty() {
                    delete_aggregate_history(conn, "agent-thread", &side_thread_ids)?;
                    delete_aggregate_history(conn, "thread", &side_thread_ids)?;
                    for sid in &side_thread_ids {
                        delete_thread_attachments(conn, sid)?;
                    }
                }

                if let Some(tid) = thread_id {
                    conn.execute(
                        "DELETE FROM orchestration_command_receipts WHERE command_id IN (
                           SELECT DISTINCT command_id FROM orchestration_events
                           WHERE aggregate_id = ?1 OR aggregate_id = ?2
                              OR (aggregate_type = 'agents' AND aggregate_id = 'agents' AND sequence < ?3)
                         )",
                        params![agent_id, tid, sequence],
                    )?;
                    conn.execute(
                        "DELETE FROM orchestration_events
                         WHERE aggregate_id = ?1 OR aggregate_id = ?2
                            OR (aggregate_type = 'agents' AND aggregate_id = 'agents' AND sequence < ?3)",
                        params![agent_id, tid, sequence],
                    )?;
                } else {
                    conn.execute(
                        "DELETE FROM orchestration_command_receipts WHERE command_id IN (
                           SELECT DISTINCT command_id FROM orchestration_events
                           WHERE aggregate_id = ?1
                              OR (aggregate_type = 'agents' AND aggregate_id = 'agents' AND sequence < ?2)
                         )",
                        params![agent_id, sequence],
                    )?;
                    conn.execute(
                        "DELETE FROM orchestration_events
                         WHERE aggregate_id = ?1
                            OR (aggregate_type = 'agents' AND aggregate_id = 'agents' AND sequence < ?2)",
                        params![agent_id, sequence],
                    )?;
                }

                conn.execute("DELETE FROM projection_agents WHERE agent_id = ?1", params![agent_id])?;
                write_changed_agents(conn, remaining_agents, sequence)?;
                conn.execute("DELETE FROM projection_agent_memories WHERE agent_id = ?1", params![agent_id])?;

                let revoked_at = now_iso();
                for routine_id in &routine_ids {
                    conn.execute(
                        "INSERT OR IGNORE INTO projection_webhook_route_revocations (route_id, created_at)
                         SELECT route_id, ?1 FROM projection_routine_webhooks WHERE routine_id = ?2",
                        params![revoked_at, routine_id],
                    )?;
                    conn.execute(
                        "DELETE FROM projection_routine_webhooks WHERE routine_id = ?1",
                        params![routine_id],
                    )?;
                    conn.execute(
                        "DELETE FROM projection_webhook_receipts WHERE owner_kind = 'agent' AND routine_id = ?1",
                        params![routine_id],
                    )?;
                }
                conn.execute("DELETE FROM projection_agent_routines WHERE agent_id = ?1", params![agent_id])?;
                conn.execute(
                    "DELETE FROM routine_flow_links WHERE from_agent_id = ?1 OR to_agent_id = ?1",
                    params![agent_id],
                )?;
                conn.execute(
                    "DELETE FROM routine_flow_positions WHERE canvas_agent_id = ?1 OR node_key = ?2",
                    params![agent_id, format!("agent:{agent_id}")],
                )?;
                conn.execute("DELETE FROM routine_flow_steps WHERE agent_id = ?1", params![agent_id])?;
                conn.execute(
                    "UPDATE workflows SET graph_revision = graph_revision + 1
                     WHERE workflow_id IN (SELECT workflow_id FROM workflow_nodes WHERE agent_id = ?1)",
                    params![agent_id],
                )?;
                conn.execute(
                    "UPDATE workflow_nodes
                     SET agent_id = NULL, fields_json = json_remove(fields_json, '$.agentId')
                     WHERE agent_id = ?1",
                    params![agent_id],
                )?;
                conn.execute("UPDATE workflow_steps SET agent_id = NULL WHERE agent_id = ?1", params![agent_id])?;
                conn.execute("DELETE FROM agent_waits WHERE agent_id = ?1", params![agent_id])?;
                conn.execute("DELETE FROM projection_reactions WHERE agent_id = ?1", params![agent_id])?;
                conn.execute("DELETE FROM projection_deliveries WHERE recipient_agent_id = ?1", params![agent_id])?;
                conn.execute("DELETE FROM projection_queue_state WHERE agent_id = ?1", params![agent_id])?;
                for sid in &side_thread_ids {
                    conn.execute("DELETE FROM projection_threads WHERE thread_id = ?1", params![sid])?;
                }
                if let Some(tid) = thread_id {
                    delete_thread_attachments(conn, tid)?;
                    conn.execute("DELETE FROM projection_threads WHERE thread_id = ?1", params![tid])?;
                }
                let outbox_ts = now_iso();
                for path in file_deletions {
                    OpenBotDatabase::enqueue_file_deletion(
                        conn,
                        &Uuid::new_v4().to_string(),
                        path,
                        "agents.rebased-after-delete",
                        &outbox_ts,
                    )?;
                }
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(())
    }
}

struct AgentStoreInner {
    database: OpenBotDatabase,
    roster: AgentRoster,
    agents_root: PathBuf,
    legacy_agents_root: PathBuf,
    shared_root: PathBuf,
    downloads_root: PathBuf,
    avatars_root: PathBuf,
    channel_images_root: PathBuf,
    duplications_root: PathBuf,
    agent_defaults: AgentDefaults,
    state: Mutex<Vec<AgentSummary>>,
}

/// High-level `AgentStore` over `OpenBotDatabase` + filesystem workspace/avatar roots (`src/backend/agent-store.ts`).
/// Seeds **zero** starter/example bots on fresh host databases and enforces `<= INPUT_LIMITS.agents` (`100`).
#[derive(Clone)]
pub struct AgentStore {
    inner: Arc<AgentStoreInner>,
}

impl AgentStore {
    pub fn open(
        database: OpenBotDatabase,
        openbot_root: impl AsRef<Path>,
    ) -> StorageResult<Self> {
        let user_data_path = database.data_dir().to_path_buf();
        Self::open_with_paths(
            user_data_path,
            openbot_root,
            database,
            DEFAULT_AGENT_DEFAULTS,
        )
    }

    pub fn open_with_paths(
        user_data_path: impl AsRef<Path>,
        openbot_root: impl AsRef<Path>,
        database: OpenBotDatabase,
        agent_defaults: AgentDefaults,
    ) -> StorageResult<Self> {
        let user_data = user_data_path.as_ref();
        let root = openbot_root.as_ref();
        let agents_root = root.join("Agents");
        let legacy_agents_root = root.join("Bots");
        let shared_root = root.join("Shared");
        let downloads_root = root.join("Downloads");
        let avatars_root = user_data.join("avatars").join("agents");
        let channel_images_root = user_data.join("avatars").join("channels");
        let duplications_root = user_data.join("agent-duplications");

        for dir in [
            &agents_root,
            &shared_root,
            &downloads_root,
            &avatars_root,
            &channel_images_root,
            &duplications_root,
        ] {
            create_dir_0700(dir)?;
        }

        let roster = AgentRoster::new(database.clone());
        let mut agents = Vec::new();
        let mut had_repairs = false;
        let raw_rows: Vec<String> = database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT agent_json FROM projection_agents ORDER BY sort_order, agent_id",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })?;
        for raw_str in raw_rows {
            let val: serde_json::Value = serde_json::from_str(&raw_str)?;
            let (agent, repaired) = read_stored_agent_value(&val)?;
            if !repaired.is_empty() {
                had_repairs = true;
            }
            agents.push(agent);
        }

        let store = Self {
            inner: Arc::new(AgentStoreInner {
                database,
                roster,
                agents_root,
                legacy_agents_root,
                shared_root,
                downloads_root,
                avatars_root,
                channel_images_root,
                duplications_root,
                agent_defaults,
                state: Mutex::new(agents),
            }),
        };

        store.restore_roster_from_events()?;
        if had_repairs {
            let guard = store.inner.state.lock();
            store.persist_locked(&guard, "agents.repaired", None)?;
        }
        store.reset_claude_fast_mode_once()?;
        store.recover_pending_duplications()?;
        store.reconcile_unclaimed_threads()?;
        Ok(store)
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.inner.database
    }

    pub fn roster(&self) -> &AgentRoster {
        &self.inner.roster
    }

    pub fn agents_root(&self) -> &Path {
        &self.inner.agents_root
    }

    pub fn shared_root(&self) -> &Path {
        &self.inner.shared_root
    }

    pub fn downloads_root(&self) -> &Path {
        &self.inner.downloads_root
    }

    pub fn avatars_root(&self) -> &Path {
        &self.inner.avatars_root
    }

    pub fn channel_images_root(&self) -> &Path {
        &self.inner.channel_images_root
    }

    pub fn agent_defaults(&self) -> &AgentDefaults {
        &self.inner.agent_defaults
    }

    pub fn list(&self) -> Vec<AgentSummary> {
        self.inner.state.lock().clone()
    }

    pub fn get(&self, id: &str) -> Option<AgentSummary> {
        self.inner.state.lock().iter().find(|a| a.id == id).cloned()
    }

    pub fn has(&self, id: &str) -> bool {
        self.inner.state.lock().iter().any(|a| a.id == id)
    }

    /// Creates a new agent prepended at index `0` (`sort_order = 0`), enforcing `< INPUT_LIMITS.agents` (`100`).
    pub fn create_agent(&self, input: &CreateAgentInput) -> StorageResult<AgentSummary> {
        let mut guard = self.inner.state.lock();
        if guard.len() >= INPUT_LIMITS.agents {
            return Err(StorageError::LimitExceeded(format!(
                "Agent host limit of {} reached.",
                INPUT_LIMITS.agents
            )));
        }
        let name = required_trimmed_text(&input.name, "Agent name", INPUT_LIMITS.agent_name)?;
        let description =
            limited_trimmed_text(&input.description, "Agent description", INPUT_LIMITS.agent_description)?;
        if !is_avatar_seed(&input.avatar_seed) {
            return Err(StorageError::Validation("Invalid avatar seed.".to_string()));
        }
        if let Some(Some(color)) = &input.avatar_color
            && !is_avatar_color(color)
        {
            return Err(StorageError::Validation("Invalid avatar color.".to_string()));
        }
        if let Some(model) = &input.model
            && !is_agent_model(model)
        {
            return Err(StorageError::Validation("Invalid agent model.".to_string()));
        }
        if let Some(acc) = &input.provider_account_id
            && !is_provider_account_id(acc)
        {
            return Err(StorageError::Validation("Invalid provider account.".to_string()));
        }

        let id = format!("agent-{}", Uuid::new_v4());
        let mut record = self.create_default_record(&id, name, String::new(), description)?;
        if let Some(provider) = input.provider {
            record.provider = provider;
        }
        if let Some(model) = &input.model {
            record.model = model.clone();
        }
        if let Some(effort) = input.reasoning_effort {
            record.reasoning_effort = effort;
        }
        if let Some(acc) = &input.provider_account_id
            && acc != DEFAULT_PROVIDER_ACCOUNT_ID
        {
            record.provider_account_id = Some(acc.clone());
        }
        record.avatar_seed = input.avatar_seed.clone();
        record.avatar_hue = input.avatar_hue;
        apply_avatar_color(&mut record, input.avatar_hue, input.avatar_color.as_ref(), true);

        create_dir_0700(Path::new(&record.workspace_path))?;
        guard.insert(0, record.clone());
        if let Err(err) = self.persist_locked(&guard, "agent.created", None) {
            guard.retain(|a| a.id != id);
            let _ = fs::remove_dir_all(&record.workspace_path);
            return Err(err);
        }
        Ok(record)
    }

    /// Gets or creates an agent by ID, **appending** (`push`) when newly created (`agent-store.ts:L1044-L1069`).
    pub fn get_or_create(
        &self,
        id: &str,
        name: Option<&str>,
        title: Option<&str>,
    ) -> StorageResult<AgentSummary> {
        if !is_valid_agent_id(id) {
            return Err(StorageError::Validation("Invalid agent id.".to_string()));
        }
        let mut guard = self.inner.state.lock();
        if let Some(existing) = guard.iter().find(|a| a.id == id).cloned() {
            create_dir_0700(Path::new(&existing.workspace_path))?;
            return Ok(existing);
        }
        if guard.len() >= INPUT_LIMITS.agents {
            return Err(StorageError::LimitExceeded(format!(
                "Agent host limit of {} reached.",
                INPUT_LIMITS.agents
            )));
        }
        let resolved_name = name
            .map(str::to_string)
            .unwrap_or_else(|| title_from_id(id));
        let resolved_title = title.unwrap_or("Local teammate").to_string();
        let record = self.create_default_record(id, resolved_name, resolved_title, String::new())?;
        create_dir_0700(Path::new(&record.workspace_path))?;
        guard.push(record.clone());
        if let Err(err) = self.persist_locked(&guard, "agent.created", None) {
            guard.retain(|a| a.id != id);
            return Err(err);
        }
        Ok(record)
    }

    pub fn ensure_thread_id(&self, id: &str) -> StorageResult<String> {
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, id)?;
        if let Some(tid) = &guard[idx].thread_id {
            return Ok(tid.clone());
        }
        let prev = guard[idx].clone();
        let tid = stable_thread_id(id);
        guard[idx].thread_id = Some(tid.clone());
        guard[idx].updated_at = Some(now_iso());
        if let Err(err) = self.persist_locked(&guard, "thread.created", None) {
            guard[idx] = prev;
            return Err(err);
        }
        Ok(tid)
    }

    pub fn restore_thread_identity(
        &self,
        id: &str,
        thread_id: Option<String>,
        updated_at: Option<String>,
    ) -> StorageResult<()> {
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, id)?;
        guard[idx].thread_id = thread_id;
        guard[idx].updated_at = updated_at;
        Ok(())
    }

    pub fn provider_of(&self, id: &str) -> Option<AgentProviderId> {
        self.inner
            .state
            .lock()
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.provider)
    }

    pub fn owns_thread(&self, thread_id: &str) -> bool {
        self.inner
            .state
            .lock()
            .iter()
            .any(|a| a.thread_id.as_deref() == Some(thread_id))
    }

    pub fn main_thread_agent_id(&self, thread_id: &str) -> Option<String> {
        self.inner
            .state
            .lock()
            .iter()
            .find(|a| a.thread_id.as_deref() == Some(thread_id))
            .map(|a| a.id.clone())
    }

    /// Truncates `preview` to `AGENT_PREVIEW_MAX_LENGTH` (`180` UTF-16 code units) (`agent-store.ts:L1433-L1446`).
    pub fn update_preview(&self, id: &str, preview: &str) -> StorageResult<()> {
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, id)?;
        let prev = guard[idx].clone();
        guard[idx].preview = slice_utf16(preview, AGENT_PREVIEW_MAX_LENGTH);
        guard[idx].updated_at = Some(now_iso());
        if let Err(err) = self.persist_locked(&guard, "agent.preview-updated", None) {
            guard[idx] = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn clear_preview(&self, id: &str) -> StorageResult<()> {
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, id)?;
        let prev = guard[idx].clone();
        guard[idx].preview.clear();
        if let Err(err) = self.persist_locked(&guard, "agent.preview-updated", None) {
            guard[idx] = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn set_hidden(&self, agent_id: &str, hidden_at: Option<&str>) -> StorageResult<AgentSummary> {
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, agent_id)?;
        let previous = guard[idx].hidden_at.clone();
        if (hidden_at.is_none() && previous.is_none())
            || (hidden_at.is_some() && previous.is_some())
        {
            return Ok(guard[idx].clone());
        }
        if let Some(ts) = hidden_at
            && (!is_bounded_string(ts, 64) || !is_iso_timestamp(ts))
        {
            return Err(StorageError::Validation(
                "Invalid hidden agent time.".to_string(),
            ));
        }
        guard[idx].hidden_at = hidden_at.map(str::to_string);
        let event_type = if hidden_at.is_none() {
            "agent.restored"
        } else {
            "agent.hidden"
        };
        if let Err(err) = self.persist_locked(&guard, event_type, None) {
            guard[idx].hidden_at = previous;
            return Err(err);
        }
        Ok(guard[idx].clone())
    }

    pub fn is_hidden(&self, agent_id: &str) -> bool {
        self.inner
            .state
            .lock()
            .iter()
            .any(|a| a.id == agent_id && a.hidden_at.is_some())
    }

    pub fn set_marketplace_source(
        &self,
        agent_id: &str,
        source: AgentMarketplaceSource,
    ) -> StorageResult<AgentSummary> {
        if !source.is_valid() {
            return Err(StorageError::Validation(
                "Invalid marketplace source.".to_string(),
            ));
        }
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, agent_id)?;
        let prev = guard[idx].clone();
        guard[idx].marketplace_source = Some(source);
        guard[idx].updated_at = Some(now_iso());
        if let Err(err) = self.persist_locked(&guard, "agent.marketplace-source-updated", None) {
            guard[idx] = prev;
            return Err(err);
        }
        Ok(guard[idx].clone())
    }

    pub fn reorder_agents(&self, ordered_ids: &[String]) -> StorageResult<Vec<AgentSummary>> {
        let mut guard = self.inner.state.lock();
        let seen: HashSet<&str> = ordered_ids.iter().map(String::as_str).collect();
        if seen.len() != ordered_ids.len() || ordered_ids.len() != guard.len() {
            return Err(StorageError::Validation(
                "Agent reorder list must be an exact permutation of current agents.".to_string(),
            ));
        }
        let by_id: HashMap<String, AgentSummary> =
            guard.iter().cloned().map(|a| (a.id.clone(), a)).collect();
        let mut reordered = Vec::with_capacity(guard.len());
        for id in ordered_ids {
            let Some(agent) = by_id.get(id) else {
                return Err(StorageError::NotFound(format!("Unknown agent: {id}")));
            };
            reordered.push(agent.clone());
        }
        let prev = guard.clone();
        *guard = reordered;
        if let Err(err) = self.persist_locked(&guard, "agents.reordered", None) {
            *guard = prev;
            return Err(err);
        }
        Ok(guard.clone())
    }

    pub fn save_reviewed_profile(
        &self,
        agent_id: &str,
        draft: &AgentProfileDraft,
    ) -> StorageResult<AgentSummary> {
        let normalized = draft
            .normalized()
            .map_err(|e| StorageError::Validation(e.to_string()))?;
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, agent_id)?;
        let prev = guard[idx].clone();
        guard[idx].name = normalized.name;
        guard[idx].title = normalized.title;
        guard[idx].description = normalized.description;
        guard[idx].avatar_seed = normalized.avatar_seed;
        guard[idx].avatar_hue = normalized.avatar_hue;
        guard[idx].avatar_url = None;
        guard[idx].updated_at = Some(now_iso());
        if normalized.avatar_hue != prev.avatar_hue {
            guard[idx].avatar_color = None;
        }
        if let Err(err) = self.persist_locked(&guard, "agent.updated", None) {
            guard[idx] = prev;
            return Err(err);
        }
        Ok(guard[idx].clone())
    }

    pub fn commit_reviewed_profile(
        &self,
        agent_id: &str,
        draft: &AgentProfileDraft,
        command_id: &str,
        layout: serde_json::Value,
    ) -> StorageResult<DuplicateAgentResult> {
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agent-profiles".to_string(),
            aggregate_id: agent_id.to_string(),
            event_type: "agent-profile.saved".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({ "agentId": agent_id }),
        }];
        let receipt = self.inner.database.dispatch_with_projection(
            command_id,
            None,
            &events,
            |_conn, _sequences| {
                let agent = self.save_reviewed_profile(agent_id, draft)?;
                Ok(DuplicateAgentResult {
                    agent,
                    layout: layout.clone(),
                })
            },
        )?;
        Ok(receipt.result)
    }

    pub fn set_session_setting_overrides(
        &self,
        agent_id: &str,
        overrides: AgentSessionSettingOverrides,
        reset_identity: Option<&str>,
    ) -> StorageResult<AgentSummary> {
        if !is_agent_session_setting_overrides(&overrides) {
            return Err(StorageError::Validation(
                "Invalid session setting overrides.".to_string(),
            ));
        }
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, agent_id)?;
        let prev = guard[idx].clone();
        if let Some(identity) = reset_identity {
            let mut resets = guard[idx].session_setting_resets.clone().unwrap_or_default();
            if !resets.iter().any(|r| r == identity) {
                resets.push(identity.to_string());
            }
            guard[idx].session_setting_resets = Some(resets);
        }
        guard[idx].session_setting_overrides = Some(overrides);
        if let Err(err) = self.persist_locked(&guard, "agent.updated", None) {
            guard[idx] = prev;
            return Err(err);
        }
        Ok(guard[idx].clone())
    }

    pub fn clear_session_setting_reset(
        &self,
        agent_id: &str,
        identity: &str,
    ) -> StorageResult<()> {
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, agent_id)?;
        let Some(current) = guard[idx].session_setting_resets.clone() else {
            return Ok(());
        };
        if !current.iter().any(|v| v == identity) {
            return Ok(());
        }
        let next: Vec<String> = current.into_iter().filter(|v| v != identity).collect();
        let prev = guard[idx].session_setting_resets.clone();
        guard[idx].session_setting_resets = if next.is_empty() { None } else { Some(next) };
        if let Err(err) = self.persist_locked(&guard, "agent.updated", None) {
            guard[idx].session_setting_resets = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn update_agent(
        &self,
        input: &UpdateAgentInput,
        initiating_agent_id: Option<&str>,
    ) -> StorageResult<AgentSummary> {
        self.update_agent_extended(
            &ExtendedUpdateAgentInput {
                base: input.clone(),
                ..Default::default()
            },
            initiating_agent_id,
        )
    }

    pub fn update_agent_extended(
        &self,
        ext: &ExtendedUpdateAgentInput,
        initiating_agent_id: Option<&str>,
    ) -> StorageResult<AgentSummary> {
        let input = &ext.base;
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, &input.agent_id)?;
        let prev = guard[idx].clone();
        let mut next = prev.clone();

        if let Some(name) = &input.name {
            next.name = required_trimmed_text(name, "Agent name", INPUT_LIMITS.agent_name)?;
        }
        if let Some(title) = &input.title {
            next.title = limited_trimmed_text(title, "Agent title", INPUT_LIMITS.agent_title)?;
        }
        if let Some(desc) = &input.description {
            next.description =
                limited_trimmed_text(desc, "Agent description", INPUT_LIMITS.agent_description)?;
        }
        if let Some(notif) = input.notifications {
            next.notifications = notif;
        }
        if let Some(provider) = input.provider {
            next.provider = provider;
        }
        if let Some(model) = &input.model {
            if !is_agent_model(model) {
                return Err(StorageError::Validation("Invalid agent model.".to_string()));
            }
            next.model = model.clone();
        }
        if let Some(effort) = input.reasoning_effort {
            next.reasoning_effort = effort;
        }
        if let Some(access) = input.access {
            next.access = Some(access);
        }
        if let Some(folders) = &ext.additional_folders {
            if !is_agent_additional_folders(folders) {
                return Err(StorageError::Validation("Invalid agent folders.".to_string()));
            }
            next.additional_folders = if folders.is_empty() { None } else { Some(folders.clone()) };
        }
        if let Some(folders) = &ext.excluded_folders {
            if !is_agent_additional_folders(folders) {
                return Err(StorageError::Validation("Invalid agent folders.".to_string()));
            }
            next.excluded_folders = if folders.is_empty() { None } else { Some(folders.clone()) };
        }
        if let Some(cu) = input.computer_use {
            next.computer_use = Some(cu);
        }
        if let Some(aa) = input.allow_automation {
            next.allow_automation = aa;
        }
        if let Some(anc) = input.allow_new_chat {
            next.allow_new_chat = anc;
        }
        if let Some(mem) = input.memory {
            next.memory = if mem { None } else { Some(false) };
        }
        if let Some(qa) = input.quick_acknowledgement {
            next.quick_acknowledgement = qa;
        }
        if let Some(obs) = ext.own_browser_session {
            next.own_browser_session = Some(obs);
            next.browser_profile_id = None;
        }
        if let Some(bp_opt) = &ext.browser_profile_id {
            match bp_opt {
                None => next.browser_profile_id = None,
                Some(bp) => {
                    if !is_browser_profile_id(bp) {
                        return Err(StorageError::Validation("Invalid browser profile.".to_string()));
                    }
                    next.browser_profile_id = Some(bp.clone());
                    next.own_browser_session = Some(true);
                }
            }
        }
        apply_update_runtime_and_appearance(&prev, &mut next, ext)?;
        next.updated_at = Some(now_iso());

        let model_changed = next.provider != prev.provider
            || next.model != prev.model
            || next.reasoning_effort != prev.reasoning_effort;
        let model_change = initiating_agent_id.and_then(|init_id| {
            model_changed.then(|| AgentModelChange {
                initiating_agent_id: init_id.to_string(),
                target_agent_id: prev.id.clone(),
                previous: AgentModelRuntime {
                    provider: prev.provider,
                    model: prev.model.clone(),
                    reasoning_effort: prev.reasoning_effort,
                },
                next: AgentModelRuntime {
                    provider: next.provider,
                    model: next.model.clone(),
                    reasoning_effort: next.reasoning_effort,
                },
            })
        });
        let event_type = if model_change.is_some() {
            "agent.model-changed"
        } else {
            "agent.updated"
        };
        guard[idx] = next.clone();
        if let Err(err) = self.persist_locked(&guard, event_type, model_change.as_ref()) {
            guard[idx] = prev;
            return Err(err);
        }
        Ok(next)
    }

    pub fn set_avatar(
        &self,
        agent_id: &str,
        image: Option<&AvatarImageInput>,
    ) -> StorageResult<AgentSummary> {
        let mut guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, agent_id)?;
        let prev_avatar = resolve_avatar_from_record(&self.inner.avatars_root, &guard[idx]);
        let prev = guard[idx].clone();

        let Some(image) = image else {
            guard[idx].avatar_url = None;
            guard[idx].updated_at = Some(now_iso());
            if let Err(err) = self.persist_locked(&guard, "agent.avatar-removed", None) {
                guard[idx] = prev;
                return Err(err);
            }
            if let Some(old) = prev_avatar {
                let _ = fs::remove_file(old.path);
            }
            return Ok(guard[idx].clone());
        };

        let mime_str = match image.mime_type {
            AvatarMimeType::Png => "image/png",
            AvatarMimeType::Jpeg => "image/jpeg",
            AvatarMimeType::Webp => "image/webp",
        };
        if !is_valid_avatar_image(mime_str, &image.bytes) {
            return Err(StorageError::Validation("Invalid avatar image.".to_string()));
        }
        let version = Uuid::new_v4().to_string();
        let ext = avatar_file_extension(image.mime_type);
        let dir = self.inner.avatars_root.join(&guard[idx].id);
        create_dir_0700(&dir)?;
        let target = dir.join(format!("{version}.{ext}"));
        write_file_atomically_0600(&target, &image.bytes)?;

        guard[idx].avatar_url = Some(agent_avatar_url(&guard[idx].id, &version, image.mime_type));
        guard[idx].updated_at = Some(now_iso());
        if let Err(err) = self.persist_locked(&guard, "agent.avatar-updated", None) {
            guard[idx] = prev;
            let _ = fs::remove_file(&target);
            return Err(err);
        }
        if let Some(old) = prev_avatar {
            let _ = fs::remove_file(old.path);
        }
        Ok(guard[idx].clone())
    }

    pub fn resolve_avatar(&self, agent_id: &str) -> StorageResult<Option<ResolvedAgentAvatar>> {
        let guard = self.inner.state.lock();
        let idx = require_agent_index(&guard, agent_id)?;
        Ok(resolve_avatar_from_record(
            &self.inner.avatars_root,
            &guard[idx],
        ))
    }

    pub fn delete_agent(&self, id: &str) -> StorageResult<Option<AgentSummary>> {
        if !is_valid_agent_id(id) {
            return Err(StorageError::Validation("Invalid agent id.".to_string()));
        }
        let mut guard = self.inner.state.lock();
        let existing = guard.iter().find(|a| a.id == id).cloned();
        let mut paths_to_remove = vec![
            self.inner.avatars_root.join(id),
            PathBuf::from(format!("{}.openbot-stage", self.inner.avatars_root.join(id).display())),
            self.inner.agents_root.join(id),
            PathBuf::from(format!("{}.openbot-stage", self.inner.agents_root.join(id).display())),
            self.inner.legacy_agents_root.join(id),
        ];
        let legacy = legacy_agent_id(id);
        if legacy != id && !guard.iter().any(|a| a.id == legacy) {
            paths_to_remove.push(self.inner.avatars_root.join(&legacy));
            paths_to_remove.push(self.inner.legacy_agents_root.join(&legacy));
        }
        let mut outbox_paths = Vec::new();
        for p in &paths_to_remove {
            if p.exists() && fs::remove_dir_all(p).is_err() {
                outbox_paths.push(p.to_string_lossy().into_owned());
            }
        }
        let _ = fs::remove_file(self.duplication_marker_path(id));

        let remaining: Vec<AgentSummary> = guard.iter().filter(|a| a.id != id).cloned().collect();
        let thread_id = existing.as_ref().and_then(|a| a.thread_id.as_deref());
        self.inner.roster.hard_delete_agent(
            &format!("agents:hard-delete:{}", Uuid::new_v4()),
            id,
            thread_id,
            &remaining,
            &outbox_paths,
        )?;
        *guard = remaining;
        Ok(existing)
    }

    pub fn hard_delete_agent(&self, id: &str) -> StorageResult<Option<AgentSummary>> {
        self.delete_agent(id)
    }

    /// Atomic `.openbot-stage` workspace & avatar duplication (`agent-store.ts:L348-L498`).
    pub fn duplicate_agent(
        &self,
        source_id: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<AgentSummary> {
        let op_id = match operation_id {
            Some(raw) => {
                if !is_uuid_v4(raw) {
                    return Err(StorageError::Validation(
                        "Invalid agent duplication operation id.".to_string(),
                    ));
                }
                raw.to_string()
            }
            None => Uuid::new_v4().to_string(),
        };
        if self
            .inner
            .database
            .command_result::<serde_json::Value>(&duplication_command_id(&op_id))?
            .is_some()
        {
            return Err(StorageError::Conflict(
                "This agent duplication operation is already committed.".to_string(),
            ));
        }

        let (source, source_avatar, initial_copy_name) = {
            let guard = self.inner.state.lock();
            if guard.len() >= INPUT_LIMITS.agents {
                return Err(StorageError::LimitExceeded(format!(
                    "Agent host limit of {} reached.",
                    INPUT_LIMITS.agents
                )));
            }
            let idx = require_agent_index(&guard, source_id)?;
            let src = guard[idx].clone();
            let avatar = resolve_avatar_from_record(&self.inner.avatars_root, &src);
            let copy_name = duplicate_agent_name(&src.name, &guard)?;
            (src, avatar, copy_name)
        };

        create_dir_0700(Path::new(&source.workspace_path))?;
        let source_profile_sig = duplication_profile_signature(&source)?;
        let source_workspace_fp = workspace_metadata_fingerprint(Path::new(&source.workspace_path))?;
        let source_avatar_fp = match &source_avatar {
            Some(av) if av.path.exists() => Some(file_fingerprint(&av.path)?),
            _ => None,
        };

        let new_id = format!("agent-{}", Uuid::new_v4());
        let mut record = build_duplicated_record(
            &self.inner.agents_root,
            &new_id,
            initial_copy_name,
            &source,
        )?;
        let final_workspace = PathBuf::from(&record.workspace_path);
        let staged_workspace = PathBuf::from(format!("{}.openbot-stage", final_workspace.display()));
        let final_avatar_dir = self.inner.avatars_root.join(&new_id);
        let staged_avatar_dir =
            PathBuf::from(format!("{}.openbot-stage", final_avatar_dir.display()));
        let marker_path = self.duplication_marker_path(&new_id);

        let stage_result = (|| -> StorageResult<AgentSummary> {
            write_pending_marker_exclusive(&marker_path, &op_id, source_id)?;
            copy_dir_preserving_symlinks(Path::new(&source.workspace_path), &staged_workspace)?;
            let mut staged_avatar_file: Option<PathBuf> = None;
            if let Some(av) = &source_avatar
                && source_avatar_fp.is_some()
            {
                create_dir_0700(&staged_avatar_dir)?;
                let ext = avatar_file_extension(av.mime_type);
                let dst = staged_avatar_dir.join(format!("{}.{ext}", av.version));
                fs::copy(&av.path, &dst)?;
                record.avatar_url = Some(agent_avatar_url(&new_id, &av.version, av.mime_type));
                staged_avatar_file = Some(dst);
            }

            let current_source = self
                .get(source_id)
                .ok_or_else(|| StorageError::NotFound(format!("Unknown agent: {source_id}")))?;
            let staged_av_fp = match &staged_avatar_file {
                Some(p) => Some(file_fingerprint(p)?),
                None => None,
            };
            let current_av_fp = match &source_avatar {
                Some(av) if source_avatar_fp.is_some() => Some(file_fingerprint(&av.path)?),
                _ => None,
            };
            if duplication_profile_signature(&current_source)? != source_profile_sig
                || workspace_metadata_fingerprint(Path::new(&source.workspace_path))?
                    != source_workspace_fp
                || staged_av_fp != source_avatar_fp
                || current_av_fp != source_avatar_fp
            {
                return Err(StorageError::Conflict(
                    "The agent changed while it was being duplicated.".to_string(),
                ));
            }

            rewrite_internal_workspace_symlinks(
                Path::new(&source.workspace_path),
                &staged_workspace,
                &final_workspace,
            )?;

            let mut guard = self.inner.state.lock();
            if guard.len() >= INPUT_LIMITS.agents {
                return Err(StorageError::LimitExceeded(format!(
                    "Agent host limit of {} reached.",
                    INPUT_LIMITS.agents
                )));
            }
            record.name = duplicate_agent_name(&source.name, &guard)?;
            fs::rename(&staged_workspace, &final_workspace)?;
            if staged_avatar_file.is_some() {
                fs::rename(&staged_avatar_dir, &final_avatar_dir)?;
            }
            guard.insert(0, record.clone());
            if let Err(err) = self.persist_locked(&guard, "agent.duplicated", None) {
                guard.retain(|a| a.id != new_id);
                return Err(err);
            }
            Ok(record)
        })();

        if stage_result.is_err() {
            let _ = fs::remove_dir_all(&staged_workspace);
            let _ = fs::remove_dir_all(&final_workspace);
            let _ = fs::remove_dir_all(&staged_avatar_dir);
            let _ = fs::remove_dir_all(&final_avatar_dir);
            let _ = fs::remove_file(&marker_path);
        }
        stage_result
    }

    pub fn committed_agent_duplication(
        &self,
        operation_id: &str,
        source_agent_id: &str,
    ) -> StorageResult<Option<DuplicateAgentResult>> {
        if !is_uuid_v4(operation_id) {
            return Err(StorageError::Validation(
                "Invalid agent duplication operation id.".to_string(),
            ));
        }
        let Some((_seq, value)) = self
            .inner
            .database
            .command_result::<serde_json::Value>(&duplication_command_id(operation_id))?
        else {
            return Ok(None);
        };
        let receipt_source = value
            .get("sourceAgentId")
            .or_else(|| value.get("sourceBotId"))
            .and_then(|v| v.as_str());
        let result_obj = value.get("result").and_then(|v| v.as_object());
        if receipt_source != Some(source_agent_id) || result_obj.is_none() {
            return Err(StorageError::Validation(
                "The agent duplication receipt is invalid.".to_string(),
            ));
        }
        let result_map = result_obj.expect("checked Some");
        let agent_val = result_map
            .get("agent")
            .or_else(|| result_map.get("bot"))
            .ok_or_else(|| {
                StorageError::Validation("The agent duplication receipt is invalid.".to_string())
            })?;
        let layout_val = result_map.get("layout").cloned().ok_or_else(|| {
            StorageError::Validation("The agent duplication receipt is invalid.".to_string())
        })?;
        let (parsed_agent, _) = read_stored_agent_value(agent_val)?;
        let live_agent = self.get(&parsed_agent.id).ok_or_else(|| {
            StorageError::NotFound("The duplicated agent no longer exists.".to_string())
        })?;
        Ok(Some(DuplicateAgentResult {
            agent: live_agent,
            layout: layout_val,
        }))
    }

    pub fn commit_agent_duplication(
        &self,
        id: &str,
        operation_id: &str,
        source_agent_id: &str,
        layout: serde_json::Value,
    ) -> StorageResult<DuplicateAgentResult> {
        let agent = self
            .get(id)
            .ok_or_else(|| StorageError::NotFound(format!("Unknown agent: {id}")))?;
        let marker = self.read_duplication_marker(id);
        let valid_marker = marker.as_ref().is_some_and(|m| {
            m.operation_id == operation_id && m.source_agent_id == source_agent_id
        });
        if !valid_marker {
            return Err(StorageError::Validation(
                "This agent duplication marker is invalid.".to_string(),
            ));
        }
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agent-duplications".to_string(),
            aggregate_id: id.to_string(),
            event_type: "agent-duplication.committed".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({
                "sourceAgentId": source_agent_id,
                "duplicateAgentId": id,
            }),
        }];
        let expected_result = DuplicateAgentResult {
            agent: agent.clone(),
            layout,
        };
        let receipt = self.inner.database.dispatch_with_projection(
            &duplication_command_id(operation_id),
            Some(operation_id),
            &events,
            |_conn, _sequences| {
                Ok(serde_json::json!({
                    "sourceAgentId": source_agent_id,
                    "result": expected_result,
                }))
            },
        )?;
        let _ = fs::remove_file(self.duplication_marker_path(id));
        let result_val = receipt
            .result
            .get("result")
            .cloned()
            .ok_or_else(|| StorageError::Validation("Invalid duplication receipt.".to_string()))?;
        let decoded: DuplicateAgentResult = serde_json::from_value(result_val)?;
        Ok(decoded)
    }

    /// Scans `agent-duplications/*.pending` and either deletes finalized markers or rolls back
    /// uncommitted duplicates (`agent-store.ts:L1265-L1311`).
    pub fn recover_pending_duplications(&self) -> StorageResult<()> {
        let entries = match fs::read_dir(&self.inner.duplications_root) {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(StorageError::Io(err)),
        };
        for entry_res in entries {
            let entry = entry_res?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let file_name = entry.file_name();
            let Some(name_str) = file_name.to_str() else {
                continue;
            };
            let Some(id) = name_str.strip_suffix(".pending") else {
                continue;
            };
            if !is_generated_agent_id(id) {
                continue;
            }
            let marker = self.read_duplication_marker(id);
            let found_agent = self.agent_by_either_spelling(id);
            let agent_id = found_agent
                .as_ref()
                .map(|a| a.id.clone())
                .unwrap_or_else(|| id.to_string());
            if let (Some(_), Some(m)) = (&found_agent, &marker)
                && let Ok(Some(committed)) =
                    self.committed_agent_duplication(&m.operation_id, &m.source_agent_id)
                && committed.agent.id == agent_id
            {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            if let Some(agent) = found_agent {
                let mut guard = self.inner.state.lock();
                guard.retain(|c| c.id != agent_id);
                let remaining = guard.clone();
                self.inner.roster.hard_delete_agent(
                    &format!("agents:duplicate-recovery:{}", Uuid::new_v4()),
                    &agent_id,
                    agent.thread_id.as_deref(),
                    &remaining,
                    &[],
                )?;
            }
            let names: Vec<String> = if agent_id == id {
                vec![id.to_string()]
            } else {
                vec![id.to_string(), agent_id]
            };
            for root in [
                &self.inner.agents_root,
                &self.inner.legacy_agents_root,
                &self.inner.avatars_root,
            ] {
                for n in &names {
                    let target = root.join(n);
                    let staged = PathBuf::from(format!("{}.openbot-stage", target.display()));
                    let _ = fs::remove_dir_all(&target);
                    let _ = fs::remove_dir_all(&staged);
                }
            }
            let _ = fs::remove_file(entry.path());
        }
        Ok(())
    }

    fn restore_roster_from_events(&self) -> StorageResult<()> {
        let replayed = self.inner.roster.latest_roster_agents()?;
        if replayed.is_empty() {
            return Ok(());
        }
        let mut readable = Vec::new();
        for val in &replayed {
            if let Ok((agent, _)) = read_stored_agent_value(val) {
                readable.push(agent);
            }
        }
        let mut guard = self.inner.state.lock();
        let present: HashSet<String> = guard.iter().map(|a| a.id.clone()).collect();
        let restored: Vec<AgentSummary> = readable
            .into_iter()
            .filter(|a| !present.contains(&a.id))
            .collect();
        if restored.is_empty() {
            return Ok(());
        }
        guard.extend(restored);
        self.persist_locked(&guard, "agents.replaced", None)
    }

    fn reset_claude_fast_mode_once(&self) -> StorageResult<()> {
        if self
            .inner
            .database
            .command_result::<serde_json::Value>(CLAUDE_FAST_MODE_RESET_COMMAND_ID)?
            .is_some()
        {
            return Ok(());
        }
        let mut guard = self.inner.state.lock();
        let agent_ids: Vec<String> = guard
            .iter()
            .filter(|a| a.provider == AgentProviderId::Claude && a.fast_mode == Some(true))
            .map(|a| a.id.clone())
            .collect();
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agent-fast-mode".to_string(),
            aggregate_id: "claude-reset".to_string(),
            event_type: "agent-fast-mode.claude-reset".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({ "agentIds": agent_ids }),
        }];
        let prev = guard.clone();
        if !agent_ids.is_empty() {
            for agent in guard.iter_mut() {
                if agent_ids.contains(&agent.id) {
                    agent.fast_mode = Some(false);
                    agent.ultrafast = None;
                }
            }
        }
        let res = self.inner.database.dispatch_with_projection(
            CLAUDE_FAST_MODE_RESET_COMMAND_ID,
            None,
            &events,
            |_conn, _seq| {
                if !agent_ids.is_empty() {
                    self.persist_locked(&guard, "agents.claude-fast-mode-reset", None)?;
                }
                Ok(serde_json::Value::Null)
            },
        );
        if let Err(err) = res {
            *guard = prev;
            return Err(err);
        }
        Ok(())
    }

    fn reconcile_unclaimed_threads(&self) -> StorageResult<()> {
        let unclaimed = self.inner.roster.unclaimed_threads()?;
        if unclaimed.is_empty() {
            return Ok(());
        }
        let mut guard = self.inner.state.lock();
        let mut adopted = 0usize;
        for entry in &unclaimed {
            if let Some(agent) = guard
                .iter_mut()
                .find(|a| a.id == entry.agent_id || legacy_agent_id(&a.id) == entry.agent_id)
                && agent.thread_id.is_none()
            {
                agent.thread_id = Some(entry.thread_id.clone());
                adopted += 1;
            }
        }
        if adopted > 0 {
            self.persist_locked(&guard, "thread.reclaimed", None)?;
        }
        Ok(())
    }

    fn agent_by_either_spelling(&self, id: &str) -> Option<AgentSummary> {
        let guard = self.inner.state.lock();
        guard
            .iter()
            .find(|a| a.id == id)
            .or_else(|| guard.iter().find(|a| legacy_agent_id(&a.id) == id))
            .cloned()
    }

    fn duplication_marker_path(&self, id: &str) -> PathBuf {
        self.inner.duplications_root.join(format!("{id}.pending"))
    }

    fn read_duplication_marker(&self, id: &str) -> Option<AgentDuplicationMarker> {
        let raw = fs::read_to_string(self.duplication_marker_path(id)).ok()?;
        let val: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let op_id = val.get("operationId")?.as_str()?;
        let stored_source = val
            .get("sourceAgentId")
            .or_else(|| val.get("sourceBotId"))?
            .as_str()?;
        if !is_uuid_v4(op_id) || stored_source.is_empty() {
            return None;
        }
        let resolved_source = self
            .agent_by_either_spelling(stored_source)
            .map(|a| a.id)
            .unwrap_or_else(|| stored_source.to_string());
        Some(AgentDuplicationMarker {
            operation_id: op_id.to_string(),
            source_agent_id: resolved_source,
        })
    }

    fn create_default_record(
        &self,
        id: &str,
        name: String,
        title: String,
        description: String,
    ) -> StorageResult<AgentSummary> {
        if !is_valid_agent_id(id) {
            return Err(StorageError::Validation("Invalid agent id.".to_string()));
        }
        let workspace_path = self
            .inner
            .agents_root
            .join(id)
            .to_string_lossy()
            .into_owned();
        let mut record = AgentSummary::new_default(id, name, workspace_path);
        record.title = title;
        record.description = description;
        record.provider = DEFAULT_AGENT_PROVIDER;
        record.model = DEFAULT_AGENT_MODEL.to_string();
        record.reasoning_effort = DEFAULT_REASONING_EFFORT;
        record.access = Some(DEFAULT_AGENT_ACCESS);
        record.computer_use = Some(true);
        record.preview = DEFAULT_AGENT_PREVIEW.to_string();
        Ok(record)
    }

    fn persist_locked(
        &self,
        agents: &[AgentSummary],
        event_type: &str,
        model_change: Option<&AgentModelChange>,
    ) -> StorageResult<()> {
        let command_id = format!("agents:{event_type}:{}", Uuid::new_v4());
        self.inner
            .roster
            .replace_agents(&command_id, agents, event_type, model_change)
    }
}

fn require_agent_index(agents: &[AgentSummary], id: &str) -> StorageResult<usize> {
    agents
        .iter()
        .position(|a| a.id == id)
        .ok_or_else(|| StorageError::NotFound(format!("Unknown agent: {id}")))
}

fn required_trimmed_text(value: &str, label: &str, max_len: usize) -> StorageResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(StorageError::Validation(format!("{label} is required.")));
    }
    if !is_bounded_string(trimmed, max_len) {
        return Err(StorageError::Validation(format!("{label} is too long.")));
    }
    Ok(trimmed.to_string())
}

fn limited_trimmed_text(value: &str, label: &str, max_len: usize) -> StorageResult<String> {
    let trimmed = value.trim();
    if !is_bounded_string(trimmed, max_len) {
        return Err(StorageError::Validation(format!("{label} is too long.")));
    }
    Ok(trimmed.to_string())
}

fn title_from_id(id: &str) -> String {
    id.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn apply_avatar_color(
    agent: &mut AgentSummary,
    hue: Option<AvatarHue>,
    color: Option<&Option<String>>,
    hue_changed: bool,
) {
    if let Some(Some(hex)) = color {
        agent.avatar_color = Some(Some(hex.clone()));
        agent.avatar_hue = nearest_avatar_hue(hex);
        return;
    }
    agent.avatar_hue = hue;
    if matches!(color, Some(None)) || hue_changed {
        agent.avatar_color = None;
    }
}

fn resolve_avatar_from_record(
    avatars_root: &Path,
    agent: &AgentSummary,
) -> Option<ResolvedAgentAvatar> {
    let url = agent.avatar_url.as_deref()?;
    let (version, mime_type) = parse_agent_avatar_url(url, &agent.id)?;
    let ext = avatar_file_extension(mime_type);
    Some(ResolvedAgentAvatar {
        path: avatars_root.join(&agent.id).join(format!("{version}.{ext}")),
        mime_type,
        version,
    })
}

fn apply_update_runtime_and_appearance(
    prev: &AgentSummary,
    next: &mut AgentSummary,
    ext: &ExtendedUpdateAgentInput,
) -> StorageResult<()> {
    let input = &ext.base;
    if let Some(fm) = input.fast_mode {
        next.fast_mode = Some(fm);
    } else if input.provider.is_some_and(|p| p != prev.provider) && next.fast_mode == Some(true) {
        next.fast_mode = Some(false);
    }
    if input.ultrafast.is_some() && input.fast_mode.is_none() {
        return Err(StorageError::Validation(
            "Invalid fast mode value.".to_string(),
        ));
    }
    if let Some(fm) = input.fast_mode {
        next.ultrafast = (fm && input.ultrafast == Some(true)).then_some(true);
    }
    if next.fast_mode != Some(true) {
        next.ultrafast = None;
    }

    match &input.provider_account_id {
        Some(None) => next.provider_account_id = None,
        Some(Some(acc)) if acc == DEFAULT_PROVIDER_ACCOUNT_ID => next.provider_account_id = None,
        Some(Some(acc)) => {
            if !is_provider_account_id(acc) {
                return Err(StorageError::Validation(
                    "Invalid provider account.".to_string(),
                ));
            }
            next.provider_account_id = Some(acc.clone());
        }
        None if input.provider.is_some_and(|p| p != prev.provider) => {
            next.provider_account_id = None;
        }
        None => {}
    }

    if let Some(bmm) = input.busy_message_mode {
        next.busy_message_mode = bmm;
    }
    if let Some(tl_opt) = input.task_limit {
        if let Some(tl) = tl_opt
            && !is_agent_task_limit(tl)
        {
            return Err(StorageError::Validation("Invalid task limit.".to_string()));
        }
        next.task_limit = tl_opt;
    }
    if let Some(bm_opt) = &input.backup_models {
        if let Some(bm) = bm_opt
            && !bm.is_valid()
        {
            return Err(StorageError::Validation(
                "Invalid backup models.".to_string(),
            ));
        }
        next.backup_models = bm_opt.clone();
    }
    if let Some(seed) = &input.avatar_seed {
        if !is_avatar_seed(seed) {
            return Err(StorageError::Validation("Invalid avatar seed.".to_string()));
        }
        next.avatar_seed = seed.clone();
    }
    if let Some(Some(color)) = &input.avatar_color
        && !is_avatar_color(color)
    {
        return Err(StorageError::Validation(
            "Invalid avatar color.".to_string(),
        ));
    }
    if input.avatar_hue.is_some() || input.avatar_color.is_some() {
        let hue = input.avatar_hue.unwrap_or(next.avatar_hue);
        let hue_changed = hue != next.avatar_hue;
        apply_avatar_color(next, hue, input.avatar_color.as_ref(), hue_changed);
    }
    if let Some(cc_opt) = &input.chat_color {
        if let Some(cc) = cc_opt
            && !is_avatar_color(cc)
        {
            return Err(StorageError::Validation("Invalid chat color.".to_string()));
        }
        next.chat_color = cc_opt.clone();
    }

    if matches!(ext.usage_limit_return, Some(None))
        || input.leave_usage_limit == Some(true)
        || (ext.usage_limit_return.is_none()
            && (next.provider != prev.provider
                || next.model != prev.model
                || next.provider_account_id != prev.provider_account_id))
    {
        next.usage_limit_return = None;
    } else if let Some(Some(ulr)) = &ext.usage_limit_return {
        if !ulr.is_valid() {
            return Err(StorageError::Validation(
                "Invalid usage limit model.".to_string(),
            ));
        }
        next.usage_limit_return = Some(ulr.clone());
    }

    match &ext.pending_runtime {
        Some(None) => next.pending_runtime = None,
        Some(Some(pr)) => {
            if !pr.is_valid() {
                return Err(StorageError::Validation(
                    "Invalid pending runtime.".to_string(),
                ));
            }
            next.pending_runtime = Some(pr.clone());
        }
        None if next.provider != prev.provider
            || next.provider_account_id != prev.provider_account_id =>
        {
            next.pending_runtime = None;
        }
        None => {}
    }

    match &ext.pinned_account_id {
        Some(None) => next.pinned_account_id = None,
        Some(Some(pa)) => {
            if !is_provider_account_id(pa) {
                return Err(StorageError::Validation(
                    "Invalid pinned account.".to_string(),
                ));
            }
            next.pinned_account_id = Some(pa.clone());
        }
        None if (next.provider != prev.provider
            || next.provider_account_id != prev.provider_account_id)
            && next
                .provider_account_id
                .as_deref()
                .unwrap_or(DEFAULT_PROVIDER_ACCOUNT_ID)
                != next.pinned_account_id.as_deref().unwrap_or("") =>
        {
            next.pinned_account_id = None;
        }
        None => {}
    }
    Ok(())
}

fn build_duplicated_record(
    agents_root: &Path,
    new_id: &str,
    copy_name: String,
    source: &AgentSummary,
) -> StorageResult<AgentSummary> {
    if !is_valid_agent_id(new_id) {
        return Err(StorageError::Validation("Invalid agent id.".to_string()));
    }
    let workspace_path = agents_root.join(new_id).to_string_lossy().into_owned();
    let mut record = AgentSummary::new_default(new_id, copy_name, &workspace_path);
    record.title = source.title.clone();
    record.description = source.description.clone();
    record.notifications = source.notifications;
    record.provider = source.provider;
    record.model = source.model.clone();
    record.reasoning_effort = source.reasoning_effort;
    record.session_setting_overrides = source.session_setting_overrides.clone();
    record.access = Some(source.access.unwrap_or(DEFAULT_AGENT_ACCESS));
    record.additional_folders = source.additional_folders.clone();
    record.excluded_folders = source.excluded_folders.as_ref().map(|folders| {
        folders
            .iter()
            .map(|f| rebase_folder_if_inside(&source.workspace_path, &workspace_path, f))
            .collect()
    });
    record.computer_use = Some(source.computer_use.unwrap_or(true));
    if source.memory == Some(false) {
        record.memory = Some(false);
    }
    record.busy_message_mode = source.busy_message_mode;
    record.task_limit = source.task_limit;
    record.backup_models = source.backup_models.clone();
    if let Some(bp) = &source.browser_profile_id {
        record.browser_profile_id = Some(bp.clone());
        record.own_browser_session = Some(true);
    }
    record.avatar_seed = source.avatar_seed.clone();
    record.avatar_hue = source.avatar_hue;
    record.avatar_color = source.avatar_color.clone();
    record.chat_color = source.chat_color.clone();
    Ok(record)
}

fn rebase_folder_if_inside(source_ws: &str, target_ws: &str, folder: &str) -> String {
    if !folder_inside(source_ws, folder) {
        return folder.to_string();
    }
    if let Ok(rel) = Path::new(folder).strip_prefix(Path::new(source_ws)) {
        return Path::new(target_ws).join(rel).to_string_lossy().into_owned();
    }
    folder.to_string()
}

fn read_stored_agent_value(
    value: &serde_json::Value,
) -> StorageResult<(AgentSummary, Vec<&'static str>)> {
    let Some(obj) = value.as_object() else {
        return Err(StorageError::Validation(
            "Stored agent profile is unreadable (profile).".to_string(),
        ));
    };
    let Some(id) = obj.get("id").and_then(|v| v.as_str()) else {
        return Err(StorageError::Validation(
            "Stored agent profile is unreadable (id).".to_string(),
        ));
    };
    if !is_valid_agent_id(id) {
        return Err(StorageError::Validation(format!(
            "Stored agent profile {id} has an invalid id."
        )));
    }
    let Some(workspace_path) = obj.get("workspacePath").and_then(|v| v.as_str()) else {
        return Err(StorageError::Validation(format!(
            "Stored agent profile {id} is missing workspacePath."
        )));
    };
    let thread_id = match obj.get("threadId") {
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if is_identifier(s) => Some(s.clone()),
        _ => {
            return Err(StorageError::Validation(format!(
                "Stored agent profile {id} has an invalid threadId."
            )));
        }
    };

    if let Ok(mut parsed) = serde_json::from_value::<AgentSummary>(value.clone())
        && parsed.validate()
    {
        if parsed.access.is_none() {
            parsed.access = Some(DEFAULT_AGENT_ACCESS);
        }
        if parsed.computer_use.is_none() {
            parsed.computer_use = Some(true);
        }
        if let Some(url) = &parsed.avatar_url
            && parse_agent_avatar_url(url, &parsed.id).is_none()
        {
            parsed.avatar_url = None;
        }
        return Ok((parsed, Vec::new()));
    }

    let mut repaired = Vec::new();
    let mut record = AgentSummary::new_default(id, title_from_id(id), workspace_path);
    record.thread_id = thread_id;
    record.access = Some(DEFAULT_AGENT_ACCESS);
    record.computer_use = Some(true);

    match obj.get("name").and_then(|v| v.as_str()) {
        Some(n) if is_bounded_string(n, INPUT_LIMITS.agent_name) => record.name = n.to_string(),
        _ => repaired.push("name"),
    }
    match obj.get("title").and_then(|v| v.as_str()) {
        Some(t) if is_bounded_string(t, INPUT_LIMITS.agent_title) => record.title = t.to_string(),
        _ => repaired.push("title"),
    }
    match obj.get("description").and_then(|v| v.as_str()) {
        Some(d) if is_bounded_string(d, INPUT_LIMITS.agent_description) => {
            record.description = d.to_string();
        }
        _ => repaired.push("description"),
    }
    match obj.get("notifications").and_then(|v| v.as_bool()) {
        Some(b) => record.notifications = b,
        None => repaired.push("notifications"),
    }
    if let Some(p_val) = obj.get("provider") {
        match serde_json::from_value::<AgentProviderId>(p_val.clone()) {
            Ok(p) => record.provider = p,
            Err(_) => repaired.push("provider"),
        }
    }
    match obj.get("model").and_then(|v| v.as_str()) {
        Some(m) if is_agent_model(m) => record.model = m.to_string(),
        _ => repaired.push("model"),
    }
    if let Some(re_val) = obj.get("reasoningEffort") {
        match serde_json::from_value::<AgentReasoningEffort>(re_val.clone()) {
            Ok(re) => record.reasoning_effort = re,
            Err(_) => repaired.push("reasoningEffort"),
        }
    }
    if let Some(a_val) = obj.get("access") {
        match serde_json::from_value::<AgentAccess>(a_val.clone()) {
            Ok(a) => record.access = Some(a),
            Err(_) => repaired.push("access"),
        }
    }
    if let Some(af_val) = obj.get("additionalFolders") {
        if let Ok(af) = serde_json::from_value::<Vec<String>>(af_val.clone())
            && is_agent_additional_folders(&af)
        {
            if !af.is_empty() {
                record.additional_folders = Some(af);
            }
        } else {
            repaired.push("additionalFolders");
            if let Some(arr) = af_val.as_array() {
                let salvaged: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .filter(|s| !s.is_empty() && is_bounded_string(s, INPUT_LIMITS.path))
                    .take(AGENT_FOLDER_LIMIT)
                    .map(str::to_string)
                    .collect();
                if !salvaged.is_empty() {
                    record.additional_folders = Some(salvaged);
                }
            }
        }
    }
    if let Some(ef_val) = obj.get("excludedFolders") {
        if let Ok(ef) = serde_json::from_value::<Vec<String>>(ef_val.clone())
            && is_agent_additional_folders(&ef)
        {
            if !ef.is_empty() {
                record.excluded_folders = Some(ef);
            }
        } else {
            repaired.push("excludedFolders");
        }
    }
    if let Some(cu_val) = obj.get("computerUse") {
        match cu_val.as_bool() {
            Some(cu) => record.computer_use = Some(cu),
            None => repaired.push("computerUse"),
        }
    }
    if let Some(mem_val) = obj.get("memory") {
        if mem_val.as_bool() == Some(false) {
            record.memory = Some(false);
        } else {
            repaired.push("memory");
        }
    }
    if let Some(bm_val) = obj.get("backupModels") {
        match serde_json::from_value::<AgentBackupModels>(bm_val.clone()) {
            Ok(bm) if bm.is_valid() => record.backup_models = Some(bm),
            _ => repaired.push("backupModels"),
        }
    }
    if let Some(bmm_val) = obj.get("busyMessageMode") {
        match serde_json::from_value::<BusyMessageMode>(bmm_val.clone()) {
            Ok(bmm) => record.busy_message_mode = Some(bmm),
            Err(_) => repaired.push("busyMessageMode"),
        }
    }
    if let Some(ssr_val) = obj.get("sessionSettingResets") {
        match serde_json::from_value::<Vec<String>>(ssr_val.clone()) {
            Ok(ssr) if is_agent_session_setting_resets(&ssr) => {
                record.session_setting_resets = Some(ssr);
            }
            _ => repaired.push("sessionSettingResets"),
        }
    }
    match obj.get("preview").and_then(|v| v.as_str()) {
        Some(p) if is_bounded_string(p, INPUT_LIMITS.message_text) => {
            record.preview = p.to_string();
        }
        _ => repaired.push("preview"),
    }
    match obj.get("updatedAt") {
        Some(serde_json::Value::Null) => record.updated_at = None,
        Some(serde_json::Value::String(s)) if is_bounded_string(s, 160) => {
            record.updated_at = Some(s.clone());
        }
        _ => repaired.push("updatedAt"),
    }
    match obj.get("avatarSeed").and_then(|v| v.as_str()) {
        Some(s) if is_avatar_seed(s) => record.avatar_seed = s.to_string(),
        _ => repaired.push("avatarSeed"),
    }
    match obj.get("avatarHue") {
        Some(serde_json::Value::Null) => record.avatar_hue = None,
        Some(v) => match serde_json::from_value::<AvatarHue>(v.clone()) {
            Ok(h) => record.avatar_hue = Some(h),
            Err(_) => repaired.push("avatarHue"),
        },
        None => repaired.push("avatarHue"),
    }
    if let Some(url) = obj.get("avatarUrl").and_then(|v| v.as_str())
        && parse_agent_avatar_url(url, id).is_some()
    {
        record.avatar_url = Some(url.to_string());
    }
    Ok((record, repaired))
}

pub(crate) fn create_dir_0700(path: &Path) -> StorageResult<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

pub(crate) fn write_file_atomically_0600(target: &Path, bytes: &[u8]) -> StorageResult<()> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    create_dir_0700(parent)?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        target
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("openbot"),
        Uuid::new_v4()
    ));
    write_exclusive_0600(&tmp, bytes)?;
    if let Err(err) = fs::rename(&tmp, target) {
        let _ = fs::remove_file(&tmp);
        return Err(StorageError::Io(err));
    }
    Ok(())
}

fn write_exclusive_0600(path: &Path, bytes: &[u8]) -> StorageResult<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_pending_marker_exclusive(
    marker_path: &Path,
    operation_id: &str,
    source_agent_id: &str,
) -> StorageResult<()> {
    if let Some(parent) = marker_path.parent() {
        create_dir_0700(parent)?;
    }
    let payload = serde_json::json!({
        "operationId": operation_id,
        "sourceAgentId": source_agent_id,
    });
    let line = format!("{}\n", serde_json::to_string(&payload)?);
    write_exclusive_0600(marker_path, line.as_bytes())
}

/// SHA-256 directory tree metadata fingerprint (`agent-store.ts:L1743-L1766`).
pub fn workspace_metadata_fingerprint(root: &Path) -> StorageResult<String> {
    let mut hasher = Sha256::new();
    visit_workspace_metadata(root, "", &mut hasher)?;
    Ok(hex_lower(&hasher.finalize()))
}

fn visit_workspace_metadata(
    directory: &Path,
    prefix: &str,
    hasher: &mut Sha256,
) -> StorageResult<()> {
    let mut entries: Vec<(String, PathBuf)> = fs::read_dir(directory)?
        .map(|res| {
            let e = res?;
            Ok((e.file_name().to_string_lossy().into_owned(), e.path()))
        })
        .collect::<StorageResult<Vec<_>>>()?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, path) in entries {
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let meta = fs::symlink_metadata(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            hasher.update(
                format!(
                    "{rel}\0{}\0{}\0{}\0{}\0",
                    meta.mode(),
                    meta.len(),
                    meta.mtime_nsec(),
                    meta.ctime_nsec()
                )
                .as_bytes(),
            );
        }
        #[cfg(not(unix))]
        {
            hasher.update(format!("{rel}\0{}\0", meta.len()).as_bytes());
        }
        let ft = meta.file_type();
        if ft.is_symlink() {
            let target = fs::read_link(&path)?;
            hasher.update(format!("link\0{}\0", target.to_string_lossy()).as_bytes());
        } else if ft.is_dir() {
            hasher.update(b"directory\0");
            visit_workspace_metadata(&path, &rel, hasher)?;
        } else if ft.is_file() {
            hasher.update(b"file\0");
        } else {
            hasher.update(b"other\0");
        }
    }
    Ok(())
}

/// SHA-256 file mode + contents fingerprint (`agent-store.ts:L1768-L1788`).
pub fn file_fingerprint(path: &Path) -> StorageResult<String> {
    let meta = fs::symlink_metadata(path)?;
    let mut hasher = Sha256::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hasher.update(format!("{}\0", meta.mode()).as_bytes());
    }
    #[cfg(not(unix))]
    {
        hasher.update(format!("{}\0", meta.len()).as_bytes());
    }
    let bytes = fs::read(path)?;
    hasher.update(&bytes);
    Ok(hex_lower(&hasher.finalize()))
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn copy_dir_preserving_symlinks(src: &Path, dst: &Path) -> StorageResult<()> {
    if dst.exists() {
        return Err(StorageError::Conflict(format!(
            "Staging path already exists: {}",
            dst.display()
        )));
    }
    create_dir_0700(dst)?;
    for entry_res in fs::read_dir(src)? {
        let entry = entry_res?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let meta = fs::symlink_metadata(&src_path)?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            let link_target = fs::read_link(&src_path)?;
            create_symlink(&link_target, &dst_path)?;
        } else if ft.is_dir() {
            copy_dir_preserving_symlinks(&src_path, &dst_path)?;
        } else if ft.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn rewrite_internal_workspace_symlinks(
    source_root: &Path,
    staged_root: &Path,
    final_root: &Path,
) -> StorageResult<()> {
    let canonical_source = fs::canonicalize(source_root).unwrap_or_else(|_| source_root.to_path_buf());
    visit_rewrite_symlinks(
        &canonical_source,
        source_root,
        final_root,
        staged_root,
        source_root,
        final_root,
    )
}

fn visit_rewrite_symlinks(
    canonical_source_root: &Path,
    source_root: &Path,
    final_root: &Path,
    staged_dir: &Path,
    source_dir: &Path,
    final_dir: &Path,
) -> StorageResult<()> {
    for entry_res in fs::read_dir(staged_dir)? {
        let entry = entry_res?;
        let name = entry.file_name();
        let staged_path = staged_dir.join(&name);
        let source_path = source_dir.join(&name);
        let final_path = final_dir.join(&name);
        let ft = fs::symlink_metadata(&staged_path)?.file_type();
        if ft.is_symlink() {
            let target = fs::read_link(&staged_path)?;
            let resolved_source_target = if target.is_absolute() {
                target.clone()
            } else {
                source_path
                    .parent()
                    .unwrap_or(source_dir)
                    .join(&target)
            };
            let mapped_final = if let Ok(canon) = fs::canonicalize(&resolved_source_target) {
                canon
                    .strip_prefix(canonical_source_root)
                    .ok()
                    .map(|rel| final_root.join(rel))
            } else {
                resolved_source_target
                    .strip_prefix(source_root)
                    .ok()
                    .map(|rel| final_root.join(rel))
            };
            let Some(final_target) = mapped_final else {
                continue;
            };
            let rewritten = if target.is_absolute() {
                final_target
            } else {
                relative_path_between(final_path.parent().unwrap_or(final_dir), &final_target)
            };
            fs::remove_file(&staged_path)?;
            create_symlink(&rewritten, &staged_path)?;
        } else if ft.is_dir() {
            visit_rewrite_symlinks(
                canonical_source_root,
                source_root,
                final_root,
                &staged_path,
                &source_path,
                &final_path,
            )?;
        }
    }
    Ok(())
}

fn relative_path_between(from_dir: &Path, to_path: &Path) -> PathBuf {
    let from_comps: Vec<_> = from_dir.components().collect();
    let to_comps: Vec<_> = to_path.components().collect();
    let mut common = 0;
    while common < from_comps.len()
        && common < to_comps.len()
        && from_comps[common] == to_comps[common]
    {
        common += 1;
    }
    let mut rel = PathBuf::new();
    for _ in common..from_comps.len() {
        rel.push("..");
    }
    for comp in &to_comps[common..] {
        rel.push(comp.as_os_str());
    }
    if rel.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        rel
    }
}

fn create_symlink(target: &Path, link_path: &Path) -> StorageResult<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link_path)?;
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScopedTestDir(PathBuf);

    impl ScopedTestDir {
        fn new(prefix: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("{prefix}_{}", Uuid::new_v4()));
            fs::create_dir_all(&dir).expect("create temp test dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScopedTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fresh_host_has_zero_starter_bots_and_enforces_prepend_append_and_100_cap() {
        let tmp = ScopedTestDir::new("openbot_agent_store");
        let db = OpenBotDatabase::open(tmp.path().join("data").join("openbot.db")).unwrap();
        let root = tmp.path().join("OpenBot");
        let store = AgentStore::open(db, &root).unwrap();
        assert!(store.list().is_empty());

        let first = store
            .create_agent(&CreateAgentInput {
                name: "Alpha".to_string(),
                description: "First".to_string(),
                provider: Some(AgentProviderId::Codex),
                model: Some("gpt-5.4".to_string()),
                reasoning_effort: Some(AgentReasoningEffort::Medium),
                provider_account_id: None,
                avatar_seed: "alpha".to_string(),
                avatar_hue: None,
                avatar_color: None,
                initial_message: "Hello Alpha".to_string(),
                operation_id: None,
            })
            .unwrap();
        let second = store
            .create_agent(&CreateAgentInput {
                name: "Beta".to_string(),
                description: "Second".to_string(),
                provider: None,
                model: None,
                reasoning_effort: None,
                provider_account_id: None,
                avatar_seed: "beta".to_string(),
                avatar_hue: None,
                avatar_color: None,
                initial_message: "Hello Beta".to_string(),
                operation_id: None,
            })
            .unwrap();
        let appended = store
            .get_or_create("helper-bot", Some("Helper"), None)
            .unwrap();
        let ids: Vec<String> = store.list().into_iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![second.id, first.id, appended.id]);

        for idx in 3..INPUT_LIMITS.agents {
            store
                .get_or_create(&format!("bot-{idx}"), Some(&format!("Bot {idx}")), None)
                .unwrap();
        }
        assert_eq!(store.list().len(), INPUT_LIMITS.agents);
        let overflow = store.get_or_create("bot-overflow", Some("Overflow"), None);
        assert!(matches!(overflow, Err(StorageError::LimitExceeded(_))));
    }

    #[test]
    fn duplicate_agent_stages_rewrites_symlinks_recovers_pending_and_cascades_delete() {
        let tmp = ScopedTestDir::new("openbot_agent_dup");
        let data_dir = tmp.path().join("data");
        let root = tmp.path().join("OpenBot");
        let db = OpenBotDatabase::open(data_dir.join("openbot.db")).unwrap();
        let store = AgentStore::open(db.clone(), &root).unwrap();

        let src = store
            .create_agent(&CreateAgentInput {
                name: "Researcher".to_string(),
                description: "Source".to_string(),
                provider: Some(AgentProviderId::Claude),
                model: Some("claude-sonnet-4-6".to_string()),
                reasoning_effort: Some(AgentReasoningEffort::High),
                provider_account_id: None,
                avatar_seed: "researcher".to_string(),
                avatar_hue: None,
                avatar_color: None,
                initial_message: "Hello Researcher".to_string(),
                operation_id: None,
            })
            .unwrap();
        let ws = Path::new(&src.workspace_path);
        fs::write(ws.join("notes.md"), b"hello").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(ws.join("notes.md"), ws.join("abs_link.md")).unwrap();

        let op_uncommitted = Uuid::new_v4().to_string();
        let dup_uncommitted = store.duplicate_agent(&src.id, Some(&op_uncommitted)).unwrap();
        assert_eq!(dup_uncommitted.name, "Researcher copy");
        #[cfg(unix)]
        {
            let target = fs::read_link(Path::new(&dup_uncommitted.workspace_path).join("abs_link.md")).unwrap();
            assert_eq!(
                target,
                Path::new(&dup_uncommitted.workspace_path).join("notes.md")
            );
        }

        let reopened = AgentStore::open(db.clone(), &root).unwrap();
        assert!(reopened.get(&dup_uncommitted.id).is_none());

        let op_committed = Uuid::new_v4().to_string();
        let dup_ok = reopened.duplicate_agent(&src.id, Some(&op_committed)).unwrap();
        let committed = reopened
            .commit_agent_duplication(
                &dup_ok.id,
                &op_committed,
                &src.id,
                serde_json::json!({"items": []}),
            )
            .unwrap();
        assert_eq!(committed.agent.id, dup_ok.id);

        let mut upd = UpdateAgentInput {
            agent_id: src.id.clone(),
            ..Default::default()
        };
        upd.model = Some("claude-opus-4-7".to_string());
        reopened.update_agent(&upd, Some(&dup_ok.id)).unwrap();
        upd.title = Some("Updated Title".to_string());
        upd.model = None;
        reopened.update_agent(&upd, None).unwrap();

        let model_events: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM orchestration_events WHERE event_type = 'agent.model-changed'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(model_events, 1);

        reopened.delete_agent(&src.id).unwrap();
        assert!(reopened.get(&src.id).is_none());
    }
}
