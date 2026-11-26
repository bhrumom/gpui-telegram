//! Multi-Bot Group-Chat `ChannelStore` — CQRS persistence & read projections across all 12
//! `projection_channel_*` tables, 1-based monotonic `sequence`, soft-delete & `superseded`,
//! content-change summary/context invalidation, `with_seen_dependencies`, `member_folders` cache,
//! and FK-safe deterministic `ChannelStore::rebuild`.
//!
//! Direct 1:1 port of `src/backend/channel-store.ts:L1-L1339` and `src/backend/channel-history.ts:L83-L105`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use openbot_contracts::channels::{
    CHANNEL_FOLDER_LIMIT, CHANNEL_PREVIEW_LIMIT, Channel, ChannelAssignment,
    ChannelAssignmentState, ChannelContext, ChannelDraft, ChannelHistorySummary, ChannelMessage,
    ChannelMessageAuthor, ChannelPage, ChannelPreview, ChannelPreviewAuthor, ChannelSummary,
    ChannelTask, ChannelTaskState, ChannelThread, SIGNED_OUT_CHANNEL_MEMBER_ID,
    with_browser_member,
};
use openbot_contracts::conversation::{
    CHANNEL_ROUTING_EVENT_ITEM_PREFIX, CONTEXT_RESET_ITEM_TYPE, ConversationMessage,
    ConversationMessageStatus, ConversationQuestionPrompt,
};
use openbot_contracts::limits::{INPUT_LIMITS, double_option, is_bounded_string};
use openbot_contracts::queue::AttachmentSummary;
use openbot_contracts::tags::{ChatTagKind, chat_tag_references, expand_chat_tag_references};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::database::{OpenBotDatabase, OrchestrationEventInput};
use crate::error::{StorageError, StorageResult};

pub const SHOWN_MESSAGE_SQL: &str =
    "(instr(message_json, '\"deleted\"') = 0 OR json_extract(message_json, '$.deleted') IS NOT 1)";

pub const MCP_SIGN_IN_ITEM_TYPE_PREFIX: &str = "mcp-sign-in:";
pub const SETTING_LINK_ITEM_TYPE_PREFIX: &str = "setting-link:";

const CHANNEL_SUMMARY_ROWS_SQL: &str = "SELECT c.channel_json AS channel_json,
  (SELECT m.message_json FROM projection_channel_messages AS m
    WHERE m.channel_id = c.channel_id
      AND (instr(m.message_json, '\"deleted\"') = 0 OR json_extract(m.message_json, '$.deleted') IS NOT 1)
      AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') != 'plan'
      AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') != 'subagents'
      AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') != 'commentary'
    ORDER BY m.sequence DESC, m.message_id DESC LIMIT 1) AS latest_json
  FROM projection_channels AS c ORDER BY c.rowid";

const TASK_COUNTS_SQL: &str = "SELECT
    COALESCE(SUM(json_extract(task_json, '$.state') = 'running'), 0) AS running,
    COALESCE(SUM(json_extract(task_json, '$.state') IN ('queued', 'waiting', 'running')), 0) AS open
  FROM projection_channel_tasks WHERE channel_id = ?1";

const CHANNEL_THREADS_SQL: &str =
    "SELECT channel_id, agent_id, thread_id FROM projection_channel_contexts ORDER BY channel_id, agent_id";

pub const UNREAD_COUNT_SQL: &str = "SELECT COUNT(*) AS unread FROM projection_channel_messages AS m
  WHERE m.channel_id = ?1
    AND m.sequence > COALESCE(
      (SELECT r.through_sequence FROM projection_channel_reads AS r
        WHERE r.channel_id = ?2 AND r.member_id = ?3), 0)
    AND (instr(m.message_json, '\"deleted\"') = 0 OR json_extract(m.message_json, '$.deleted') IS NOT 1)
    AND json_extract(m.message_json, '$.author.id') IS NOT ?4
    AND json_extract(m.message_json, '$.author.id') IS NOT ?5
    AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') != 'plan'
    AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') != 'subagents'
    AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') != 'channel-change'
    AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') != 'context-reset'
    AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') != 'commentary'
    AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') NOT LIKE ?6
    AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') NOT LIKE ?7
    AND COALESCE(json_extract(m.message_json, '$.message.itemType'), '') NOT LIKE ?8";

