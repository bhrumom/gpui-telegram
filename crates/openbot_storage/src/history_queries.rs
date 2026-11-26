//! Agent Conversation History Queries & `history_search` / `history_read` Tools.
//!
//! Direct 1:1 port of:
//! - `src/backend/database/agent-history-queries.ts`
//! - `src/backend/agent/history-tools.ts`
//! - `src/backend/database/conversation-queries.ts:L710-L716`
//!
//! Enforces channel-context isolation (`projection_channel_contexts` excluded), `acrossResets: true`
//! with `beforeNewChat: true` tagging prior to the latest `context-reset`, `[work steps] ... [end work steps]`
//! tool-only trace formatting, relative/date/ISO `window_bound`, base64url cursors with `resetMessageId`
//! invalidation, `8k`/`16k` JSON caps, and `redact_secrets` on every excerpt/slice.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, NaiveDate, SecondsFormat, TimeZone, Utc};
use openbot_contracts::conversation::{
    AGENT_EXCHANGE_ITEM_TYPE, AGENT_WELCOME_ITEM_TYPE, AgentExchangeDirection,
    CHANNEL_RESUME_ITEM_TYPE, CONVERSATION_TRACE_STEP_LIMIT, CONTEXT_RESET_ITEM_TYPE,
    ConversationMessage, ConversationMessageAuthor, ConversationTraceStepKind,
    ConversationTurnTrace, ROUTINE_QUIET_ANSWER_ITEM_TYPE,
};
use openbot_contracts::limits::utf16_len;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::database::OpenBotDatabase;
use crate::error::{StorageError, StorageResult};
use crate::redact::redact_secrets;

pub const MAIN_CHAT_TASK_ID: &str = "main";
pub const THREADS_RESULT_CHARACTERS: usize = 8_000;
pub const SEARCH_ALL_THREADS: usize = 8;
pub const SEARCH_ALL_CHARACTERS: usize = 8_000;
pub const READ_RESULT_CHARACTERS: usize = 16_000;
pub const SEARCH_SCAN_ROWS: usize = 200;
pub const REQUEST_EXCERPT_CHARACTERS: usize = 200;
pub const REPLY_EXCERPT_CHARACTERS: usize = 400;

pub const READ_CURSOR_HINT: &str = "Send nextCursor to history_read as before for older messages.";
pub const NO_MATCH_HINT: &str =
    "No message holds all the words. Search again with fewer or shorter words.";
pub const HISTORY_UNAVAILABLE_MSG: &str =
    "History is unavailable for this request. Read recent history again, or use channel_history for channel work.";

pub const ORDERED_THREAD_MESSAGES: &str = "WITH ordered AS (
  SELECT created_at, ordinal, message_id, author, item_type, group_start, group_first, group_id, turn_rank
  FROM projection_thread_messages WHERE thread_id = ?
)";
pub const ORDER_KEY_COLUMNS: &str =
    "group_start, group_first, group_id, turn_rank, created_at, ordinal, message_id";
pub const ORDER_KEY_DESC: &str =
    "group_start DESC, group_first DESC, group_id DESC, turn_rank DESC, created_at DESC, ordinal DESC, message_id DESC";

const CHAT_VISUAL_ITEM_TYPE_PREFIX: &str = "visual-reply:";