const CHANNEL_PREVIEW_SOURCE_LIMIT: usize = CHANNEL_PREVIEW_LIMIT * 16;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelChange {
    pub channel: Channel,
    pub messages: Vec<ChannelMessage>,
    pub tasks: Vec<ChannelTask>,
    pub assignments: Vec<ChannelAssignment>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelUpdateChanges {
    pub messages: Vec<ChannelMessage>,
    pub tasks: Vec<ChannelTask>,
    pub assignments: Vec<ChannelAssignment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelOlderExtent {
    pub older_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelAgentWork {
    pub assignment: ChannelAssignment,
    pub channel: Channel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelExecutionThread {
    pub id: String,
    pub thread_id: String,
    pub channel_id: String,
}

pub type MemberFoldersCallback = Arc<dyn Fn(&[String]) + Send + Sync>;

#[derive(Clone, Copy, Debug, Default)]
struct TaskCounts {
    running: u64,
    open: u64,
}

#[derive(Clone, Debug)]
struct MemberFolderSource {
    key: String,
    folders: Vec<String>,
    agent_ids: Vec<String>,
}

#[derive(Default)]
struct ChannelStoreCache {
    completed_message_changes: HashMap<String, u64>,
    unread_counts: HashMap<String, HashMap<String, u64>>,
    task_counts: HashMap<String, TaskCounts>,
    counts_data_version: Option<i64>,
    member_folders: Option<HashMap<String, Vec<String>>>,
}

#[derive(Clone)]
pub struct ChannelStore {
    database: OpenBotDatabase,
    cache: Arc<Mutex<ChannelStoreCache>>,
    member_folders_changed: Option<MemberFoldersCallback>,
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn new_event(
    aggregate_type: &str,
    aggregate_id: &str,
    event_type: &str,
    payload: Value,
) -> OrchestrationEventInput {
    OrchestrationEventInput {
        event_id: Uuid::new_v4().to_string(),
        aggregate_type: aggregate_type.to_string(),
        aggregate_id: aggregate_id.to_string(),
        event_type: event_type.to_string(),
        occurred_at: now_iso(),
        payload,
    }
}

pub fn deleted_channel_message(entry: &ChannelMessage) -> ChannelMessage {
    let blank = ConversationMessage::new_simple(
        entry.message.id.clone(),
        entry.message.author,
        String::new(),
        entry.message.created_at.clone(),
        ConversationMessageStatus::Completed,
    );
    ChannelMessage {
        id: entry.id.clone(),
        channel_id: entry.channel_id.clone(),
        sequence: entry.sequence,
        author: entry.author.clone(),
        task_id: entry.task_id.clone(),
        superseded: entry.superseded,
        message: blank,
        deleted: Some(true),
        sender_task_id: None,
    }
}

#[inline]
pub fn is_deleted_channel_message(entry: &ChannelMessage) -> bool {
    entry.deleted == Some(true)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RenderedModelMessage<'a> {
    id: &'a str,
    sequence: u64,
    author: &'a ChannelMessageAuthor,
    task_id: &'a Option<String>,
    superseded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    reply_to_message_id: &'a Option<Option<String>>,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: &'a Option<Vec<AttachmentSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    question_prompt: &'a Option<ConversationQuestionPrompt>,
}

fn render_model_message(msg: &ChannelMessage, sequence: u64) -> String {
    serde_json::to_string(&RenderedModelMessage {
        id: &msg.id,
        sequence,
        author: &msg.author,
        task_id: &msg.task_id,
        superseded: false,
        reply_to_message_id: &msg.message.reply_to_message_id,
        text: &msg.message.text,
        attachments: &msg.message.attachments,
        question_prompt: &msg.message.question_prompt,
    })
    .unwrap_or_default()
}

pub fn changes_model_content(before: &ChannelMessage, after: &ChannelMessage) -> bool {
    render_model_message(before, before.sequence) != render_model_message(after, before.sequence)
}

fn decode_stored_channel(json_str: &str) -> StorageResult<Channel> {
    let raw: Value = serde_json::from_str(json_str)?;
    Channel::decode_normalized(&raw).map_err(|e| StorageError::Validation(e.to_string()))
}

fn decode_stored_message(json_str: &str) -> StorageResult<ChannelMessage> {
    serde_json::from_str::<ChannelMessage>(json_str)
        .map_err(|_| StorageError::Validation("Invalid stored channel message.".to_string()))
}

fn decode_stored_task(json_str: &str) -> StorageResult<ChannelTask> {
    serde_json::from_str::<ChannelTask>(json_str)
        .map_err(|_| StorageError::Validation("Invalid stored channel task.".to_string()))
}

fn decode_stored_assignment_value(raw: &Value) -> StorageResult<ChannelAssignment> {
    let Value::Object(mut map) = raw.clone() else {
        return Err(StorageError::Validation(
            "Invalid stored channel assignment.".to_string(),
        ));
    };
    if !map.get("requestMessageId").is_some_and(Value::is_string) {
        map.insert("requestMessageId".to_string(), Value::Null);
    }
    let valid_resources = map
        .get("resources")
        .and_then(Value::as_array)
        .is_some_and(|arr| arr.iter().all(Value::is_string));
    if !valid_resources {
        map.insert(
            "resources".to_string(),
            serde_json::json!(["host"]),
        );
    }
    serde_json::from_value::<ChannelAssignment>(Value::Object(map))
        .map_err(|_| StorageError::Validation("Invalid stored channel assignment.".to_string()))
}

fn decode_stored_assignment(json_str: &str) -> StorageResult<ChannelAssignment> {
    let raw: Value = serde_json::from_str(json_str)?;
    decode_stored_assignment_value(&raw)
}

fn assignment_state_str(state: ChannelAssignmentState) -> &'static str {
    match state {
        ChannelAssignmentState::Queued => "queued",
        ChannelAssignmentState::Starting => "starting",
        ChannelAssignmentState::Running => "running",
        ChannelAssignmentState::Completed => "completed",
        ChannelAssignmentState::Failed => "failed",
        ChannelAssignmentState::Interrupted => "interrupted",
    }
}

fn member_folder_source(
    folders: Option<&[String]>,
    agent_ids: Vec<String>,
    archived: bool,
) -> Option<MemberFolderSource> {
    let folders = folders?;
    if archived || folders.is_empty() {
        return None;
    }
    let key = serde_json::to_string(&(folders, &agent_ids)).unwrap_or_default();
    Some(MemberFolderSource {
        key,
        folders: folders.to_vec(),
        agent_ids,
    })
}

fn stored_member_folder_source(
    folders_json: Option<String>,
    members_json: Option<String>,
    archived: bool,
) -> Option<MemberFolderSource> {
    let folders: Vec<String> = folders_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .filter(|list| {
            list.len() <= CHANNEL_FOLDER_LIMIT
                && list
                    .iter()
                    .all(|p| !p.is_empty() && is_bounded_string(p, INPUT_LIMITS.path))
        })?;
    let agent_ids: Vec<String> = members_json
        .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        .map(|arr| {
            arr.into_iter()
                .filter_map(|v| v.get("agentId")?.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    member_folder_source(Some(&folders), agent_ids, archived)
}

fn slice_utf16(text: &str, max_units: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().take(max_units).collect();
    String::from_utf16_lossy(&units)
}

pub fn markdown_preview_text(body: &str) -> String {
    let refs = chat_tag_references(body);
    let mut expanded = String::with_capacity(body.len());
    let mut cursor = 0usize;
    for r in refs {
        if r.start >= cursor && r.end <= body.len() {
            expanded.push_str(&body[cursor..r.start]);
            if r.kind == ChatTagKind::Quote {
                expanded.push('“');
                expanded.push_str(&r.name);
                expanded.push('”');
            } else {
                expanded.push_str(&expand_chat_tag_references(
                    &body[r.start..r.end],
                    None::<fn(&openbot_contracts::tags::ChatTagReference) -> Option<String>>,
                    false,
                ));
            }
            cursor = r.end;
        }
    }
    expanded.push_str(&body[cursor..]);

    let mut plain = String::with_capacity(expanded.len());
    for raw_line in expanded.lines() {
        let mut line = raw_line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            continue;
        }
        if (line.chars().all(|c| c == '-' || c == ' ') && line.matches('-').count() >= 3)
            || (line.chars().all(|c| c == '*' || c == ' ') && line.matches('*').count() >= 3)
        {
            continue;
        }
        while let Some(rest) = line.strip_prefix('>') {
            line = rest.trim_start();
        }
        let hashes = line.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' ') {
            line = line[hashes + 1..].trim_start();
        } else if let Some(rest) = line
            .strip_prefix("- [ ] ")
            .or_else(|| line.strip_prefix("- [x] "))
            .or_else(|| line.strip_prefix("- [X] "))
            .or_else(|| line.strip_prefix("- "))
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("+ "))
        {
            line = rest;
        } else {
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0
                && line.as_bytes().get(digits) == Some(&b'.')
                && line.as_bytes().get(digits + 1) == Some(&b' ')
            {
                line = &line[digits + 2..];
            }
        }
        plain.push_str(&strip_inline_markdown(line));
        plain.push(' ');
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_inline_markdown(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '<'
            && let Some(close) = chars[i + 1..].iter().position(|c| *c == '>')
        {
            out.push(' ');
            i += close + 2;
            continue;
        }
        if (chars[i] == '!' && chars.get(i + 1) == Some(&'[')) || chars[i] == '[' {
            let is_img = chars[i] == '!';
            let open_br = if is_img { i + 1 } else { i };
            if let Some(close_rel) = chars[open_br + 1..].iter().position(|c| *c == ']') {
                let close_br = open_br + 1 + close_rel;
                if chars.get(close_br + 1) == Some(&'(')
                    && let Some(close_p_rel) = chars[close_br + 2..].iter().position(|c| *c == ')')
                {
                    let label: String = chars[open_br + 1..close_br].iter().collect();
                    let href: String =
                        chars[close_br + 2..close_br + 2 + close_p_rel].iter().collect();
                    if is_img && label.trim().is_empty() {
                        let fallback = href
                            .rsplit(['/', '\\'])
                            .next()
                            .filter(|s| !s.is_empty())
                            .unwrap_or(&href);
                        out.push_str(fallback);
                    } else {
                        out.push_str(label.trim());
                    }
                    i = close_br + 2 + close_p_rel + 1;
                    continue;
                }
            }
        }
        if matches!(chars[i], '`' | '~') {
            i += 1;
            continue;
        }
        if matches!(chars[i], '*' | '_') {
            let prev_word = i > 0 && chars[i - 1].is_alphanumeric();
            let next_word = chars.get(i + 1).is_some_and(|c| c.is_alphanumeric());
            if prev_word && next_word {
                out.push(chars[i]);
            }
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

pub fn preview_text(entry: &ChannelMessage) -> String {
    let source = slice_utf16(&entry.message.text, CHANNEL_PREVIEW_SOURCE_LIMIT);
    let text = markdown_preview_text(&source);
    if !text.is_empty() {
        return slice_utf16(&text, CHANNEL_PREVIEW_LIMIT);
    }
    if entry.message.question_prompt.is_some() {
        return "Asked a question".to_string();
    }
    if entry
        .message
        .attachments
        .as_ref()
        .is_some_and(|a| !a.is_empty())
    {
        return "Sent an attachment".to_string();
    }
    "Sent a message".to_string()
}

fn delete_receipts_of_tx(conn: &Connection, command_ids: &[String]) -> StorageResult<()> {
    if command_ids.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "DELETE FROM orchestration_command_receipts
         WHERE command_id = ?1
           AND NOT EXISTS (SELECT 1 FROM orchestration_events WHERE command_id = ?1)",
    )?;
    for cid in command_ids {
        stmt.execute(params![cid])?;
    }
    Ok(())
}

fn delete_aggregate_history_tx(
    conn: &Connection,
    aggregate_type: &str,
    aggregate_ids: &[String],
) -> StorageResult<()> {
    if aggregate_ids.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", aggregate_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql_params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + aggregate_ids.len());
    sql_params.push(&aggregate_type);
    for id in aggregate_ids {
        sql_params.push(id);
    }
    conn.execute(
        &format!(
            "DELETE FROM orchestration_command_receipts WHERE command_id IN (
               SELECT DISTINCT command_id FROM orchestration_events
               WHERE aggregate_type = ? AND aggregate_id IN ({placeholders})
             )"
        ),
        params_from_iter(sql_params.iter().copied()),
    )?;
    conn.execute(
        &format!(
            "DELETE FROM orchestration_events WHERE aggregate_type = ? AND aggregate_id IN ({placeholders})"
        ),
        params_from_iter(sql_params),
    )?;
    Ok(())
}

fn delete_thread_attachments_tx(conn: &Connection, thread_id: &str) -> StorageResult<()> {
    conn.execute(
        "DELETE FROM projection_attachments WHERE owner_kind = 'thread-message' AND owner_id >= ?1 AND owner_id < ?2",
        params![format!("{thread_id}:"), format!("{thread_id};")],
    )?;
    Ok(())
}

fn revoke_channel_routine_webhooks_tx(
    conn: &Connection,
    routine_ids: &[String],
    forget: bool,
) -> StorageResult<()> {
    if routine_ids.is_empty() {
        return Ok(());
    }
    let now = now_iso();
    for rid in routine_ids {
        conn.execute(
            "INSERT OR IGNORE INTO projection_webhook_route_revocations (route_id, created_at)
             SELECT route_id, ?1 FROM projection_channel_routine_webhooks WHERE routine_id = ?2",
            params![now, rid],
        )?;
        conn.execute(
            "DELETE FROM projection_channel_routine_webhooks WHERE routine_id = ?1",
            params![rid],
        )?;
        if forget {
            conn.execute(
                "DELETE FROM projection_webhook_receipts WHERE owner_kind = 'channel' AND routine_id = ?1",
                params![rid],
            )?;
        }
    }
    Ok(())
}

impl ChannelStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self {
            database,
            cache: Arc::new(Mutex::new(ChannelStoreCache::default())),
            member_folders_changed: None,
        }
    }

    pub fn with_member_folders_callback(
        database: OpenBotDatabase,
        callback: MemberFoldersCallback,
    ) -> Self {
        Self {
            database,
            cache: Arc::new(Mutex::new(ChannelStoreCache::default())),
            member_folders_changed: Some(callback),
        }
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    pub fn completed_message_changes(&self, channel_id: &str) -> u64 {
        self.cache
            .lock()
            .completed_message_changes
            .get(channel_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn member_folders(&self, agent_id: &str) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            if !conn.is_autocommit() {
                return Ok(Self::read_member_folders_tx(conn)?
                    .remove(agent_id)
                    .unwrap_or_default());
            }
            {
                let guard = self.cache.lock();
                if let Some(cached) = &guard.member_folders {
                    return Ok(cached.get(agent_id).cloned().unwrap_or_default());
                }
            }
            let computed = Self::read_member_folders_tx(conn)?;
            let out = computed.get(agent_id).cloned().unwrap_or_default();
            self.cache.lock().member_folders = Some(computed);
            Ok(out)
        })
    }

    fn read_member_folders_tx(conn: &Connection) -> StorageResult<HashMap<String, Vec<String>>> {
        let mut stmt = conn.prepare(
            "SELECT json_extract(channel_json, '$.folders'),
                    json_extract(channel_json, '$.members'),
                    COALESCE(json_extract(channel_json, '$.archived'), 0)
             FROM projection_channels
             WHERE json_array_length(channel_json, '$.folders') > 0
             ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)? != 0,
            ))
        })?;
        let mut folders: HashMap<String, Vec<String>> = HashMap::new();
        for item in rows {
            let (f_json, m_json, archived) = item?;
            let Some(source) = stored_member_folder_source(f_json, m_json, archived) else {
                continue;
            };
            for agent_id in source.agent_ids {
                let entry = folders.entry(agent_id).or_default();
                for path in &source.folders {
                    if !entry.contains(path) {
                        entry.push(path.clone());
                    }
                }
            }
        }
        Ok(folders)
    }

    fn replace_member_folders_tx(
        &self,
        conn: &Connection,
        channel_id: &str,
        next: Option<MemberFolderSource>,
    ) -> StorageResult<()> {
        let prev_row: Option<(Option<String>, Option<String>, bool)> = conn
            .query_row(
                "SELECT json_extract(channel_json, '$.folders'),
                        json_extract(channel_json, '$.members'),
                        COALESCE(json_extract(channel_json, '$.archived'), 0)
                 FROM projection_channels WHERE channel_id = ?1",
                params![channel_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)? != 0,
                    ))
                },
            )
            .optional()?;
        let previous = prev_row.and_then(|(f, m, a)| stored_member_folder_source(f, m, a));
        if previous.as_ref().map(|p| &p.key) == next.as_ref().map(|n| &n.key) {
            return Ok(());
        }
        self.cache.lock().member_folders = None;
        if let Some(cb) = &self.member_folders_changed {
            let mut affected = Vec::new();
            let mut seen = HashSet::new();
            for id in previous
                .iter()
                .flat_map(|p| p.agent_ids.iter())
                .chain(next.iter().flat_map(|n| n.agent_ids.iter()))
            {
                if seen.insert(id.clone()) {
                    affected.push(id.clone());
                }
            }
            cb(&affected);
        }
        Ok(())
    }

    fn forget_counts(&self, channel_id: &str) {
        let mut guard = self.cache.lock();
        guard.unread_counts.remove(channel_id);
        guard.task_counts.remove(channel_id);
    }

    fn keeps_counts_tx(&self, conn: &Connection) -> StorageResult<bool> {
        if !conn.is_autocommit() {
            return Ok(false);
        }
        let data_version: i64 =
            conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        let mut guard = self.cache.lock();
        if guard.counts_data_version != Some(data_version) {
            guard.unread_counts.clear();
            guard.task_counts.clear();
            guard.counts_data_version = Some(data_version);
        }
        Ok(true)
    }

    fn unread_count_tx(
        &self,
        conn: &Connection,
        channel_id: &str,
        member_id: &str,
        excluded_id: &str,
        keep: bool,
    ) -> StorageResult<u64> {
        let key = format!("{member_id}\n{excluded_id}");
        if keep
            && let Some(val) = self
                .cache
                .lock()
                .unread_counts
                .get(channel_id)
                .and_then(|m| m.get(&key))
                .copied()
        {
            return Ok(val);
        }
        let count: i64 = conn.query_row(
            UNREAD_COUNT_SQL,
            params![
                channel_id,
                channel_id,
                member_id,
                member_id,
                excluded_id,
                format!("{CHANNEL_ROUTING_EVENT_ITEM_PREFIX}%"),
                format!("{MCP_SIGN_IN_ITEM_TYPE_PREFIX}%"),
                format!("{SETTING_LINK_ITEM_TYPE_PREFIX}%"),
            ],
            |r| r.get(0),
        )?;
        let count_u64 = count.max(0) as u64;
        if keep {
            self.cache
                .lock()
                .unread_counts
                .entry(channel_id.to_string())
                .or_default()
                .insert(key, count_u64);
        }
        Ok(count_u64)
    }

    fn task_counts_tx(
        &self,
        conn: &Connection,
        channel_id: &str,
        keep: bool,
    ) -> StorageResult<TaskCounts> {
        if keep
            && let Some(counts) = self.cache.lock().task_counts.get(channel_id).copied()
        {
            return Ok(counts);
        }
        let counts = conn.query_row(TASK_COUNTS_SQL, params![channel_id], |r| {
            Ok(TaskCounts {
                running: r.get::<_, i64>(0)?.max(0) as u64,
                open: r.get::<_, i64>(1)?.max(0) as u64,
            })
        })?;
        if keep {
            self.cache
                .lock()
                .task_counts
                .insert(channel_id.to_string(), counts);
        }
        Ok(counts)
    }
}