#[derive(Debug, Clone, Default)]
pub struct AgentHistorySelection {
    pub before: Option<String>,
    pub message_id: Option<String>,
    pub author: Option<String>,
    pub outgoing: bool,
    pub across_resets: bool,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentHistoryRow {
    pub message: ConversationMessage,
    pub provider_session_id: Option<String>,
    pub before_reset: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentHistoryPage {
    pub reset_message_id: Option<String>,
    pub messages: Vec<AgentHistoryRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ThreadListCursorPosition {
    pub updated_at: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HistoryListCursor {
    version: u8,
    list: String,
    after: Option<String>,
    last: Option<ThreadListCursorPosition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HistoryPageCursor {
    version: u8,
    thread_id: String,
    reset_message_id: Option<String>,
    anchor_message_id: String,
    query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryThreadSummary {
    pub task_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    pub last_activity_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reply: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAttachmentInfo {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAnsweredBy {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub message_id: String,
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_to: Option<Vec<String>>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<HistoryAnsweredBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_new_chat: Option<bool>,
    pub text: String,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub total_characters: usize,
    pub attachments: Vec<HistoryAttachmentInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_steps: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_steps_offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_work_steps_offset: Option<Option<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments_omitted: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet: Option<String>,
}

pub fn wrap_openbot_tool_result<T: Serialize>(value: &T) -> Value {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    serde_json::json!({
        "success": true,
        "contentItems": [{ "type": "inputText", "text": text }],
    })
}

fn wrapped_result_chars<T: Serialize>(value: &T) -> usize {
    let wrapped = wrap_openbot_tool_result(value);
    serde_json::to_string(&wrapped).map_or(usize::MAX, |s| utf16_len(&s))
}

fn utf16_slice(text: &str, start: usize, len: usize) -> (String, usize) {
    let units: Vec<u16> = text.encode_utf16().collect();
    if start >= units.len() || len == 0 {
        return (String::new(), 0);
    }
    let mut end = (start + len).min(units.len());
    if end > start && end < units.len() && (0xD800..=0xDBFF).contains(&units[end - 1]) {
        end -= 1;
    }
    let slice = &units[start..end];
    (String::from_utf16_lossy(slice), end - start)
}

pub fn excerpt_text(text: &str, size: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let units: Vec<u16> = line.encode_utf16().collect();
    if units.len() <= size {
        return line;
    }
    if size == 0 {
        return "…".to_string();
    }
    let mut cut_end = size - 1;
    if cut_end > 0 && (0xD800..=0xDBFF).contains(&units[cut_end - 1]) {
        cut_end -= 1;
    }
    let cut = String::from_utf16_lossy(&units[..cut_end]);
    let space_u16 = cut.rfind(' ').map(|byte_idx| utf16_len(&cut[..byte_idx]));
    if let Some(sp) = space_u16
        && sp > size / 2
    {
        let trimmed = String::from_utf16_lossy(&units[..sp]);
        format!("{trimmed}…")
    } else {
        format!("{cut}…")
    }
}

/// Exact port of `windowBound` (`history-tools.ts:L26-L38`).
pub fn window_bound(text: &str, now_ms: i64) -> Option<String> {
    if utf16_len(text) > 40 || text.is_empty() {
        return None;
    }
    let bytes = text.as_bytes();
    if (2..=5).contains(&bytes.len()) {
        let last = bytes[bytes.len() - 1];
        let unit_ms = match last {
            b'm' => Some(60_000i64),
            b'h' => Some(3_600_000i64),
            b'd' => Some(86_400_000i64),
            b'w' => Some(604_800_000i64),
            _ => None,
        };
        if let Some(unit) = unit_ms
            && bytes[..bytes.len() - 1].iter().all(u8::is_ascii_digit)
            && let Ok(n) = text[..text.len() - 1].parse::<i64>()
        {
            let target_ms = now_ms.saturating_sub(n.saturating_mul(unit));
            let dt = Utc.timestamp_millis_opt(target_ms).single()?;
            return Some(dt.to_rfc3339_opts(SecondsFormat::Millis, true));
        }
    }
    if bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        let date = NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
        if date.format("%Y-%m-%d").to_string() != text {
            return None;
        }
        let dt = date.and_hms_milli_opt(0, 0, 0, 0)?.and_utc();
        return Some(dt.to_rfc3339_opts(SecondsFormat::Millis, true));
    }
    if bytes.len() >= 16
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15]
            .into_iter()
            .all(|idx| bytes[idx].is_ascii_digit())
    {
        if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
            return Some(dt.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Millis, true));
        }
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M"))
        {
            return Some(naive.and_utc().to_rfc3339_opts(SecondsFormat::Millis, true));
        }
    }
    None
}

/// Formats tool-only steps inside `[work steps] ... [end work steps]` (`history-tools.ts:L40-L50`).
/// Never emits raw `Reasoning` steps.
pub fn format_trace_work_steps(trace: &ConversationTurnTrace) -> Option<String> {
    let mut steps: Vec<String> = Vec::new();
    for step in &trace.steps {
        if step.kind != ConversationTraceStepKind::Tool {
            continue;
        }
        let status = if step.failed == Some(true) {
            "failed"
        } else if step.ended_at.is_none() {
            "unfinished"
        } else {
            "completed"
        };
        let name = match &step.command {
            Some(cmd) => format!("$ {cmd}"),
            None => format!("tool {}", step.text.as_deref().unwrap_or("step")),
        };
        if let Some(err) = &step.error {
            steps.push(format!("{name} ({status})\n{err}"));
        } else {
            steps.push(format!("{name} ({status})"));
        }
    }
    if steps.is_empty() {
        return None;
    }
    let mut lines = Vec::with_capacity(steps.len() + 3);
    lines.push("[work steps]".to_string());
    lines.extend(steps);
    if trace.steps.len() >= CONVERSATION_TRACE_STEP_LIMIT {
        lines.push("(later steps left out)".to_string());
    }
    lines.push("[end work steps]".to_string());
    Some(lines.join("\n"))
}

/// Parses `"quoted phrases"` and bare words (`history-tools.ts:L57-L64`).
pub fn parse_search_terms(query: &str) -> Vec<String> {
    let lower = query.to_lowercase();
    let mut terms = Vec::new();
    let chars: Vec<char> = lower.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] == '"'
            && let Some(close_rel) = chars[i + 1..].iter().position(|c| *c == '"')
            && close_rel > 0
        {
            let phrase: String = chars[i + 1..i + 1 + close_rel].iter().collect();
            let trimmed = phrase.trim();
            if !trimmed.is_empty() {
                terms.push(trimmed.to_string());
            }
            i += 1 + close_rel + 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        let cleaned = word.trim_matches('"').trim();
        if !cleaned.is_empty() {
            terms.push(cleaned.to_string());
        }
    }
    terms
}

/// Case-insensitive AND-all-terms substring match returning earliest UTF-16 offset (`history-tools.ts:L66-L75`).
pub fn search_match(text: &str, terms: &[String]) -> Option<usize> {
    if terms.is_empty() {
        return Some(0);
    }
    let lower = text.to_lowercase();
    let mut earliest = utf16_len(&lower);
    for term in terms {
        let byte_idx = lower.find(term.as_str())?;
        let u16_pos = utf16_len(&lower[..byte_idx]);
        earliest = earliest.min(u16_pos);
    }
    Some(earliest)
}

fn encode_list_cursor(after: Option<&str>, last: Option<&ThreadListCursorPosition>) -> String {
    let payload = HistoryListCursor {
        version: 1,
        list: "threads".to_string(),
        after: after.map(str::to_string),
        last: last.cloned(),
    };
    let json = serde_json::to_string(&payload).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(json.as_bytes())
}

fn decode_list_cursor(text: &str) -> Option<HistoryListCursor> {
    if text.is_empty() || utf16_len(text) > 4_096 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(text.as_bytes()).ok()?;
    let parsed: HistoryListCursor = serde_json::from_slice(&bytes).ok()?;
    if parsed.version != 1 || parsed.list != "threads" {
        return None;
    }
    if let Some(last) = &parsed.last
        && (last.updated_at.is_empty()
            || utf16_len(&last.updated_at) > 512
            || last.thread_id.is_empty()
            || utf16_len(&last.thread_id) > 512)
    {
        return None;
    }
    Some(parsed)
}

fn encode_page_cursor(
    thread_id: &str,
    reset_message_id: Option<&str>,
    anchor_message_id: &str,
    query: Option<&str>,
) -> String {
    let payload = HistoryPageCursor {
        version: 1,
        thread_id: thread_id.to_string(),
        reset_message_id: reset_message_id.map(str::to_string),
        anchor_message_id: anchor_message_id.to_string(),
        query: query.map(str::to_string),
    };
    let json = serde_json::to_string(&payload).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(json.as_bytes())
}

fn decode_page_cursor(text: &str) -> Option<HistoryPageCursor> {
    if text.is_empty() || utf16_len(text) > 4_096 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(text.as_bytes()).ok()?;
    let parsed: HistoryPageCursor = serde_json::from_slice(&bytes).ok()?;
    if parsed.version != 1
        || parsed.thread_id.is_empty()
        || utf16_len(&parsed.thread_id) > 512
        || parsed.anchor_message_id.is_empty()
        || utf16_len(&parsed.anchor_message_id) > 512
        || parsed
            .reset_message_id
            .as_ref()
            .is_some_and(|r| r.is_empty() || utf16_len(r) > 512)
    {
        return None;
    }
    Some(parsed)
}

pub fn history_cursor_thread(text: &str) -> Option<String> {
    decode_page_cursor(text).map(|c| c.thread_id)
}

pub fn is_history_browse(tool: &str, args: Option<&Value>) -> bool {
    if tool != "history_search" {
        return false;
    }
    match args {
        None | Some(Value::Null) => true,
        Some(Value::Object(map)) => !map.contains_key("query") && !map.contains_key("taskId"),
        _ => false,
    }
}

fn chat_visual_attachment_id(message: &ConversationMessage) -> Option<&str> {
    let item_type = message.item_type.as_deref()?;
    if !item_type.starts_with(CHAT_VISUAL_ITEM_TYPE_PREFIX) {
        return None;
    }
    let attachments = message.attachments.as_ref()?;
    if attachments.len() != 1 {
        return None;
    }
    let att = &attachments[0];
    let mime_base = att.mime_type.split(';').next().unwrap_or("").trim();
    mime_base
        .eq_ignore_ascii_case("text/html")
        .then_some(att.id.as_str())
}

const HISTORY_REFERENCE_UNAVAILABLE: &str =
    "History reference is unavailable. Read recent history again.";

#[derive(Debug, Clone)]
pub struct StoredAgentThreadSummary {
    pub task_id: String,
    pub agent_id: String,
    pub thread_id: String,
    pub title: String,
    pub outcome: Option<String>,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub has_branch: bool,
    pub routine_id: Option<String>,
}

#[derive(Clone)]
pub struct AgentHistoryQueries {
    database: OpenBotDatabase,
}

impl AgentHistoryQueries {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    pub fn is_channel_context_thread(&self, thread_id: &str) -> StorageResult<bool> {
        self.database.with_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT 1 FROM projection_channel_contexts WHERE thread_id = ?1",
                    params![thread_id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .is_some())
        })
    }

    pub fn read(
        &self,
        agent_id: &str,
        thread_id: &str,
        selection: &AgentHistorySelection,
    ) -> StorageResult<AgentHistoryPage> {
        self.database
            .with_conn(|conn| Self::read_tx(conn, agent_id, thread_id, selection))
    }

    fn read_tx(
        conn: &Connection,
        agent_id: &str,
        thread_id: &str,
        selection: &AgentHistorySelection,
    ) -> StorageResult<AgentHistoryPage> {
        let owner: Option<String> = conn
            .query_row(
                "SELECT thread_id FROM projection_threads WHERE thread_id = ?1 AND agent_id = ?2",
                params![thread_id, agent_id],
                |r| r.get(0),
            )
            .optional()?;
        if owner.is_none() {
            return Err(StorageError::NotFound(
                "History is unavailable in this conversation.".to_string(),
            ));
        }
        let cte = format!(
            "{ORDERED_THREAD_MESSAGES}, visible AS (
              SELECT ordered.*, message.message_json FROM ordered
              JOIN projection_thread_messages message ON message.thread_id = ? AND message.message_id = ordered.message_id
            ), boundary AS (
              SELECT * FROM visible WHERE item_type = ? AND author = 'system'
              AND json_extract(message_json, '$.source') = 'system' AND json_extract(message_json, '$.status') = 'completed'
              ORDER BY {ORDER_KEY_DESC} LIMIT 1
            ), allowed AS (
              SELECT * FROM visible WHERE ? OR NOT EXISTS (SELECT 1 FROM boundary)
              OR ({ORDER_KEY_COLUMNS}) > (SELECT {ORDER_KEY_COLUMNS} FROM boundary)
            )"
        );
        let across_i64 = i64::from(selection.across_resets);
        let reset_message_id: Option<String> = conn
            .query_row(
                &format!("{cte} SELECT message_id FROM boundary"),
                params![thread_id, thread_id, CONTEXT_RESET_ITEM_TYPE, across_i64],
                |r| r.get(0),
            )
            .optional()?;

        let anchor_req = selection
            .message_id
            .as_deref()
            .or(selection.before.as_deref());
        let anchor_message_id: Option<String> = match anchor_req {
            Some(aid) => {
                let direct: Option<String> = conn
                    .query_row(
                        &format!("{cte} SELECT message_id FROM allowed WHERE message_id = ?"),
                        params![
                            thread_id,
                            thread_id,
                            CONTEXT_RESET_ITEM_TYPE,
                            across_i64,
                            aid
                        ],
                        |r| r.get(0),
                    )
                    .optional()?;
                let resolved = match direct {
                    Some(id) => Some(id),
                    None => conn
                        .query_row(
                            &format!(
                                "{cte} SELECT message_id FROM allowed
                                 WHERE json_extract(message_json, '$.exchange.messageId') = ?
                                 ORDER BY {ORDER_KEY_DESC} LIMIT 1"
                            ),
                            params![
                                thread_id,
                                thread_id,
                                CONTEXT_RESET_ITEM_TYPE,
                                across_i64,
                                aid
                            ],
                            |r| r.get(0),
                        )
                        .optional()?,
                };
                if resolved.is_none() {
                    return Err(StorageError::NotFound(
                        HISTORY_REFERENCE_UNAVAILABLE.to_string(),
                    ));
                }
                resolved
            }
            None => None,
        };

        let exact_msg_id = selection
            .message_id
            .as_ref()
            .and(anchor_message_id.as_deref());
        let before_msg_id = selection.before.as_ref().and(anchor_message_id.as_deref());
        let outgoing_i64 = i64::from(selection.outgoing);
        let author_opt = selection.author.as_deref();
        let sql = format!(
            "{cte}
             SELECT message_json,
               COALESCE(({ORDER_KEY_COLUMNS}) < (SELECT {ORDER_KEY_COLUMNS} FROM boundary), 0) AS before_reset,
               (SELECT provider_session_id FROM projection_turns
                WHERE thread_id = ? AND turn_id = json_extract(allowed.message_json, '$.turnId')) AS provider_session_id
             FROM allowed
             WHERE (author IN ('user', 'assistant', 'agent') OR (? AND author = 'system'
                 AND item_type = '{AGENT_EXCHANGE_ITEM_TYPE}'
                 AND json_extract(message_json, '$.exchange.direction') = 'outgoing'))
               AND COALESCE(item_type, '') NOT IN ('commentary', '{AGENT_WELCOME_ITEM_TYPE}', '{CHANNEL_RESUME_ITEM_TYPE}')
               AND COALESCE(json_extract(message_json, '$.delivery.status'), 'completed') IN ('completed', 'failed', 'interrupted')
               AND (? IS NULL OR author = ?)
               AND (? IS NULL OR message_id = ?)
               AND (? IS NULL OR ({ORDER_KEY_COLUMNS}) < (SELECT {ORDER_KEY_COLUMNS} FROM allowed WHERE message_id = ?))
             ORDER BY {ORDER_KEY_DESC} LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                thread_id,
                thread_id,
                CONTEXT_RESET_ITEM_TYPE,
                across_i64,
                thread_id,
                outgoing_i64,
                author_opt,
                author_opt,
                exact_msg_id,
                exact_msg_id,
                before_msg_id,
                before_msg_id,
                selection.limit as i64,
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)? == 1,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )?;
        let mut messages = Vec::new();
        for item in rows {
            let (m_json, before_reset, provider_session_id) = item?;
            let message: ConversationMessage = serde_json::from_str(&m_json)?;
            messages.push(AgentHistoryRow {
                message,
                provider_session_id,
                before_reset,
            });
        }
        Ok(AgentHistoryPage {
            reset_message_id,
            messages,
        })
    }

    pub fn turn_trace(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> StorageResult<Option<ConversationTurnTrace>> {
        self.database.with_conn(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT json_extract(message_json, '$.trace')
                     FROM projection_thread_messages
                     WHERE thread_id = ?1 AND turn_id = ?2
                       AND json_type(message_json, '$.trace') = 'object'
                     LIMIT 1",
                    params![thread_id, turn_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(raw) = raw.filter(|s| !s.is_empty()) else {
                return Ok(None);
            };
            Ok(serde_json::from_str::<ConversationTurnTrace>(&raw)
                .ok()
                .filter(ConversationTurnTrace::is_valid))
        })
    }
}