impl ChannelStore {
    pub fn create(&self, channel_id: &str, draft: ChannelDraft) -> Channel {
        Channel {
            id: channel_id.to_string(),
            draft,
            archived: false,
            revision: 0,
            created_at: now_iso(),
            image_url: None,
            folders: None,
        }
    }

    pub fn exists(&self, channel_id: &str) -> StorageResult<bool> {
        self.database.with_conn(|conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM projection_channels WHERE channel_id = ?1",
                    params![channel_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
    }

    pub fn get(&self, channel_id: &str) -> StorageResult<Channel> {
        self.database.with_conn(|conn| Self::get_tx(conn, channel_id))
    }

    fn get_tx(conn: &Connection, channel_id: &str) -> StorageResult<Channel> {
        let json_str: Option<String> = conn
            .query_row(
                "SELECT channel_json FROM projection_channels WHERE channel_id = ?1",
                params![channel_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(json_str) = json_str else {
            return Err(StorageError::NotFound("Channel not found.".to_string()));
        };
        decode_stored_channel(&json_str)
    }

    pub fn ids(&self) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT channel_id FROM projection_channels ORDER BY rowid")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for item in rows {
                out.push(item?);
            }
            Ok(out)
        })
    }

    pub fn archived_ids(&self) -> StorageResult<HashSet<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT channel_id FROM projection_channels WHERE json_extract(channel_json, '$.archived') = 1",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut out = HashSet::new();
            for item in rows {
                out.insert(item?);
            }
            Ok(out)
        })
    }

    pub fn list(
        &self,
        member_id: &str,
        signed_out_messages_are_theirs: bool,
    ) -> StorageResult<Vec<ChannelSummary>> {
        let excluded_id = if signed_out_messages_are_theirs {
            SIGNED_OUT_CHANNEL_MEMBER_ID
        } else {
            member_id
        };
        self.database.with_conn(|conn| {
            let keeps_counts = self.keeps_counts_tx(conn)?;
            let mut threads_by_channel: HashMap<String, Vec<ChannelThread>> = HashMap::new();
            {
                let mut t_stmt = conn.prepare(CHANNEL_THREADS_SQL)?;
                let t_rows = t_stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        ChannelThread {
                            agent_id: r.get::<_, String>(1)?,
                            thread_id: r.get::<_, String>(2)?,
                        },
                    ))
                })?;
                for item in t_rows {
                    let (cid, thread) = item?;
                    threads_by_channel.entry(cid).or_default().push(thread);
                }
            }
            let mut stmt = conn.prepare(CHANNEL_SUMMARY_ROWS_SQL)?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?;
            let mut summaries = Vec::new();
            for item in rows {
                let (c_json, latest_json) = item?;
                let channel = decode_stored_channel(&c_json)?;
                let latest = match latest_json.filter(|s| !s.is_empty()) {
                    Some(raw) => Some(decode_stored_message(&raw)?),
                    None => None,
                };
                let task_counts = self.task_counts_tx(conn, &channel.id, keeps_counts)?;
                let unread =
                    self.unread_count_tx(conn, &channel.id, member_id, excluded_id, keeps_counts)?;
                let last_message = latest.map(|entry| ChannelPreview {
                    author_name: entry.author.name.clone(),
                    author: Some(ChannelPreviewAuthor {
                        kind: entry.author.kind,
                        id: entry.author.id.clone(),
                    }),
                    status: Some(entry.message.status),
                    text: preview_text(&entry),
                    at: entry.message.created_at.clone(),
                });
                let threads = threads_by_channel
                    .remove(&channel.id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| {
                        channel
                            .draft
                            .members
                            .iter()
                            .any(|m| m.agent_id == t.agent_id)
                    })
                    .collect();
                summaries.push(ChannelSummary {
                    channel,
                    unread_count: unread,
                    active_tasks: task_counts.running,
                    open_tasks: Some(task_counts.open),
                    last_message,
                    threads: Some(threads),
                });
            }
            Ok(summaries)
        })
    }

    pub fn messages(
        &self,
        channel_id: &str,
        before: Option<u64>,
        limit: Option<usize>,
    ) -> StorageResult<Vec<ChannelMessage>> {
        self.database
            .with_conn(|conn| Self::messages_tx(conn, channel_id, before, limit))
    }

    fn messages_tx(
        conn: &Connection,
        channel_id: &str,
        before: Option<u64>,
        limit: Option<usize>,
    ) -> StorageResult<Vec<ChannelMessage>> {
        let before_val = before
            .and_then(|v| i64::try_from(v).ok())
            .unwrap_or(i64::MAX);
        let limit_val = limit.and_then(|v| i64::try_from(v).ok()).unwrap_or(-1);
        let sql = format!(
            "SELECT message_json FROM projection_channel_messages
             WHERE channel_id = ?1 AND sequence < ?2 AND {SHOWN_MESSAGE_SQL}
             ORDER BY sequence DESC, message_id DESC LIMIT ?3"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![channel_id, before_val, limit_val], |r| {
            r.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for item in rows {
            out.push(decode_stored_message(&item?)?);
        }
        out.reverse();
        Ok(out)
    }

    pub fn message(
        &self,
        channel_id: &str,
        message_id: &str,
    ) -> StorageResult<Option<ChannelMessage>> {
        self.database.with_conn(|conn| {
            let sql = format!(
                "SELECT message_json FROM projection_channel_messages
                 WHERE channel_id = ?1 AND message_id = ?2 AND {SHOWN_MESSAGE_SQL}"
            );
            let raw: Option<String> = conn
                .query_row(&sql, params![channel_id, message_id], |r| r.get(0))
                .optional()?;
            raw.map(|s| decode_stored_message(&s)).transpose()
        })
    }

    pub fn task(&self, channel_id: &str, task_id: &str) -> StorageResult<Option<ChannelTask>> {
        self.database.with_conn(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT task_json FROM projection_channel_tasks WHERE channel_id = ?1 AND task_id = ?2",
                    params![channel_id, task_id],
                    |r| r.get(0),
                )
                .optional()?;
            raw.map(|s| decode_stored_task(&s)).transpose()
        })
    }

    pub fn tasks(&self, channel_id: &str) -> StorageResult<Vec<ChannelTask>> {
        self.database
            .with_conn(|conn| Self::tasks_tx(conn, channel_id))
    }

    fn tasks_tx(conn: &Connection, channel_id: &str) -> StorageResult<Vec<ChannelTask>> {
        let mut stmt = conn.prepare(
            "SELECT task_json FROM projection_channel_tasks WHERE channel_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![channel_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for item in rows {
            out.push(decode_stored_task(&item?)?);
        }
        Ok(out)
    }
}

impl ChannelStore {
    pub fn assignments(&self, channel_id: &str) -> StorageResult<Vec<ChannelAssignment>> {
        self.database
            .with_conn(|conn| Self::assignments_tx(conn, channel_id))
    }

    fn assignments_tx(conn: &Connection, channel_id: &str) -> StorageResult<Vec<ChannelAssignment>> {
        let mut stmt = conn.prepare(
            "SELECT assignment_json FROM projection_channel_assignments WHERE channel_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![channel_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for item in rows {
            out.push(decode_stored_assignment(&item?)?);
        }
        Ok(out)
    }

    pub fn agent_assignments(
        &self,
        channel_id: &str,
        agent_id: &str,
    ) -> StorageResult<Vec<ChannelAssignment>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT assignment_json FROM projection_channel_assignments
                 WHERE channel_id = ?1 AND json_extract(assignment_json, '$.agentId') = ?2
                 ORDER BY rowid",
            )?;
            let rows = stmt.query_map(params![channel_id, agent_id], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for item in rows {
                out.push(decode_stored_assignment(&item?)?);
            }
            Ok(out)
        })
    }

    pub fn assignment(
        &self,
        channel_id: &str,
        assignment_id: &str,
    ) -> StorageResult<Option<ChannelAssignment>> {
        self.database.with_conn(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT assignment_json FROM projection_channel_assignments WHERE channel_id = ?1 AND assignment_id = ?2",
                    params![channel_id, assignment_id],
                    |r| r.get(0),
                )
                .optional()?;
            raw.map(|s| decode_stored_assignment(&s)).transpose()
        })
    }

    pub fn assignments_in_states(
        &self,
        channel_id: &str,
        states: &[ChannelAssignmentState],
    ) -> StorageResult<Vec<ChannelAssignment>> {
        if states.is_empty() {
            return Ok(Vec::new());
        }
        let state_strs: Vec<&str> = states.iter().copied().map(assignment_state_str).collect();
        self.database.with_conn(|conn| {
            let placeholders = std::iter::repeat_n("?", state_strs.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT assignment_json FROM projection_channel_assignments
                 WHERE channel_id = ? AND json_extract(assignment_json, '$.state') IN ({placeholders})
                 ORDER BY rowid"
            );
            let mut sql_params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + state_strs.len());
            sql_params.push(&channel_id);
            for s in &state_strs {
                sql_params.push(s);
            }
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(sql_params), |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for item in rows {
                out.push(decode_stored_assignment(&item?)?);
            }
            Ok(out)
        })
    }

    pub fn has_assignment_in_state(&self, states: &[ChannelAssignmentState]) -> StorageResult<bool> {
        if states.is_empty() {
            return Ok(false);
        }
        let state_strs: Vec<&str> = states.iter().copied().map(assignment_state_str).collect();
        self.database.with_conn(|conn| {
            let placeholders = std::iter::repeat_n("?", state_strs.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT 1 FROM projection_channel_assignments
                 WHERE json_extract(assignment_json, '$.state') IN ({placeholders}) LIMIT 1"
            );
            let found: Option<i64> = conn
                .query_row(&sql, params_from_iter(state_strs.iter()), |r| r.get(0))
                .optional()?;
            Ok(found.is_some())
        })
    }

    pub fn has_agent_assignment_in_state(
        &self,
        agent_id: &str,
        states: &[ChannelAssignmentState],
    ) -> StorageResult<bool> {
        if states.is_empty() {
            return Ok(false);
        }
        let state_strs: Vec<&str> = states.iter().copied().map(assignment_state_str).collect();
        self.database.with_conn(|conn| {
            let placeholders = std::iter::repeat_n("?", state_strs.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT 1 FROM projection_channel_assignments
                 WHERE json_extract(assignment_json, '$.agentId') = ?
                   AND json_extract(assignment_json, '$.state') IN ({placeholders}) LIMIT 1"
            );
            let mut sql_params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + state_strs.len());
            sql_params.push(&agent_id);
            for s in &state_strs {
                sql_params.push(s);
            }
            let found: Option<i64> = conn
                .query_row(&sql, params_from_iter(sql_params), |r| r.get(0))
                .optional()?;
            Ok(found.is_some())
        })
    }

    pub fn agent_work(
        &self,
        agent_id: &str,
        states: &[ChannelAssignmentState],
    ) -> StorageResult<Vec<ChannelAgentWork>> {
        if states.is_empty() {
            return Ok(Vec::new());
        }
        let state_strs: Vec<&str> = states.iter().copied().map(assignment_state_str).collect();
        self.database.with_conn(|conn| {
            let placeholders = std::iter::repeat_n("?", state_strs.len())
                .collect::<Vec<_>>()
                .join(", ");
            let ranking: String = (0..state_strs.len())
                .map(|idx| format!("WHEN ? THEN {idx}"))
                .collect::<Vec<_>>()
                .join(" ");
            let sql = format!(
                "SELECT assignments.assignment_json, channels.channel_json
                 FROM projection_channel_assignments AS assignments
                 JOIN projection_channels AS channels ON channels.channel_id = assignments.channel_id
                 WHERE json_extract(assignments.assignment_json, '$.agentId') = ?
                   AND json_extract(assignments.assignment_json, '$.state') IN ({placeholders})
                   AND COALESCE(json_extract(channels.channel_json, '$.archived'), 0) = 0
                 ORDER BY CASE json_extract(assignments.assignment_json, '$.state') {ranking} END,
                          assignments.rowid"
            );
            let mut sql_params: Vec<&dyn rusqlite::ToSql> =
                Vec::with_capacity(1 + state_strs.len() * 2);
            sql_params.push(&agent_id);
            for s in &state_strs {
                sql_params.push(s);
            }
            for s in &state_strs {
                sql_params.push(s);
            }
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(sql_params), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut out = Vec::new();
            for item in rows {
                let (a_json, c_json) = item?;
                out.push(ChannelAgentWork {
                    assignment: decode_stored_assignment(&a_json)?,
                    channel: decode_stored_channel(&c_json)?,
                });
            }
            Ok(out)
        })
    }

    pub fn assignment_for_delivery(
        &self,
        delivery_id: &str,
    ) -> StorageResult<Option<ChannelAssignment>> {
        self.database.with_conn(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT assignment_json FROM projection_channel_assignments WHERE delivery_id = ?1",
                    params![delivery_id],
                    |r| r.get(0),
                )
                .optional()?;
            raw.map(|s| decode_stored_assignment(&s)).transpose()
        })
    }
}

impl ChannelStore {
    pub fn with_seen_dependencies(
        &self,
        channel_id: &str,
        tasks: Vec<ChannelTask>,
    ) -> StorageResult<Vec<ChannelTask>> {
        if !tasks.iter().any(|t| t.state == ChannelTaskState::Waiting) {
            return Ok(tasks);
        }
        let all_assignments = self.assignments(channel_id)?;
        let mut started: HashMap<String, isize> = HashMap::new();
        let mut read_at: HashMap<String, HashMap<String, isize>> = HashMap::new();
        for (idx, assignment) in all_assignments.iter().enumerate() {
            let order = idx as isize;
            started.insert(assignment.task_id.clone(), order);
            let read_map = read_at.entry(assignment.task_id.clone()).or_default();
            for dep_id in &assignment.awaited_task_ids {
                read_map.insert(dep_id.clone(), order);
            }
        }
        Ok(tasks
            .into_iter()
            .map(|mut task| {
                if task.state != ChannelTaskState::Waiting {
                    return task;
                }
                let seen: Vec<String> = task
                    .dependencies
                    .iter()
                    .filter(|dep_id| {
                        read_at
                            .get(&task.id)
                            .and_then(|m| m.get(dep_id.as_str()))
                            .is_some_and(|&at| started.get(dep_id.as_str()).copied().unwrap_or(-1) < at)
                    })
                    .cloned()
                    .collect();
                if !seen.is_empty() {
                    task.seen_dependencies = Some(seen);
                }
                task
            })
            .collect())
    }

    pub fn older_extent(&self, channel_id: &str, before: u64) -> StorageResult<ChannelOlderExtent> {
        self.database
            .with_conn(|conn| Self::older_extent_tx(conn, channel_id, before))
    }

    fn older_extent_tx(
        conn: &Connection,
        channel_id: &str,
        before: u64,
    ) -> StorageResult<ChannelOlderExtent> {
        let sql = format!(
            "SELECT COUNT(*) AS older_count,
               (SELECT json_extract(message_json, '$.message.createdAt') FROM projection_channel_messages
                WHERE channel_id = ?1 AND {SHOWN_MESSAGE_SQL} ORDER BY sequence LIMIT 1) AS oldest_at
             FROM projection_channel_messages WHERE channel_id = ?2 AND sequence < ?3 AND {SHOWN_MESSAGE_SQL}"
        );
        let (older_count, oldest_at) = conn.query_row(
            &sql,
            params![channel_id, channel_id, before as i64],
            |r| Ok((r.get::<_, i64>(0)?.max(0) as u64, r.get::<_, Option<String>>(1)?)),
        )?;
        Ok(ChannelOlderExtent {
            older_count,
            oldest_at,
        })
    }

    pub fn page_without_extent(
        &self,
        channel_id: &str,
        before: Option<u64>,
        limit: usize,
    ) -> StorageResult<ChannelPage> {
        self.database.with_conn(|conn| {
            let mut messages = Self::messages_tx(conn, channel_id, before, Some(limit + 1))?;
            let has_older = messages.len() > limit;
            if has_older {
                messages.remove(0);
            }
            let older_cursor = if has_older {
                messages.first().map(|m| m.sequence)
            } else {
                None
            };
            let through_sequence = Self::latest_sequence_tx(conn, channel_id)?;
            Ok(ChannelPage {
                channel: Self::get_tx(conn, channel_id)?,
                messages,
                tasks: Self::tasks_tx(conn, channel_id)?,
                older_cursor,
                through_sequence,
                older_count: None,
                oldest_at: None,
                waits: None,
            })
        })
    }

    pub fn page(
        &self,
        channel_id: &str,
        before: Option<u64>,
        limit: usize,
    ) -> StorageResult<ChannelPage> {
        let mut page = self.page_without_extent(channel_id, before, limit)?;
        if let Some(cursor) = page.older_cursor {
            let ext = self.older_extent(channel_id, cursor)?;
            page.older_count = Some(ext.older_count);
            page.oldest_at = ext.oldest_at;
        }
        Ok(page)
    }

    fn latest_sequence_tx(conn: &Connection, channel_id: &str) -> StorageResult<u64> {
        Ok(Self::messages_tx(conn, channel_id, None, Some(1))?
            .first()
            .map_or(0, |m| m.sequence))
    }

    fn stored_sequence_tx(conn: &Connection, channel_id: &str) -> StorageResult<u64> {
        let max_seq: Option<i64> = conn.query_row(
            "SELECT MAX(sequence) FROM projection_channel_messages WHERE channel_id = ?1",
            params![channel_id],
            |r| r.get(0),
        )?;
        Ok(max_seq.unwrap_or(0).max(0) as u64)
    }

    pub fn commit(&self, operation_id: &str, change: ChannelChange) -> StorageResult<Channel> {
        self.commit_with_hook(operation_id, change, |_, _| Ok(()))
    }