impl AgentHistoryQueries {
    pub fn main_thread_id(&self, agent_id: &str) -> StorageResult<Option<String>> {
        self.database.with_conn(|conn| {
            conn.query_row(
                "SELECT thread_id FROM projection_agents WHERE agent_id = ?1",
                params![agent_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn recent_threads(
        &self,
        agent_id: &str,
        after: Option<&str>,
        limit: usize,
        last: Option<&ThreadListCursorPosition>,
    ) -> StorageResult<Vec<StoredAgentThreadSummary>> {
        let last_upd = last.map(|l| l.updated_at.as_str());
        let last_tid = last.map(|l| l.thread_id.as_str());
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT COALESCE(legacy_task_id, thread_id) AS task_id,
                        agent_id, thread_id, title, outcome, updated_at, archived_at,
                        branch_source_thread_id, branch_source_message_id, branch_through_message_id,
                        routine_id
                 FROM projection_agent_threads
                 WHERE agent_id = ?1
                   AND (?2 IS NULL OR updated_at >= ?2)
                   AND (?3 IS NULL OR updated_at < ?3 OR (updated_at = ?3 AND thread_id > ?4))
                   AND NOT EXISTS (
                     SELECT 1 FROM projection_channel_contexts cc WHERE cc.thread_id = projection_agent_threads.thread_id
                   )
                 ORDER BY updated_at DESC, thread_id
                 LIMIT ?5",
            )?;
            let rows = stmt.query_map(
                params![agent_id, after, last_upd, last_tid, limit as i64],
                |r| {
                    let b1: Option<String> = r.get(7)?;
                    let b2: Option<String> = r.get(8)?;
                    let b3: Option<String> = r.get(9)?;
                    Ok(StoredAgentThreadSummary {
                        task_id: r.get(0)?,
                        agent_id: r.get(1)?,
                        thread_id: r.get(2)?,
                        title: r.get(3)?,
                        outcome: r.get(4)?,
                        updated_at: r.get(5)?,
                        closed_at: r.get(6)?,
                        has_branch: b1.is_some() && b2.is_some() && b3.is_some(),
                        routine_id: r.get(10)?,
                    })
                },
            )?;
            let mut out = Vec::new();
            for item in rows {
                out.push(item?);
            }
            Ok(out)
        })
    }

    pub fn resolve_task_thread(&self, agent_id: &str, task_id: &str) -> StorageResult<String> {
        if task_id == MAIN_CHAT_TASK_ID {
            let Some(main_tid) = self.main_thread_id(agent_id)? else {
                return Err(StorageError::NotFound(HISTORY_UNAVAILABLE_MSG.to_string()));
            };
            if self.is_channel_context_thread(&main_tid)? {
                return Err(StorageError::Forbidden(HISTORY_UNAVAILABLE_MSG.to_string()));
            }
            return Ok(main_tid);
        }
        let resolved: Option<String> = self.database.with_conn(|conn| {
            conn.query_row(
                "SELECT thread_id FROM projection_agent_threads
                 WHERE agent_id = ?1
                   AND (legacy_task_id = ?2 OR (legacy_task_id IS NULL AND thread_id = ?2))",
                params![agent_id, task_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(StorageError::from)
        })?;
        let Some(tid) = resolved else {
            return Err(StorageError::NotFound(HISTORY_UNAVAILABLE_MSG.to_string()));
        };
        if self.is_channel_context_thread(&tid)? {
            return Err(StorageError::Forbidden(HISTORY_UNAVAILABLE_MSG.to_string()));
        }
        Ok(tid)
    }
}

#[derive(Debug, Clone, Default)]
struct ParsedSearchArgs {
    task_id: Option<String>,
    query: Option<String>,
    after: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Default)]
struct ParsedReadArgs {
    task_id: Option<String>,
    message_id: Option<String>,
    before: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
    include_work_steps: bool,
    work_steps_offset: Option<usize>,
}

fn validate_id_field(val: &Value, field: &str) -> StorageResult<String> {
    let Some(s) = val.as_str() else {
        return Err(StorageError::Validation(format!("{field} must be a string.")));
    };
    let len = utf16_len(s);
    if !(1..=512).contains(&len) {
        return Err(StorageError::Validation(format!(
            "{field} must be 1..=512 characters."
        )));
    }
    Ok(s.to_string())
}

fn validate_cursor_field(val: &Value, field: &str) -> StorageResult<String> {
    let Some(s) = val.as_str() else {
        return Err(StorageError::Validation(format!("{field} must be a string.")));
    };
    let len = utf16_len(s);
    if !(1..=4_096).contains(&len) {
        return Err(StorageError::Validation(format!(
            "{field} must be 1..=4096 characters."
        )));
    }
    Ok(s.to_string())
}

fn parse_search_args(raw: Option<&Value>, now_ms: i64) -> StorageResult<ParsedSearchArgs> {
    let Some(val) = raw.filter(|v| !v.is_null()) else {
        return Ok(ParsedSearchArgs::default());
    };
    let Some(map) = val.as_object() else {
        return Err(StorageError::Validation(
            "history_search arguments must be an object.".to_string(),
        ));
    };
    for key in map.keys() {
        if !matches!(
            key.as_str(),
            "taskId" | "query" | "after" | "cursor" | "limit"
        ) {
            return Err(StorageError::Validation(format!(
                "Unrecognized argument: {key}"
            )));
        }
    }
    let task_id = map
        .get("taskId")
        .map(|v| validate_id_field(v, "taskId"))
        .transpose()?;
    let query = match map.get("query") {
        Some(v) => {
            let Some(s) = v.as_str() else {
                return Err(StorageError::Validation("query must be a string.".to_string()));
            };
            let trimmed = s.trim();
            if utf16_len(trimmed) > 200 {
                return Err(StorageError::Validation(
                    "query must be at most 200 characters.".to_string(),
                ));
            }
            Some(trimmed.to_string())
        }
        None => None,
    };
    let after = match map.get("after") {
        Some(v) => {
            let Some(s) = v.as_str() else {
                return Err(StorageError::Validation("after must be a string.".to_string()));
            };
            let trimmed = s.trim();
            if window_bound(trimmed, now_ms).is_none() {
                return Err(StorageError::Validation(
                    "Use 24h, 7d, a date or an ISO time.".to_string(),
                ));
            }
            Some(trimmed.to_string())
        }
        None => None,
    };
    let cursor = map
        .get("cursor")
        .map(|v| validate_cursor_field(v, "cursor"))
        .transpose()?;
    let limit = match map.get("limit") {
        Some(v) => {
            let Some(n) = v.as_i64() else {
                return Err(StorageError::Validation(
                    "limit must be an integer 1..=10.".to_string(),
                ));
            };
            if !(1..=10).contains(&n) {
                return Err(StorageError::Validation(
                    "limit must be an integer 1..=10.".to_string(),
                ));
            }
            Some(n as usize)
        }
        None => None,
    };
    if let Some(cur) = &cursor {
        let valid_combo = if decode_list_cursor(cur).is_none() {
            query.is_some()
        } else {
            query.is_none() && task_id.is_none()
        };
        if !valid_combo {
            return Err(StorageError::Validation(
                "A cursor continues the call that gave it. Send a history_search cursor with the same query, a chat list cursor with no query, and a history_read cursor to history_read as before.".to_string(),
            ));
        }
    }
    Ok(ParsedSearchArgs {
        task_id,
        query,
        after,
        cursor,
        limit,
    })
}

fn parse_read_args(raw: Option<&Value>) -> StorageResult<ParsedReadArgs> {
    let Some(val) = raw.filter(|v| !v.is_null()) else {
        return Ok(ParsedReadArgs::default());
    };
    let Some(map) = val.as_object() else {
        return Err(StorageError::Validation(
            "history_read arguments must be an object.".to_string(),
        ));
    };
    if map.contains_key("cursor") {
        return Err(StorageError::Validation(format!(
            "There is no cursor argument. {READ_CURSOR_HINT}"
        )));
    }
    for key in map.keys() {
        if !matches!(
            key.as_str(),
            "taskId"
                | "messageId"
                | "before"
                | "offset"
                | "limit"
                | "includeWorkSteps"
                | "workStepsOffset"
        ) {
            return Err(StorageError::Validation(format!(
                "Unrecognized argument: {key}"
            )));
        }
    }
    let task_id = map
        .get("taskId")
        .map(|v| validate_id_field(v, "taskId"))
        .transpose()?;
    let message_id = map
        .get("messageId")
        .map(|v| validate_id_field(v, "messageId"))
        .transpose()?;
    let before = map
        .get("before")
        .map(|v| validate_cursor_field(v, "before"))
        .transpose()?;
    let offset = match map.get("offset") {
        Some(v) => {
            let Some(n) = v.as_u64() else {
                return Err(StorageError::Validation(
                    "offset must be a non-negative integer.".to_string(),
                ));
            };
            Some(n as usize)
        }
        None => None,
    };
    let limit = match map.get("limit") {
        Some(v) => {
            let Some(n) = v.as_i64() else {
                return Err(StorageError::Validation(
                    "limit must be an integer 1..=20.".to_string(),
                ));
            };
            if !(1..=20).contains(&n) {
                return Err(StorageError::Validation(
                    "limit must be an integer 1..=20.".to_string(),
                ));
            }
            Some(n as usize)
        }
        None => None,
    };
    let include_work_steps = match map.get("includeWorkSteps") {
        Some(v) => v.as_bool().ok_or_else(|| {
            StorageError::Validation("includeWorkSteps must be a boolean.".to_string())
        })?,
        None => false,
    };
    let work_steps_offset = match map.get("workStepsOffset") {
        Some(v) => {
            let Some(n) = v.as_u64() else {
                return Err(StorageError::Validation(
                    "workStepsOffset must be a non-negative integer.".to_string(),
                ));
            };
            Some(n as usize)
        }
        None => None,
    };
    if message_id.is_some() && before.is_some() {
        return Err(StorageError::Validation(
            "Provide either messageId or before, not both.".to_string(),
        ));
    }
    if offset.is_some() && message_id.is_none() {
        return Err(StorageError::Validation(
            "offset requires messageId.".to_string(),
        ));
    }
    if work_steps_offset.is_some() && (message_id.is_none() || !include_work_steps) {
        return Err(StorageError::Validation(
            "workStepsOffset requires messageId and includeWorkSteps: true.".to_string(),
        ));
    }
    Ok(ParsedReadArgs {
        task_id,
        message_id,
        before,
        offset,
        limit,
        include_work_steps,
        work_steps_offset,
    })
}

fn read_content_fingerprint(message: &ConversationMessage) -> String {
    let mut cloned = message.clone();
    if let Some(ex) = &mut cloned.exchange {
        ex.deliveries.clear();
    }
    serde_json::to_string(&cloned).unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryThreadsPayload {
    pub now: String,
    pub time_zone: String,
    pub threads: Vec<HistoryThreadSummary>,
    pub more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub next: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySearchAllPayload {
    pub messages: Vec<HistoryEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub more: bool,
    pub next: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPagePayload {
    pub messages: Vec<HistoryEntry>,
    pub next_cursor: Option<String>,
    pub order: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

pub type CustomRedactorFn = Arc<dyn Fn(&str) -> String + Send + Sync>;
pub type CapturedStepsLookupFn =
    Arc<dyn Fn(&str, &str) -> Option<String> + Send + Sync>;
pub type VisualPageTextFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Clone)]
pub struct HistoryTools {
    queries: AgentHistoryQueries,
    custom_redact: Option<CustomRedactorFn>,
    captured_steps: Option<CapturedStepsLookupFn>,
    page_text: Option<VisualPageTextFn>,
}

impl HistoryTools {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self {
            queries: AgentHistoryQueries::new(database),
            custom_redact: None,
            captured_steps: None,
            page_text: None,
        }
    }

    pub fn with_hooks(
        database: OpenBotDatabase,
        custom_redact: Option<CustomRedactorFn>,
        captured_steps: Option<CapturedStepsLookupFn>,
        page_text: Option<VisualPageTextFn>,
    ) -> Self {
        Self {
            queries: AgentHistoryQueries::new(database),
            custom_redact,
            captured_steps,
            page_text,
        }
    }

    pub fn queries(&self) -> &AgentHistoryQueries {
        &self.queries
    }

    fn redact(&self, text: &str) -> String {
        let base = redact_secrets(text);
        match &self.custom_redact {
            Some(f) => f(&base),
            None => base,
        }
    }
}

impl HistoryTools {
    pub fn history_search(
        &self,
        agent_id: &str,
        current_thread_id: &str,
        args: Option<&Value>,
    ) -> StorageResult<Value> {
        if self.queries.is_channel_context_thread(current_thread_id)? {
            return Err(StorageError::Forbidden(HISTORY_UNAVAILABLE_MSG.to_string()));
        }
        let now_ms = Utc::now().timestamp_millis();
        let parsed = parse_search_args(args, now_ms)?;
        if parsed.query.is_none() && parsed.task_id.is_none() {
            let out = self.threads_with_parsed(agent_id, current_thread_id, &parsed, now_ms)?;
            return Ok(serde_json::to_value(out)?);
        }
        if parsed.query.is_some() && parsed.task_id.is_none() && parsed.cursor.is_none() {
            let out = self.search_all_with_parsed(agent_id, current_thread_id, &parsed, now_ms)?;
            return Ok(serde_json::to_value(out)?);
        }
        let target_thread_id = if let Some(tid_arg) = &parsed.task_id {
            self.queries.resolve_task_thread(agent_id, tid_arg)?
        } else if let Some(cur) = &parsed.cursor
            && let Some(cur_tid) = history_cursor_thread(cur)
        {
            if self.queries.is_channel_context_thread(&cur_tid)? {
                return Err(StorageError::Forbidden(HISTORY_UNAVAILABLE_MSG.to_string()));
            }
            cur_tid
        } else {
            current_thread_id.to_string()
        };
        let out = self.page_search(agent_id, &target_thread_id, &parsed, now_ms)?;
        Ok(serde_json::to_value(out)?)
    }

    pub fn history_read(
        &self,
        agent_id: &str,
        current_thread_id: &str,
        args: Option<&Value>,
    ) -> StorageResult<Value> {
        if self.queries.is_channel_context_thread(current_thread_id)? {
            return Err(StorageError::Forbidden(HISTORY_UNAVAILABLE_MSG.to_string()));
        }
        let parsed = parse_read_args(args)?;
        let target_thread_id = match &parsed.task_id {
            Some(tid_arg) => self.queries.resolve_task_thread(agent_id, tid_arg)?,
            None => current_thread_id.to_string(),
        };
        let out = self.page_read(agent_id, &target_thread_id, &parsed)?;
        Ok(serde_json::to_value(out)?)
    }

    pub fn threads(
        &self,
        agent_id: &str,
        current_thread_id: &str,
        args: Option<&Value>,
    ) -> StorageResult<HistoryThreadsPayload> {
        if self.queries.is_channel_context_thread(current_thread_id)? {
            return Err(StorageError::Forbidden(HISTORY_UNAVAILABLE_MSG.to_string()));
        }
        let now_ms = Utc::now().timestamp_millis();
        let parsed = parse_search_args(args, now_ms)?;
        if parsed.query.is_some() || parsed.task_id.is_some() {
            return Err(StorageError::Validation(HISTORY_UNAVAILABLE_MSG.to_string()));
        }
        self.threads_with_parsed(agent_id, current_thread_id, &parsed, now_ms)
    }

    fn threads_with_parsed(
        &self,
        agent_id: &str,
        current_thread_id: &str,
        input: &ParsedSearchArgs,
        now_ms: i64,
    ) -> StorageResult<HistoryThreadsPayload> {
        let list_cursor = match &input.cursor {
            Some(c) => Some(decode_list_cursor(c).ok_or_else(|| {
                StorageError::Validation(HISTORY_UNAVAILABLE_MSG.to_string())
            })?),
            None => None,
        };
        let after: Option<String> = match &list_cursor {
            Some(cur) => cur.after.clone(),
            None => input
                .after
                .as_deref()
                .and_then(|a| window_bound(a, now_ms)),
        };
        let limit = input.limit.unwrap_or(8);

        let latest_msg = |tid: &str, author: &str| -> Option<ConversationMessage> {
            self.queries
                .read(
                    agent_id,
                    tid,
                    &AgentHistorySelection {
                        author: Some(author.to_string()),
                        across_resets: true,
                        limit: 1,
                        ..Default::default()
                    },
                )
                .ok()
                .and_then(|p| p.messages.into_iter().next().map(|r| r.message))
        };

        let build_entry =
            |task_id: &str, tid: &str, rec: Option<&StoredAgentThreadSummary>| -> HistoryThreadSummary {
                let req = latest_msg(tid, "user");
                let rep = latest_msg(tid, "assistant");
                let mut activity: Vec<String> = Vec::new();
                if let Some(m) = &req {
                    activity.push(m.created_at.clone());
                }
                if let Some(m) = &rep {
                    activity.push(m.created_at.clone());
                }
                if activity.is_empty()
                    && let Some(r) = rec
                {
                    activity.push(r.updated_at.clone());
                }
                activity.sort();
                let kind = match rec {
                    None => Some("chat".to_string()),
                    Some(r) if r.routine_id.is_some() => Some("routine".to_string()),
                    Some(r) if r.has_branch => Some("branch".to_string()),
                    _ => None,
                };
                let title = match rec {
                    Some(r) => excerpt_text(&self.redact(&r.title), REQUEST_EXCERPT_CHARACTERS),
                    None => "Main chat".to_string(),
                };
                let last_req = req
                    .map(|m| excerpt_text(&self.redact(&m.text), REQUEST_EXCERPT_CHARACTERS))
                    .filter(|s| !s.is_empty() && *s != title);
                let last_rep =
                    rep.map(|m| excerpt_text(&self.redact(&m.text), REPLY_EXCERPT_CHARACTERS));
                HistoryThreadSummary {
                    task_id: task_id.to_string(),
                    title,
                    kind,
                    archived: rec.and_then(|r| r.closed_at.as_ref().map(|_| true)),
                    current: (tid == current_thread_id).then_some(true),
                    outcome: rec.and_then(|r| r.outcome.clone()),
                    last_activity_at: activity.pop(),
                    last_request: last_req,
                    last_reply: last_rep,
                }
            };

        let is_shown = |entry: &HistoryThreadSummary| -> bool {
            after
                .as_deref()
                .is_none_or(|bound| entry.last_activity_at.as_deref().unwrap_or("") >= bound)
        };

        let main_entry = if input.cursor.is_none() {
            self.queries
                .main_thread_id(agent_id)?
                .map(|tid| build_entry(MAIN_CHAT_TASK_ID, &tid, None))
                .filter(is_shown)
        } else {
            None
        };

        let records = self.queries.recent_threads(
            agent_id,
            after.as_deref(),
            limit + 1,
            list_cursor.as_ref().and_then(|c| c.last.as_ref()),
        )?;
        let take_count = limit.saturating_sub(usize::from(main_entry.is_some()));
        let mut page: Vec<(StoredAgentThreadSummary, HistoryThreadSummary)> = records
            .iter()
            .take(take_count)
            .map(|r| (r.clone(), build_entry(&r.task_id, &r.thread_id, Some(r))))
            .collect();
        let mut more = records.len() > page.len();
        let now_iso = Utc
            .timestamp_millis_opt(now_ms)
            .single()
            .unwrap_or_else(Utc::now)
            .to_rfc3339_opts(SecondsFormat::Millis, true);

        let make_payload =
            |page_slice: &[(StoredAgentThreadSummary, HistoryThreadSummary)], has_more: bool| {
                let mut threads: Vec<HistoryThreadSummary> = Vec::new();
                if let Some(m) = &main_entry {
                    threads.push(m.clone());
                }
                for (_, entry) in page_slice {
                    if is_shown(entry) {
                        threads.push(entry.clone());
                    }
                }
                threads.sort_by(|a, b| {
                    b.last_activity_at
                        .as_deref()
                        .unwrap_or("")
                        .cmp(a.last_activity_at.as_deref().unwrap_or(""))
                });
                let next_cursor = has_more.then(|| {
                    let last_pos = page_slice.last().map(|(rec, _)| ThreadListCursorPosition {
                        updated_at: rec.updated_at.clone(),
                        thread_id: rec.thread_id.clone(),
                    });
                    encode_list_cursor(after.as_deref(), last_pos.as_ref())
                });
                HistoryThreadsPayload {
                    now: now_iso.clone(),
                    time_zone: "UTC".to_string(),
                    threads,
                    more: has_more,
                    next_cursor,
                    next: if has_more {
                        "Send nextCursor as cursor, with no query, for older chats. history_read with a taskId reads one.".to_string()
                    } else {
                        "history_read with a taskId reads one.".to_string()
                    },
                }
            };

        let min_keep = usize::from(main_entry.is_none());
        let mut payload = make_payload(&page, more);
        while wrapped_result_chars(&payload) > THREADS_RESULT_CHARACTERS && page.len() > min_keep {
            page.pop();
            more = true;
            payload = make_payload(&page, more);
        }
        Ok(payload)
    }
}

impl HistoryTools {
    pub fn search_all(
        &self,
        agent_id: &str,
        current_thread_id: &str,
        args: Option<&Value>,
    ) -> StorageResult<HistorySearchAllPayload> {
        if self.queries.is_channel_context_thread(current_thread_id)? {
            return Err(StorageError::Forbidden(HISTORY_UNAVAILABLE_MSG.to_string()));
        }
        let now_ms = Utc::now().timestamp_millis();
        let parsed = parse_search_args(args, now_ms)?;
        if parsed.query.is_none() || parsed.task_id.is_some() || parsed.cursor.is_some() {
            return Err(StorageError::Validation(HISTORY_UNAVAILABLE_MSG.to_string()));
        }
        self.search_all_with_parsed(agent_id, current_thread_id, &parsed, now_ms)
    }

    fn search_all_with_parsed(
        &self,
        agent_id: &str,
        current_thread_id: &str,
        input: &ParsedSearchArgs,
        now_ms: i64,
    ) -> StorageResult<HistorySearchAllPayload> {
        let after = input
            .after
            .as_deref()
            .and_then(|a| window_bound(a, now_ms));
        let limit = input.limit.unwrap_or(5);

        let mut conversations: Vec<(Option<String>, String, bool)> = Vec::new();
        if let Some(main_tid) = self.queries.main_thread_id(agent_id)?
            && !self.queries.is_channel_context_thread(&main_tid)?
        {
            conversations.push((Some(MAIN_CHAT_TASK_ID.to_string()), main_tid, false));
        }
        let recents =
            self.queries
                .recent_threads(agent_id, after.as_deref(), SEARCH_ALL_THREADS, None)?;
        let recent_count = recents.len();
        for rec in recents {
            conversations.push((Some(rec.task_id), rec.thread_id, true));
        }

        let mut ordered: Vec<(Option<String>, String, bool)> = Vec::with_capacity(conversations.len() + 1);
        for item in &conversations {
            if item.1 == current_thread_id {
                ordered.push(item.clone());
            }
        }
        for item in conversations {
            if item.1 != current_thread_id {
                ordered.push(item);
            }
        }
        if !ordered.iter().any(|(_, tid, _)| tid == current_thread_id) {
            ordered.insert(0, (None, current_thread_id.to_string(), false));
        }

        let mut result = HistorySearchAllPayload {
            messages: Vec::new(),
            next_cursor: None,
            more: recent_count >= SEARCH_ALL_THREADS,
            next: "nextCursor continues this chat; a taskId searches another one further."
                .to_string(),
        };

        for (conv_task_id, conv_thread_id, _) in ordered {
            if result.messages.len() >= limit {
                result.more = true;
                break;
            }
            let is_current = conv_thread_id == current_thread_id;
            let sub_args = ParsedSearchArgs {
                task_id: None,
                query: input.query.clone(),
                after: input.after.clone(),
                cursor: None,
                limit: Some(limit - result.messages.len()),
            };
            let page_res = self.page_search(agent_id, &conv_thread_id, &sub_args, now_ms);
            let page = match page_res {
                Ok(p) => p,
                Err(e) if is_current => return Err(e),
                Err(_) => continue,
            };
            if let Some(nc) = page.next_cursor {
                result.more = true;
                if is_current {
                    result.next_cursor = Some(nc);
                }
            }
            for mut msg in page.messages {
                if !is_current
                    && let Some(tid) = &conv_task_id
                {
                    msg.task_id = Some(tid.clone());
                }
                result.messages.push(msg);
            }
            let mut cut = false;
            while wrapped_result_chars(&result) > SEARCH_ALL_CHARACTERS
                && result.messages.len() > 1
            {
                result.messages.pop();
                cut = true;
            }
            if cut {
                result.more = true;
                break;
            }
        }
        if result.messages.is_empty() && !result.more {
            result.next = format!("{NO_MATCH_HINT} {}", result.next);
        }
        Ok(result)
    }

    fn page_search(
        &self,
        agent_id: &str,
        thread_id: &str,
        search: &ParsedSearchArgs,
        now_ms: i64,
    ) -> StorageResult<HistoryPagePayload> {
        let Some(query_str) = search.query.as_deref() else {
            return Err(StorageError::Validation(HISTORY_UNAVAILABLE_MSG.to_string()));
        };
        let after = search
            .after
            .as_deref()
            .and_then(|a| window_bound(a, now_ms));
        let cursor = match &search.cursor {
            Some(c) => Some(decode_page_cursor(c).ok_or_else(|| {
                StorageError::Validation(HISTORY_UNAVAILABLE_MSG.to_string())
            })?),
            None => None,
        };
        self.page_inner(
            agent_id,
            thread_id,
            true,
            Some(query_str),
            after.as_deref(),
            cursor,
            None,
            None,
            0,
            search.limit.unwrap_or(5),
            false,
            0,
        )
    }

    fn page_read(
        &self,
        agent_id: &str,
        thread_id: &str,
        read: &ParsedReadArgs,
    ) -> StorageResult<HistoryPagePayload> {
        let cursor = read.before.as_deref().and_then(decode_page_cursor);
        let before_msg_id = if read.before.is_some() && cursor.is_none() {
            read.before.as_deref()
        } else {
            None
        };
        self.page_inner(
            agent_id,
            thread_id,
            false,
            None,
            None,
            cursor,
            before_msg_id,
            read.message_id.as_deref(),
            read.offset.unwrap_or(0),
            read.limit.unwrap_or(10),
            read.include_work_steps,
            read.work_steps_offset.unwrap_or(0),
        )
    }
}

impl HistoryTools {
    #[allow(clippy::too_many_arguments)]
    fn page_inner(
        &self,
        agent_id: &str,
        thread_id: &str,
        is_search: bool,
        query: Option<&str>,
        after: Option<&str>,
        cursor: Option<HistoryPageCursor>,
        before_message_id: Option<&str>,
        exact_message_id: Option<&str>,
        requested_offset: usize,
        limit: usize,
        include_work_steps: bool,
        work_steps_offset: usize,
    ) -> StorageResult<HistoryPagePayload> {
        if self.queries.is_channel_context_thread(thread_id)? {
            return Err(StorageError::Forbidden(HISTORY_UNAVAILABLE_MSG.to_string()));
        }
        if let Some(cur) = &cursor
            && (cur.thread_id != thread_id || cur.query.as_deref() != query)
        {
            return Err(StorageError::Validation(HISTORY_UNAVAILABLE_MSG.to_string()));
        }
        let terms = query.map(parse_search_terms).unwrap_or_default();
        let scan_limit = if is_search { SEARCH_SCAN_ROWS } else { limit };
        let selection = AgentHistorySelection {
            before: cursor
                .as_ref()
                .map(|c| c.anchor_message_id.clone())
                .or_else(|| before_message_id.map(str::to_string)),
            message_id: exact_message_id.map(str::to_string),
            author: None,
            outgoing: true,
            across_resets: true,
            limit: if exact_message_id.is_some() {
                1
            } else {
                scan_limit + 1
            },
        };

        let page = match self.queries.read(agent_id, thread_id, &selection) {
            Ok(p) => p,
            Err(StorageError::NotFound(msg)) if msg == HISTORY_REFERENCE_UNAVAILABLE => {
                if exact_message_id.is_some() {
                    return Err(StorageError::Validation(
                        "This chat has no such message. Use a messageId from an earlier answer of this chat, or the Message ID of a teammate request. For another chat, send its taskId again.".to_string(),
                    ));
                }
                if before_message_id.is_some() {
                    return Err(StorageError::Validation(
                        "Use nextCursor or a messageId from an earlier answer of this chat. For another chat, send its taskId again.".to_string(),
                    ));
                }
                return Err(StorageError::NotFound(HISTORY_UNAVAILABLE_MSG.to_string()));
            }
            Err(other) => return Err(other),
        };

        if let Some(cur) = &cursor
            && cur.reset_message_id != page.reset_message_id
        {
            return Err(StorageError::Validation(HISTORY_UNAVAILABLE_MSG.to_string()));
        }

        let make_cursor = |anchor_id: &str| {
            encode_page_cursor(
                thread_id,
                page.reset_message_id.as_deref(),
                anchor_id,
                query,
            )
        };

        let mut result = HistoryPagePayload {
            messages: Vec::new(),
            next_cursor: None,
            order: "newest-first".to_string(),
            next: (!is_search).then(|| READ_CURSOR_HINT.to_string()),
        };
        let maximum = if is_search {
            SEARCH_ALL_CHARACTERS
        } else {
            READ_RESULT_CHARACTERS
        };
        let mut traces_cache: HashMap<String, Option<String>> = HashMap::new();
        let mut more = page.messages.len() > scan_limit;

        for row in page.messages.iter().take(scan_limit) {
            if result.messages.len() >= limit {
                more = true;
                break;
            }
            let message = &row.message;
            if let Some(bound) = after
                && message.created_at.as_str() < bound
            {
                more = false;
                break;
            }
            let visual_att_id = chat_visual_attachment_id(message);
            let visual_page_text = match (visual_att_id, exact_message_id, &self.page_text) {
                (Some(att_id), Some(_), Some(fetch)) => fetch(att_id),
                _ => None,
            };
            let raw_full = match visual_page_text {
                Some(pt) => format!("{}\n\n{pt}", message.text),
                None => message.text.clone(),
            };
            let text = self.redact(&raw_full);
            let Some(match_u16) = search_match(&text, &terms) else {
                result.next_cursor = Some(make_cursor(&message.id));
                continue;
            };
            let total_u16 = utf16_len(&text);
            let mut offset = if is_search { 0 } else { requested_offset };
            if offset > total_u16 {
                return Err(StorageError::Validation(HISTORY_UNAVAILABLE_MSG.to_string()));
            }
            if is_search {
                offset = match_u16.saturating_sub(100);
            }
            let window_len = if is_search { 400 } else { 12_000 };
            let (excerpt, excerpt_u16) = utf16_slice(&text, offset, window_len);
            let sent_to = message
                .exchange
                .as_ref()
                .filter(|ex| ex.direction == AgentExchangeDirection::Outgoing)
                .map(|ex| ex.recipient_agent_ids.clone());
            let author_str = if sent_to.is_some() {
                "assistant"
            } else {
                match message.author {
                    ConversationMessageAuthor::User => "user",
                    ConversationMessageAuthor::Assistant => "assistant",
                    ConversationMessageAuthor::Agent => "agent",
                    ConversationMessageAuthor::System => "system",
                }
            };
            let answered_by = message
                .ui_block
                .as_ref()
                .and_then(|b| b.state.responded_by.as_ref())
                .map(|s| HistoryAnsweredBy {
                    id: s.id.clone(),
                    name: self.redact(&s.name),
                });
            let attachments = if is_search {
                Vec::new()
            } else {
                message
                    .attachments
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|f| HistoryAttachmentInfo {
                        id: f.id.clone(),
                        name: self.redact(&f.name),
                        mime_type: self.redact(&f.mime_type),
                        size: f.size,
                    })
                    .collect()
            };
            let mut entry = HistoryEntry {
                task_id: None,
                message_id: message.id.clone(),
                author: author_str.to_string(),
                sent_to,
                created_at: message.created_at.clone(),
                answered_by,
                before_new_chat: row.before_reset.then_some(true),
                text: excerpt,
                offset,
                next_offset: (offset + excerpt_u16 < total_u16).then_some(offset + excerpt_u16),
                total_characters: total_u16,
                attachments,
                work_steps: None,
                work_steps_offset: None,
                next_work_steps_offset: None,
                attachments_omitted: None,
                page: (visual_att_id.is_some() && exact_message_id.is_none()).then(|| {
                    "history_read with this messageId, and the taskId of this entry if it has one, gives the page text.".to_string()
                }),
                quiet: (message.item_type.as_deref() == Some(ROUTINE_QUIET_ANSWER_ITEM_TYPE)).then(|| {
                    "Ended with nothing to report: no notification. The chat folds it under its run or update; the phone app does not show it.".to_string()
                }),
            };

            if include_work_steps {
                self.populate_work_steps(
                    thread_id,
                    row,
                    work_steps_offset,
                    &mut traces_cache,
                    &mut entry,
                )?;
            }

            result.messages.push(entry);
            result.next_cursor = Some(make_cursor(&message.id));
            if wrapped_result_chars(&result) > maximum {
                if result.messages.len() > 1 {
                    result.messages.pop();
                    let last_id = result
                        .messages
                        .last()
                        .map_or(message.id.as_str(), |e| e.message_id.as_str())
                        .to_string();
                    result.next_cursor = Some(make_cursor(&last_id));
                    more = true;
                    break;
                }
                Self::truncate_single_entry_to_budget(&mut result, offset, total_u16, maximum);
            }
        }

        if !more {
            result.next_cursor = None;
        }
        if result.next_cursor.is_none() {
            result.next = None;
        }
        self.verify_page_consistency(agent_id, thread_id, &page, &result, maximum)?;
        Ok(result)
    }
}

impl HistoryTools {
    fn populate_work_steps(
        &self,
        thread_id: &str,
        row: &AgentHistoryRow,
        work_steps_offset: usize,
        traces_cache: &mut HashMap<String, Option<String>>,
        entry: &mut HistoryEntry,
    ) -> StorageResult<()> {
        let mut raw_steps = match (
            row.provider_session_id.as_deref(),
            row.message.turn_id.as_deref(),
            &self.captured_steps,
        ) {
            (Some(sid), Some(tid), Some(fetch)) => fetch(sid, tid),
            _ => None,
        };
        if raw_steps.is_none()
            && let Some(tid) = row.message.turn_id.as_deref()
        {
            if !traces_cache.contains_key(tid) {
                let formatted = self
                    .queries
                    .turn_trace(thread_id, tid)?
                    .and_then(|tr| format_trace_work_steps(&tr));
                traces_cache.insert(tid.to_string(), formatted);
            }
            raw_steps = traces_cache.get(tid).cloned().flatten();
        }
        let step_text = self.redact(
            raw_steps
                .as_deref()
                .unwrap_or("Captured work steps are unavailable."),
        );
        let step_total = utf16_len(&step_text);
        if work_steps_offset > step_total {
            return Err(StorageError::Validation(HISTORY_UNAVAILABLE_MSG.to_string()));
        }
        let mut size = 4_000usize.min(step_total - work_steps_offset);
        loop {
            let (cand, _) = utf16_slice(&step_text, work_steps_offset, size);
            let inner_json = serde_json::to_string(&cand).unwrap_or_default();
            let double_escaped = serde_json::to_string(&inner_json).unwrap_or_default();
            if utf16_len(&double_escaped) <= 6_000 || size == 0 {
                break;
            }
            size /= 2;
        }
        let (sliced, taken_u16) = utf16_slice(&step_text, work_steps_offset, size);
        entry.work_steps = Some(sliced);
        entry.work_steps_offset = Some(work_steps_offset);
        entry.next_work_steps_offset =
            Some((work_steps_offset + taken_u16 < step_total).then_some(work_steps_offset + taken_u16));
        Ok(())
    }

    fn truncate_single_entry_to_budget(
        result: &mut HistoryPagePayload,
        offset: usize,
        total_u16: usize,
        maximum: usize,
    ) {
        if !result.messages.is_empty() {
            let att_len = result.messages[0].attachments.len();
            result.messages[0].attachments_omitted = Some(att_len);
            result.messages[0].attachments.clear();
            let original = result.messages[0].text.clone();
            let mut low = 0usize;
            let mut high = utf16_len(&original);
            while low < high {
                let mid = (low + high).div_ceil(2);
                let (cand, cand_len) = utf16_slice(&original, 0, mid);
                result.messages[0].text = cand;
                result.messages[0].next_offset = Some(offset + cand_len);
                if wrapped_result_chars(result) <= maximum {
                    low = mid;
                } else {
                    high = mid - 1;
                }
            }
            let (final_text, final_len) = utf16_slice(&original, 0, low);
            result.messages[0].text = final_text;
            result.messages[0].next_offset =
                (offset + final_len < total_u16).then_some(offset + final_len);
        }
    }

    fn verify_page_consistency(
        &self,
        agent_id: &str,
        thread_id: &str,
        page: &AgentHistoryPage,
        result: &HistoryPagePayload,
        maximum: usize,
    ) -> StorageResult<()> {
        for entry in &result.messages {
            let checked = self.queries.read(
                agent_id,
                thread_id,
                &AgentHistorySelection {
                    message_id: Some(entry.message_id.clone()),
                    outgoing: true,
                    across_resets: true,
                    limit: 1,
                    ..Default::default()
                },
            )?;
            let orig_msg = page
                .messages
                .iter()
                .find(|r| r.message.id == entry.message_id)
                .map(|r| &r.message);
            let checked_msg = checked.messages.first().map(|r| &r.message);
            let matches_orig = match (checked_msg, orig_msg) {
                (Some(c), Some(o)) => read_content_fingerprint(c) == read_content_fingerprint(o),
                _ => false,
            };
            if checked.reset_message_id != page.reset_message_id
                || checked.messages.len() != 1
                || !matches_orig
            {
                return Err(StorageError::Conflict(HISTORY_UNAVAILABLE_MSG.to_string()));
            }
        }
        if wrapped_result_chars(result) > maximum {
            return Err(StorageError::LimitExceeded(
                HISTORY_UNAVAILABLE_MSG.to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_bound_and_search_terms_match_upstream() {
        let now_ms = 1_764_115_200_000i64; // 2025-11-26T00:00:00.000Z
        assert_eq!(
            window_bound("24h", now_ms).as_deref(),
            Some("2025-11-25T00:00:00.000Z")
        );
        assert_eq!(
            window_bound("2025-11-01", now_ms).as_deref(),
            Some("2025-11-01T00:00:00.000Z")
        );
        assert!(window_bound("2025-02-30", now_ms).is_none());
        assert_eq!(
            parse_search_terms(r#"  "exact phrase" Beta   "second one" "#),
            vec![
                "exact phrase".to_string(),
                "beta".to_string(),
                "second one".to_string(),
            ]
        );
        assert_eq!(
            search_match("Alpha exact phrase and BETA here", &["exact phrase".into(), "beta".into()]),
            Some(6)
        );
    }
}