    fn commit_with_hook<H>(
        &self,
        operation_id: &str,
        mut change: ChannelChange,
        written: H,
    ) -> StorageResult<Channel>
    where
        H: FnOnce(&Connection, i64) -> StorageResult<()>,
    {
        let command_id = format!("channels:{operation_id}");
        if let Some((_, prev_val)) = self.database.command_result::<Value>(&command_id)? {
            return Channel::decode_normalized(&prev_val)
                .map_err(|e| StorageError::Validation(e.to_string()));
        }
        with_browser_member(&mut change.channel.draft);
        change.channel.revision += 1;
        let updated_channel = change.channel.clone();

        change.messages = self.database.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT 1 FROM projection_channel_messages
                 WHERE channel_id = ?1 AND message_id = ?2 AND json_extract(message_json, '$.deleted') = 1",
            )?;
            let mut kept = Vec::with_capacity(change.messages.len());
            for msg in change.messages {
                if is_deleted_channel_message(&msg)
                    || stmt
                        .query_row(params![updated_channel.id, msg.id], |r| r.get::<_, i64>(0))
                        .optional()?
                        .is_none()
                {
                    kept.push(msg);
                }
            }
            Ok(kept)
        })?;

        let payload_val = serde_json::to_value(&change)?;
        let event = new_event("channel", &updated_channel.id, "channel.changed", payload_val);
        let receipt = self.database.dispatch_with_projection(
            &command_id,
            None,
            &[event],
            |conn, sequences| {
                self.project_tx(conn, &change)?;
                if let Some(&seq) = sequences.first() {
                    written(conn, seq)?;
                }
                Ok(updated_channel.clone())
            },
        )?;
        Ok(receipt.result)
    }

    pub fn update(
        &self,
        channel: Channel,
        changes: ChannelUpdateChanges,
        operation_id: Option<&str>,
    ) -> StorageResult<Channel> {
        let owned_op;
        let op_id = match operation_id {
            Some(s) => s,
            None => {
                owned_op = Uuid::new_v4().to_string();
                &owned_op
            }
        };
        self.commit(
            op_id,
            ChannelChange {
                channel,
                messages: changes.messages,
                tasks: changes.tasks,
                assignments: changes.assignments,
            },
        )
    }
}

impl ChannelStore {
    pub fn capture_message(
        &self,
        channel: Channel,
        message: ChannelMessage,
    ) -> StorageResult<Channel> {
        let msg_id_json = serde_json::to_string(&message.id)?;
        let op_prefix = format!("capture:{}:{msg_id_json}:", channel.id);
        let cmd_prefix = format!("channels:{op_prefix}");
        let cmd_upper = format!("{cmd_prefix}\u{ffff}");
        let op_id = format!("{op_prefix}{}", Uuid::new_v4());
        self.commit_with_hook(
            &op_id,
            ChannelChange {
                channel,
                messages: vec![message],
                tasks: Vec::new(),
                assignments: Vec::new(),
            },
            |conn, sequence| {
                let mut stmt = conn.prepare(
                    "DELETE FROM orchestration_events
                     WHERE command_id > ?1 AND command_id < ?2 AND sequence < ?3
                       AND sequence > (SELECT MIN(sequence) FROM orchestration_events WHERE command_id > ?1 AND command_id < ?2)
                     RETURNING command_id",
                )?;
                let rows = stmt.query_map(params![cmd_prefix, cmd_upper, sequence], |r| {
                    r.get::<_, String>(0)
                })?;
                let mut superseded = Vec::new();
                for item in rows {
                    superseded.push(item?);
                }
                delete_receipts_of_tx(conn, &superseded)
            },
        )
    }

    pub fn delete_message(
        &self,
        mut channel: Channel,
        entry: &ChannelMessage,
    ) -> StorageResult<Channel> {
        let deleted = deleted_channel_message(entry);
        channel.revision += 1;
        let updated = channel.clone();
        let payload = ChannelChange {
            channel,
            messages: vec![deleted.clone()],
            tasks: Vec::new(),
            assignments: Vec::new(),
        };
        let command_id = format!(
            "channels:message-deleted:{}:{}:{}",
            updated.id,
            entry.id,
            Uuid::new_v4()
        );
        let needle = serde_json::to_string(&entry.id)?;
        let event = new_event(
            "channel",
            &updated.id,
            "channel.changed",
            serde_json::to_value(&payload)?,
        );
        let receipt = self.database.dispatch_with_projection(
            &command_id,
            None,
            &[event],
            |conn, _| {
                self.project_tx(conn, &payload)?;
                let mut stmt = conn.prepare(
                    "SELECT sequence, payload_json FROM orchestration_events
                     WHERE aggregate_type = 'channel' AND aggregate_id = ?1
                       AND event_type = 'channel.changed' AND instr(payload_json, ?2) > 0",
                )?;
                let rows = stmt.query_map(params![updated.id, needle], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?;
                let mut events = Vec::new();
                for item in rows {
                    events.push(item?);
                }
                let mut update_stmt =
                    conn.prepare("UPDATE orchestration_events SET payload_json = ?1 WHERE sequence = ?2")?;
                for (seq, p_json) in events {
                    let Ok(mut val) = serde_json::from_str::<Value>(&p_json) else {
                        continue;
                    };
                    let Some(arr) = val.get_mut("messages").and_then(Value::as_array_mut) else {
                        continue;
                    };
                    let mut changed = false;
                    for item in arr.iter_mut() {
                        if let Ok(msg) = serde_json::from_value::<ChannelMessage>(item.clone())
                            && msg.id == entry.id
                            && !is_deleted_channel_message(&msg)
                        {
                            let mut scrubbed = deleted.clone();
                            scrubbed.sequence = msg.sequence;
                            *item = serde_json::to_value(&scrubbed)?;
                            changed = true;
                        }
                    }
                    if changed {
                        update_stmt.execute(params![serde_json::to_string(&val)?, seq])?;
                    }
                }
                Ok(updated.clone())
            },
        )?;
        Ok(receipt.result)
    }

    pub fn delete_history(&self, channel: &Channel) -> StorageResult<bool> {
        let through_sequence = self
            .database
            .with_conn(|conn| Self::stored_sequence_tx(conn, &channel.id))?;
        if through_sequence == 0 {
            return Ok(false);
        }
        let mut updated = channel.clone();
        updated.revision += 1;
        let change = ChannelChange {
            channel: updated.clone(),
            messages: Vec::new(),
            tasks: Vec::new(),
            assignments: Vec::new(),
        };
        let next_summary = ChannelHistorySummary {
            version: self.summary(&channel.id)?.version + 1,
            through_sequence,
            text: String::new(),
        };
        let command_id = format!("channels:history-deleted:{}:{}", channel.id, Uuid::new_v4());
        let ev1 = new_event(
            "channel",
            &channel.id,
            "channel.changed",
            serde_json::to_value(&change)?,
        );
        let ev2 = new_event(
            "channel",
            &channel.id,
            "channel.summarized",
            serde_json::to_value(&next_summary)?,
        );
        self.database
            .dispatch_with_projection(&command_id, None, &[ev1, ev2], |conn, sequences| {
                self.project_tx(conn, &change)?;
                let ids: Vec<String> = {
                    let sql = format!(
                        "SELECT message_id FROM projection_channel_messages WHERE channel_id = ?1 AND {SHOWN_MESSAGE_SQL}"
                    );
                    let mut s = conn.prepare(&sql)?;
                    let r = s.query_map(params![channel.id], |row| row.get::<_, String>(0))?;
                    let mut out = Vec::new();
                    for item in r {
                        out.push(item?);
                    }
                    out
                };
                for msg_id in ids {
                    let raw: Option<String> = conn
                        .query_row(
                            "SELECT message_json FROM projection_channel_messages WHERE channel_id = ?1 AND message_id = ?2",
                            params![channel.id, msg_id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(raw_str) = raw
                        && let Ok(parsed) = serde_json::from_str::<ChannelMessage>(&raw_str)
                    {
                        let scrubbed = serde_json::to_string(&deleted_channel_message(&parsed))?;
                        conn.execute(
                            "UPDATE projection_channel_messages SET message_json = ?1 WHERE channel_id = ?2 AND message_id = ?3",
                            params![scrubbed, channel.id, msg_id],
                        )?;
                    }
                }
                conn.execute(
                    "INSERT INTO projection_channel_summaries VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(channel_id) DO UPDATE SET
                       version = excluded.version,
                       through_sequence = excluded.through_sequence,
                       text = excluded.text",
                    params![
                        channel.id,
                        next_summary.version as i64,
                        next_summary.through_sequence as i64,
                        next_summary.text,
                    ],
                )?;
                let own_seqs: HashSet<i64> = sequences.iter().copied().collect();
                let candidate_seqs: Vec<i64> = {
                    let mut s = conn.prepare(
                        "SELECT sequence FROM orchestration_events
                         WHERE aggregate_type = 'channel' AND aggregate_id = ?1
                           AND event_type IN ('channel.changed', 'channel.summarized')",
                    )?;
                    let r = s.query_map(params![channel.id], |row| row.get::<_, i64>(0))?;
                    let mut out = Vec::new();
                    for item in r {
                        let seq = item?;
                        if !own_seqs.contains(&seq) {
                            out.push(seq);
                        }
                    }
                    out
                };
                for seq in candidate_seqs {
                    let row: Option<(String, String)> = conn
                        .query_row(
                            "SELECT event_type, payload_json FROM orchestration_events WHERE sequence = ?1",
                            params![seq],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                    let Some((ev_type, p_json)) = row else {
                        continue;
                    };
                    let Ok(mut val) = serde_json::from_str::<Value>(&p_json) else {
                        continue;
                    };
                    if ev_type == "channel.summarized" {
                        if val.get("text").and_then(Value::as_str).is_some_and(|t| !t.is_empty()) {
                            if let Some(obj) = val.as_object_mut() {
                                obj.insert("text".to_string(), Value::String(String::new()));
                                conn.execute(
                                    "UPDATE orchestration_events SET payload_json = ?1 WHERE sequence = ?2",
                                    params![serde_json::to_string(&val)?, seq],
                                )?;
                            }
                        }
                        continue;
                    }
                    let Some(arr) = val.get_mut("messages").and_then(Value::as_array_mut) else {
                        continue;
                    };
                    let mut changed = false;
                    for item in arr.iter_mut() {
                        if let Ok(msg) = serde_json::from_value::<ChannelMessage>(item.clone())
                            && !is_deleted_channel_message(&msg)
                        {
                            *item = serde_json::to_value(deleted_channel_message(&msg))?;
                            changed = true;
                        }
                    }
                    if changed {
                        conn.execute(
                            "UPDATE orchestration_events SET payload_json = ?1 WHERE sequence = ?2",
                            params![serde_json::to_string(&val)?, seq],
                        )?;
                    }
                }
                Ok(updated.clone())
            })?;
        Ok(true)
    }
}

impl ChannelStore {
    fn project_tx(&self, conn: &Connection, change: &ChannelChange) -> StorageResult<()> {
        self.forget_counts(&change.channel.id);
        let next_folder_src = member_folder_source(
            change.channel.folders.as_deref(),
            change
                .channel
                .draft
                .members
                .iter()
                .map(|m| m.agent_id.clone())
                .collect(),
            change.channel.archived,
        );
        self.replace_member_folders_tx(conn, &change.channel.id, next_folder_src)?;
        conn.execute(
            "INSERT INTO projection_channels VALUES (?1, ?2)
             ON CONFLICT(channel_id) DO UPDATE SET channel_json = excluded.channel_json",
            params![change.channel.id, serde_json::to_string(&change.channel)?],
        )?;

        for message in &change.messages {
            let existing: Option<(i64, String, Option<String>, Option<i64>)> = conn
                .query_row(
                    "SELECT sequence, message_json,
                            json_extract(message_json, '$.message.status'),
                            json_extract(message_json, '$.deleted')
                     FROM projection_channel_messages WHERE channel_id = ?1 AND message_id = ?2",
                    params![change.channel.id, message.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            if existing.as_ref().is_some_and(|(_, _, _, del)| *del == Some(1))
                && !is_deleted_channel_message(message)
            {
                continue;
            }
            let assigned_seq: i64 = match &existing {
                Some((seq, _, _, _)) => *seq,
                None => conn.query_row(
                    "SELECT COALESCE(MAX(sequence), 0) + 1 FROM projection_channel_messages WHERE channel_id = ?1",
                    params![change.channel.id],
                    |r| r.get(0),
                )?,
            };
            let mut stored_msg = message.clone();
            stored_msg.sequence = assigned_seq.max(1) as u64;
            conn.execute(
                "INSERT INTO projection_channel_messages VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(channel_id, message_id) DO UPDATE SET message_json = excluded.message_json",
                params![
                    change.channel.id,
                    stored_msg.id,
                    assigned_seq,
                    serde_json::to_string(&stored_msg)?,
                ],
            )?;
            if let Some((_, prev_json, prev_status, _)) = existing {
                let before_parsed = serde_json::from_str::<ChannelMessage>(&prev_json).ok();
                let content_changed = match &before_parsed {
                    Some(before) => changes_model_content(before, &stored_msg),
                    None => true,
                };
                if content_changed {
                    if prev_status.as_deref() != Some("streaming") {
                        let mut guard = self.cache.lock();
                        let counter = guard
                            .completed_message_changes
                            .entry(change.channel.id.clone())
                            .or_insert(0);
                        *counter += 1;
                    }
                    conn.execute(
                        "UPDATE projection_channel_summaries
                         SET version = version + 1, through_sequence = 0, text = ''
                         WHERE channel_id = ?1 AND through_sequence >= ?2",
                        params![change.channel.id, assigned_seq],
                    )?;
                    conn.execute(
                        "UPDATE projection_channel_contexts
                         SET through_sequence = 0, summary_version = 0
                         WHERE channel_id = ?1 AND through_sequence >= ?2",
                        params![change.channel.id, assigned_seq],
                    )?;
                }
            }
        }

        for task in &change.tasks {
            conn.execute(
                "INSERT INTO projection_channel_tasks VALUES (?1, ?2, ?3)
                 ON CONFLICT(task_id) DO UPDATE SET task_json = excluded.task_json",
                params![task.id, change.channel.id, serde_json::to_string(task)?],
            )?;
        }
        for assignment in &change.assignments {
            conn.execute(
                "INSERT INTO projection_channel_assignments VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(assignment_id) DO UPDATE SET
                   delivery_id = excluded.delivery_id,
                   assignment_json = excluded.assignment_json",
                params![
                    assignment.id,
                    change.channel.id,
                    assignment.task_id,
                    assignment.delivery_id,
                    serde_json::to_string(assignment)?,
                ],
            )?;
        }
        Ok(())
    }

    pub fn context(&self, channel_id: &str, agent_id: &str) -> StorageResult<ChannelContext> {
        let existing: Option<ChannelContext> = self.database.with_conn(|conn| {
            conn.query_row(
                "SELECT thread_id, session_id, through_sequence, summary_version
                 FROM projection_channel_contexts WHERE channel_id = ?1 AND agent_id = ?2",
                params![channel_id, agent_id],
                |r| {
                    Ok(ChannelContext {
                        thread_id: r.get(0)?,
                        session_id: r.get(1)?,
                        through_sequence: r.get::<_, i64>(2)?.max(0) as u64,
                        summary_version: r.get::<_, i64>(3)?.max(0) as u64,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
        })?;
        if let Some(ctx) = existing {
            return Ok(ctx);
        }
        let thread_id = format!("openbot-thread-{}", Uuid::new_v4());
        let command_id = format!("channel-context:{channel_id}:{agent_id}");
        let event = new_event(
            "channel",
            channel_id,
            "channel.context-created",
            serde_json::json!({ "agentId": agent_id, "threadId": thread_id }),
        );
        let receipt = self.database.dispatch_with_projection(
            &command_id,
            None,
            &[event],
            |conn, sequences| {
                let now = now_iso();
                let channel = Self::get_tx(conn, channel_id)?;
                conn.execute(
                    "INSERT INTO projection_threads VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
                    params![
                        thread_id,
                        agent_id,
                        channel.draft.name,
                        now,
                        now,
                        sequences.first().copied().unwrap_or(0),
                    ],
                )?;
                conn.execute(
                    "INSERT INTO projection_channel_contexts(channel_id, agent_id, thread_id)
                     VALUES (?1, ?2, ?3)",
                    params![channel_id, agent_id, thread_id],
                )?;
                Ok(ChannelContext {
                    thread_id: thread_id.clone(),
                    session_id: None,
                    through_sequence: 0,
                    summary_version: 0,
                })
            },
        )?;
        Ok(receipt.result)
    }

    pub fn execution_threads(&self) -> StorageResult<Vec<ChannelExecutionThread>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT channel_id, agent_id, thread_id FROM projection_channel_contexts",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ChannelExecutionThread {
                    channel_id: r.get(0)?,
                    id: r.get(1)?,
                    thread_id: r.get(2)?,
                })
            })?;
            let mut out = Vec::new();
            for item in rows {
                out.push(item?);
            }
            Ok(out)
        })
    }

    pub fn context_threads(&self, channel_id: &str) -> StorageResult<Vec<String>> {
        self.database
            .with_conn(|conn| Self::context_threads_tx(conn, channel_id))
    }

    fn context_threads_tx(conn: &Connection, channel_id: &str) -> StorageResult<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT thread_id FROM projection_channel_contexts WHERE channel_id = ?1 ORDER BY agent_id",
        )?;
        let rows = stmt.query_map(params![channel_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item?);
        }
        Ok(out)
    }
}

impl ChannelStore {
    pub fn contexts(&self, channel_id: &str) -> StorageResult<Vec<ChannelThread>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT agent_id, thread_id FROM projection_channel_contexts
                 WHERE channel_id = ?1 ORDER BY agent_id",
            )?;
            let rows = stmt.query_map(params![channel_id], |r| {
                Ok(ChannelThread {
                    agent_id: r.get(0)?,
                    thread_id: r.get(1)?,
                })
            })?;
            let mut out = Vec::new();
            for item in rows {
                out.push(item?);
            }
            Ok(out)
        })
    }

    pub fn context_reset_sequence(&self, channel_id: &str) -> StorageResult<u64> {
        self.database.with_conn(|conn| {
            let sql = format!(
                "SELECT MAX(sequence) FROM projection_channel_messages
                 WHERE channel_id = ?1 AND message_id >= 'context-reset-' AND message_id < 'context-reset.'
                   AND {SHOWN_MESSAGE_SQL}
                   AND json_extract(message_json, '$.message.itemType') = ?2
                   AND json_extract(message_json, '$.message.author') = 'system'
                   AND json_extract(message_json, '$.message.source') = 'system'"
            );
            let max_seq: Option<i64> =
                conn.query_row(&sql, params![channel_id, CONTEXT_RESET_ITEM_TYPE], |r| {
                    r.get(0)
                })?;
            Ok(max_seq.unwrap_or(0).max(0) as u64)
        })
    }

    pub fn context_threads_for_agent(&self, agent_id: &str) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT thread_id FROM projection_channel_contexts
                 WHERE agent_id = ?1 ORDER BY channel_id",
            )?;
            let rows = stmt.query_map(params![agent_id], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for item in rows {
                out.push(item?);
            }
            Ok(out)
        })
    }

    pub fn clear_agent_contexts(&self, agent_id: &str) -> StorageResult<()> {
        let thread_ids = self.context_threads_for_agent(agent_id)?;
        if thread_ids.is_empty() {
            return Ok(());
        }
        let command_id = format!(
            "channel-agent-contexts-cleared:{agent_id}:{}",
            Uuid::new_v4()
        );
        let event = new_event(
            "channel-agent",
            agent_id,
            "channel.agent-contexts-cleared",
            serde_json::json!({ "threadIds": thread_ids }),
        );
        self.database.dispatch_with_projection(
            &command_id,
            None,
            &[event],
            |conn, sequences| {
                let now = now_iso();
                let seq = sequences.first().copied().unwrap_or(0);
                for tid in &thread_ids {
                    let cid: Option<String> = conn
                        .query_row(
                            "SELECT channel_id FROM projection_channel_contexts WHERE thread_id = ?1",
                            params![tid],
                            |r| r.get(0),
                        )
                        .optional()?;
                    let thread_meta: Option<(String, String)> = conn
                        .query_row(
                            "SELECT title, created_at FROM projection_threads WHERE thread_id = ?1",
                            params![tid],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                    let (Some(channel_id), Some((title, created_at))) = (cid, thread_meta) else {
                        continue;
                    };
                    delete_aggregate_history_tx(conn, "thread", std::slice::from_ref(tid))?;
                    delete_thread_attachments_tx(conn, tid)?;
                    conn.execute(
                        "DELETE FROM projection_channel_contexts WHERE thread_id = ?1",
                        params![tid],
                    )?;
                    conn.execute(
                        "DELETE FROM projection_threads WHERE thread_id = ?1",
                        params![tid],
                    )?;
                    conn.execute(
                        "INSERT INTO projection_threads VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
                        params![tid, agent_id, title, created_at, now, seq],
                    )?;
                    conn.execute(
                        "INSERT INTO projection_channel_contexts(channel_id, agent_id, thread_id)
                         VALUES (?1, ?2, ?3)",
                        params![channel_id, agent_id, tid],
                    )?;
                }
                Ok(())
            },
        )?;
        Ok(())
    }

    pub fn all_context_threads(&self) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT thread_id FROM projection_channel_contexts")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for item in rows {
                out.push(item?);
            }
            Ok(out)
        })
    }

    pub fn channel_for_thread(&self, thread_id: &str) -> StorageResult<Option<String>> {
        self.database.with_conn(|conn| {
            conn.query_row(
                "SELECT channel_id FROM projection_channel_contexts WHERE thread_id = ?1",
                params![thread_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn accept_context(
        &self,
        channel_id: &str,
        agent_id: &str,
        session_id: &str,
        through_sequence: u64,
        summary_version: u64,
    ) -> StorageResult<()> {
        let unchanged = self.database.with_conn(|conn| {
            let row: Option<(Option<String>, i64, i64)> = conn
                .query_row(
                    "SELECT session_id, through_sequence, summary_version
                     FROM projection_channel_contexts WHERE channel_id = ?1 AND agent_id = ?2",
                    params![channel_id, agent_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            Ok(row.is_some_and(|(sid, ts, sv)| {
                sid.as_deref() == Some(session_id)
                    && ts == through_sequence as i64
                    && sv == summary_version as i64
            }))
        })?;
        if unchanged {
            return Ok(());
        }
        let command_id = format!("channel-context-accepted:{}", Uuid::new_v4());
        let event = new_event(
            "channel",
            channel_id,
            "channel.context-accepted",
            serde_json::json!({
                "agentId": agent_id,
                "sessionId": session_id,
                "throughSequence": through_sequence,
                "summaryVersion": summary_version,
            }),
        );
        self.database
            .dispatch_with_projection(&command_id, None, &[event], |conn, _| {
                conn.execute(
                    "UPDATE projection_channel_contexts
                     SET session_id = ?1, through_sequence = ?2, summary_version = ?3
                     WHERE channel_id = ?4 AND agent_id = ?5",
                    params![
                        session_id,
                        through_sequence as i64,
                        summary_version as i64,
                        channel_id,
                        agent_id,
                    ],
                )?;
                Ok(())
            })?;
        Ok(())
    }

    pub fn reset_contexts_for_session(&self, session_id: &str) -> StorageResult<()> {
        let pairs: Vec<(String, String)> = self.database.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT channel_id, agent_id FROM projection_channel_contexts
                 WHERE session_id = ?1 AND through_sequence > 0",
            )?;
            let rows =
                stmt.query_map(params![session_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let mut out = Vec::new();
            for item in rows {
                out.push(item?);
            }
            Ok(out)
        })?;
        for (channel_id, agent_id) in pairs {
            self.accept_context(&channel_id, &agent_id, session_id, 0, 0)?;
        }
        Ok(())
    }
}

impl ChannelStore {
    pub fn summary(&self, channel_id: &str) -> StorageResult<ChannelHistorySummary> {
        self.database.with_conn(|conn| {
            let row: Option<ChannelHistorySummary> = conn
                .query_row(
                    "SELECT version, through_sequence, text
                     FROM projection_channel_summaries WHERE channel_id = ?1",
                    params![channel_id],
                    |r| {
                        Ok(ChannelHistorySummary {
                            version: r.get::<_, i64>(0)?.max(0) as u64,
                            through_sequence: r.get::<_, i64>(1)?.max(0) as u64,
                            text: r.get(2)?,
                        })
                    },
                )
                .optional()?;
            Ok(row.unwrap_or(ChannelHistorySummary {
                version: 0,
                through_sequence: 0,
                text: String::new(),
            }))
        })
    }

    pub fn save_summary(
        &self,
        channel_id: &str,
        summary: &ChannelHistorySummary,
    ) -> StorageResult<()> {
        let command_id = format!("channel-summary:{channel_id}:{}", summary.version);
        let event = new_event(
            "channel",
            channel_id,
            "channel.summarized",
            serde_json::to_value(summary)?,
        );
        self.database
            .dispatch_with_projection(&command_id, None, &[event], |conn, _| {
                conn.execute(
                    "INSERT INTO projection_channel_summaries VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(channel_id) DO UPDATE SET
                       version = excluded.version,
                       through_sequence = excluded.through_sequence,
                       text = excluded.text",
                    params![
                        channel_id,
                        summary.version as i64,
                        summary.through_sequence as i64,
                        summary.text,
                    ],
                )?;
                Ok(())
            })?;
        Ok(())
    }

    pub fn adopt_reads(
        &self,
        source_member_id: &str,
        target_member_id: &str,
    ) -> StorageResult<()> {
        if source_member_id == target_member_id {
            return Ok(());
        }
        let rows: Vec<(String, u64)> = self.database.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT channel_id, through_sequence FROM projection_channel_reads WHERE member_id = ?1",
            )?;
            let mapped = stmt.query_map(params![source_member_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.max(0) as u64))
            })?;
            let mut out = Vec::new();
            for item in mapped {
                out.push(item?);
            }
            Ok(out)
        })?;
        for (channel_id, through_sequence) in rows {
            let has_target = self.database.with_conn(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT 1 FROM projection_channel_reads WHERE channel_id = ?1 AND member_id = ?2",
                        params![channel_id, target_member_id],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()?
                    .is_some())
            })?;
            if has_target {
                continue;
            }
            let op_id = format!("adopt:{source_member_id}:{channel_id}");
            self.mark_read(&channel_id, target_member_id, through_sequence, &op_id)?;
        }
        Ok(())
    }

    pub fn mark_read(
        &self,
        channel_id: &str,
        member_id: &str,
        requested_sequence: u64,
        operation_id: &str,
    ) -> StorageResult<Channel> {
        let stored_seq = self
            .database
            .with_conn(|conn| Self::stored_sequence_tx(conn, channel_id))?;
        let through_sequence = requested_sequence.min(stored_seq);
        let command_id = format!("channel-read:{member_id}:{operation_id}");
        let event = new_event(
            "channel",
            channel_id,
            "channel.read",
            serde_json::json!({
                "memberId": member_id,
                "throughSequence": through_sequence,
            }),
        );
        let receipt = self.database.dispatch_with_projection(
            &command_id,
            None,
            &[event],
            |conn, _| {
                self.cache.lock().unread_counts.remove(channel_id);
                conn.execute(
                    "INSERT INTO projection_channel_reads VALUES (?1, ?2, ?3)
                     ON CONFLICT(channel_id, member_id) DO UPDATE SET
                       through_sequence = MAX(through_sequence, excluded.through_sequence)",
                    params![channel_id, member_id, through_sequence as i64],
                )?;
                Self::get_tx(conn, channel_id)
            },
        )?;
        Ok(receipt.result)
    }

    pub fn delete(&self, channel_id: &str, operation_id: Option<&str>) -> StorageResult<()> {
        if !self.exists(channel_id)? {
            return Err(StorageError::NotFound("Channel not found.".to_string()));
        }
        let thread_ids = self.context_threads(channel_id)?;
        let owned_op;
        let op_id = match operation_id {
            Some(s) => s,
            None => {
                owned_op = Uuid::new_v4().to_string();
                &owned_op
            }
        };
        let command_id = format!("channel-delete:{op_id}");
        let event = new_event(
            "channel",
            channel_id,
            "channel.deleted",
            serde_json::json!({ "threadIds": thread_ids }),
        );
        self.database
            .dispatch_with_projection(&command_id, None, &[event], |conn, _| {
                self.forget_counts(channel_id);
                self.replace_member_folders_tx(conn, channel_id, None)?;
                let memory_ids: Vec<String> = {
                    let mut s = conn.prepare(
                        "SELECT memory_id FROM projection_channel_memories WHERE channel_id = ?1",
                    )?;
                    let r = s.query_map(params![channel_id], |row| row.get(0))?;
                    let mut out = Vec::new();
                    for item in r {
                        out.push(item?);
                    }
                    out
                };
                let routine_ids = Self::channel_routine_ids_tx(conn, channel_id)?;
                revoke_channel_routine_webhooks_tx(conn, &routine_ids, true)?;
                delete_aggregate_history_tx(conn, "channel", &[channel_id.to_string()])?;
                delete_aggregate_history_tx(conn, "channel-memory", &memory_ids)?;
                delete_aggregate_history_tx(conn, "channel-routine", &routine_ids)?;
                delete_aggregate_history_tx(conn, "channel-routine-run", &routine_ids)?;
                if !routine_ids.is_empty() {
                    let placeholders = std::iter::repeat_n("?", routine_ids.len())
                        .collect::<Vec<_>>()
                        .join(", ");
                    conn.execute(
                        &format!(
                            "DELETE FROM projection_channel_routine_triggers WHERE routine_id IN ({placeholders})"
                        ),
                        params_from_iter(routine_ids.iter()),
                    )?;
                }
                for table in [
                    "projection_channel_contexts",
                    "projection_channel_summaries",
                    "projection_channel_reads",
                    "projection_channel_assignments",
                    "projection_channel_tasks",
                    "projection_channel_messages",
                    "projection_channel_routine_runs",
                    "projection_channel_routines",
                    "projection_channel_memories",
                ] {
                    conn.execute(
                        &format!("DELETE FROM {table} WHERE channel_id = ?1"),
                        params![channel_id],
                    )?;
                }
                conn.execute(
                    "DELETE FROM projection_channels WHERE channel_id = ?1",
                    params![channel_id],
                )?;
                for tid in &thread_ids {
                    delete_aggregate_history_tx(conn, "thread", std::slice::from_ref(tid))?;
                    delete_thread_attachments_tx(conn, tid)?;
                    conn.execute(
                        "DELETE FROM projection_threads WHERE thread_id = ?1",
                        params![tid],
                    )?;
                }
                Ok(())
            })?;
        Ok(())
    }
}

impl ChannelStore {
    fn channel_routine_ids_tx(conn: &Connection, channel_id: &str) -> StorageResult<Vec<String>> {
        let mut ids = HashSet::new();
        {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT aggregate_id FROM orchestration_events
                 WHERE aggregate_type = 'channel-routine'
                   AND json_extract(payload_json, '$.ownerId') = ?1",
            )?;
            let rows = stmt.query_map(params![channel_id], |r| r.get::<_, String>(0))?;
            for item in rows {
                ids.insert(item?);
            }
        }
        for table in ["projection_channel_routines", "projection_channel_routine_runs"] {
            let mut stmt = conn.prepare(&format!(
                "SELECT DISTINCT routine_id FROM {table} WHERE channel_id = ?1"
            ))?;
            let rows = stmt.query_map(params![channel_id], |r| r.get::<_, String>(0))?;
            for item in rows {
                ids.insert(item?);
            }
        }
        Ok(ids.into_iter().collect())
    }

    pub fn rebuild(&self, channel_id: &str) -> StorageResult<()> {
        self.forget_counts(channel_id);
        self.database.with_tx(|conn| {
            Self::drop_rebuild_temp_tables(conn)?;
            conn.execute(
                "CREATE TEMP TABLE _openbot_rebuild_memories AS
                 SELECT * FROM projection_channel_memories WHERE channel_id = ?1",
                params![channel_id],
            )?;
            conn.execute(
                "CREATE TEMP TABLE _openbot_rebuild_routines AS
                 SELECT * FROM projection_channel_routines WHERE channel_id = ?1",
                params![channel_id],
            )?;
            conn.execute(
                "CREATE TEMP TABLE _openbot_rebuild_triggers AS
                 SELECT * FROM projection_channel_routine_triggers
                 WHERE routine_id IN (SELECT routine_id FROM _openbot_rebuild_routines)",
                [],
            )?;
            conn.execute(
                "CREATE TEMP TABLE _openbot_rebuild_webhooks AS
                 SELECT * FROM projection_channel_routine_webhooks
                 WHERE routine_id IN (SELECT routine_id FROM _openbot_rebuild_routines)",
                [],
            )?;
            conn.execute(
                "CREATE TEMP TABLE _openbot_rebuild_runs AS
                 SELECT * FROM projection_channel_routine_runs WHERE channel_id = ?1",
                params![channel_id],
            )?;

            let channel_events: Vec<(String, String, String, i64)> = {
                let mut stmt = conn.prepare(
                    "SELECT event_type, payload_json, occurred_at, sequence
                     FROM orchestration_events
                     WHERE aggregate_type = 'channel' AND aggregate_id = ?1
                     ORDER BY sequence",
                )?;
                let rows = stmt.query_map(params![channel_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?;
                let mut out = Vec::new();
                for item in rows {
                    out.push(item?);
                }
                out
            };

            let memory_events: Vec<(String, String, i64)> = {
                let mut stmt = conn.prepare(
                    "SELECT event_type, payload_json, sequence
                     FROM orchestration_events
                     WHERE aggregate_type = 'channel-memory'
                       AND (json_extract(payload_json, '$.memory.channelId') = ?1
                         OR json_extract(payload_json, '$.channelId') = ?1)
                     ORDER BY sequence",
                )?;
                let rows = stmt.query_map(params![channel_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?;
                let mut out = Vec::new();
                for item in rows {
                    out.push(item?);
                }
                out
            };

            for suffix in [
                "assignments",
                "tasks",
                "messages",
                "summaries",
                "reads",
                "contexts",
                "memories",
                "routine_runs",
            ] {
                conn.execute(
                    &format!("DELETE FROM projection_channel_{suffix} WHERE channel_id = ?1"),
                    params![channel_id],
                )?;
            }
            conn.execute(
                "DELETE FROM projection_channel_routines WHERE channel_id = ?1",
                params![channel_id],
            )?;
            conn.execute(
                "DELETE FROM projection_channels WHERE channel_id = ?1",
                params![channel_id],
            )?;

            for (event_type, payload_json, occurred_at, sequence) in channel_events {
                let val: Value = serde_json::from_str(&payload_json)?;
                let Some(obj) = val.as_object() else {
                    return Err(StorageError::Validation("Invalid channel event.".to_string()));
                };
                match event_type.as_str() {
                    "channel.changed" => {
                        let raw_channel = obj.get("channel").ok_or_else(|| {
                            StorageError::Validation("Invalid channel change event.".to_string())
                        })?;
                        let channel = Channel::decode_normalized(raw_channel)
                            .map_err(|_| StorageError::Validation("Invalid channel change event.".to_string()))?;
                        let messages: Vec<ChannelMessage> = serde_json::from_value(
                            obj.get("messages").cloned().unwrap_or(Value::Null),
                        )
                        .map_err(|_| StorageError::Validation("Invalid channel change event.".to_string()))?;
                        let tasks: Vec<ChannelTask> = serde_json::from_value(
                            obj.get("tasks").cloned().unwrap_or(Value::Null),
                        )
                        .map_err(|_| StorageError::Validation("Invalid channel change event.".to_string()))?;
                        let raw_assigns = obj
                            .get("assignments")
                            .and_then(Value::as_array)
                            .ok_or_else(|| {
                                StorageError::Validation("Invalid channel change event.".to_string())
                            })?;
                        let mut assignments = Vec::with_capacity(raw_assigns.len());
                        for item in raw_assigns {
                            assignments.push(decode_stored_assignment_value(item)?);
                        }
                        self.project_tx(
                            conn,
                            &ChannelChange {
                                channel,
                                messages,
                                tasks,
                                assignments,
                            },
                        )?;
                    }
                    "channel.context-created" => {
                        let (Some(agent_id), Some(thread_id)) = (
                            obj.get("agentId").and_then(Value::as_str),
                            obj.get("threadId").and_then(Value::as_str),
                        ) else {
                            return Err(StorageError::Validation(
                                "Invalid channel context event.".to_string(),
                            ));
                        };
                        let ch_name = Self::get_tx(conn, channel_id)?.draft.name;
                        conn.execute(
                            "INSERT OR IGNORE INTO projection_threads VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
                            params![thread_id, agent_id, ch_name, occurred_at, occurred_at, sequence],
                        )?;
                        conn.execute(
                            "INSERT INTO projection_channel_contexts(channel_id, agent_id, thread_id)
                             VALUES (?1, ?2, ?3)",
                            params![channel_id, agent_id, thread_id],
                        )?;
                    }
                    "channel.context-accepted" => {
                        let (Some(agent_id), Some(session_id), Some(ts), Some(sv)) = (
                            obj.get("agentId").and_then(Value::as_str),
                            obj.get("sessionId").and_then(Value::as_str),
                            obj.get("throughSequence").and_then(Value::as_i64),
                            obj.get("summaryVersion").and_then(Value::as_i64),
                        ) else {
                            return Err(StorageError::Validation(
                                "Invalid context acceptance event.".to_string(),
                            ));
                        };
                        conn.execute(
                            "UPDATE projection_channel_contexts
                             SET session_id = ?1, through_sequence = ?2, summary_version = ?3
                             WHERE channel_id = ?4 AND agent_id = ?5",
                            params![session_id, ts, sv, channel_id, agent_id],
                        )?;
                    }
                    "channel.summarized" => {
                        let (Some(ver), Some(ts), Some(text)) = (
                            obj.get("version").and_then(Value::as_i64),
                            obj.get("throughSequence").and_then(Value::as_i64),
                            obj.get("text").and_then(Value::as_str),
                        ) else {
                            return Err(StorageError::Validation(
                                "Invalid channel summary event.".to_string(),
                            ));
                        };
                        conn.execute(
                            "INSERT OR REPLACE INTO projection_channel_summaries VALUES (?1, ?2, ?3, ?4)",
                            params![channel_id, ver, ts, text],
                        )?;
                    }
                    "channel.read" => {
                        let (Some(member_id), Some(ts)) = (
                            obj.get("memberId").and_then(Value::as_str),
                            obj.get("throughSequence").and_then(Value::as_i64),
                        ) else {
                            return Err(StorageError::Validation(
                                "Invalid channel read event.".to_string(),
                            ));
                        };
                        conn.execute(
                            "INSERT INTO projection_channel_reads VALUES (?1, ?2, ?3)
                             ON CONFLICT(channel_id, member_id) DO UPDATE SET
                               through_sequence = MAX(through_sequence, excluded.through_sequence)",
                            params![channel_id, member_id, ts],
                        )?;
                    }
                    _ => {
                        return Err(StorageError::Validation("Unknown channel event.".to_string()));
                    }
                }
            }

            let channel_rebuilt: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM projection_channels WHERE channel_id = ?1",
                    params![channel_id],
                    |r| r.get(0),
                )
                .optional()?;
            if channel_rebuilt.is_some() {
                Self::restore_memories_and_routines_tx(conn, channel_id, &memory_events)?;
            }
            Self::drop_rebuild_temp_tables(conn)?;
            Ok(())
        })
    }
}

impl ChannelStore {
    fn drop_rebuild_temp_tables(conn: &Connection) -> StorageResult<()> {
        conn.execute_batch(
            "DROP TABLE IF EXISTS temp._openbot_rebuild_runs;
             DROP TABLE IF EXISTS temp._openbot_rebuild_webhooks;
             DROP TABLE IF EXISTS temp._openbot_rebuild_triggers;
             DROP TABLE IF EXISTS temp._openbot_rebuild_routines;
             DROP TABLE IF EXISTS temp._openbot_rebuild_memories;",
        )?;
        Ok(())
    }

    fn restore_memories_and_routines_tx(
        conn: &Connection,
        channel_id: &str,
        memory_events: &[(String, String, i64)],
    ) -> StorageResult<()> {
        if memory_events.is_empty() {
            conn.execute(
                "INSERT OR IGNORE INTO projection_channel_memories
                 SELECT * FROM temp._openbot_rebuild_memories",
                [],
            )?;
        } else {
            for (ev_type, p_json, seq) in memory_events {
                let val: Value = serde_json::from_str(p_json)?;
                match ev_type.as_str() {
                    "channel-memory.created" | "channel-memory.updated" => {
                        if let Some(mem) = val.get("memory").and_then(Value::as_object)
                            && let (
                                Some(id),
                                Some(text),
                                Some(origin),
                                Some(created_at),
                                Some(updated_at),
                            ) = (
                                mem.get("id").and_then(Value::as_str),
                                mem.get("text").and_then(Value::as_str),
                                mem.get("origin").and_then(Value::as_str),
                                mem.get("createdAt").and_then(Value::as_str),
                                mem.get("updatedAt").and_then(Value::as_str),
                            )
                        {
                            let source_turn_id =
                                mem.get("sourceTurnId").and_then(Value::as_str);
                            conn.execute(
                                "INSERT INTO projection_channel_memories (
                                   memory_id, channel_id, text, normalized_text, origin,
                                   source_turn_id, created_at, updated_at, last_event_sequence
                                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                                 ON CONFLICT(memory_id) DO UPDATE SET
                                   text = excluded.text,
                                   normalized_text = excluded.normalized_text,
                                   origin = excluded.origin,
                                   source_turn_id = excluded.source_turn_id,
                                   updated_at = excluded.updated_at,
                                   last_event_sequence = excluded.last_event_sequence",
                                params![
                                    id,
                                    channel_id,
                                    text,
                                    text,
                                    origin,
                                    source_turn_id,
                                    created_at,
                                    updated_at,
                                    seq,
                                ],
                            )?;
                        }
                    }
                    "channel-memory.deleted" => {
                        if let Some(id) = val.get("memoryId").and_then(Value::as_str) {
                            conn.execute(
                                "DELETE FROM projection_channel_memories
                                 WHERE channel_id = ?1 AND memory_id = ?2",
                                params![channel_id, id],
                            )?;
                        }
                    }
                    _ => {}
                }
            }
        }
        conn.execute(
            "INSERT OR IGNORE INTO projection_channel_routines
             SELECT * FROM temp._openbot_rebuild_routines",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO projection_channel_routine_triggers
             SELECT * FROM temp._openbot_rebuild_triggers",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO projection_channel_routine_webhooks
             SELECT * FROM temp._openbot_rebuild_webhooks",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO projection_channel_routine_runs
             SELECT * FROM temp._openbot_rebuild_runs",
            [],
        )?;
        Ok(())
    }
}
