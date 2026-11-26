//! `ThreadStore` — `projection_threads`, `projection_agent_threads` (`<= 50` open side-threads,
//! `<= 8` agent-started unfinished & auto-archive, branching), 0-based `ordinal`
//! `projection_thread_messages` + window-computed order keys, `projection_deleted_thread_messages`
//! soft-delete tombstones & event scrubbing, `projection_thread_activities`, `projection_turns`,
//! `projection_thread_summaries`, `ConversationReadStore`, `ThreadReplay`,
//! `projection_pinned_thread_messages` (`<= 50` / `<= 200`, `"pinned-by-person"` guard),
//! `projection_provider_sessions`, and atomic `<= 20` `agent_waits`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openbot_contracts::agents::{
    AgentProviderId, AgentSummary, AgentTaskOrigin, AgentTaskOutcome,
};
use openbot_contracts::conversation::{
    AGENT_EXCHANGE_ITEM_TYPE, AGENT_WELCOME_ITEM_TYPE, AgentExchangeDirection,
    CHANNEL_RESUME_ITEM_TYPE, CONVERSATION_MEMORY_CHANGES_ITEM_TYPE, CONVERSATION_PLAN_ITEM_TYPE,
    CONVERSATION_SUBAGENTS_ITEM_TYPE, CONTEXT_RESET_ITEM_TYPE, ConversationMessage,
    ConversationMessageAuthor, ConversationMessageSender, ConversationMessageStatus,
    ConversationReadState, ConversationSnapshot, HOSTED_SITE_EVENT_ITEM_PREFIX,
    INSTRUCTIONS_CHANGE_ITEM_TYPE, ImageGenerationInfo, MESSAGE_PIN_TEXT_LIMIT,
    MESSAGE_PIN_THREAD_TITLE_LIMIT, MESSAGE_PINS_LIMIT, MESSAGE_PINS_LIST_LIMIT,
    MODEL_SWITCH_ITEM_PREFIX, MessagePinner, NO_ANSWER_NOTICE_ITEM_TYPE,
    PLAN_CONTINUATION_ITEM_TYPE, PinnedMessage, PinnedMessageAuthor, PinnedMessagesSnapshot,
    ROUTINE_EVENT_ITEM_PREFIX, ROUTINE_QUIET_ANSWER_ITEM_TYPE, ROUTINE_RUN_EVENT_ITEM_PREFIX,
    SIGN_IN_FAILURE_ITEM_PREFIX, SKILL_SUGGESTION_ITEM_PREFIX,
};
use openbot_contracts::limits::INPUT_LIMITS;
use openbot_contracts::queue::AttachmentSummary;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, named_params, params, params_from_iter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent_store::{
    AgentRoster, UnclaimedThread, delete_aggregate_history, delete_receipts_of,
    delete_thread_attachments, ensure_thread_projection_row, now_iso, slice_utf16,
};
use crate::database::{OpenBotDatabase, OrchestrationEventInput};
use crate::error::{StorageError, StorageResult};

pub const AGENT_STARTED_UNFINISHED_TASKS: usize = 8;
pub const AGENT_STARTED_FINISHED_KEPT: usize = 8;
pub const PAGE_MESSAGE_LIMIT: usize = 100;
const QUOTE_MATCH_LIMIT: usize = 20;
const REMEMBERED_STREAMED_EVENTS: usize = 1024;
const CONVERSATION_UPDATED_EVENT: &str = "conversation.snapshot-updated";

pub const ORDERED_THREAD_MESSAGES_CTE: &str = "WITH ordered AS (
  SELECT created_at, ordinal, message_id, author, item_type, group_start, group_first, group_id, turn_rank
  FROM projection_thread_messages WHERE thread_id = ?
)";
pub(crate) const ORDER_KEY_COLUMNS: &str =
    "group_start, group_first, group_id, turn_rank, created_at, ordinal, message_id";
pub(crate) const ORDER_KEY_DESC: &str =
    "group_start DESC, group_first DESC, group_id DESC, turn_rank DESC, created_at DESC, ordinal DESC, message_id DESC";

pub const READ_EXCLUDED_ITEM_TYPES: [&str; 11] = [
    "commentary",
    CONVERSATION_PLAN_ITEM_TYPE,
    CONVERSATION_SUBAGENTS_ITEM_TYPE,
    CONVERSATION_MEMORY_CHANGES_ITEM_TYPE,
    PLAN_CONTINUATION_ITEM_TYPE,
    NO_ANSWER_NOTICE_ITEM_TYPE,
    INSTRUCTIONS_CHANGE_ITEM_TYPE,
    ROUTINE_QUIET_ANSWER_ITEM_TYPE,
    "agent_attachment",
    AGENT_EXCHANGE_ITEM_TYPE,
    CONTEXT_RESET_ITEM_TYPE,
];

pub const READ_EXCLUDED_ITEM_TYPE_PREFIXES: [&str; 9] = [
    "chat-visual:",
    "skill-event:",
    ROUTINE_EVENT_ITEM_PREFIX,
    ROUTINE_RUN_EVENT_ITEM_PREFIX,
    HOSTED_SITE_EVENT_ITEM_PREFIX,
    "marketplace-suggestion:",
    "setting-link:",
    MODEL_SWITCH_ITEM_PREFIX,
    SIGN_IN_FAILURE_ITEM_PREFIX,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentThreadTitleSource {
    Auto,
    Agent,
    User,
}

impl AgentThreadTitleSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Agent => "agent",
            Self::User => "user",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "auto" => Some(Self::Auto),
            "agent" => Some(Self::Agent),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListPosition {
    pub updated_at: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadBranch {
    pub source_thread_id: String,
    pub source_message_id: String,
    pub through_message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadRecord {
    pub task_id: String,
    pub agent_id: String,
    pub thread_id: String,
    pub origin: AgentTaskOrigin,
    pub title: String,
    pub title_source: AgentThreadTitleSource,
    pub outcome: Option<AgentTaskOutcome>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub branch: Option<AgentThreadBranch>,
    pub routine_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentThreadInput {
    pub agent_id: String,
    pub origin: AgentTaskOrigin,
    pub title: String,
    pub title_source: AgentThreadTitleSource,
    pub routine_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentThreadBranchInput {
    pub agent_id: String,
    pub request_id: String,
    pub thread_id: String,
    pub title: String,
    pub source_thread_id: String,
    pub source_message_id: String,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ConversationMarkerExclusions {
    pub exclude_routine_events: bool,
    pub exclude_routine_run_events: bool,
    pub exclude_hosted_site_events: bool,
    pub exclude_routine_quiet_answers: bool,
    pub exclude_setting_links: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ConversationPageAnchor {
    Latest,
    Before {
        cursor: String,
    },
    Around {
        #[serde(rename = "messageId")]
        message_id: String,
    },
}

impl Default for ConversationPageAnchor {
    fn default() -> Self {
        Self::Latest
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPageInfo {
    pub has_older: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub older_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPage {
    pub agent_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub thread_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub active_turn_id: Option<String>,
    pub revision: u64,
    pub messages: Vec<ConversationMessage>,
    pub references: BTreeMap<String, ConversationMessage>,
    pub page_info: ConversationPageInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSearchMatch {
    pub agent_id: String,
    pub message: ConversationMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSearchPage {
    pub results: Vec<ConversationSearchMatch>,
    pub total: u64,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationFileSearchResult {
    pub agent_id: String,
    pub message_id: String,
    pub created_at: String,
    pub attachment: AttachmentSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationFileSearchPage {
    pub results: Vec<ConversationFileSearchResult>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentMessageStats {
    pub first_sent_at: Option<String>,
    pub sent_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredThreadSummary {
    pub id: String,
    pub thread_id: String,
    pub through_message_id: Option<String>,
    pub text: String,
    pub estimated_tokens: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedThreadMessage {
    pub thread_id: String,
    pub message: ConversationMessage,
    pub pinned_at: String,
    pub pinned_by: MessagePinner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PinRefusalReason {
    NotFound,
    Limit,
    PinnedByPerson,
    NotPinnableChat,
}

impl PinRefusalReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not-found",
            Self::Limit => "limit",
            Self::PinnedByPerson => "pinned-by-person",
            Self::NotPinnableChat => "not-pinnable-chat",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderSessionState {
    Active,
    Inactive,
    Failed,
}

impl ProviderSessionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "active" => Some(Self::Active),
            "inactive" => Some(Self::Inactive),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSession {
    pub id: String,
    pub thread_id: String,
    pub provider: AgentProviderId,
    pub external_session_id: String,
    pub model: String,
    pub effort: String,
    pub state: ProviderSessionState,
    pub created_at: String,
    pub updated_at: String,
    pub resume_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BindProviderSessionInput {
    pub thread_id: String,
    pub provider: AgentProviderId,
    pub external_session_id: String,
    pub model: String,
    pub effort: String,
    pub resume_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWait {
    pub id: String,
    pub agent_id: String,
    pub note: String,
    pub request_message_id: String,
    pub request_text: String,
    pub routine_id: Option<String>,
    pub created_at: String,
    pub woken_at: Option<String>,
    pub wake_delivery_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenAgentWaitInput {
    pub agent_id: String,
    pub request_message_id: String,
    pub request_text: String,
    pub note: String,
    pub routine_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StreamingMessageWrite {
    pub snapshot: ConversationSnapshot,
    pub message_id: String,
    pub event_type: String,
    pub detail: serde_json::Value,
    pub command_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppendConversationMessageInput {
    pub agent_id: String,
    pub thread_id: String,
    pub active_turn_id: Option<String>,
    pub message: ConversationMessage,
    pub event_type: String,
    pub detail: serde_json::Value,
    pub command_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PersistConversationChangesInput {
    pub agent_id: String,
    pub thread_id: String,
    pub active_turn_id: Option<String>,
    pub changed_messages: Vec<ConversationMessage>,
    pub removed_message_ids: Vec<String>,
    pub deleted_message_ids: Vec<String>,
    pub event_type: String,
    pub detail: serde_json::Value,
    pub command_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpsertProviderHistoryMessageInput {
    pub agent_id: String,
    pub thread_id: String,
    pub active_turn_id: Option<Option<String>>,
    pub message: ConversationMessage,
    pub event_type: Option<String>,
    pub detail: serde_json::Value,
    pub command_id: Option<String>,
}

const ROLE_RANK_SQL: &str = "CASE
      WHEN author = 'user' THEN 0
      WHEN author = 'assistant' THEN CASE WHEN item_type IN ('commentary', 'plan') THEN 1 ELSE 3 END
      WHEN json_extract(message_json, '$.exchange.direction') = 'incoming' THEN 0
      ELSE 2
    END";
const STEERED_SQL: &str = "json_extract(message_json, '$.steered') IS 1";

fn refresh_order_sql(filter: &str) -> String {
    format!(
        "WITH ranked AS (
  SELECT message_id, created_at, ordinal, NULLIF(turn_id, '') AS turn, {ROLE_RANK_SQL} AS role_rank,
    {STEERED_SQL} AS steered, item_type = 'question_prompt' AS question
  FROM projection_thread_messages WHERE thread_id = :thread {filter}
), steers AS (
  SELECT *, role_rank = 0 AND created_at > CASE WHEN steered THEN MIN(created_at) OVER (PARTITION BY turn)
    ELSE MIN(CASE WHEN question THEN created_at END) OVER (PARTITION BY turn) END AS late_steer
  FROM ranked
), keyed AS (
  SELECT message_id,
    CASE WHEN turn IS NULL THEN created_at ELSE MIN(created_at) OVER turn_by_time END AS group_start,
    CASE WHEN turn IS NULL THEN ordinal ELSE FIRST_VALUE(ordinal) OVER turn_by_time END AS group_first,
    CASE WHEN turn IS NULL THEN 'message:' || message_id ELSE 'turn:' || turn END AS group_id,
    CASE WHEN turn IS NULL THEN 0
      WHEN SUM(role_rank = 0) OVER turn_by_time >= 2 OR SUM(late_steer) OVER turn_by_time >= 1 THEN 4
      ELSE role_rank END AS turn_rank
  FROM steers
  WINDOW turn_by_time AS (PARTITION BY turn ORDER BY created_at, ordinal)
)
UPDATE projection_thread_messages AS message
SET group_start = keyed.group_start, group_first = keyed.group_first,
  group_id = keyed.group_id, turn_rank = keyed.turn_rank
FROM keyed
WHERE message.thread_id = :thread AND message.message_id = keyed.message_id
  AND (message.group_start, message.group_first, message.group_id, message.turn_rank)
    IS NOT (keyed.group_start, keyed.group_first, keyed.group_id, keyed.turn_rank)"
    )
}

/// Recomputes `group_start`, `group_first`, `group_id`, and `turn_rank` (`0..=4`) (`thread-message-order.ts:L43-L45`).
pub fn refresh_thread_order(conn: &Connection, thread_id: &str) -> StorageResult<()> {
    let sql = refresh_order_sql("");
    conn.execute(&sql, named_params! { ":thread": thread_id })?;
    Ok(())
}

#[derive(Clone, Debug)]
struct MessageOrderInputs {
    turn: Option<String>,
    inputs: String,
    keyed: bool,
}

fn read_order_inputs(
    conn: &Connection,
    thread_id: &str,
    message_ids: &[String],
) -> StorageResult<HashMap<String, MessageOrderInputs>> {
    let sql = format!(
        "SELECT NULLIF(turn_id, '') AS turn,
           created_at || char(0) || ordinal || char(0) || ({ROLE_RANK_SQL}) || char(0) || ({STEERED_SQL}) AS inputs,
           group_id IS NOT NULL AS keyed
         FROM projection_thread_messages WHERE thread_id = ?1 AND message_id = ?2"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut result = HashMap::new();
    let seen: HashSet<&str> = message_ids.iter().map(String::as_str).collect();
    for mid in seen {
        let row = stmt
            .query_row(params![thread_id, mid], |r| {
                Ok(MessageOrderInputs {
                    turn: r.get(0)?,
                    inputs: r.get(1)?,
                    keyed: r.get::<_, i64>(2)? == 1,
                })
            })
            .optional()?;
        if let Some(inputs) = row {
            result.insert(mid.to_string(), inputs);
        }
    }
    Ok(result)
}

fn refresh_changed_order(
    conn: &Connection,
    thread_id: &str,
    before: &HashMap<String, MessageOrderInputs>,
    message_ids: &[String],
) -> StorageResult<()> {
    let after = read_order_inputs(conn, thread_id, message_ids)?;
    let mut turns = HashSet::new();
    let mut singles = HashSet::new();
    let seen: HashSet<&str> = message_ids.iter().map(String::as_str).collect();
    for mid in seen {
        let prev = before.get(mid);
        let curr = after.get(mid);
        if let (Some(p), Some(c)) = (prev, curr)
            && c.keyed
            && p.inputs == c.inputs
            && p.turn == c.turn
        {
            continue;
        }
        if let Some(t) = prev.and_then(|p| p.turn.as_ref()) {
            turns.insert(t.clone());
        }
        if let Some(c) = curr {
            if let Some(t) = &c.turn {
                turns.insert(t.clone());
            } else {
                singles.insert(mid.to_string());
            }
        }
    }
    if !turns.is_empty() {
        let sql = refresh_order_sql("AND turn_id = :key");
        let mut stmt = conn.prepare_cached(&sql)?;
        for turn in turns {
            stmt.execute(named_params! { ":thread": thread_id, ":key": turn })?;
        }
    }
    if !singles.is_empty() {
        let sql = refresh_order_sql("AND message_id = :key");
        let mut stmt = conn.prepare_cached(&sql)?;
        for mid in singles {
            stmt.execute(named_params! { ":thread": thread_id, ":key": mid })?;
        }
    }
    Ok(())
}

pub fn item_type_counts_as_unread(item_type: Option<&str>) -> bool {
    let Some(it) = item_type else {
        return true;
    };
    if READ_EXCLUDED_ITEM_TYPES.contains(&it) {
        return false;
    }
    !READ_EXCLUDED_ITEM_TYPE_PREFIXES
        .iter()
        .any(|prefix| it.starts_with(prefix))
}

pub fn can_pin_message(message: &ConversationMessage) -> bool {
    message.author != ConversationMessageAuthor::System
        && message.status != ConversationMessageStatus::Streaming
}

/// Remaps IDs & `replyToMessageId` and strips `delivery`, `reaction`, `reactions` (`agent-thread-store.ts:L501-L518`).
pub fn copy_branch_messages(
    messages: &[ConversationMessage],
    files: &HashMap<String, AttachmentSummary>,
) -> Vec<ConversationMessage> {
    let id_map: HashMap<&str, String> = messages
        .iter()
        .map(|m| (m.id.as_str(), Uuid::new_v4().to_string()))
        .collect();
    let map_file = |att: &AttachmentSummary| files.get(&att.id).cloned().unwrap_or_else(|| att.clone());
    messages
        .iter()
        .map(|message| {
            let mut copy = message.clone();
            copy.id = id_map
                .get(message.id.as_str())
                .cloned()
                .unwrap_or_else(|| message.id.clone());
            copy.delivery = None;
            copy.reaction = None;
            copy.reactions = None;
            copy.reply_to_message_id = message
                .reply_to_message_id
                .as_ref()
                .and_then(|opt| opt.as_deref())
                .and_then(|orig| id_map.get(orig).cloned())
                .map(Some);
            if let Some(atts) = &message.attachments {
                copy.attachments = Some(atts.iter().map(map_file).collect());
            }
            if let Some(vr) = &message.voice_recording {
                copy.voice_recording = Some(map_file(vr));
            }
            copy
        })
        .collect()
}

/// Exact Rust port of `packages/contracts/src/conversation-order.ts:sortConversationMessages`.
pub fn sort_conversation_messages(messages: &mut [ConversationMessage]) {
    if messages.len() <= 1 {
        return;
    }
    let mut groups: HashMap<String, (i64, usize)> = HashMap::new();
    let mut group_keys: Vec<String> = Vec::with_capacity(messages.len());
    for (idx, msg) in messages.iter().enumerate() {
        let key = match msg.turn_id.as_deref().filter(|t| !t.is_empty()) {
            Some(tid) => format!("turn:{tid}"),
            None => format!("message:{idx}"),
        };
        let ts = parse_message_time(&msg.created_at);
        groups
            .entry(key.clone())
            .and_modify(|(started_at, first_idx)| {
                *started_at = (*started_at).min(ts);
                *first_idx = (*first_idx).min(idx);
            })
            .or_insert((ts, idx));
        group_keys.push(key);
    }

    let steered_at = compute_turn_steer_times(messages);
    let mut indexed: Vec<(usize, ConversationMessage)> = messages
        .iter()
        .cloned()
        .enumerate()
        .collect();

    indexed.sort_by(|(li, left), (ri, right)| {
        let lg_key = &group_keys[*li];
        let rg_key = &group_keys[*ri];
        if lg_key != rg_key
            && let (Some(lg), Some(rg)) = (groups.get(lg_key), groups.get(rg_key))
        {
            if lg.0 != rg.0 {
                return lg.0.cmp(&rg.0);
            }
            if lg.1 != rg.1 {
                return lg.1.cmp(&rg.1);
            }
        }
        if let (Some(lt), Some(rt)) = (
            left.turn_id.as_deref().filter(|s| !s.is_empty()),
            right.turn_id.as_deref().filter(|s| !s.is_empty()),
        ) && lt == rt
        {
            let steer_time = steered_at.get(lt).copied();
            let l_steered = steer_time.is_some_and(|st| parse_message_time(&left.created_at) >= st);
            let r_steered =
                steer_time.is_some_and(|st| parse_message_time(&right.created_at) >= st);
            if l_steered != r_steered {
                return l_steered.cmp(&r_steered);
            }
            if !l_steered {
                let rank_cmp = turn_message_rank(left).cmp(&turn_message_rank(right));
                if rank_cmp != std::cmp::Ordering::Equal {
                    return rank_cmp;
                }
            }
        }
        let time_cmp = parse_message_time(&left.created_at).cmp(&parse_message_time(&right.created_at));
        if time_cmp != std::cmp::Ordering::Equal {
            return time_cmp;
        }
        li.cmp(ri)
    });

    for (slot, (_, msg)) in messages.iter_mut().zip(indexed) {
        *slot = msg;
    }
}

fn compute_turn_steer_times(messages: &[ConversationMessage]) -> HashMap<String, i64> {
    let mut user_times: HashMap<String, Vec<i64>> = HashMap::new();
    let mut first_times: HashMap<String, i64> = HashMap::new();
    let mut question_times: HashMap<String, i64> = HashMap::new();
    for msg in messages {
        let Some(tid) = msg.turn_id.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let t = parse_message_time(&msg.created_at);
        first_times
            .entry(tid.to_string())
            .and_modify(|v| *v = (*v).min(t))
            .or_insert(t);
        if msg.question_prompt.is_some() {
            question_times
                .entry(tid.to_string())
                .and_modify(|v| *v = (*v).min(t))
                .or_insert(t);
        }
        if turn_message_rank(msg) == 0 {
            user_times.entry(tid.to_string()).or_default().push(t);
        }
    }
    let mut steered_at = HashMap::new();
    for (tid, mut times) in user_times {
        times.sort_unstable();
        if let Some(&second) = times.get(1) {
            steered_at.insert(tid, second);
        }
    }
    for msg in messages {
        let Some(tid) = msg.turn_id.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        if turn_message_rank(msg) != 0 {
            continue;
        }
        let t = parse_message_time(&msg.created_at);
        let starts_after = if msg.steered == Some(true) {
            first_times.get(tid).copied()
        } else {
            question_times.get(tid).copied()
        };
        if let Some(sa) = starts_after
            && t > sa
        {
            steered_at
                .entry(tid.to_string())
                .and_modify(|cur| *cur = (*cur).min(t))
                .or_insert(t);
        }
    }
    steered_at
}

fn parse_message_time(created_at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(created_at)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(i64::MAX)
}

fn turn_message_rank(message: &ConversationMessage) -> u8 {
    if message
        .exchange
        .as_ref()
        .is_some_and(|ex| ex.direction == AgentExchangeDirection::Incoming)
        || message.author == ConversationMessageAuthor::User
    {
        return 0;
    }
    if message.author == ConversationMessageAuthor::Assistant
        && matches!(message.item_type.as_deref(), Some("commentary" | "plan"))
    {
        return 1;
    }
    if message
        .exchange
        .as_ref()
        .is_some_and(|ex| ex.direction == AgentExchangeDirection::Outgoing)
    {
        return 2;
    }
    if message.author == ConversationMessageAuthor::Assistant {
        return 3;
    }
    2
}

pub fn deleted_thread_message_ids(
    conn: &Connection,
    thread_id: &str,
) -> StorageResult<HashSet<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT message_id FROM projection_deleted_thread_messages WHERE thread_id = ?1",
    )?;
    let rows = stmt.query_map(params![thread_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<HashSet<_>, _>>()?)
}

pub fn thread_message_ids(conn: &Connection, thread_id: &str) -> StorageResult<Vec<String>> {
    let mut stmt =
        conn.prepare_cached("SELECT message_id FROM projection_thread_messages WHERE thread_id = ?1")?;
    let rows = stmt.query_map(params![thread_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn is_deleted_thread_message(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> StorageResult<bool> {
    let found: Option<i64> = conn
        .prepare_cached(
            "SELECT 1 FROM projection_deleted_thread_messages WHERE thread_id = ?1 AND message_id = ?2",
        )?
        .query_row(params![thread_id, message_id], |r| r.get(0))
        .optional()?;
    Ok(found.is_some())
}

pub fn without_deleted_messages(
    conn: &Connection,
    thread_id: &str,
    messages: &[ConversationMessage],
) -> StorageResult<Vec<ConversationMessage>> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    let deleted = deleted_thread_message_ids(conn, thread_id)?;
    if deleted.is_empty() {
        return Ok(messages.to_vec());
    }
    Ok(messages
        .iter()
        .filter(|m| !deleted.contains(&m.id))
        .cloned()
        .collect())
}

/// Soft-deletes a single thread message: records tombstone, unpins, rewinds read cursors,
/// and scrubs `orchestration_events` (`deleted-thread-messages.ts:L43-L61`).
pub fn record_deleted_thread_message(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
    deleted_at: &str,
) -> StorageResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO projection_deleted_thread_messages (thread_id, message_id, deleted_at)
         VALUES (?1, ?2, ?3)",
        params![thread_id, message_id, deleted_at],
    )?;
    unpin_thread_message_conn(conn, thread_id, message_id)?;
    let previous = previous_shown_message_id(conn, thread_id, message_id)?;
    conn.execute(
        "UPDATE projection_thread_reads SET through_message_id = ?1
         WHERE thread_id = ?2 AND through_message_id = ?3",
        params![previous, thread_id, message_id],
    )?;
    conn.execute(
        "UPDATE projection_thread_read_baselines SET through_message_id = ?1
         WHERE thread_id = ?2 AND through_message_id = ?3",
        params![previous, thread_id, message_id],
    )?;
    scrub_deleted_message_events(conn, thread_id, message_id)
}

fn previous_shown_message_id(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> StorageResult<Option<String>> {
    let sql = format!(
        "{ORDERED_THREAD_MESSAGES_CTE}
         SELECT message_id FROM ordered
         WHERE ({ORDER_KEY_COLUMNS}) < (SELECT {ORDER_KEY_COLUMNS} FROM ordered WHERE message_id = ?)
         ORDER BY {ORDER_KEY_DESC} LIMIT 1"
    );
    Ok(conn
        .prepare_cached(&sql)?
        .query_row(params![thread_id, message_id], |r| r.get(0))
        .optional()?)
}

fn scrub_deleted_message_events(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> StorageResult<()> {
    let json_quoted = serde_json::to_string(message_id)?;
    let needle = json_quoted
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(message_id);
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT sequence, payload_json FROM orchestration_events
             WHERE aggregate_type = 'thread' AND aggregate_id = ?1 AND instr(payload_json, ?2) > 0",
        )?;
        let iter = stmt.query_map(params![thread_id, needle], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        iter.collect::<Result<Vec<_>, _>>()?
    };
    let mut update =
        conn.prepare_cached("UPDATE orchestration_events SET payload_json = ?1 WHERE sequence = ?2")?;
    for (seq, raw) in rows {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw)
            && let Some(scrubbed) = scrub_payload(&val, |v| {
                v.get("id").and_then(|id| id.as_str()) == Some(message_id)
            })
        {
            update.execute(params![serde_json::to_string(&scrubbed)?, seq])?;
        }
    }
    Ok(())
}

/// Tombstones all messages in a thread, unpins all, nulls read cursors/baselines, and scrubs
/// `orchestration_events` + `projection_thread_activities` (`deleted-thread-messages.ts:L96-L106`).
pub fn record_deleted_thread_history(
    conn: &Connection,
    thread_id: &str,
    deleted_at: &str,
) -> StorageResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO projection_deleted_thread_messages (thread_id, message_id, deleted_at)
         SELECT thread_id, message_id, ?1 FROM projection_thread_messages WHERE thread_id = ?2",
        params![deleted_at, thread_id],
    )?;
    unpin_thread_history_conn(conn, thread_id)?;
    conn.execute(
        "UPDATE projection_thread_reads SET through_message_id = NULL WHERE thread_id = ?1",
        params![thread_id],
    )?;
    conn.execute(
        "UPDATE projection_thread_read_baselines SET through_message_id = NULL WHERE thread_id = ?1",
        params![thread_id],
    )?;
    scrub_all_messages_in_table(
        conn,
        "orchestration_events",
        "aggregate_type = 'thread' AND aggregate_id = ?1",
        thread_id,
    )?;
    scrub_all_messages_in_table(
        conn,
        "projection_thread_activities",
        "thread_id = ?1",
        thread_id,
    )
}

fn scrub_all_messages_in_table(
    conn: &Connection,
    table: &str,
    where_clause: &str,
    thread_id: &str,
) -> StorageResult<()> {
    let row_ids: Vec<i64> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT rowid FROM {table}
             WHERE {where_clause}
               AND (instr(payload_json, 'Message') > 0 OR instr(payload_json, '\"snapshot\"') > 0)"
        ))?;
        let iter = stmt.query_map(params![thread_id], |r| r.get(0))?;
        iter.collect::<Result<Vec<_>, _>>()?
    };
    if row_ids.is_empty() {
        return Ok(());
    }
    let mut read_stmt = conn.prepare(&format!("SELECT payload_json FROM {table} WHERE rowid = ?1"))?;
    let mut update_stmt =
        conn.prepare(&format!("UPDATE {table} SET payload_json = ?1 WHERE rowid = ?2"))?;
    for row_id in row_ids {
        let raw_opt: Option<String> = read_stmt
            .query_row(params![row_id], |r| r.get(0))
            .optional()?;
        if let Some(raw) = raw_opt
            && let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw)
            && let Some(scrubbed) =
                scrub_payload(&val, |v| v.get("id").and_then(|id| id.as_str()).is_some())
        {
            update_stmt.execute(params![serde_json::to_string(&scrubbed)?, row_id])?;
        }
    }
    Ok(())
}

fn scrub_payload<F>(payload: &serde_json::Value, is_deleted: F) -> Option<serde_json::Value>
where
    F: Fn(&serde_json::Value) -> bool,
{
    let obj = payload.as_object()?;
    let mut next = obj.clone();
    let mut changed = false;

    if let Some(serde_json::Value::Object(snap_obj)) = obj.get("snapshot")
        && let Some(serde_json::Value::Array(msgs)) = snap_obj.get("messages")
        && msgs.iter().any(&is_deleted)
    {
        let mut next_snap = snap_obj.clone();
        let kept: Vec<serde_json::Value> = msgs
            .iter()
            .filter(|m| !is_deleted(m))
            .cloned()
            .collect();
        next_snap.insert("messages".to_string(), serde_json::Value::Array(kept));
        next.insert("snapshot".to_string(), serde_json::Value::Object(next_snap));
        changed = true;
    }

    if let Some(serde_json::Value::Array(changed_msgs)) = obj.get("changedMessages")
        && changed_msgs.iter().any(&is_deleted)
    {
        let kept: Vec<serde_json::Value> = changed_msgs
            .iter()
            .filter(|m| !is_deleted(m))
            .cloned()
            .collect();
        next.insert(
            "changedMessages".to_string(),
            serde_json::Value::Array(kept),
        );
        changed = true;
    }

    for key in ["appendedMessage", "streamedMessage", "importedMessage"] {
        if let Some(entry) = obj.get(key)
            && is_deleted(entry)
        {
            next.remove(key);
            if !next.get("changedMessages").is_some_and(|v| v.is_array()) {
                next.insert(
                    "changedMessages".to_string(),
                    serde_json::Value::Array(Vec::new()),
                );
            }
            changed = true;
        }
    }

    changed.then_some(serde_json::Value::Object(next))
}

pub fn pinned_thread_messages_conn(
    conn: &Connection,
    thread_ids: &[String],
    limit: usize,
) -> StorageResult<Vec<PinnedThreadMessage>> {
    if thread_ids.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let ids_json = serde_json::to_string(thread_ids)?;
    let mut stmt = conn.prepare_cached(
        "SELECT pins.thread_id, pins.pinned_at, pins.pinned_by_kind, pins.pinned_by_id,
                pins.pinned_by_name, messages.message_json
         FROM projection_pinned_thread_messages AS pins
         JOIN projection_thread_messages AS messages
           ON messages.thread_id = pins.thread_id AND messages.message_id = pins.message_id
         WHERE pins.thread_id IN (SELECT value FROM json_each(?1))
         ORDER BY pins.pinned_at DESC, pins.rowid DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![ids_json, limit as i64], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, String>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (thread_id, pinned_at, kind, by_id, by_name, msg_json) = row?;
        let message: ConversationMessage = serde_json::from_str(&msg_json)?;
        out.push(PinnedThreadMessage {
            thread_id,
            message,
            pinned_at,
            pinned_by: decode_pinner_row(&kind, by_id, by_name),
        });
    }
    Ok(out)
}

pub fn pinned_thread_message_count_conn(conn: &Connection, thread_id: &str) -> StorageResult<usize> {
    let count: i64 = conn
        .prepare_cached(
            "SELECT COUNT(*) FROM projection_pinned_thread_messages AS pins
             JOIN projection_thread_messages AS messages
               ON messages.thread_id = pins.thread_id AND messages.message_id = pins.message_id
             WHERE pins.thread_id = ?1",
        )?
        .query_row(params![thread_id], |r| r.get(0))?;
    Ok(count.max(0) as usize)
}

pub fn thread_message_pinner_conn(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> StorageResult<Option<MessagePinner>> {
    let row_opt = conn
        .prepare_cached(
            "SELECT pinned_by_kind, pinned_by_id, pinned_by_name
             FROM projection_pinned_thread_messages
             WHERE thread_id = ?1 AND message_id = ?2",
        )?
        .query_row(params![thread_id, message_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .optional()?;
    Ok(row_opt.map(|(k, id, name)| decode_pinner_row(&k, id, name)))
}

pub fn pin_thread_message_conn(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
    pinned_at: &str,
    pinned_by: &MessagePinner,
) -> StorageResult<()> {
    let (kind, by_id, by_name) = match pinned_by {
        MessagePinner::Agent { agent_id } => ("agent", Some(agent_id.as_str()), None),
        MessagePinner::Person { sender } => (
            "person",
            sender.as_ref().map(|s| s.id.as_str()),
            sender.as_ref().map(|s| s.name.as_str()),
        ),
    };
    conn.execute(
        "INSERT OR IGNORE INTO projection_pinned_thread_messages
           (thread_id, message_id, pinned_at, pinned_by_kind, pinned_by_id, pinned_by_name)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![thread_id, message_id, pinned_at, kind, by_id, by_name],
    )?;
    Ok(())
}

pub fn unpin_thread_message_conn(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> StorageResult<()> {
    conn.execute(
        "DELETE FROM projection_pinned_thread_messages WHERE thread_id = ?1 AND message_id = ?2",
        params![thread_id, message_id],
    )?;
    Ok(())
}

pub fn unpin_thread_history_conn(conn: &Connection, thread_id: &str) -> StorageResult<()> {
    conn.execute(
        "DELETE FROM projection_pinned_thread_messages WHERE thread_id = ?1",
        params![thread_id],
    )?;
    Ok(())
}

pub fn thread_messages_containing_conn(
    conn: &Connection,
    thread_id: &str,
    quote: &str,
    limit: usize,
) -> StorageResult<Vec<ConversationMessage>> {
    let mut stmt = conn.prepare_cached(
        "SELECT message_json FROM projection_thread_messages
         WHERE thread_id = ?1 AND instr(lower(json_extract(message_json, '$.text')), lower(?2)) > 0
         ORDER BY ordinal DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![thread_id, quote, limit as i64], |r| {
        r.get::<_, String>(0)
    })?;
    let mut out = Vec::new();
    for raw in rows {
        out.push(serde_json::from_str(&raw?)?);
    }
    Ok(out)
}

fn decode_pinner_row(
    kind: &str,
    by_id: Option<String>,
    by_name: Option<String>,
) -> MessagePinner {
    if kind == "agent"
        && let Some(agent_id) = by_id
    {
        return MessagePinner::Agent { agent_id };
    }
    let sender = match (by_id, by_name) {
        (Some(id), Some(name)) if !id.is_empty() => Some(ConversationMessageSender { id, name }),
        _ => None,
    };
    MessagePinner::Person { sender }
}

pub fn format_pinned_message(
    pin: &PinnedThreadMessage,
    side_thread: Option<(&str, &str)>,
) -> PinnedMessage {
    let message = &pin.message;
    let from_person =
        message.author == ConversationMessageAuthor::User && message.sender_agent_id.is_none();
    let trimmed = message.text.trim();
    let raw_text = if !trimmed.is_empty() {
        trimmed.to_string()
    } else {
        message
            .attachments
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let pinned_by = match &pin.pinned_by {
        MessagePinner::Person {
            sender: Some(sender),
        } => MessagePinner::Person {
            sender: Some(ConversationMessageSender {
                id: sender.id.clone(),
                name: slice_utf16(&sender.name, 120),
            }),
        },
        other => other.clone(),
    };
    PinnedMessage {
        message_id: message.id.clone(),
        task_id: side_thread.map(|(tid, _)| tid.to_string()),
        thread_title: side_thread.map(|(_, title)| slice_utf16(title, MESSAGE_PIN_THREAD_TITLE_LIMIT)),
        pinned_at: pin.pinned_at.clone(),
        pinned_by,
        author: if message.author == ConversationMessageAuthor::User {
            PinnedMessageAuthor::User
        } else {
            PinnedMessageAuthor::Agent
        },
        sender: if from_person {
            message.sender_member.as_ref().map(|s| ConversationMessageSender {
                id: s.id.clone(),
                name: slice_utf16(&s.name, 120),
            })
        } else {
            None
        },
        sender_agent_id: message.sender_agent_id.clone(),
        created_at: message.created_at.clone(),
        text: slice_utf16(&raw_text, MESSAGE_PIN_TEXT_LIMIT),
    }
}

/// `MessagePinStore` enforcing `<= 50` pins per thread, `<= 200` cross-thread list,
/// `can_pin_message`, and `"pinned-by-person"` refusal when an agent tries to unpin a human pin.
#[derive(Clone)]
pub struct MessagePinStore {
    database: OpenBotDatabase,
}

impl MessagePinStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn list(&self, agent_id: &str) -> StorageResult<PinnedMessagesSnapshot> {
        let roster = AgentRoster::new(self.database.clone());
        let Some(agent) = roster.get_agent(agent_id)? else {
            return Ok(PinnedMessagesSnapshot {
                agent_id: agent_id.to_string(),
                pins: Vec::new(),
            });
        };
        let side_threads = AgentThreadStore::new(self.database.clone());
        let mut thread_ids = Vec::new();
        if let Some(main_tid) = &agent.thread_id {
            thread_ids.push(main_tid.clone());
        }
        let side_records = side_threads.list_all_for_agent(agent_id)?;
        for rec in &side_records {
            thread_ids.push(rec.thread_id.clone());
        }
        let side_by_thread: HashMap<&str, (&str, &str)> = side_records
            .iter()
            .map(|r| (r.thread_id.as_str(), (r.task_id.as_str(), r.title.as_str())))
            .collect();

        let raw_pins = self
            .database
            .with_conn(|conn| pinned_thread_messages_conn(conn, &thread_ids, MESSAGE_PINS_LIST_LIMIT))?;
        let pins = raw_pins
            .into_iter()
            .filter(|p| can_pin_message(&p.message))
            .map(|p| {
                let side = if agent.thread_id.as_deref() == Some(p.thread_id.as_str()) {
                    None
                } else {
                    side_by_thread.get(p.thread_id.as_str()).copied()
                };
                format_pinned_message(&p, side)
            })
            .collect();
        Ok(PinnedMessagesSnapshot {
            agent_id: agent_id.to_string(),
            pins,
        })
    }

    pub fn set_by_person(
        &self,
        agent_id: &str,
        task_id: Option<&str>,
        message_id: &str,
        pinned: bool,
        sender: Option<ConversationMessageSender>,
    ) -> StorageResult<Result<PinnedMessagesSnapshot, PinRefusalReason>> {
        let Some(thread_id) = self.resolve_pinnable_thread(agent_id, task_id)? else {
            return Ok(Err(PinRefusalReason::NotFound));
        };
        let pinner = MessagePinner::Person { sender };
        match self.write_pin(agent_id, &thread_id, message_id, pinned, &pinner)? {
            Some(PinRefusalReason::PinnedByPerson | PinRefusalReason::NotPinnableChat) => {
                Ok(Err(PinRefusalReason::NotFound))
            }
            Some(reason) => Ok(Err(reason)),
            None => Ok(Ok(self.list(agent_id)?)),
        }
    }

    pub fn set_by_agent(
        &self,
        agent_id: &str,
        thread_id: &str,
        message_id: &str,
        pinned: bool,
    ) -> StorageResult<Result<PinnedMessagesSnapshot, PinRefusalReason>> {
        if !self.is_pinnable_thread(agent_id, thread_id)? {
            return Ok(Err(PinRefusalReason::NotPinnableChat));
        }
        let pinner = MessagePinner::Agent {
            agent_id: agent_id.to_string(),
        };
        match self.write_pin(agent_id, thread_id, message_id, pinned, &pinner)? {
            Some(reason) => Ok(Err(reason)),
            None => Ok(Ok(self.list(agent_id)?)),
        }
    }

    pub fn find_by_quote(
        &self,
        agent_id: &str,
        thread_id: &str,
        quote: &str,
        exclude: &HashSet<String>,
        for_pinning: bool,
    ) -> StorageResult<Option<ConversationMessage>> {
        if !self.is_pinnable_thread(agent_id, thread_id)? {
            return Ok(None);
        }
        self.database.with_conn(|conn| {
            let candidates = thread_messages_containing_conn(conn, thread_id, quote, QUOTE_MATCH_LIMIT)?;
            for msg in candidates {
                if !can_pin_message(&msg) || exclude.contains(&msg.id) {
                    continue;
                }
                if for_pinning || thread_message_pinner_conn(conn, thread_id, &msg.id)?.is_some() {
                    return Ok(Some(msg));
                }
            }
            Ok(None)
        })
    }

    fn write_pin(
        &self,
        agent_id: &str,
        thread_id: &str,
        message_id: &str,
        pinned: bool,
        pinner: &MessagePinner,
    ) -> StorageResult<Option<PinRefusalReason>> {
        self.database.with_tx(|conn| {
            let current = thread_message_pinner_conn(conn, thread_id, message_id)?;
            if !pinned {
                let Some(curr) = current else {
                    return Ok(None);
                };
                if matches!(pinner, MessagePinner::Agent { .. })
                    && matches!(curr, MessagePinner::Person { .. })
                {
                    return Ok(Some(PinRefusalReason::PinnedByPerson));
                }
                unpin_thread_message_conn(conn, thread_id, message_id)?;
                return Ok(None);
            }
            let msg_opt: Option<String> = conn
                .prepare_cached(
                    "SELECT message_json FROM projection_thread_messages
                     WHERE thread_id = ?1 AND message_id = ?2
                       AND EXISTS (
                         SELECT 1 FROM projection_threads AS owner
                         WHERE owner.thread_id = projection_thread_messages.thread_id
                           AND owner.agent_id = ?3
                       )",
                )?
                .query_row(params![thread_id, message_id, agent_id], |r| r.get(0))
                .optional()?;
            let Some(msg_json) = msg_opt else {
                return Ok(Some(PinRefusalReason::NotFound));
            };
            let msg: ConversationMessage = serde_json::from_str(&msg_json)?;
            if !can_pin_message(&msg) {
                return Ok(Some(PinRefusalReason::NotFound));
            }
            if current.is_some() {
                return Ok(None);
            }
            if pinned_thread_message_count_conn(conn, thread_id)? >= MESSAGE_PINS_LIMIT {
                return Ok(Some(PinRefusalReason::Limit));
            }
            pin_thread_message_conn(conn, thread_id, &msg.id, &now_iso(), pinner)?;
            Ok(None)
        })
    }

    fn resolve_pinnable_thread(
        &self,
        agent_id: &str,
        task_id: Option<&str>,
    ) -> StorageResult<Option<String>> {
        let roster = AgentRoster::new(self.database.clone());
        let Some(agent) = roster.get_agent(agent_id)? else {
            return Ok(None);
        };
        let Some(tid) = task_id else {
            return Ok(agent.thread_id);
        };
        let side = AgentThreadStore::new(self.database.clone()).task(tid)?;
        Ok(side.filter(|s| s.agent_id == agent_id).map(|s| s.thread_id))
    }

    fn is_pinnable_thread(&self, agent_id: &str, thread_id: &str) -> StorageResult<bool> {
        let roster = AgentRoster::new(self.database.clone());
        let Some(agent) = roster.get_agent(agent_id)? else {
            return Ok(false);
        };
        if agent.thread_id.as_deref() == Some(thread_id) {
            return Ok(true);
        }
        let side = AgentThreadStore::new(self.database.clone()).task_for_thread(thread_id)?;
        Ok(side.is_some_and(|s| s.agent_id == agent_id))
    }
}

/// `ProviderSessionStore` over `projection_provider_sessions` (`provider-sessions.ts:L40-L268`).
#[derive(Clone)]
pub struct ProviderSessionStore {
    database: OpenBotDatabase,
}

impl ProviderSessionStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn active_provider_session(
        &self,
        thread_id: &str,
        provider: AgentProviderId,
    ) -> StorageResult<Option<ProviderSession>> {
        self.database.with_conn(|conn| {
            let row_opt = conn
                .prepare_cached(
                    "SELECT id, thread_id, provider, external_session_id, model, effort, state,
                            created_at, updated_at, resume_cursor
                     FROM projection_provider_sessions
                     WHERE thread_id = ?1 AND provider = ?2 AND state = 'active'
                     ORDER BY created_at DESC, last_event_sequence DESC LIMIT 1",
                )?
                .query_row(params![thread_id, provider.as_str()], decode_provider_session_row)
                .optional()?;
            Ok(row_opt.transpose()?.flatten())
        })
    }

    pub fn list_provider_sessions(&self, thread_id: &str) -> StorageResult<Vec<ProviderSession>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT id, thread_id, provider, external_session_id, model, effort, state,
                        created_at, updated_at, resume_cursor
                 FROM projection_provider_sessions
                 WHERE thread_id = ?1
                 ORDER BY created_at, last_event_sequence",
            )?;
            let rows = stmt.query_map(params![thread_id], decode_provider_session_row)?;
            let mut out = Vec::new();
            for item in rows {
                if let Some(sess) = item?? {
                    out.push(sess);
                }
            }
            Ok(out)
        })
    }

    pub fn list_provider_session_ids(
        &self,
        external_session_id: Option<&str>,
    ) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT id FROM projection_provider_sessions
                 WHERE ?1 IS NULL OR external_session_id = ?1",
            )?;
            let rows = stmt.query_map(params![external_session_id], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn list_external_session_ids(&self) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT DISTINCT external_session_id FROM projection_provider_sessions",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn active_provider_session_threads(&self, agent_id: &str) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT DISTINCT session.thread_id
                 FROM projection_provider_sessions session
                 JOIN projection_threads thread ON thread.thread_id = session.thread_id
                 WHERE thread.agent_id = ?1 AND session.state = 'active'",
            )?;
            let rows = stmt.query_map(params![agent_id], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Binds a new `'active'` provider session after deactivating any prior `'active'` rows for `thread_id`.
    pub fn bind_provider_session(
        &self,
        input: BindProviderSessionInput,
    ) -> StorageResult<ProviderSession> {
        let now = now_iso();
        let session = ProviderSession {
            id: Uuid::new_v4().to_string(),
            thread_id: input.thread_id.clone(),
            provider: input.provider,
            external_session_id: input.external_session_id.clone(),
            model: input.model,
            effort: input.effort,
            state: ProviderSessionState::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
            resume_cursor: Some(
                input
                    .resume_cursor
                    .unwrap_or_else(|| input.external_session_id.clone()),
            ),
        };
        let command_id = format!(
            "provider-session:bind:{}:{}:{}",
            input.thread_id,
            input.provider.as_str(),
            input.external_session_id
        );
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread".to_string(),
            aggregate_id: input.thread_id.clone(),
            event_type: "provider-session.bound".to_string(),
            occurred_at: now.clone(),
            payload: serde_json::to_value(&session)?,
        }];
        let receipt = self.database.dispatch_with_projection(
            &command_id,
            None,
            &events,
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(0);
                conn.execute(
                    "UPDATE projection_provider_sessions SET state = 'inactive', updated_at = ?1
                     WHERE thread_id = ?2 AND state = 'active'",
                    params![now, session.thread_id],
                )?;
                conn.execute(
                    "INSERT INTO projection_provider_sessions (
                       id, thread_id, provider, external_session_id, model, effort, state,
                       created_at, updated_at, resume_cursor, last_event_sequence
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        session.id,
                        session.thread_id,
                        session.provider.as_str(),
                        session.external_session_id,
                        session.model,
                        session.effort,
                        session.state.as_str(),
                        session.created_at,
                        session.updated_at,
                        session.resume_cursor,
                        seq
                    ],
                )?;
                Ok(session.clone())
            },
        )?;
        Ok(receipt.result)
    }

    pub fn deactivate_provider_sessions(&self, thread_id: &str) -> StorageResult<()> {
        let active: Vec<String> = self
            .list_provider_sessions(thread_id)?
            .into_iter()
            .filter(|s| s.state == ProviderSessionState::Active)
            .map(|s| s.id)
            .collect();
        if active.is_empty() {
            return Ok(());
        }
        let command_id = format!("provider-session:deactivate:{thread_id}:{}", Uuid::new_v4());
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread".to_string(),
            aggregate_id: thread_id.to_string(),
            event_type: "provider-session.deactivated".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({ "sessionIds": active }),
        }];
        let _ = self.database.dispatch_with_projection(
            &command_id,
            None,
            &events,
            |conn, sequences| {
                conn.execute(
                    "UPDATE projection_provider_sessions
                     SET state = 'inactive', updated_at = ?1, last_event_sequence = ?2
                     WHERE thread_id = ?3 AND state = 'active'",
                    params![now_iso(), sequences.first().copied().unwrap_or(0), thread_id],
                )?;
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(())
    }

    pub fn update_provider_session_config(
        &self,
        session_id: &str,
        thread_id: &str,
        model: &str,
        effort: &str,
    ) -> StorageResult<()> {
        let command_id = format!("provider-session:config:{session_id}:{model}:{effort}");
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread".to_string(),
            aggregate_id: thread_id.to_string(),
            event_type: "provider-session.config-updated".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({ "sessionId": session_id, "model": model, "effort": effort }),
        }];
        let _ = self.database.dispatch_with_projection(
            &command_id,
            None,
            &events,
            |conn, sequences| {
                conn.execute(
                    "UPDATE projection_provider_sessions
                     SET model = ?1, effort = ?2, updated_at = ?3, last_event_sequence = ?4
                     WHERE id = ?5",
                    params![
                        model,
                        effort,
                        now_iso(),
                        sequences.first().copied().unwrap_or(0),
                        session_id
                    ],
                )?;
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(())
    }
}

fn decode_provider_session_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StorageResult<Option<ProviderSession>>> {
    let id: String = row.get(0)?;
    let thread_id: String = row.get(1)?;
    let provider_raw: String = row.get(2)?;
    let external_session_id: String = row.get(3)?;
    let model: String = row.get(4)?;
    let effort: String = row.get(5)?;
    let state_raw: String = row.get(6)?;
    let created_at: String = row.get(7)?;
    let updated_at: String = row.get(8)?;
    let resume_cursor: Option<String> = row.get(9)?;

    let Some(provider) = AgentProviderId::parse(&provider_raw) else {
        return Ok(Ok(None));
    };
    let Some(state) = ProviderSessionState::parse(&state_raw) else {
        return Ok(Err(StorageError::Validation(format!(
            "Invalid provider session state: {state_raw}"
        ))));
    };
    Ok(Ok(Some(ProviderSession {
        id,
        thread_id,
        provider,
        external_session_id,
        model,
        effort,
        state,
        created_at,
        updated_at,
        resume_cursor,
    })))
}

/// `AgentWaitStore` over `agent_waits` enforcing atomic `<= 20` (`INPUT_LIMITS.agent_waits`) cap.
#[derive(Clone)]
pub struct AgentWaitStore {
    database: OpenBotDatabase,
}

impl AgentWaitStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn list(&self, agent_id: &str) -> StorageResult<Vec<AgentWait>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT wait_id, agent_id, request_delivery_id, request_text, note,
                        routine_id, created_at, woken_at, wake_delivery_id
                 FROM agent_waits WHERE agent_id = ?1 ORDER BY created_at, wait_id",
            )?;
            let rows = stmt.query_map(params![agent_id], decode_wait_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn get(&self, agent_id: &str, wait_id: &str) -> StorageResult<Option<AgentWait>> {
        self.database.with_conn(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT wait_id, agent_id, request_delivery_id, request_text, note,
                            routine_id, created_at, woken_at, wake_delivery_id
                     FROM agent_waits WHERE agent_id = ?1 AND wait_id = ?2",
                )?
                .query_row(params![agent_id, wait_id], decode_wait_row)
                .optional()?)
        })
    }

    /// Atomically inserts a wait iff `COUNT(*) < INPUT_LIMITS.agent_waits` (`20`) (`agent-wait-store.ts:L60-L84`).
    pub fn open(&self, input: OpenAgentWaitInput) -> StorageResult<AgentWait> {
        let wait_id = Uuid::new_v4().to_string();
        let created_at = now_iso();
        let changed = self.database.with_conn(|conn| {
            Ok(conn.execute(
                "INSERT INTO agent_waits
                   (wait_id, agent_id, request_delivery_id, request_text, note, routine_id, created_at)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7
                 WHERE (SELECT COUNT(*) FROM agent_waits WHERE agent_id = ?8) < ?9",
                params![
                    wait_id,
                    input.agent_id,
                    input.request_message_id,
                    input.request_text,
                    input.note,
                    input.routine_id,
                    created_at,
                    input.agent_id,
                    INPUT_LIMITS.agent_waits as i64
                ],
            )?)
        })?;
        if changed == 0 {
            return Err(StorageError::LimitExceeded(format!(
                "Agent wait limit of {} reached.",
                INPUT_LIMITS.agent_waits
            )));
        }
        self.get(&input.agent_id, &wait_id)?
            .ok_or_else(|| StorageError::NotFound("The new wait is missing.".to_string()))
    }

    pub fn count(&self, agent_id: &str) -> StorageResult<usize> {
        self.database.with_conn(|conn| {
            let n: i64 = conn
                .prepare_cached("SELECT COUNT(*) FROM agent_waits WHERE agent_id = ?1")?
                .query_row(params![agent_id], |r| r.get(0))?;
            Ok(n.max(0) as usize)
        })
    }

    pub fn count_by_agent(&self) -> StorageResult<HashMap<String, usize>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT agent_id, COUNT(*) FROM agent_waits GROUP BY agent_id")?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.max(0) as usize))
            })?;
            Ok(rows.collect::<Result<HashMap<_, _>, _>>()?)
        })
    }

    pub fn delete(&self, agent_id: &str, wait_id: &str) -> StorageResult<bool> {
        self.database.with_conn(|conn| {
            let changes = conn.execute(
                "DELETE FROM agent_waits WHERE agent_id = ?1 AND wait_id = ?2",
                params![agent_id, wait_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn for_routine_run(&self, agent_id: &str, routine_id: &str) -> StorageResult<Vec<AgentWait>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT wait_id, agent_id, request_delivery_id, request_text, note,
                        routine_id, created_at, woken_at, wake_delivery_id
                 FROM agent_waits
                 WHERE agent_id = ?1 AND (routine_id IS NULL OR routine_id = ?2)
                 ORDER BY created_at, wait_id",
            )?;
            let rows = stmt.query_map(params![agent_id, routine_id], decode_wait_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn mark_woken(
        &self,
        agent_id: &str,
        wait_ids: &[String],
        delivery_id: &str,
    ) -> StorageResult<()> {
        if wait_ids.is_empty() {
            return Ok(());
        }
        let woke_at = now_iso();
        let placeholders = (0..wait_ids.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        self.database.with_conn(|conn| {
            let mut args: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(3 + wait_ids.len());
            args.push(&woke_at);
            args.push(&delivery_id);
            args.push(&agent_id);
            for wid in wait_ids {
                args.push(wid);
            }
            conn.execute(
                &format!(
                    "UPDATE agent_waits SET woken_at = ?, wake_delivery_id = ?
                     WHERE agent_id = ? AND wait_id IN ({placeholders})"
                ),
                params_from_iter(args),
            )?;
            Ok(())
        })
    }

    pub fn woken_by(&self, delivery_id: &str) -> StorageResult<Vec<AgentWait>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT wait_id, agent_id, request_delivery_id, request_text, note,
                        routine_id, created_at, woken_at, wake_delivery_id
                 FROM agent_waits WHERE wake_delivery_id = ?1 ORDER BY created_at, wait_id",
            )?;
            let rows = stmt.query_map(params![delivery_id], decode_wait_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }
}

fn decode_wait_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentWait> {
    Ok(AgentWait {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        request_message_id: row.get(2)?,
        request_text: row.get(3)?,
        note: row.get(4)?,
        routine_id: row.get(5)?,
        created_at: row.get(6)?,
        woken_at: row.get(7)?,
        wake_delivery_id: row.get(8)?,
    })
}

const SELECT_AGENT_THREAD_SQL: &str = "SELECT thread_id, agent_id, COALESCE(legacy_task_id, thread_id) AS task_id,
    origin, title, title_source, outcome, error, created_at, updated_at, archived_at,
    branch_source_thread_id, branch_source_message_id, branch_through_message_id, routine_id
  FROM projection_agent_threads";

const BY_TASK_ID_SQL: &str = "(legacy_task_id = ? OR (legacy_task_id IS NULL AND thread_id = ?))";

/// `AgentThreadStore` over `projection_agent_threads` (`agent-thread-store.ts:L75-L573`).
#[derive(Clone)]
pub struct AgentThreadStore {
    database: OpenBotDatabase,
}

impl AgentThreadStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn task(&self, task_id: &str) -> StorageResult<Option<AgentThreadRecord>> {
        self.database.with_conn(|conn| {
            let row = conn
                .prepare_cached(&format!(
                    "{SELECT_AGENT_THREAD_SQL} WHERE legacy_task_id = ?1 OR (legacy_task_id IS NULL AND thread_id = ?1)"
                ))?
                .query_row(params![task_id], decode_agent_thread_row)
                .optional()?;
            row.transpose()
        })
    }

    pub fn thread_for_routine(
        &self,
        agent_id: &str,
        routine_id: &str,
    ) -> StorageResult<Option<AgentThreadRecord>> {
        self.database.with_conn(|conn| {
            let row = conn
                .prepare_cached(&format!(
                    "{SELECT_AGENT_THREAD_SQL} WHERE agent_id = ?1 AND routine_id = ?2"
                ))?
                .query_row(params![agent_id, routine_id], decode_agent_thread_row)
                .optional()?;
            row.transpose()
        })
    }

    pub fn task_for_thread(&self, thread_id: &str) -> StorageResult<Option<AgentThreadRecord>> {
        self.database.with_conn(|conn| {
            let row = conn
                .prepare_cached(&format!("{SELECT_AGENT_THREAD_SQL} WHERE thread_id = ?1"))?
                .query_row(params![thread_id], decode_agent_thread_row)
                .optional()?;
            row.transpose()
        })
    }

    pub fn list(&self, agent_id: &str) -> StorageResult<Vec<AgentThreadRecord>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(&format!(
                "{SELECT_AGENT_THREAD_SQL} WHERE agent_id = ?1
                 ORDER BY archived_at IS NOT NULL, updated_at DESC, thread_id LIMIT ?2"
            ))?;
            let rows = stmt.query_map(
                params![agent_id, INPUT_LIMITS.agent_tasks as i64],
                decode_agent_thread_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .collect::<StorageResult<Vec<_>>>()
        })
    }

    pub fn list_all_for_agent(&self, agent_id: &str) -> StorageResult<Vec<AgentThreadRecord>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(&format!(
                "{SELECT_AGENT_THREAD_SQL} WHERE agent_id = ?1 ORDER BY updated_at DESC, thread_id"
            ))?;
            let rows = stmt.query_map(params![agent_id], decode_agent_thread_row)?;
            rows.collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .collect::<StorageResult<Vec<_>>>()
        })
    }

    pub fn archived_page(
        &self,
        agent_id: &str,
        last: Option<&ThreadListPosition>,
        limit: usize,
    ) -> StorageResult<Vec<AgentThreadRecord>> {
        let last_updated = last.map(|p| p.updated_at.as_str());
        let last_tid = last.map(|p| p.thread_id.as_str());
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(&format!(
                "{SELECT_AGENT_THREAD_SQL} WHERE agent_id = ?1 AND archived_at IS NOT NULL
                 AND (?2 IS NULL OR updated_at < ?2 OR (updated_at = ?2 AND thread_id > ?3))
                 ORDER BY updated_at DESC, thread_id LIMIT ?4"
            ))?;
            let rows = stmt.query_map(
                params![agent_id, last_updated, last_tid, limit as i64],
                decode_agent_thread_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .collect::<StorageResult<Vec<_>>>()
        })
    }

    pub fn recent(
        &self,
        agent_id: &str,
        after: Option<&str>,
        limit: usize,
        last: Option<&ThreadListPosition>,
    ) -> StorageResult<Vec<AgentThreadRecord>> {
        let last_updated = last.map(|p| p.updated_at.as_str());
        let last_tid = last.map(|p| p.thread_id.as_str());
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(&format!(
                "{SELECT_AGENT_THREAD_SQL} WHERE agent_id = ?1 AND (?2 IS NULL OR updated_at >= ?2)
                 AND (?3 IS NULL OR updated_at < ?3 OR (updated_at = ?3 AND thread_id > ?4))
                 ORDER BY updated_at DESC, thread_id LIMIT ?5"
            ))?;
            let rows = stmt.query_map(
                params![agent_id, after, last_updated, last_tid, limit as i64],
                decode_agent_thread_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .collect::<StorageResult<Vec<_>>>()
        })
    }

    pub fn execution_threads(&self) -> StorageResult<Vec<(String, String)>> {
        self.database.with_conn(|conn| {
            let mut stmt =
                conn.prepare_cached("SELECT agent_id, thread_id FROM projection_agent_threads")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn open_execution_threads(&self) -> StorageResult<Vec<(String, String)>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT agent_id, thread_id FROM projection_agent_threads
                 WHERE archived_at IS NULL ORDER BY updated_at DESC, thread_id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn open_task_ids(&self, agent_id: &str) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT COALESCE(legacy_task_id, thread_id) FROM projection_agent_threads
                 WHERE agent_id = ?1 AND archived_at IS NULL ORDER BY created_at, thread_id",
            )?;
            let rows = stmt.query_map(params![agent_id], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn task_ids_for_agent(&self, agent_id: &str) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT COALESCE(legacy_task_id, thread_id) FROM projection_agent_threads WHERE agent_id = ?1",
            )?;
            let rows = stmt.query_map(params![agent_id], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn thread_ids_for_agent(&self, agent_id: &str) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT thread_id FROM projection_agent_threads WHERE agent_id = ?1",
            )?;
            let rows = stmt.query_map(params![agent_id], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn titles(&self, agent_id: &str) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT title FROM projection_agent_threads WHERE agent_id = ?1")?;
            let rows = stmt.query_map(params![agent_id], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn branch_for(
        &self,
        agent_id: &str,
        request_id: &str,
    ) -> StorageResult<Option<AgentThreadRecord>> {
        let cmd = format!("agent-thread-branch:{agent_id}:{request_id}");
        let Some((_seq, val)) = self.database.command_result::<serde_json::Value>(&cmd)? else {
            return Ok(None);
        };
        let Some(tid) = val.get("threadId").and_then(|v| v.as_str()) else {
            return Ok(None);
        };
        Ok(self.task_for_thread(tid)?.filter(|t| t.agent_id == agent_id))
    }

    pub fn branch_source(
        &self,
        thread_id: &str,
        message_id: &str,
    ) -> StorageResult<Option<Vec<ConversationMessage>>> {
        self.database.with_conn(|conn| {
            let chosen: Option<i64> = conn
                .prepare_cached(
                    "SELECT 1 FROM projection_thread_messages
                     WHERE thread_id = ?1 AND message_id = ?2 AND author = 'assistant'
                       AND status != 'streaming' AND group_start IS NOT NULL",
                )?
                .query_row(params![thread_id, message_id], |r| r.get(0))
                .optional()?;
            if chosen.is_none() {
                return Ok(None);
            }
            let mut stmt = conn.prepare_cached(
                "SELECT message.message_json FROM projection_thread_messages AS message
                 JOIN projection_thread_messages AS chosen
                   ON chosen.thread_id = message.thread_id AND chosen.message_id = ?1
                 WHERE message.thread_id = ?2
                   AND (message.group_start, message.group_first, message.group_id)
                     <= (chosen.group_start, chosen.group_first, chosen.group_id)
                 ORDER BY message.ordinal, message.message_id",
            )?;
            let rows = stmt.query_map(params![message_id, thread_id], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for raw in rows {
                out.push(serde_json::from_str(&raw?)?);
            }
            Ok(Some(out))
        })
    }

    pub fn branch(&self, input: &AgentThreadBranchInput) -> StorageResult<AgentThreadRecord> {
        if let Some(existing) = self.branch_for(&input.agent_id, &input.request_id)? {
            return Ok(existing);
        }
        if self.open_task_ids(&input.agent_id)?.len() >= INPUT_LIMITS.agent_tasks {
            return Err(StorageError::LimitExceeded(format!(
                "Open side-thread limit of {} reached.",
                INPUT_LIMITS.agent_tasks
            )));
        }
        if input.messages.is_empty() {
            return Err(StorageError::Validation(
                "A branch needs at least one message.".to_string(),
            ));
        }
        let now = now_iso();
        let command_id = format!("agent-thread-branch:{}:{}", input.agent_id, input.request_id);
        let events = [
            OrchestrationEventInput {
                event_id: Uuid::new_v4().to_string(),
                aggregate_type: "agent-thread".to_string(),
                aggregate_id: input.thread_id.clone(),
                event_type: "agent-thread.branched".to_string(),
                occurred_at: now.clone(),
                payload: serde_json::json!({
                    "agentId": input.agent_id,
                    "origin": "user",
                    "sourceThreadId": input.source_thread_id,
                    "sourceMessageId": input.source_message_id,
                }),
            },
            OrchestrationEventInput {
                event_id: Uuid::new_v4().to_string(),
                aggregate_type: "thread".to_string(),
                aggregate_id: input.thread_id.clone(),
                event_type: "thread.branched".to_string(),
                occurred_at: now.clone(),
                payload: serde_json::json!({
                    "detail": {
                        "sourceThreadId": input.source_thread_id,
                        "sourceMessageId": input.source_message_id,
                    },
                    "snapshot": {
                        "agentId": input.agent_id,
                        "threadId": input.thread_id,
                        "activeTurnId": null,
                        "revision": 0,
                        "messages": input.messages,
                    },
                    "recovery": { "turnProviderSessionIds": {} },
                }),
            },
        ];
        let receipt = self.database.dispatch_with_projection(
            &command_id,
            None,
            &events,
            |conn, sequences| {
                let thread_seq = sequences.first().copied().unwrap_or(0);
                let conv_seq = sequences.get(1).copied().unwrap_or(thread_seq);
                conn.execute(
                    "INSERT INTO projection_threads VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
                    params![input.thread_id, input.agent_id, input.title, now, now, conv_seq],
                )?;
                for (ordinal, msg) in input.messages.iter().enumerate() {
                    insert_thread_message_row(conn, &input.thread_id, msg, ordinal as i64, conv_seq)?;
                }
                refresh_thread_order(conn, &input.thread_id)?;
                let through_id: String = conn.query_row(
                    &format!(
                        "SELECT message_id FROM projection_thread_messages
                         WHERE thread_id = ?1 ORDER BY {ORDER_KEY_DESC} LIMIT 1"
                    ),
                    params![input.thread_id],
                    |r| r.get(0),
                )?;
                conn.execute(
                    "INSERT INTO projection_agent_threads
                       (thread_id, agent_id, legacy_task_id, origin, title, title_source, outcome,
                        error, created_at, updated_at, archived_at, last_event_sequence,
                        branch_source_thread_id, branch_source_message_id, branch_through_message_id)
                     VALUES (?1, ?2, NULL, 'user', ?3, 'auto', NULL, NULL, ?4, ?5, NULL, ?6, ?7, ?8, ?9)",
                    params![
                        input.thread_id,
                        input.agent_id,
                        input.title,
                        now,
                        now,
                        thread_seq,
                        input.source_thread_id,
                        input.source_message_id,
                        through_id
                    ],
                )?;
                Ok(serde_json::json!({ "threadId": input.thread_id }))
            },
        )?;
        let tid = receipt
            .result
            .get("threadId")
            .and_then(|v| v.as_str())
            .unwrap_or(&input.thread_id);
        self.task_for_thread(tid)?
            .ok_or_else(|| StorageError::NotFound("The branch was not stored.".to_string()))
    }

    /// Creates a new side-thread enforcing `< INPUT_LIMITS.agent_tasks` (`50`) open threads.
    pub fn create(&self, input: AgentThreadInput) -> StorageResult<AgentThreadRecord> {
        if input.title.trim().is_empty() {
            return Err(StorageError::Validation(
                "Side-thread title is required.".to_string(),
            ));
        }
        if self.open_task_ids(&input.agent_id)?.len() >= INPUT_LIMITS.agent_tasks {
            return Err(StorageError::LimitExceeded(format!(
                "Open side-thread limit of {} reached.",
                INPUT_LIMITS.agent_tasks
            )));
        }
        let thread_id = format!("openbot-thread-{}", Uuid::new_v4());
        let now = now_iso();
        let origin_str = match input.origin {
            AgentTaskOrigin::User => "user",
            AgentTaskOrigin::Agent => "agent",
        };
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agent-thread".to_string(),
            aggregate_id: thread_id.clone(),
            event_type: "agent-thread.created".to_string(),
            occurred_at: now.clone(),
            payload: serde_json::json!({ "agentId": input.agent_id, "origin": origin_str }),
        }];
        let _ = self.database.dispatch_with_projection(
            &format!("agent-thread:{thread_id}"),
            None,
            &events,
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(0);
                conn.execute(
                    "INSERT INTO projection_threads VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
                    params![thread_id, input.agent_id, input.title, now, now, seq],
                )?;
                conn.execute(
                    "INSERT INTO projection_agent_threads
                       (thread_id, agent_id, legacy_task_id, origin, title, title_source, outcome,
                        error, created_at, updated_at, archived_at, last_event_sequence, routine_id)
                     VALUES (?1, ?2, NULL, ?3, ?4, ?5, NULL, NULL, ?6, ?7, NULL, ?8, ?9)",
                    params![
                        thread_id,
                        input.agent_id,
                        origin_str,
                        input.title,
                        input.title_source.as_str(),
                        now,
                        now,
                        seq,
                        input.routine_id
                    ],
                )?;
                Ok(serde_json::Value::Null)
            },
        )?;
        self.task_for_thread(&thread_id)?
            .ok_or_else(|| StorageError::NotFound("The thread was not stored.".to_string()))
    }

    /// Enforces `<= 8` agent-started unfinished tasks and auto-archives older finished
    /// agent-started tasks keeping `<= 8` (`agent-tasks.ts:L637-L652`).
    pub fn start_agent_task_thread<F>(
        &self,
        agent_id: &str,
        title: &str,
        is_finished: F,
    ) -> StorageResult<(AgentThreadRecord, Vec<AgentThreadRecord>)>
    where
        F: Fn(&AgentThreadRecord) -> bool,
    {
        let open_ids = self.open_task_ids(agent_id)?;
        let mut open = Vec::with_capacity(open_ids.len());
        for tid in open_ids {
            if let Some(rec) = self.task(&tid)? {
                open.push(rec);
            }
        }
        let is_started_by_agent =
            |t: &AgentThreadRecord| t.origin == AgentTaskOrigin::Agent && t.routine_id.is_none();
        let unfinished_agent = open
            .iter()
            .filter(|t| is_started_by_agent(t) && !is_finished(t))
            .count();
        if unfinished_agent >= AGENT_STARTED_UNFINISHED_TASKS {
            return Err(StorageError::LimitExceeded(format!(
                "Agent-started unfinished task limit of {AGENT_STARTED_UNFINISHED_TASKS} reached."
            )));
        }
        let mut finished_by_agent: Vec<AgentThreadRecord> = open
            .iter()
            .filter(|t| is_started_by_agent(t) && is_finished(t))
            .cloned()
            .collect();
        finished_by_agent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let others = open.len().saturating_sub(finished_by_agent.len());
        let room = INPUT_LIMITS
            .agent_tasks
            .saturating_sub(1)
            .saturating_sub(others);
        let kept = AGENT_STARTED_FINISHED_KEPT.min(room);
        let archived: Vec<AgentThreadRecord> = finished_by_agent.into_iter().skip(kept).collect();
        if open.len().saturating_sub(archived.len()) >= INPUT_LIMITS.agent_tasks {
            return Err(StorageError::LimitExceeded(format!(
                "Open side-thread limit of {} reached.",
                INPUT_LIMITS.agent_tasks
            )));
        }
        for rec in &archived {
            self.close(&rec.task_id)?;
        }
        let created = self.create(AgentThreadInput {
            agent_id: agent_id.to_string(),
            origin: AgentTaskOrigin::Agent,
            title: title.trim().to_string(),
            title_source: AgentThreadTitleSource::Agent,
            routine_id: None,
        })?;
        Ok((created, archived))
    }

    pub fn rename(&self, thread_id: &str, title: &str) -> StorageResult<()> {
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agent-thread".to_string(),
            aggregate_id: thread_id.to_string(),
            event_type: "agent-thread.renamed".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({}),
        }];
        let _ = self.database.dispatch_with_projection(
            &format!("agent-thread-renamed:{thread_id}:{}", Uuid::new_v4()),
            None,
            &events,
            |conn, sequences| {
                conn.execute(
                    "UPDATE projection_agent_threads
                     SET title = ?1, title_source = 'user', last_event_sequence = ?2
                     WHERE thread_id = ?3",
                    params![title, sequences.first().copied().unwrap_or(0), thread_id],
                )?;
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(())
    }

    pub fn retitle(&self, thread_id: &str, title: &str) -> StorageResult<()> {
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agent-thread".to_string(),
            aggregate_id: thread_id.to_string(),
            event_type: "agent-thread.renamed".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({}),
        }];
        let _ = self.database.dispatch_with_projection(
            &format!("agent-thread-retitled:{thread_id}:{}", Uuid::new_v4()),
            None,
            &events,
            |conn, sequences| {
                conn.execute(
                    "UPDATE projection_agent_threads
                     SET title = ?1, last_event_sequence = ?2
                     WHERE thread_id = ?3 AND title_source <> 'user'",
                    params![title, sequences.first().copied().unwrap_or(0), thread_id],
                )?;
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(())
    }

    pub fn set_outcome(&self, task_id: &str, outcome: AgentTaskOutcome) -> StorageResult<()> {
        let out_str = match outcome {
            AgentTaskOutcome::Completed => "completed",
            AgentTaskOutcome::Failed => "failed",
            AgentTaskOutcome::Interrupted => "interrupted",
        };
        self.database.with_conn(|conn| {
            conn.execute(
                &format!(
                    "UPDATE projection_agent_threads SET outcome = ?1, updated_at = ?2 WHERE {}",
                    BY_TASK_ID_SQL.replacen('?', "?3", 1).replacen('?', "?3", 1)
                ),
                params![out_str, now_iso(), task_id],
            )?;
            Ok(())
        })
    }

    pub fn set_error(&self, task_id: &str, error: Option<&str>) -> StorageResult<()> {
        self.database.with_conn(|conn| {
            conn.execute(
                "UPDATE projection_agent_threads SET error = ?1
                 WHERE legacy_task_id = ?2 OR (legacy_task_id IS NULL AND thread_id = ?2)",
                params![error, task_id],
            )?;
            Ok(())
        })
    }

    pub fn touch(&self, task_id: &str) -> StorageResult<()> {
        self.database.with_conn(|conn| {
            conn.execute(
                "UPDATE projection_agent_threads SET updated_at = ?1
                 WHERE legacy_task_id = ?2 OR (legacy_task_id IS NULL AND thread_id = ?2)",
                params![now_iso(), task_id],
            )?;
            Ok(())
        })
    }

    pub fn close(&self, task_id: &str) -> StorageResult<()> {
        let now = now_iso();
        self.database.with_conn(|conn| {
            conn.execute(
                "UPDATE projection_agent_threads SET archived_at = ?1, updated_at = ?1
                 WHERE (legacy_task_id = ?2 OR (legacy_task_id IS NULL AND thread_id = ?2))
                   AND archived_at IS NULL",
                params![now, task_id],
            )?;
            Ok(())
        })
    }

    pub fn reopen(&self, task_id: &str) -> StorageResult<()> {
        if let Some(rec) = self.task(task_id)?
            && rec.closed_at.is_some()
            && self.open_task_ids(&rec.agent_id)?.len() >= INPUT_LIMITS.agent_tasks
        {
            return Err(StorageError::LimitExceeded(format!(
                "Open side-thread limit of {} reached.",
                INPUT_LIMITS.agent_tasks
            )));
        }
        self.database.with_conn(|conn| {
            conn.execute(
                "UPDATE projection_agent_threads SET archived_at = NULL, updated_at = ?1
                 WHERE legacy_task_id = ?2 OR (legacy_task_id IS NULL AND thread_id = ?2)",
                params![now_iso(), task_id],
            )?;
            Ok(())
        })
    }

    pub fn delete(&self, task_id: &str) -> StorageResult<()> {
        let Some(thread) = self.task(task_id)? else {
            return Ok(());
        };
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agent-threads".to_string(),
            aggregate_id: thread.agent_id.clone(),
            event_type: "agent-thread.deleted".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({ "threadIds": [thread.thread_id] }),
        }];
        let _ = self.database.dispatch_with_projection(
            &format!("agent-thread-deleted:{}:{}", thread.thread_id, Uuid::new_v4()),
            None,
            &events,
            |conn, _seq| {
                delete_agent_thread_rows(conn, std::slice::from_ref(&thread))?;
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(())
    }

    pub fn delete_for_agent(&self, agent_id: &str) -> StorageResult<()> {
        let threads = self.list_all_for_agent(agent_id)?;
        if threads.is_empty() {
            return Ok(());
        }
        let tids: Vec<String> = threads.iter().map(|t| t.thread_id.clone()).collect();
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agent-threads".to_string(),
            aggregate_id: agent_id.to_string(),
            event_type: "agent-thread.agent-deleted".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({ "threadIds": tids }),
        }];
        let _ = self.database.dispatch_with_projection(
            &format!("agent-threads-deleted:{agent_id}:{}", Uuid::new_v4()),
            None,
            &events,
            |conn, _seq| {
                delete_agent_thread_rows(conn, &threads)?;
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(())
    }
}

fn delete_agent_thread_rows(
    conn: &Connection,
    threads: &[AgentThreadRecord],
) -> StorageResult<()> {
    let legacy_task_ids: Vec<String> = threads
        .iter()
        .filter(|t| t.task_id != t.thread_id)
        .map(|t| t.task_id.clone())
        .collect();
    delete_aggregate_history(conn, "agent-task", &legacy_task_ids)?;
    let thread_ids: Vec<String> = threads.iter().map(|t| t.thread_id.clone()).collect();
    delete_aggregate_history(conn, "agent-thread", &thread_ids)?;
    delete_aggregate_history(conn, "thread", &thread_ids)?;
    for tid in &thread_ids {
        delete_thread_attachments(conn, tid)?;
        conn.execute("DELETE FROM projection_threads WHERE thread_id = ?1", params![tid])?;
    }
    Ok(())
}

fn decode_agent_thread_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StorageResult<AgentThreadRecord>> {
    let thread_id: String = row.get(0)?;
    let agent_id: String = row.get(1)?;
    let task_id: String = row.get(2)?;
    let origin_raw: String = row.get(3)?;
    let title: String = row.get(4)?;
    let title_source_raw: String = row.get(5)?;
    let outcome_raw: Option<String> = row.get(6)?;
    let error: Option<String> = row.get(7)?;
    let created_at: String = row.get(8)?;
    let updated_at: String = row.get(9)?;
    let closed_at: Option<String> = row.get(10)?;
    let src_tid: Option<String> = row.get(11)?;
    let src_mid: Option<String> = row.get(12)?;
    let through_mid: Option<String> = row.get(13)?;
    let routine_id: Option<String> = row.get(14)?;

    let branch_cols = [src_tid.is_some(), src_mid.is_some(), through_mid.is_some()]
        .into_iter()
        .filter(|b| *b)
        .count();
    if branch_cols != 0 && branch_cols != 3 {
        return Ok(Err(StorageError::Validation(
            "Incomplete branch link.".to_string(),
        )));
    }
    let origin = match origin_raw.as_str() {
        "user" => AgentTaskOrigin::User,
        "agent" => AgentTaskOrigin::Agent,
        _ => {
            return Ok(Err(StorageError::Validation(format!(
                "Unknown thread origin: {origin_raw}"
            ))));
        }
    };
    let Some(title_source) = AgentThreadTitleSource::parse(&title_source_raw) else {
        return Ok(Err(StorageError::Validation(format!(
            "Unknown title source: {title_source_raw}"
        ))));
    };
    let outcome = match outcome_raw.as_deref() {
        None => None,
        Some("completed") => Some(AgentTaskOutcome::Completed),
        Some("failed") => Some(AgentTaskOutcome::Failed),
        Some("interrupted") => Some(AgentTaskOutcome::Interrupted),
        Some(other) => {
            return Ok(Err(StorageError::Validation(format!(
                "Unknown thread outcome: {other}"
            ))));
        }
    };
    let branch = match (src_tid, src_mid, through_mid) {
        (Some(source_thread_id), Some(source_message_id), Some(through_message_id)) => {
            Some(AgentThreadBranch {
                source_thread_id,
                source_message_id,
                through_message_id,
            })
        }
        _ => None,
    };
    Ok(Ok(AgentThreadRecord {
        task_id,
        agent_id,
        thread_id,
        origin,
        title,
        title_source,
        outcome,
        error,
        created_at,
        updated_at,
        closed_at,
        branch,
        routine_id,
    }))
}

fn author_str(author: ConversationMessageAuthor) -> &'static str {
    match author {
        ConversationMessageAuthor::User => "user",
        ConversationMessageAuthor::Assistant => "assistant",
        ConversationMessageAuthor::Agent => "agent",
        ConversationMessageAuthor::System => "system",
    }
}

fn status_str(status: ConversationMessageStatus) -> &'static str {
    match status {
        ConversationMessageStatus::Streaming => "streaming",
        ConversationMessageStatus::Completed => "completed",
        ConversationMessageStatus::Failed => "failed",
        ConversationMessageStatus::Interrupted => "interrupted",
    }
}

fn insert_thread_message_row(
    conn: &Connection,
    thread_id: &str,
    message: &ConversationMessage,
    ordinal: i64,
    sequence: i64,
) -> StorageResult<()> {
    let json = serde_json::to_string(message)?;
    conn.execute(
        "INSERT INTO projection_thread_messages (
           thread_id, message_id, turn_id, author, status, item_type, created_at,
           ordinal, message_json, last_event_sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(thread_id, message_id) DO UPDATE SET
           turn_id = excluded.turn_id,
           author = excluded.author,
           status = excluded.status,
           item_type = excluded.item_type,
           created_at = excluded.created_at,
           ordinal = excluded.ordinal,
           message_json = excluded.message_json,
           last_event_sequence = excluded.last_event_sequence",
        params![
            thread_id,
            message.id,
            message.turn_id,
            author_str(message.author),
            status_str(message.status),
            message.item_type,
            message.created_at,
            ordinal,
            json,
            sequence
        ],
    )?;
    for att in message.attachments.as_deref().unwrap_or(&[]) {
        let owner_id = format!("{thread_id}:{}", message.id);
        let att_id = format!("{owner_id}:{}", att.id);
        conn.execute(
            "INSERT OR REPLACE INTO projection_attachments
               (attachment_id, owner_kind, owner_id, name, path, metadata_json, created_at, last_event_sequence)
             VALUES (?1, 'thread-message', ?2, ?3, '', ?4, ?5, ?6)",
            params![
                att_id,
                owner_id,
                att.name,
                serde_json::to_string(att)?,
                message.created_at,
                sequence
            ],
        )?;
    }
    Ok(())
}

fn record_usage_message(
    conn: &Connection,
    agent_id: &str,
    message: &ConversationMessage,
    provider: AgentProviderId,
    model: &str,
) -> StorageResult<()> {
    if !matches!(
        message.author,
        ConversationMessageAuthor::User | ConversationMessageAuthor::Assistant
    ) {
        return Ok(());
    }
    if message.author == ConversationMessageAuthor::Assistant
        && (message.status != ConversationMessageStatus::Completed
            || message
                .item_type
                .as_deref()
                .is_some_and(|it| it != "agentMessage" && it != "final_answer"))
    {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO agent_usage_activity
           (agent_id, activity_id, session_id, turn_id, provider, model, kind, occurred_at)
         SELECT ?1, ?2, '', ?3, ?4, ?5, ?6, ?7
         WHERE COALESCE((SELECT applied_at FROM schema_migrations WHERE version = 15), '') <= ?7",
        params![
            agent_id,
            format!("message:{}", message.id),
            message.turn_id.as_deref().unwrap_or(""),
            provider.as_str(),
            model,
            author_str(message.author),
            message.created_at
        ],
    )?;
    Ok(())
}

type OrderKeyTuple = (String, i64, String, i64, String, i64, String);

fn conversation_marker_sql_filter(exclusions: ConversationMarkerExclusions) -> String {
    let mut parts = vec![format!(
        "AND COALESCE(item_type, '') NOT IN ('{AGENT_WELCOME_ITEM_TYPE}', '{CHANNEL_RESUME_ITEM_TYPE}')"
    )];
    if exclusions.exclude_routine_events {
        parts.push(format!(
            "AND COALESCE(item_type, '') NOT LIKE 'skill-event:%' AND COALESCE(item_type, '') NOT LIKE '{ROUTINE_EVENT_ITEM_PREFIX}%' AND COALESCE(item_type, '') NOT LIKE 'marketplace-suggestion:%'"
        ));
    }
    if exclusions.exclude_routine_run_events {
        parts.push(format!(
            "AND COALESCE(item_type, '') NOT LIKE '{ROUTINE_RUN_EVENT_ITEM_PREFIX}%'"
        ));
    }
    if exclusions.exclude_hosted_site_events {
        parts.push(format!(
            "AND COALESCE(item_type, '') NOT LIKE '{HOSTED_SITE_EVENT_ITEM_PREFIX}%'"
        ));
    }
    if exclusions.exclude_routine_quiet_answers {
        parts.push(format!(
            "AND COALESCE(item_type, '') != '{ROUTINE_QUIET_ANSWER_ITEM_TYPE}'"
        ));
    }
    if exclusions.exclude_setting_links {
        parts.push("AND COALESCE(item_type, '') NOT LIKE 'setting-link:%'".to_string());
    }
    parts.join("\n")
}

fn unread_sql_filter() -> String {
    let mut parts = vec!["author != 'user'".to_string()];
    for it in READ_EXCLUDED_ITEM_TYPES {
        parts.push(format!("COALESCE(item_type, '') != '{it}'"));
    }
    for prefix in READ_EXCLUDED_ITEM_TYPE_PREFIXES {
        parts.push(format!("COALESCE(item_type, '') NOT LIKE '{prefix}%'"));
    }
    parts.join(" AND ")
}

fn message_order_key_conn(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> StorageResult<Option<OrderKeyTuple>> {
    let sql = format!(
        "{ORDERED_THREAD_MESSAGES_CTE}
         SELECT {ORDER_KEY_COLUMNS} FROM ordered WHERE message_id = ?"
    );
    Ok(conn
        .prepare_cached(&sql)?
        .query_row(params![thread_id, message_id], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })
        .optional()?)
}

pub fn supported_conversation_cursor_conn(
    conn: &Connection,
    thread_id: &str,
    through_message_id: Option<&str>,
    exclusions: ConversationMarkerExclusions,
) -> StorageResult<Option<String>> {
    let Some(through_id) = through_message_id else {
        return Ok(None);
    };
    let Some(k) = message_order_key_conn(conn, thread_id, through_id)? else {
        return Ok(None);
    };
    let filter = conversation_marker_sql_filter(exclusions);
    let sql = format!(
        "{ORDERED_THREAD_MESSAGES_CTE}
         SELECT message_id FROM ordered
         WHERE ({ORDER_KEY_COLUMNS}) <= (?, ?, ?, ?, ?, ?, ?)
           {filter}
         ORDER BY {ORDER_KEY_DESC}
         LIMIT 1"
    );
    Ok(conn
        .prepare_cached(&sql)?
        .query_row(
            params![thread_id, k.0, k.1, k.2, k.3, k.4, k.5, k.6],
            |r| r.get(0),
        )
        .optional()?)
}

/// `ConversationReadStore` over `projection_thread_reads` and `projection_thread_read_baselines`.
#[derive(Clone)]
pub struct ConversationReadStore {
    database: OpenBotDatabase,
}

impl ConversationReadStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn list_states(
        &self,
        member_id: &str,
        agents: &[AgentSummary],
        exclusions: ConversationMarkerExclusions,
    ) -> StorageResult<BTreeMap<String, ConversationReadState>> {
        let mut map = BTreeMap::new();
        for agent in agents {
            let state = self.read_state_for_thread(member_id, agent.thread_id.as_deref(), exclusions)?;
            map.insert(agent.id.clone(), state);
        }
        Ok(map)
    }

    pub fn read_state_for_thread(
        &self,
        member_id: &str,
        thread_id: Option<&str>,
        exclusions: ConversationMarkerExclusions,
    ) -> StorageResult<ConversationReadState> {
        let Some(tid) = thread_id else {
            return Ok(empty_read_state());
        };
        let stored = self.stored_cursor(tid, member_id)?;
        let cursor = match stored {
            None => {
                let baseline = self.baseline_cursor(tid)?;
                let init = match baseline {
                    Some(ref b) if !self.message_exists(tid, b)? => self.latest_message_id(tid)?,
                    other => other,
                };
                self.save_cursor(tid, member_id, init.as_deref(), "initialized")?;
                init
            }
            Some(Some(ref c)) if !self.message_exists(tid, c)? => {
                let latest = self.latest_message_id(tid)?;
                self.save_cursor(tid, member_id, latest.as_deref(), "initialized")?;
                latest
            }
            Some(other) => other,
        };
        let state = self.state_from_database(tid, cursor.as_deref())?;
        self.with_supported_cursor(tid, state, exclusions)
    }

    pub fn mark_read_for_thread(
        &self,
        member_id: &str,
        thread_id: Option<&str>,
        through_message_id: Option<&str>,
        exclusions: ConversationMarkerExclusions,
    ) -> StorageResult<ConversationReadState> {
        let Some(tid) = thread_id else {
            return Ok(empty_read_state());
        };
        let requested_key = match through_message_id {
            Some(mid) => {
                let k = self.database.with_conn(|c| message_order_key_conn(c, tid, mid))?;
                if k.is_none() {
                    return self.read_state_for_thread(member_id, Some(tid), exclusions);
                }
                k
            }
            None => None,
        };
        let stored = self.stored_cursor(tid, member_id)?.flatten();
        let mut next = through_message_id.map(str::to_string);
        if let Some(ref s) = stored
            && (through_message_id.is_none() || self.is_after(tid, s, requested_key.as_ref())?)
        {
            next = Some(s.clone());
        }
        self.save_cursor(tid, member_id, next.as_deref(), "marked")?;
        let state = self.state_from_database(tid, next.as_deref())?;
        self.with_supported_cursor(tid, state, exclusions)
    }

    pub fn mark_unread_for_thread(
        &self,
        member_id: &str,
        thread_id: Option<&str>,
        from_message_id: Option<&str>,
        exclusions: ConversationMarkerExclusions,
    ) -> StorageResult<ConversationReadState> {
        let Some(tid) = thread_id else {
            return Ok(empty_read_state());
        };
        let through = match from_message_id {
            Some(mid) => {
                let key = self
                    .database
                    .with_conn(|c| message_order_key_conn(c, tid, mid))?
                    .ok_or_else(|| {
                        StorageError::NotFound("Read boundary message is unavailable.".to_string())
                    })?;
                self.message_before(tid, &key)?
            }
            None => None,
        };
        self.save_cursor(tid, member_id, through.as_deref(), "marked")?;
        let state = self.state_from_database(tid, through.as_deref())?;
        self.with_supported_cursor(tid, state, exclusions)
    }

    pub fn adopt_member_state(
        &self,
        source_member_id: &str,
        target_member_id: &str,
    ) -> StorageResult<()> {
        if source_member_id == target_member_id {
            return Ok(());
        }
        let rows: Vec<(String, Option<String>)> = self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT thread_id, through_message_id FROM projection_thread_reads WHERE member_id = ?1",
            )?;
            let iter = stmt.query_map(params![source_member_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(iter.collect::<Result<Vec<_>, _>>()?)
        })?;
        for (tid, cursor) in rows {
            if self.stored_cursor(&tid, target_member_id)?.is_none() {
                self.save_cursor(&tid, target_member_id, cursor.as_deref(), "initialized")?;
            }
        }
        Ok(())
    }

    fn with_supported_cursor(
        &self,
        thread_id: &str,
        mut state: ConversationReadState,
        exclusions: ConversationMarkerExclusions,
    ) -> StorageResult<ConversationReadState> {
        state.through_message_id = self.database.with_conn(|conn| {
            supported_conversation_cursor_conn(
                conn,
                thread_id,
                state.through_message_id.as_deref(),
                exclusions,
            )
        })?;
        Ok(state)
    }

    fn stored_cursor(&self, thread_id: &str, member_id: &str) -> StorageResult<Option<Option<String>>> {
        self.database.with_conn(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT through_message_id FROM projection_thread_reads
                     WHERE thread_id = ?1 AND member_id = ?2",
                )?
                .query_row(params![thread_id, member_id], |r| r.get::<_, Option<String>>(0))
                .optional()?)
        })
    }

    fn baseline_cursor(&self, thread_id: &str) -> StorageResult<Option<String>> {
        self.database.with_conn(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT through_message_id FROM projection_thread_read_baselines WHERE thread_id = ?1",
                )?
                .query_row(params![thread_id], |r| r.get::<_, Option<String>>(0))
                .optional()?
                .flatten())
        })
    }

    fn message_exists(&self, thread_id: &str, message_id: &str) -> StorageResult<bool> {
        self.database.with_conn(|conn| {
            let found: Option<i64> = conn
                .prepare_cached(
                    "SELECT 1 FROM projection_thread_messages
                     WHERE thread_id = ?1 AND message_id = ?2 LIMIT 1",
                )?
                .query_row(params![thread_id, message_id], |r| r.get(0))
                .optional()?;
            Ok(found.is_some())
        })
    }

    fn latest_message_id(&self, thread_id: &str) -> StorageResult<Option<String>> {
        self.database.with_conn(|conn| {
            let sql = format!(
                "{ORDERED_THREAD_MESSAGES_CTE}
                 SELECT message_id FROM ordered ORDER BY {ORDER_KEY_DESC} LIMIT 1"
            );
            Ok(conn
                .prepare_cached(&sql)?
                .query_row(params![thread_id], |r| r.get(0))
                .optional()?)
        })
    }

    fn is_after(
        &self,
        thread_id: &str,
        candidate_id: &str,
        boundary: Option<&OrderKeyTuple>,
    ) -> StorageResult<bool> {
        let Some(k) = boundary else {
            return Ok(false);
        };
        self.database.with_conn(|conn| {
            let sql = format!(
                "{ORDERED_THREAD_MESSAGES_CTE}
                 SELECT 1 FROM ordered
                 WHERE message_id = ? AND ({ORDER_KEY_COLUMNS}) > (?, ?, ?, ?, ?, ?, ?)"
            );
            let found: Option<i64> = conn
                .prepare_cached(&sql)?
                .query_row(
                    params![thread_id, candidate_id, k.0, k.1, k.2, k.3, k.4, k.5, k.6],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
    }

    fn message_before(&self, thread_id: &str, k: &OrderKeyTuple) -> StorageResult<Option<String>> {
        self.database.with_conn(|conn| {
            let sql = format!(
                "{ORDERED_THREAD_MESSAGES_CTE}
                 SELECT message_id FROM ordered
                 WHERE ({ORDER_KEY_COLUMNS}) < (?, ?, ?, ?, ?, ?, ?)
                 ORDER BY {ORDER_KEY_DESC} LIMIT 1"
            );
            Ok(conn
                .prepare_cached(&sql)?
                .query_row(
                    params![thread_id, k.0, k.1, k.2, k.3, k.4, k.5, k.6],
                    |r| r.get(0),
                )
                .optional()?)
        })
    }

    fn state_from_database(
        &self,
        thread_id: &str,
        through_message_id: Option<&str>,
    ) -> StorageResult<ConversationReadState> {
        self.database.with_conn(|conn| {
            let boundary = match through_message_id {
                Some(mid) => message_order_key_conn(conn, thread_id, mid)?,
                None => None,
            };
            let filter = unread_sql_filter();
            let row_opt: Option<(String, i64)> = if let Some(k) = boundary {
                let sql = format!(
                    "{ORDERED_THREAD_MESSAGES_CTE}
                     SELECT message_id, COUNT(*) OVER () AS unread_count FROM ordered
                     WHERE ({ORDER_KEY_COLUMNS}) > (?, ?, ?, ?, ?, ?, ?) AND {filter}
                     ORDER BY {ORDER_KEY_COLUMNS} LIMIT 1"
                );
                conn.prepare_cached(&sql)?
                    .query_row(
                        params![thread_id, k.0, k.1, k.2, k.3, k.4, k.5, k.6],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?
            } else {
                let sql = format!(
                    "{ORDERED_THREAD_MESSAGES_CTE}
                     SELECT message_id, COUNT(*) OVER () AS unread_count FROM ordered
                     WHERE {filter}
                     ORDER BY {ORDER_KEY_COLUMNS} LIMIT 1"
                );
                conn.prepare_cached(&sql)?
                    .query_row(params![thread_id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?
            };
            match row_opt {
                Some((first_id, cnt)) => Ok(ConversationReadState {
                    unread_count: cnt.max(0) as u64,
                    first_unread_message_id: Some(first_id),
                    through_message_id: through_message_id.map(str::to_string),
                }),
                None => Ok(ConversationReadState {
                    unread_count: 0,
                    first_unread_message_id: None,
                    through_message_id: through_message_id.map(str::to_string),
                }),
            }
        })
    }

    fn save_cursor(
        &self,
        thread_id: &str,
        member_id: &str,
        through_message_id: Option<&str>,
        event: &str,
    ) -> StorageResult<()> {
        if let Some(existing) = self.stored_cursor(thread_id, member_id)?
            && existing.as_deref() == through_message_id
        {
            return Ok(());
        }
        let updated_at = now_iso();
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread-read".to_string(),
            aggregate_id: format!("{thread_id}:{member_id}"),
            event_type: format!("thread-read.{event}"),
            occurred_at: updated_at.clone(),
            payload: serde_json::json!({
                "threadId": thread_id,
                "memberId": member_id,
                "throughMessageId": through_message_id,
            }),
        }];
        let _ = self.database.dispatch_with_projection(
            &format!("thread-read:{event}:{thread_id}:{member_id}:{}", Uuid::new_v4()),
            None,
            &events,
            |conn, _seq| {
                conn.execute(
                    "INSERT INTO projection_thread_reads
                       (thread_id, member_id, through_message_id, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(thread_id, member_id) DO UPDATE SET
                       through_message_id = excluded.through_message_id,
                       updated_at = excluded.updated_at",
                    params![thread_id, member_id, through_message_id, updated_at],
                )?;
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(())
    }
}

fn empty_read_state() -> ConversationReadState {
    ConversationReadState {
        unread_count: 0,
        first_unread_message_id: None,
        through_message_id: None,
    }
}

#[derive(Default)]
struct StreamedEventCache {
    order: Vec<(String, String, String)>,
    map: HashMap<(String, String, String), i64>,
}

impl StreamedEventCache {
    fn get(&self, key: &(String, String, String)) -> Option<i64> {
        self.map.get(key).copied()
    }

    fn set(&mut self, key: (String, String, String), sequence: i64) {
        if self.map.insert(key.clone(), sequence).is_some() {
            self.order.retain(|k| k != &key);
        }
        self.order.push(key);
        if self.order.len() > REMEMBERED_STREAMED_EVENTS {
            let oldest = self.order.remove(0);
            self.map.remove(&oldest);
        }
    }
}

/// Unified `ThreadStore` facade over `projection_threads`, `projection_thread_messages`,
/// `projection_deleted_thread_messages`, `projection_thread_activities`, `projection_turns`,
/// `projection_thread_summaries`, `AgentThreadStore`, `MessagePinStore`, `ProviderSessionStore`,
/// `AgentWaitStore`, `ConversationReadStore`, and `ThreadReplay`.
#[derive(Clone)]
pub struct ThreadStore {
    database: OpenBotDatabase,
    roster: AgentRoster,
    agent_threads: AgentThreadStore,
    pins: MessagePinStore,
    provider_sessions: ProviderSessionStore,
    waits: AgentWaitStore,
    reads: ConversationReadStore,
    streamed_events: Arc<Mutex<StreamedEventCache>>,
}

impl ThreadStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self {
            roster: AgentRoster::new(database.clone()),
            agent_threads: AgentThreadStore::new(database.clone()),
            pins: MessagePinStore::new(database.clone()),
            provider_sessions: ProviderSessionStore::new(database.clone()),
            waits: AgentWaitStore::new(database.clone()),
            reads: ConversationReadStore::new(database.clone()),
            database,
            streamed_events: Arc::new(Mutex::new(StreamedEventCache::default())),
        }
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    pub fn agent_threads(&self) -> &AgentThreadStore {
        &self.agent_threads
    }

    pub fn pins(&self) -> &MessagePinStore {
        &self.pins
    }

    pub fn provider_sessions(&self) -> &ProviderSessionStore {
        &self.provider_sessions
    }

    pub fn waits(&self) -> &AgentWaitStore {
        &self.waits
    }

    pub fn reads(&self) -> &ConversationReadStore {
        &self.reads
    }

    pub fn unclaimed_threads(&self) -> StorageResult<Vec<UnclaimedThread>> {
        self.roster.unclaimed_threads()
    }

    pub fn read_conversation(
        &self,
        agent_id: &str,
        thread_id: Option<&str>,
    ) -> StorageResult<ConversationSnapshot> {
        let Some(tid) = thread_id else {
            return Ok(ConversationSnapshot {
                agent_id: agent_id.to_string(),
                thread_id: None,
                active_turn_id: None,
                revision: 0,
                messages: Vec::new(),
            });
        };
        self.database.with_conn(|conn| {
            let thread_opt: Option<(Option<String>, i64)> = conn
                .prepare_cached(
                    "SELECT active_turn_id, last_event_sequence
                     FROM projection_threads WHERE thread_id = ?1 AND agent_id = ?2",
                )?
                .query_row(params![tid, agent_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some((active_turn_id, rev)) = thread_opt else {
                return Ok(ConversationSnapshot {
                    agent_id: agent_id.to_string(),
                    thread_id: Some(tid.to_string()),
                    active_turn_id: None,
                    revision: 0,
                    messages: Vec::new(),
                });
            };
            let mut stmt = conn.prepare_cached(
                "SELECT message_json FROM projection_thread_messages
                 WHERE thread_id = ?1 ORDER BY created_at, ordinal, message_id",
            )?;
            let rows = stmt.query_map(params![tid], |r| r.get::<_, String>(0))?;
            let mut messages = Vec::new();
            for raw in rows {
                messages.push(serde_json::from_str(&raw?)?);
            }
            sort_conversation_messages(&mut messages);
            Ok(ConversationSnapshot {
                agent_id: agent_id.to_string(),
                thread_id: Some(tid.to_string()),
                active_turn_id,
                revision: rev.max(0) as u64,
                messages,
            })
        })
    }

    pub fn read_turn_conversation(
        &self,
        agent_id: &str,
        thread_id: &str,
        turn_id: &str,
    ) -> StorageResult<ConversationSnapshot> {
        self.database.with_conn(|conn| {
            let thread_opt: Option<(Option<String>, i64)> = conn
                .prepare_cached(
                    "SELECT active_turn_id, last_event_sequence
                     FROM projection_threads WHERE thread_id = ?1 AND agent_id = ?2",
                )?
                .query_row(params![thread_id, agent_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some((active_turn_id, rev)) = thread_opt else {
                return Ok(ConversationSnapshot {
                    agent_id: agent_id.to_string(),
                    thread_id: Some(thread_id.to_string()),
                    active_turn_id: None,
                    revision: 0,
                    messages: Vec::new(),
                });
            };
            let mut stmt = conn.prepare_cached(
                "SELECT message_json FROM projection_thread_messages
                 WHERE thread_id = ?1 AND turn_id = ?2 ORDER BY created_at, ordinal, message_id",
            )?;
            let rows = stmt.query_map(params![thread_id, turn_id], |r| r.get::<_, String>(0))?;
            let mut messages = Vec::new();
            for raw in rows {
                messages.push(serde_json::from_str(&raw?)?);
            }
            sort_conversation_messages(&mut messages);
            Ok(ConversationSnapshot {
                agent_id: agent_id.to_string(),
                thread_id: Some(thread_id.to_string()),
                active_turn_id,
                revision: rev.max(0) as u64,
                messages,
            })
        })
    }

    pub fn read_latest_turn_id(&self, turn_ids: &[String]) -> StorageResult<Option<String>> {
        if turn_ids.is_empty() {
            return Ok(None);
        }
        let ids_json = serde_json::to_string(turn_ids)?;
        self.database.with_conn(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT turn_id FROM projection_turns
                     WHERE turn_id IN (SELECT value FROM json_each(?1))
                     ORDER BY started_at DESC, last_event_sequence DESC LIMIT 1",
                )?
                .query_row(params![ids_json], |r| r.get(0))
                .optional()?)
        })
    }

    pub fn read_active_turn_id(
        &self,
        agent_id: &str,
        thread_id: Option<&str>,
    ) -> StorageResult<Option<String>> {
        let Some(tid) = thread_id else {
            return Ok(None);
        };
        self.database.with_conn(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT active_turn_id FROM projection_threads
                     WHERE thread_id = ?1 AND agent_id = ?2",
                )?
                .query_row(params![tid, agent_id], |r| r.get::<_, Option<String>>(0))
                .optional()?
                .flatten())
        })
    }

    pub fn read_running_turn_ids(&self) -> StorageResult<HashMap<String, Vec<String>>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT thread_id, turn_id FROM projection_turns WHERE status = 'running'",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let mut map: HashMap<String, Vec<String>> = HashMap::new();
            for row in rows {
                let (tid, turn_id) = row?;
                map.entry(tid).or_default().push(turn_id);
            }
            Ok(map)
        })
    }

    pub fn read_conversation_revision(
        &self,
        agent_id: &str,
        thread_id: Option<&str>,
    ) -> StorageResult<u64> {
        let Some(tid) = thread_id else {
            return Ok(0);
        };
        self.database.with_conn(|conn| {
            let rev: Option<i64> = conn
                .prepare_cached(
                    "SELECT last_event_sequence FROM projection_threads
                     WHERE thread_id = ?1 AND agent_id = ?2",
                )?
                .query_row(params![tid, agent_id], |r| r.get(0))
                .optional()?;
            Ok(rev.unwrap_or(0).max(0) as u64)
        })
    }

    pub fn read_sent_message_stats(&self) -> StorageResult<SentMessageStats> {
        self.database.with_conn(|conn| {
            let (first_sent_at, cnt): (Option<String>, i64) = conn
                .prepare_cached(
                    "SELECT MIN(created_at), COUNT(*) FROM projection_thread_messages
                     WHERE author = 'user' AND item_type IS NOT 'routine'",
                )?
                .query_row([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(SentMessageStats {
                first_sent_at,
                sent_count: cnt.max(0) as u64,
            })
        })
    }

    pub fn read_conversation_messages(
        &self,
        agent_id: &str,
        thread_id: Option<&str>,
        message_ids: &[String],
    ) -> StorageResult<Vec<ConversationMessage>> {
        let Some(tid) = thread_id else {
            return Ok(Vec::new());
        };
        if message_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids_json = serde_json::to_string(message_ids)?;
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT message_json FROM projection_thread_messages
                 WHERE thread_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))
                   AND EXISTS (
                     SELECT 1 FROM projection_threads AS owner
                     WHERE owner.thread_id = projection_thread_messages.thread_id
                       AND owner.agent_id = ?3
                   )
                 ORDER BY created_at, ordinal, message_id",
            )?;
            let rows = stmt.query_map(params![tid, ids_json, agent_id], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for raw in rows {
                out.push(serde_json::from_str(&raw?)?);
            }
            Ok(out)
        })
    }

    pub fn read_conversation_messages_of_item_type(
        &self,
        agent_id: &str,
        thread_id: &str,
        item_type: &str,
    ) -> StorageResult<Vec<ConversationMessage>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT message_json FROM projection_thread_messages
                 WHERE thread_id = ?1 AND item_type = ?2
                   AND EXISTS (
                     SELECT 1 FROM projection_threads AS owner
                     WHERE owner.thread_id = projection_thread_messages.thread_id
                       AND owner.agent_id = ?3
                   )
                 ORDER BY created_at, ordinal, message_id",
            )?;
            let rows =
                stmt.query_map(params![thread_id, item_type, agent_id], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for raw in rows {
                out.push(serde_json::from_str(&raw?)?);
            }
            Ok(out)
        })
    }

    pub fn read_conversation_recovery_messages(
        &self,
        agent_id: &str,
        thread_id: Option<&str>,
        active_turn_id: Option<&str>,
    ) -> StorageResult<Vec<ConversationMessage>> {
        let Some(tid) = thread_id else {
            return Ok(Vec::new());
        };
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT message_json FROM projection_thread_messages
                 WHERE thread_id = ?1
                   AND EXISTS (
                     SELECT 1 FROM projection_threads AS owner
                     WHERE owner.thread_id = projection_thread_messages.thread_id
                       AND owner.agent_id = ?2
                   )
                   AND (
                     (?3 IS NOT NULL AND turn_id = ?3 AND status = 'streaming')
                     OR (json_type(message_json, '$.questionPrompt.resolution') = 'null'
                         AND json_type(message_json, '$.questionPrompt') = 'object')
                     OR (json_extract(message_json, '$.itemType') = 'subagents'
                         AND EXISTS (
                           SELECT 1 FROM json_each(message_json, '$.subagents')
                           WHERE json_extract(value, '$.status') = 'running'
                         ))
                   )
                 ORDER BY created_at, ordinal, message_id",
            )?;
            let rows = stmt.query_map(params![tid, agent_id, active_turn_id], |r| {
                r.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for raw in rows {
                out.push(serde_json::from_str(&raw?)?);
            }
            Ok(out)
        })
    }

    pub fn read_conversation_page(
        &self,
        agent_id: &str,
        thread_id: Option<&str>,
        anchor: &ConversationPageAnchor,
        requested_limit: usize,
        exclusions: ConversationMarkerExclusions,
    ) -> StorageResult<ConversationPage> {
        if requested_limit == 0 {
            return Err(StorageError::Validation(
                "The conversation page limit is invalid.".to_string(),
            ));
        }
        let limit = requested_limit.min(PAGE_MESSAGE_LIMIT);
        let empty = ConversationPage {
            agent_id: agent_id.to_string(),
            thread_id: thread_id.map(str::to_string),
            active_turn_id: None,
            revision: 0,
            messages: Vec::new(),
            references: BTreeMap::new(),
            page_info: ConversationPageInfo {
                has_older: false,
                older_cursor: None,
                older_count: None,
                oldest_at: None,
            },
        };
        let Some(tid) = thread_id else {
            return Ok(empty);
        };
        self.database.with_conn(|conn| {
            let thread_opt: Option<(Option<String>, i64)> = conn
                .prepare_cached(
                    "SELECT active_turn_id, last_event_sequence
                     FROM projection_threads WHERE thread_id = ?1 AND agent_id = ?2",
                )?
                .query_row(params![tid, agent_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some((active_turn_id, rev)) = thread_opt else {
                return Ok(empty);
            };
            let keys = query_conversation_page_keys(conn, tid, anchor, limit, exclusions)?;
            let mut messages = Vec::with_capacity(keys.len());
            if !keys.is_empty() {
                let ids: Vec<String> = keys.iter().map(|k| k.6.clone()).collect();
                let ids_json = serde_json::to_string(&ids)?;
                let mut stmt = conn.prepare_cached(
                    "SELECT message_id, message_json FROM projection_thread_messages
                     WHERE thread_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
                )?;
                let rows = stmt.query_map(params![tid, ids_json], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                let mut json_map = HashMap::new();
                for row in rows {
                    let (mid, mj) = row?;
                    json_map.insert(mid, mj);
                }
                for key in &keys {
                    if let Some(raw) = json_map.get(&key.6) {
                        messages.push(serde_json::from_str::<ConversationMessage>(raw)?);
                    }
                }
            }
            let page_ids: HashSet<&str> = messages.iter().map(|m| m.id.as_str()).collect();
            let ref_ids: Vec<String> = messages
                .iter()
                .filter_map(|m| m.reply_to_message_id.as_ref().and_then(|o| o.as_deref()))
                .filter(|rid| !page_ids.contains(rid))
                .map(str::to_string)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            let mut references = BTreeMap::new();
            if !ref_ids.is_empty() {
                let ref_json = serde_json::to_string(&ref_ids)?;
                let filter = conversation_marker_sql_filter(exclusions);
                let sql = format!(
                    "SELECT message_id, message_json FROM projection_thread_messages
                     WHERE thread_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))
                     {filter}"
                );
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map(params![tid, ref_json], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                for row in rows {
                    let (mid, mj) = row?;
                    references.insert(mid, serde_json::from_str(&mj)?);
                }
            }
            let (older_count, oldest_at) = match keys.first() {
                Some(first) => count_conversation_rows_before(conn, tid, first, exclusions)?,
                None => (0, None),
            };
            let has_older = older_count > 0;
            Ok(ConversationPage {
                agent_id: agent_id.to_string(),
                thread_id: Some(tid.to_string()),
                active_turn_id,
                revision: rev.max(0) as u64,
                messages,
                references,
                page_info: ConversationPageInfo {
                    has_older,
                    older_cursor: (has_older && !keys.is_empty())
                        .then(|| encode_page_cursor(&keys[0]))
                        .transpose()?,
                    older_count: has_older.then_some(older_count),
                    oldest_at,
                },
            })
        })
    }

    pub fn search_conversation_messages(
        &self,
        query: &str,
        agent_id: Option<&str>,
        cursor: Option<&str>,
        requested_limit: usize,
        include_side_threads: bool,
    ) -> StorageResult<ConversationSearchPage> {
        let normalized = collapse_whitespace_lower(query);
        if normalized.is_empty() {
            return Ok(ConversationSearchPage {
                results: Vec::new(),
                total: 0,
                next_cursor: None,
            });
        }
        if requested_limit == 0 {
            return Err(StorageError::Validation(
                "The conversation page limit is invalid.".to_string(),
            ));
        }
        let limit = requested_limit.min(PAGE_MESSAGE_LIMIT);
        let offset = match cursor {
            Some(c) => decode_search_cursor(c)?,
            None => 0,
        };
        let pattern = format!("%{}%", escape_like(&normalized));
        let side_excl = if include_side_threads {
            "AND message.thread_id NOT IN (SELECT thread_id FROM projection_channel_contexts)
             AND message.thread_id NOT IN (SELECT thread_id FROM projection_agent_threads WHERE archived_at IS NOT NULL)"
        } else {
            "AND message.thread_id NOT IN (SELECT thread_id FROM projection_channel_contexts)
             AND message.thread_id NOT IN (SELECT thread_id FROM projection_agent_threads)"
        };
        let sql = format!(
            "WITH matches AS MATERIALIZED (
               SELECT message.rowid AS message_rowid, thread.agent_id, message.created_at,
                      message.ordinal, message.message_id
               FROM projection_thread_messages message
               JOIN projection_threads thread ON thread.thread_id = message.thread_id
               WHERE LOWER(openbot_collapse_whitespace(json_extract(message.message_json, '$.text'))) LIKE ?1 ESCAPE '\\'
                 AND COALESCE(json_extract(message.message_json, '$.delivery.status'), '') NOT IN ('queued', 'cancelled')
                 AND COALESCE(message.item_type, '') NOT IN ('commentary', '{ROUTINE_QUIET_ANSWER_ITEM_TYPE}', '{AGENT_WELCOME_ITEM_TYPE}', '{CHANNEL_RESUME_ITEM_TYPE}')
                 AND COALESCE(message.item_type, '') NOT LIKE '{SKILL_SUGGESTION_ITEM_PREFIX}%'
                 AND COALESCE(message.item_type, '') NOT LIKE 'skill-event:%'
                 AND COALESCE(message.item_type, '') NOT LIKE '{ROUTINE_EVENT_ITEM_PREFIX}%'
                 AND COALESCE(message.item_type, '') NOT LIKE '{ROUTINE_RUN_EVENT_ITEM_PREFIX}%'
                 AND COALESCE(message.item_type, '') NOT LIKE '{HOSTED_SITE_EVENT_ITEM_PREFIX}%'
                 AND COALESCE(message.item_type, '') NOT LIKE 'marketplace-suggestion:%'
                 AND COALESCE(message.item_type, '') NOT LIKE 'setting-link:%'
                 AND json_extract(message.message_json, '$.routine') IS NULL
                 {side_excl}
                 AND (?2 IS NULL OR thread.agent_id = ?2)
             )
             SELECT counted.count, page.agent_id, message.message_json,
                    COALESCE(side.legacy_task_id, side.thread_id)
             FROM (SELECT COUNT(*) AS count FROM matches) counted
             LEFT JOIN (
               SELECT * FROM matches
               ORDER BY created_at DESC, ordinal DESC, message_id DESC
               LIMIT ?3 OFFSET ?4
             ) page
             LEFT JOIN projection_thread_messages message ON message.rowid = page.message_rowid
             LEFT JOIN projection_agent_threads side ON side.thread_id = message.thread_id
             ORDER BY page.created_at DESC, page.ordinal DESC, page.message_id DESC"
        );
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                params![pattern, agent_id, limit as i64, offset as i64],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )?;
            let mut total = 0u64;
            let mut results = Vec::new();
            for row in rows {
                let (cnt, aid_opt, mj_opt, task_id) = row?;
                total = cnt.max(0) as u64;
                if let (Some(aid), Some(mj)) = (aid_opt, mj_opt) {
                    results.push(ConversationSearchMatch {
                        agent_id: aid,
                        message: serde_json::from_str(&mj)?,
                        task_id,
                    });
                }
            }
            let next_offset = offset + results.len();
            let next_cursor = ((next_offset as u64) < total)
                .then(|| encode_search_cursor(next_offset))
                .transpose()?;
            Ok(ConversationSearchPage {
                results,
                total,
                next_cursor,
            })
        })
    }

    pub fn search_conversation_files(
        &self,
        query: &str,
        cursor: Option<&str>,
        requested_limit: usize,
    ) -> StorageResult<ConversationFileSearchPage> {
        if requested_limit == 0 {
            return Err(StorageError::Validation(
                "The conversation page limit is invalid.".to_string(),
            ));
        }
        let limit = requested_limit.min(PAGE_MESSAGE_LIMIT);
        let offset = match cursor {
            Some(c) => decode_search_cursor(c)?,
            None => 0,
        };
        let normalized = collapse_whitespace_lower(query);
        let pattern = (!normalized.is_empty()).then(|| format!("%{}%", escape_like(&normalized)));
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT thread.agent_id, message.message_id, attachment.created_at, attachment.metadata_json
                 FROM projection_attachments attachment
                 JOIN projection_threads thread
                   ON thread.thread_id = substr(attachment.owner_id, 1, instr(attachment.owner_id, ':') - 1)
                 JOIN projection_thread_messages message
                   ON message.thread_id = thread.thread_id
                  AND message.message_id = substr(attachment.owner_id, instr(attachment.owner_id, ':') + 1)
                 WHERE attachment.owner_kind = 'thread-message'
                   AND instr(attachment.owner_id, ':') > 1
                   AND COALESCE(json_extract(message.message_json, '$.delivery.status'), '') NOT IN ('queued', 'cancelled')
                   AND (?1 IS NULL OR openbot_lowercase(openbot_collapse_whitespace(attachment.name)) LIKE ?1 ESCAPE '\\')
                   AND message.thread_id NOT IN (SELECT thread_id FROM projection_channel_contexts)
                   AND message.thread_id NOT IN (SELECT thread_id FROM projection_agent_threads)
                 ORDER BY attachment.created_at DESC, attachment.attachment_id DESC
                 LIMIT ?2 OFFSET ?3",
            )?;
            let rows = stmt.query_map(
                params![pattern, (limit + 1) as i64, offset as i64],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )?;
            let collected: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
            let has_more = collected.len() > limit;
            let mut results = Vec::new();
            for (aid, mid, cat, meta_json) in collected.into_iter().take(limit) {
                if let Ok(att) = serde_json::from_str::<AttachmentSummary>(&meta_json)
                    && att.is_valid()
                {
                    results.push(ConversationFileSearchResult {
                        agent_id: aid,
                        message_id: mid,
                        created_at: cat,
                        attachment: att,
                    });
                }
            }
            let next_cursor = has_more
                .then(|| encode_search_cursor(offset + limit))
                .transpose()?;
            Ok(ConversationFileSearchPage {
                results,
                next_cursor,
            })
        })
    }

    pub fn save_thread_summary(
        &self,
        thread_id: &str,
        through_message_id: Option<&str>,
        text: &str,
        estimated_tokens: i64,
    ) -> StorageResult<StoredThreadSummary> {
        let summary = StoredThreadSummary {
            id: Uuid::new_v4().to_string(),
            thread_id: thread_id.to_string(),
            through_message_id: through_message_id.map(str::to_string),
            text: text.to_string(),
            estimated_tokens,
            created_at: now_iso(),
        };
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread".to_string(),
            aggregate_id: thread_id.to_string(),
            event_type: "thread.summary-created".to_string(),
            occurred_at: summary.created_at.clone(),
            payload: serde_json::to_value(&summary)?,
        }];
        let receipt = self.database.dispatch_with_projection(
            &format!("thread-summary:{}", summary.id),
            None,
            &events,
            |conn, sequences| {
                conn.execute(
                    "INSERT INTO projection_thread_summaries
                       (summary_id, thread_id, through_message_id, summary_text, estimated_tokens, created_at, last_event_sequence)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        summary.id,
                        summary.thread_id,
                        summary.through_message_id,
                        summary.text,
                        summary.estimated_tokens,
                        summary.created_at,
                        sequences.first().copied().unwrap_or(0)
                    ],
                )?;
                Ok(summary.clone())
            },
        )?;
        Ok(receipt.result)
    }

    pub fn latest_thread_summary(
        &self,
        thread_id: &str,
    ) -> StorageResult<Option<StoredThreadSummary>> {
        self.database.with_conn(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT summary_id, thread_id, through_message_id, summary_text, estimated_tokens, created_at
                     FROM projection_thread_summaries WHERE thread_id = ?1 ORDER BY created_at DESC LIMIT 1",
                )?
                .query_row(params![thread_id], |r| {
                    Ok(StoredThreadSummary {
                        id: r.get(0)?,
                        thread_id: r.get(1)?,
                        through_message_id: r.get(2)?,
                        text: r.get(3)?,
                        estimated_tokens: r.get(4)?,
                        created_at: r.get(5)?,
                    })
                })
                .optional()?)
        })
    }

    /// Full conversation snapshot write with 0-based `ordinal`, tombstone exclusion,
    /// `refresh_thread_order`, and `prune_conversation_snapshots` (`conversation-writer.ts:L55-L192`).
    pub fn persist_conversation(
        &self,
        written: &ConversationSnapshot,
        event_type: &str,
        detail: &serde_json::Value,
        command_id: Option<&str>,
    ) -> StorageResult<ConversationSnapshot> {
        let Some(thread_id) = written.thread_id.clone() else {
            return Ok(written.clone());
        };
        let (filtered_messages, recovery) = self.database.with_conn(|conn| {
            let msgs = without_deleted_messages(conn, &thread_id, &written.messages)?;
            let rec = conversation_recovery_state(conn, &thread_id, written.active_turn_id.as_deref())?;
            Ok((msgs, rec))
        })?;
        let mut snapshot = written.clone();
        snapshot.messages = filtered_messages;

        let resolved_cmd = command_id
            .map(str::to_string)
            .unwrap_or_else(|| format!("conversation:{event_type}:{}", Uuid::new_v4()));
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread".to_string(),
            aggregate_id: thread_id.clone(),
            event_type: event_type.to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({
                "detail": detail,
                "snapshot": snapshot,
                "recovery": recovery,
            }),
        }];
        let agent = self
            .roster
            .get_agent(&snapshot.agent_id)?
            .ok_or_else(|| {
                StorageError::NotFound(format!("Unknown agent for conversation: {}", snapshot.agent_id))
            })?;
        let receipt = self.database.dispatch_with_projection(
            &resolved_cmd,
            None,
            &events,
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(snapshot.revision as i64);
                let exists: Option<i64> = conn
                    .prepare_cached("SELECT 1 FROM projection_threads WHERE thread_id = ?1")?
                    .query_row(params![thread_id], |r| r.get(0))
                    .optional()?;
                if exists.is_none() {
                    ensure_thread_projection_row(conn, &agent, seq)?;
                }
                conn.execute(
                    "UPDATE projection_threads
                     SET active_turn_id = ?1, updated_at = ?2, last_event_sequence = ?3
                     WHERE thread_id = ?4",
                    params![snapshot.active_turn_id, now_iso(), seq, thread_id],
                )?;

                let keep_ids: HashSet<&str> =
                    snapshot.messages.iter().map(|m| m.id.as_str()).collect();
                for existing_id in thread_message_ids(conn, &thread_id)? {
                    if !keep_ids.contains(existing_id.as_str()) {
                        conn.execute(
                            "DELETE FROM projection_attachments
                             WHERE owner_kind = 'thread-message' AND owner_id = ?1",
                            params![format!("{thread_id}:{existing_id}")],
                        )?;
                        conn.execute(
                            "DELETE FROM projection_thread_messages
                             WHERE thread_id = ?1 AND message_id = ?2",
                            params![thread_id, existing_id],
                        )?;
                    }
                }
                for (ordinal, msg) in snapshot.messages.iter().enumerate() {
                    record_usage_message(conn, &agent.id, msg, agent.provider, &agent.model)?;
                    insert_thread_message_row(conn, &thread_id, msg, ordinal as i64, seq)?;
                }
                insert_activity_and_turn_updates(
                    conn,
                    &thread_id,
                    event_type,
                    detail,
                    snapshot.active_turn_id.as_deref(),
                    seq,
                    &Uuid::new_v4().to_string(),
                    &now_iso(),
                )?;
                refresh_thread_order(conn, &thread_id)?;
                prune_conversation_snapshots(conn, &thread_id, seq)?;
                Ok(seq.max(0) as u64)
            },
        )?;
        snapshot.revision = receipt.result;
        Ok(snapshot)
    }

    pub fn persist_streaming_message(&self, input: &StreamingMessageWrite) -> StorageResult<u64> {
        let Some(thread_id) = input.snapshot.thread_id.as_deref() else {
            return Ok(input.snapshot.revision);
        };
        if self
            .database
            .with_conn(|c| is_deleted_thread_message(c, thread_id, &input.message_id))?
        {
            return self.thread_revision(thread_id);
        }
        let Some(message) = input
            .snapshot
            .messages
            .iter()
            .find(|m| m.id == input.message_id)
        else {
            return self.thread_revision(thread_id);
        };
        let agent = self
            .roster
            .get_agent(&input.snapshot.agent_id)?
            .ok_or_else(|| {
                StorageError::NotFound(format!(
                    "Unknown agent for conversation: {}",
                    input.snapshot.agent_id
                ))
            })?;
        let resolved_cmd = input
            .command_id
            .clone()
            .unwrap_or_else(|| format!("conversation:{}:{}", input.event_type, Uuid::new_v4()));
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread".to_string(),
            aggregate_id: thread_id.to_string(),
            event_type: input.event_type.clone(),
            occurred_at: now_iso(),
            payload: serde_json::json!({
                "detail": input.detail,
                "streamedMessage": message,
                "activeTurnId": input.snapshot.active_turn_id,
            }),
        }];
        let cache_key = (
            thread_id.to_string(),
            input.event_type.clone(),
            input.message_id.clone(),
        );
        let remembered = self.streamed_events.lock().get(&cache_key);
        let receipt = self.database.dispatch_with_projection(
            &resolved_cmd,
            None,
            &events,
            |conn, sequences| {
                let seq = sequences
                    .first()
                    .copied()
                    .unwrap_or(input.snapshot.revision as i64);
                ensure_thread_exists_for_agent(conn, &agent, thread_id, seq)?;
                supersede_streamed_message_event(
                    conn,
                    thread_id,
                    &input.message_id,
                    &input.event_type,
                    seq,
                    remembered,
                )?;
                let before = read_order_inputs(conn, thread_id, std::slice::from_ref(&message.id))?;
                let ordinal = existing_or_next_ordinal(conn, thread_id, &message.id)?;
                insert_thread_message_row(conn, thread_id, message, ordinal, seq)?;
                refresh_changed_order(conn, thread_id, &before, std::slice::from_ref(&message.id))?;
                record_usage_message(conn, &agent.id, message, agent.provider, &agent.model)?;
                conn.execute(
                    "UPDATE projection_threads
                     SET active_turn_id = ?1, updated_at = ?2, last_event_sequence = ?3
                     WHERE thread_id = ?4",
                    params![input.snapshot.active_turn_id, now_iso(), seq, thread_id],
                )?;
                Ok(seq.max(0) as u64)
            },
        )?;
        self.streamed_events
            .lock()
            .set(cache_key, receipt.result as i64);
        Ok(receipt.result)
    }

    pub fn append_conversation_message(
        &self,
        input: &AppendConversationMessageInput,
    ) -> StorageResult<u64> {
        if self
            .database
            .with_conn(|c| is_deleted_thread_message(c, &input.thread_id, &input.message.id))?
        {
            return self.thread_revision(&input.thread_id);
        }
        let agent = self.roster.get_agent(&input.agent_id)?.ok_or_else(|| {
            StorageError::NotFound(format!(
                "Unknown agent thread for conversation append: {}",
                input.agent_id
            ))
        })?;
        let resolved_cmd = input
            .command_id
            .clone()
            .unwrap_or_else(|| format!("conversation:{}:{}", input.event_type, Uuid::new_v4()));
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread".to_string(),
            aggregate_id: input.thread_id.clone(),
            event_type: input.event_type.clone(),
            occurred_at: input.message.created_at.clone(),
            payload: serde_json::json!({
                "detail": input.detail,
                "appendedMessage": input.message,
                "activeTurnId": input.active_turn_id,
            }),
        }];
        let receipt = self.database.dispatch_with_projection(
            &resolved_cmd,
            None,
            &events,
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(0);
                ensure_thread_exists_for_agent(conn, &agent, &input.thread_id, seq)?;
                let ordinal = next_ordinal_conn(conn, &input.thread_id)?;
                insert_thread_message_row(conn, &input.thread_id, &input.message, ordinal, seq)?;
                refresh_changed_order(
                    conn,
                    &input.thread_id,
                    &HashMap::new(),
                    std::slice::from_ref(&input.message.id),
                )?;
                record_usage_message(
                    conn,
                    &agent.id,
                    &input.message,
                    agent.provider,
                    &agent.model,
                )?;
                conn.execute(
                    "UPDATE projection_threads
                     SET active_turn_id = ?1, updated_at = ?2, last_event_sequence = ?3
                     WHERE thread_id = ?4",
                    params![
                        input.active_turn_id,
                        input.message.created_at,
                        seq,
                        input.thread_id
                    ],
                )?;
                let turn_ref = input
                    .message
                    .turn_id
                    .as_deref()
                    .or(input.active_turn_id.as_deref());
                conn.execute(
                    "INSERT INTO projection_thread_activities
                       (activity_id, thread_id, turn_id, activity_type, payload_json, created_at, last_event_sequence)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        Uuid::new_v4().to_string(),
                        input.thread_id,
                        turn_ref,
                        input.event_type,
                        serde_json::to_string(&input.detail)?,
                        input.message.created_at,
                        seq
                    ],
                )?;
                Ok(seq.max(0) as u64)
            },
        )?;
        Ok(receipt.result)
    }

    pub fn changed_conversation_messages(
        &self,
        thread_id: &str,
        written: &[ConversationMessage],
    ) -> StorageResult<Vec<ConversationMessage>> {
        self.database.with_conn(|conn| {
            let messages = without_deleted_messages(conn, thread_id, written)?;
            if messages.is_empty() {
                return Ok(Vec::new());
            }
            let ids: Vec<String> = messages.iter().map(|m| m.id.clone()).collect();
            let ids_json = serde_json::to_string(&ids)?;
            let mut stmt = conn.prepare_cached(
                "SELECT message_id, message_json FROM projection_thread_messages
                 WHERE thread_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
            )?;
            let rows = stmt.query_map(params![thread_id, ids_json], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut stored = HashMap::new();
            for row in rows {
                let (mid, mj) = row?;
                stored.insert(mid, serde_json::from_str::<ConversationMessage>(&mj)?);
            }
            Ok(messages
                .into_iter()
                .filter(|msg| match stored.get(&msg.id) {
                    None => true,
                    Some(existing) => {
                        without_queue_positions(&merge_live_message(existing, msg))
                            != without_queue_positions(existing)
                    }
                })
                .collect())
        })
    }

    pub fn persist_conversation_changes(
        &self,
        input: &PersistConversationChangesInput,
    ) -> StorageResult<u64> {
        let mut removed_set: HashSet<String> = input.removed_message_ids.iter().cloned().collect();
        for did in &input.deleted_message_ids {
            removed_set.insert(did.clone());
        }
        let mut removed_ids: Vec<String> = removed_set.into_iter().collect();
        removed_ids.sort();

        let mut deleted = self
            .database
            .with_conn(|c| deleted_thread_message_ids(c, &input.thread_id))?;
        for did in &input.deleted_message_ids {
            deleted.insert(did.clone());
        }
        let changed_messages: Vec<ConversationMessage> = input
            .changed_messages
            .iter()
            .filter(|m| !deleted.contains(&m.id))
            .cloned()
            .collect();

        let agent = self.roster.get_agent(&input.agent_id)?.ok_or_else(|| {
            StorageError::NotFound(format!(
                "Unknown agent for conversation changes: {}",
                input.agent_id
            ))
        })?;
        let auto_cmd = input.command_id.is_none();
        let resolved_cmd = input
            .command_id
            .clone()
            .unwrap_or_else(|| format!("conversation:{}:{}", input.event_type, Uuid::new_v4()));
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread".to_string(),
            aggregate_id: input.thread_id.clone(),
            event_type: input.event_type.clone(),
            occurred_at: now_iso(),
            payload: serde_json::json!({
                "detail": input.detail,
                "changedMessages": changed_messages,
                "removedMessageIds": removed_ids,
                "activeTurnId": input.active_turn_id,
            }),
        }];
        let receipt = self.database.dispatch_with_projection(
            &resolved_cmd,
            None,
            &events,
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(0);
                ensure_thread_exists_for_agent(conn, &agent, &input.thread_id, seq)?;
                let mut written_ids: Vec<String> =
                    changed_messages.iter().map(|m| m.id.clone()).collect();
                written_ids.extend(removed_ids.iter().cloned());
                let before = read_order_inputs(conn, &input.thread_id, &written_ids)?;

                let mut append_ordinal: Option<i64> = None;
                for imported in &changed_messages {
                    let existing_row: Option<(i64, String)> = conn
                        .prepare_cached(
                            "SELECT ordinal, message_json FROM projection_thread_messages
                             WHERE thread_id = ?1 AND message_id = ?2",
                        )?
                        .query_row(params![input.thread_id, imported.id], |r| {
                            Ok((r.get(0)?, r.get(1)?))
                        })
                        .optional()?;
                    let (ordinal, merged) = match existing_row {
                        Some((ord, raw)) => {
                            let existing: ConversationMessage = serde_json::from_str(&raw)?;
                            (ord, merge_live_message(&existing, imported))
                        }
                        None => {
                            let base = match append_ordinal {
                                Some(v) => v,
                                None => next_ordinal_conn(conn, &input.thread_id)?,
                            };
                            append_ordinal = Some(base + 1);
                            (base, imported.clone())
                        }
                    };
                    conn.execute(
                        "DELETE FROM projection_attachments
                         WHERE owner_kind = 'thread-message' AND owner_id = ?1",
                        params![format!("{}:{}", input.thread_id, merged.id)],
                    )?;
                    insert_thread_message_row(conn, &input.thread_id, &merged, ordinal, seq)?;
                    record_usage_message(conn, &agent.id, &merged, agent.provider, &agent.model)?;
                }

                let deleted_at = now_iso();
                for did in &input.deleted_message_ids {
                    record_deleted_thread_message(conn, &input.thread_id, did, &deleted_at)?;
                }
                for rid in &removed_ids {
                    conn.execute(
                        "DELETE FROM projection_attachments
                         WHERE owner_kind = 'thread-message' AND owner_id = ?1",
                        params![format!("{}:{rid}", input.thread_id)],
                    )?;
                    conn.execute(
                        "DELETE FROM projection_thread_messages
                         WHERE thread_id = ?1 AND message_id = ?2",
                        params![input.thread_id, rid],
                    )?;
                }

                refresh_changed_order(conn, &input.thread_id, &before, &written_ids)?;
                conn.execute(
                    "UPDATE projection_threads
                     SET active_turn_id = ?1, updated_at = ?2, last_event_sequence = ?3
                     WHERE thread_id = ?4",
                    params![input.active_turn_id, now_iso(), seq, input.thread_id],
                )?;
                let act_id = if input.event_type == CONVERSATION_UPDATED_EVENT {
                    format!("{CONVERSATION_UPDATED_EVENT}:{seq}")
                } else {
                    Uuid::new_v4().to_string()
                };
                insert_activity_and_turn_updates(
                    conn,
                    &input.thread_id,
                    &input.event_type,
                    &input.detail,
                    input.active_turn_id.as_deref(),
                    seq,
                    &act_id,
                    &now_iso(),
                )?;
                if input.event_type == CONVERSATION_UPDATED_EVENT && auto_cmd {
                    supersede_previous_conversation_update(
                        conn,
                        &input.thread_id,
                        seq,
                        &changed_messages,
                        input.active_turn_id.as_deref(),
                    )?;
                }
                Ok(seq.max(0) as u64)
            },
        )?;
        Ok(receipt.result)
    }

    pub fn delete_thread_history(&self, agent_id: &str, thread_id: &str) -> StorageResult<u64> {
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread".to_string(),
            aggregate_id: thread_id.to_string(),
            event_type: "conversation.history-deleted".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({
                "detail": {},
                "changedMessages": [],
                "removedMessageIds": [],
                "activeTurnId": null,
            }),
        }];
        let receipt = self.database.dispatch_with_projection(
            &format!("conversation:conversation.history-deleted:{}", Uuid::new_v4()),
            None,
            &events,
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(0);
                let owner: Option<String> = conn
                    .prepare_cached("SELECT agent_id FROM projection_threads WHERE thread_id = ?1")?
                    .query_row(params![thread_id], |r| r.get(0))
                    .optional()?;
                if owner.as_deref() != Some(agent_id) {
                    return Err(StorageError::NotFound(format!(
                        "Unknown agent for conversation changes: {agent_id}"
                    )));
                }
                record_deleted_thread_history(conn, thread_id, &now_iso())?;
                delete_thread_attachments(conn, thread_id)?;
                conn.execute(
                    "DELETE FROM projection_thread_messages WHERE thread_id = ?1",
                    params![thread_id],
                )?;
                conn.execute(
                    "DELETE FROM projection_thread_summaries WHERE thread_id = ?1",
                    params![thread_id],
                )?;
                let summary_cmds: Vec<String> = {
                    let mut stmt = conn.prepare(
                        "SELECT DISTINCT command_id FROM orchestration_events
                         WHERE aggregate_type = 'thread' AND aggregate_id = ?1
                           AND event_type = 'thread.summary-created'",
                    )?;
                    let rows = stmt.query_map(params![thread_id], |r| r.get(0))?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                conn.execute(
                    "DELETE FROM projection_thread_activities
                     WHERE thread_id = ?1 AND last_event_sequence IN (
                       SELECT sequence FROM orchestration_events
                       WHERE aggregate_type = 'thread' AND aggregate_id = ?1
                         AND event_type = 'thread.summary-created'
                     )",
                    params![thread_id],
                )?;
                conn.execute(
                    "DELETE FROM orchestration_events
                     WHERE aggregate_type = 'thread' AND aggregate_id = ?1
                       AND event_type = 'thread.summary-created'",
                    params![thread_id],
                )?;
                delete_receipts_of(conn, &summary_cmds)?;
                conn.execute(
                    "UPDATE projection_threads SET updated_at = ?1, last_event_sequence = ?2
                     WHERE thread_id = ?3",
                    params![now_iso(), seq, thread_id],
                )?;
                Ok(seq.max(0) as u64)
            },
        )?;
        Ok(receipt.result)
    }

    pub fn upsert_provider_history_message(
        &self,
        input: &UpsertProviderHistoryMessageInput,
    ) -> StorageResult<u64> {
        let event_type = input
            .event_type
            .as_deref()
            .unwrap_or("provider-history.message-imported");
        if self
            .database
            .with_conn(|c| is_deleted_thread_message(c, &input.thread_id, &input.message.id))?
        {
            return self.thread_revision(&input.thread_id);
        }
        if input.active_turn_id.is_none() {
            let unchanged = self.database.with_conn(|conn| {
                let raw_opt: Option<String> = conn
                    .prepare_cached(
                        "SELECT message_json FROM projection_thread_messages
                         WHERE thread_id = ?1 AND message_id = ?2",
                    )?
                    .query_row(params![input.thread_id, input.message.id], |r| r.get(0))
                    .optional()?;
                if let Some(raw) = raw_opt {
                    let existing: ConversationMessage = serde_json::from_str(&raw)?;
                    let merged = merge_imported_message(&existing, &input.message);
                    return Ok(serde_json::to_string(&merged)? == serde_json::to_string(&existing)?);
                }
                Ok(false)
            })?;
            if unchanged {
                return self.thread_revision(&input.thread_id);
            }
        }
        let agent = self.roster.get_agent(&input.agent_id)?.ok_or_else(|| {
            StorageError::NotFound(format!(
                "Unknown agent thread for provider history: {}",
                input.agent_id
            ))
        })?;
        let mut payload = serde_json::json!({
            "detail": input.detail,
            "importedMessage": input.message,
        });
        if let Some(active_opt) = &input.active_turn_id
            && let Some(obj) = payload.as_object_mut()
        {
            obj.insert(
                "activeTurnId".to_string(),
                match active_opt {
                    Some(tid) => serde_json::Value::String(tid.clone()),
                    None => serde_json::Value::Null,
                },
            );
        }
        let resolved_cmd = input
            .command_id
            .clone()
            .unwrap_or_else(|| format!("conversation:{event_type}:{}", Uuid::new_v4()));
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "thread".to_string(),
            aggregate_id: input.thread_id.clone(),
            event_type: event_type.to_string(),
            occurred_at: input.message.created_at.clone(),
            payload,
        }];
        let receipt = self.database.dispatch_with_projection(
            &resolved_cmd,
            None,
            &events,
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(0);
                ensure_thread_exists_for_agent(conn, &agent, &input.thread_id, seq)?;
                let existing_row: Option<(i64, String)> = conn
                    .prepare_cached(
                        "SELECT ordinal, message_json FROM projection_thread_messages
                         WHERE thread_id = ?1 AND message_id = ?2",
                    )?
                    .query_row(params![input.thread_id, input.message.id], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?;
                let (ordinal, merged) = match existing_row {
                    Some((ord, raw)) => {
                        let existing: ConversationMessage = serde_json::from_str(&raw)?;
                        (ord, merge_imported_message(&existing, &input.message))
                    }
                    None => (next_ordinal_conn(conn, &input.thread_id)?, input.message.clone()),
                };
                let before =
                    read_order_inputs(conn, &input.thread_id, std::slice::from_ref(&merged.id))?;
                insert_thread_message_row(conn, &input.thread_id, &merged, ordinal, seq)?;
                refresh_changed_order(conn, &input.thread_id, &before, std::slice::from_ref(&merged.id))?;
                record_usage_message(conn, &agent.id, &merged, agent.provider, &agent.model)?;
                match &input.active_turn_id {
                    Some(active_opt) => {
                        conn.execute(
                            "UPDATE projection_threads
                             SET active_turn_id = ?1, updated_at = ?2, last_event_sequence = ?3
                             WHERE thread_id = ?4",
                            params![active_opt, now_iso(), seq, input.thread_id],
                        )?;
                    }
                    None => {
                        conn.execute(
                            "UPDATE projection_threads SET updated_at = ?1, last_event_sequence = ?2
                             WHERE thread_id = ?3",
                            params![now_iso(), seq, input.thread_id],
                        )?;
                    }
                }
                let turn_ref = merged
                    .turn_id
                    .as_deref()
                    .or_else(|| input.active_turn_id.as_ref().and_then(|o| o.as_deref()));
                conn.execute(
                    "INSERT INTO projection_thread_activities
                       (activity_id, thread_id, turn_id, activity_type, payload_json, created_at, last_event_sequence)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        Uuid::new_v4().to_string(),
                        input.thread_id,
                        turn_ref,
                        event_type,
                        serde_json::to_string(&input.detail)?,
                        input.message.created_at,
                        seq
                    ],
                )?;
                Ok(seq.max(0) as u64)
            },
        )?;
        Ok(receipt.result)
    }

    /// Rebuilds all child projections for `thread_id` deterministically from `orchestration_events`
    /// while preserving `projection_deleted_thread_messages` tombstones (`thread-replay.ts:L31-L250`).
    pub fn rebuild_thread_projection(&self, thread_id: &str) -> StorageResult<ConversationSnapshot> {
        let agent_id = self.database.with_tx(|conn| {
            let owner: String = conn
                .prepare_cached("SELECT agent_id FROM projection_threads WHERE thread_id = ?1")?
                .query_row(params![thread_id], |r| r.get(0))
                .optional()?
                .ok_or_else(|| StorageError::NotFound(format!("Unknown OpenBot thread: {thread_id}")))?;
            replay_thread_events_in_tx(conn, &owner, thread_id)?;
            Ok(owner)
        })?;
        self.read_conversation(&agent_id, Some(thread_id))
    }

    fn thread_revision(&self, thread_id: &str) -> StorageResult<u64> {
        self.database.with_conn(|conn| {
            let rev: Option<i64> = conn
                .prepare_cached(
                    "SELECT last_event_sequence FROM projection_threads WHERE thread_id = ?1",
                )?
                .query_row(params![thread_id], |r| r.get(0))
                .optional()?;
            Ok(rev.unwrap_or(0).max(0) as u64)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageCursorWire {
    version: u8,
    #[serde(default)]
    group_start: Option<String>,
    #[serde(default)]
    group_first: Option<i64>,
    #[serde(default)]
    group_id: Option<String>,
    #[serde(default)]
    turn_rank: Option<i64>,
    created_at: String,
    ordinal: i64,
    message_id: String,
}

fn encode_page_cursor(key: &OrderKeyTuple) -> StorageResult<String> {
    let wire = PageCursorWire {
        version: 2,
        group_start: Some(key.0.clone()),
        group_first: Some(key.1),
        group_id: Some(key.2.clone()),
        turn_rank: Some(key.3),
        created_at: key.4.clone(),
        ordinal: key.5,
        message_id: key.6.clone(),
    };
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&wire)?))
}

fn decode_page_cursor(
    conn: &Connection,
    thread_id: &str,
    cursor_str: &str,
) -> StorageResult<OrderKeyTuple> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor_str.trim_end_matches('='))
        .map_err(|_| StorageError::Validation("The conversation page cursor is invalid.".to_string()))?;
    let wire: PageCursorWire = serde_json::from_slice(&bytes)
        .map_err(|_| StorageError::Validation("The conversation page cursor is invalid.".to_string()))?;
    if wire.version == 2
        && let (Some(gs), Some(gf), Some(gid), Some(tr)) =
            (wire.group_start, wire.group_first, wire.group_id, wire.turn_rank)
    {
        return Ok((gs, gf, gid, tr, wire.created_at, wire.ordinal, wire.message_id));
    }
    if wire.version == 1 {
        if let Some(found) = message_order_key_conn(conn, thread_id, &wire.message_id)? {
            return Ok(found);
        }
        return Ok((
            wire.created_at.clone(),
            wire.ordinal,
            format!("message:{}", wire.message_id),
            0,
            wire.created_at,
            wire.ordinal,
            wire.message_id,
        ));
    }
    Err(StorageError::Validation(
        "The conversation page cursor is invalid.".to_string(),
    ))
}

fn encode_search_cursor(offset: usize) -> StorageResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(
        &serde_json::json!({ "version": 1, "offset": offset }),
    )?))
}

fn decode_search_cursor(raw: &str) -> StorageResult<usize> {
    let bytes = URL_SAFE_NO_PAD
        .decode(raw.trim_end_matches('='))
        .map_err(|_| StorageError::Validation("Invalid search cursor.".to_string()))?;
    let val: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| StorageError::Validation("Invalid search cursor.".to_string()))?;
    if val.get("version").and_then(|v| v.as_u64()) != Some(1) {
        return Err(StorageError::Validation("Invalid search cursor.".to_string()));
    }
    let off = val
        .get("offset")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| StorageError::Validation("Invalid search cursor.".to_string()))?;
    Ok(off as usize)
}

fn collapse_whitespace_lower(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn read_order_key_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OrderKeyTuple> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn whole_groups(rows: &[OrderKeyTuple], target: usize, cap: usize) -> Vec<OrderKeyTuple> {
    let mut taken = Vec::new();
    let mut start = 0;
    while start < rows.len() && taken.len() < target {
        let gid = &rows[start].2;
        let mut end = start + 1;
        while end < rows.len() && &rows[end].2 == gid {
            end += 1;
        }
        if end > cap {
            if taken.is_empty() {
                taken.extend_from_slice(&rows[..cap.min(rows.len())]);
            }
            break;
        }
        taken.extend_from_slice(&rows[start..end]);
        start = end;
    }
    taken
}

fn query_conversation_page_keys(
    conn: &Connection,
    thread_id: &str,
    anchor: &ConversationPageAnchor,
    limit: usize,
    exclusions: ConversationMarkerExclusions,
) -> StorageResult<Vec<OrderKeyTuple>> {
    let filter = conversation_marker_sql_filter(exclusions);
    match anchor {
        ConversationPageAnchor::Latest => {
            let sql = format!(
                "{ORDERED_THREAD_MESSAGES_CTE}
                 SELECT {ORDER_KEY_COLUMNS} FROM ordered
                 WHERE 1 = 1 {filter}
                 ORDER BY {ORDER_KEY_DESC} LIMIT ?"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![thread_id, (PAGE_MESSAGE_LIMIT + 1) as i64], read_order_key_row)?
                .collect::<Result<Vec<_>, _>>()?;
            let mut groups = whole_groups(&rows, limit, PAGE_MESSAGE_LIMIT);
            groups.reverse();
            Ok(groups)
        }
        ConversationPageAnchor::Before { cursor } => {
            let k = decode_page_cursor(conn, thread_id, cursor)?;
            let sql = format!(
                "{ORDERED_THREAD_MESSAGES_CTE}
                 SELECT {ORDER_KEY_COLUMNS} FROM ordered
                 WHERE ({ORDER_KEY_COLUMNS}) < (?, ?, ?, ?, ?, ?, ?)
                 {filter}
                 ORDER BY {ORDER_KEY_DESC} LIMIT ?"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(
                    params![
                        thread_id,
                        k.0,
                        k.1,
                        k.2,
                        k.3,
                        k.4,
                        k.5,
                        k.6,
                        (PAGE_MESSAGE_LIMIT + 1) as i64
                    ],
                    read_order_key_row,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let mut groups = whole_groups(&rows, limit, PAGE_MESSAGE_LIMIT);
            groups.reverse();
            Ok(groups)
        }
        ConversationPageAnchor::Around { message_id } => {
            let anchor_sql = format!(
                "{ORDERED_THREAD_MESSAGES_CTE}
                 SELECT {ORDER_KEY_COLUMNS} FROM ordered WHERE message_id = ? {filter}"
            );
            let Some(ak) = conn
                .prepare(&anchor_sql)?
                .query_row(params![thread_id, message_id], read_order_key_row)
                .optional()?
            else {
                return Ok(Vec::new());
            };
            let newer_sql = format!(
                "{ORDERED_THREAD_MESSAGES_CTE}
                 SELECT {ORDER_KEY_COLUMNS} FROM ordered
                 WHERE ({ORDER_KEY_COLUMNS}) > (?, ?, ?, ?, ?, ?, ?)
                 {filter}
                 ORDER BY {ORDER_KEY_COLUMNS} LIMIT ?"
            );
            let newer_rows: Vec<OrderKeyTuple> = conn
                .prepare(&newer_sql)?
                .query_map(
                    params![
                        thread_id,
                        ak.0,
                        ak.1,
                        ak.2,
                        ak.3,
                        ak.4,
                        ak.5,
                        ak.6,
                        (PAGE_MESSAGE_LIMIT + 1) as i64
                    ],
                    read_order_key_row,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let after_in_turn = newer_rows
                .iter()
                .position(|r| r.2 != ak.2)
                .unwrap_or(newer_rows.len());
            let reserved = after_in_turn
                .min(PAGE_MESSAGE_LIMIT - 1)
                .max(limit.div_ceil(2));
            let older_cap = PAGE_MESSAGE_LIMIT.saturating_sub(reserved);
            let older_sql = format!(
                "{ORDERED_THREAD_MESSAGES_CTE}
                 SELECT {ORDER_KEY_COLUMNS} FROM ordered
                 WHERE ({ORDER_KEY_COLUMNS}) <= (?, ?, ?, ?, ?, ?, ?)
                 {filter}
                 ORDER BY {ORDER_KEY_DESC} LIMIT ?"
            );
            let older_rows: Vec<OrderKeyTuple> = conn
                .prepare(&older_sql)?
                .query_map(
                    params![
                        thread_id,
                        ak.0,
                        ak.1,
                        ak.2,
                        ak.3,
                        ak.4,
                        ak.5,
                        ak.6,
                        (older_cap + 1) as i64
                    ],
                    read_order_key_row,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let mut older = whole_groups(&older_rows, (limit / 2) + 1, older_cap);
            older.reverse();
            let newer_target = limit.saturating_sub(older.len()).max(1);
            let newer_cap = PAGE_MESSAGE_LIMIT.saturating_sub(older.len());
            let newer = whole_groups(&newer_rows, newer_target, newer_cap);
            older.extend(newer);
            Ok(older)
        }
    }
}

fn count_conversation_rows_before(
    conn: &Connection,
    thread_id: &str,
    k: &OrderKeyTuple,
    exclusions: ConversationMarkerExclusions,
) -> StorageResult<(u64, Option<String>)> {
    let filter = conversation_marker_sql_filter(exclusions);
    let sql = format!(
        "{ORDERED_THREAD_MESSAGES_CTE}
         SELECT COUNT(*) AS older_count,
           (SELECT created_at FROM projection_thread_messages
            WHERE thread_id = ? {filter}
            ORDER BY created_at, ordinal, message_id LIMIT 1) AS oldest_at
         FROM ordered
         WHERE ({ORDER_KEY_COLUMNS}) < (?, ?, ?, ?, ?, ?, ?)
         {filter}"
    );
    let (cnt, oldest): (i64, Option<String>) = conn.prepare(&sql)?.query_row(
        params![thread_id, thread_id, k.0, k.1, k.2, k.3, k.4, k.5, k.6],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok((cnt.max(0) as u64, oldest))
}

/// Strictly 0-based `ordinal` (`conversation-writer.ts:L323-L328`, `L877-L884`).
fn next_ordinal_conn(conn: &Connection, thread_id: &str) -> StorageResult<i64> {
    let next: i64 = conn
        .prepare_cached(
            "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM projection_thread_messages WHERE thread_id = ?1",
        )?
        .query_row(params![thread_id], |r| r.get(0))?;
    Ok(next)
}

fn existing_or_next_ordinal(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> StorageResult<i64> {
    let existing: Option<i64> = conn
        .prepare_cached(
            "SELECT ordinal FROM projection_thread_messages WHERE thread_id = ?1 AND message_id = ?2",
        )?
        .query_row(params![thread_id, message_id], |r| r.get(0))
        .optional()?;
    match existing {
        Some(ord) => Ok(ord),
        None => next_ordinal_conn(conn, thread_id),
    }
}

fn ensure_thread_exists_for_agent(
    conn: &Connection,
    agent: &AgentSummary,
    thread_id: &str,
    sequence: i64,
) -> StorageResult<()> {
    let owner: Option<String> = conn
        .prepare_cached("SELECT agent_id FROM projection_threads WHERE thread_id = ?1")?
        .query_row(params![thread_id], |r| r.get(0))
        .optional()?;
    match owner {
        Some(ref existing) if existing != &agent.id => Err(StorageError::Conflict(format!(
            "Thread {thread_id} belongs to another agent ({existing} != {}).",
            agent.id
        ))),
        Some(_) => Ok(()),
        None => {
            if agent.thread_id.as_deref() != Some(thread_id) {
                return Err(StorageError::NotFound(format!(
                    "Unknown thread {thread_id} for agent {}.",
                    agent.id
                )));
            }
            ensure_thread_projection_row(conn, agent, sequence)
        }
    }
}

fn conversation_recovery_state(
    conn: &Connection,
    thread_id: &str,
    active_turn_id: Option<&str>,
) -> StorageResult<serde_json::Value> {
    let mut map = serde_json::Map::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT turn_id, provider_session_id FROM projection_turns WHERE thread_id = ?1",
        )?;
        let rows = stmt.query_map(params![thread_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (tid, sid) = row?;
            map.insert(
                tid,
                match sid {
                    Some(s) => serde_json::Value::String(s),
                    None => serde_json::Value::Null,
                },
            );
        }
    }
    if let Some(active_tid) = active_turn_id
        && !map.contains_key(active_tid)
    {
        let active_sess: Option<String> = conn
            .prepare_cached(
                "SELECT id FROM projection_provider_sessions
                 WHERE thread_id = ?1 AND state = 'active'
                 ORDER BY created_at DESC, last_event_sequence DESC LIMIT 1",
            )?
            .query_row(params![thread_id], |r| r.get(0))
            .optional()?;
        map.insert(
            active_tid.to_string(),
            match active_sess {
                Some(s) => serde_json::Value::String(s),
                None => serde_json::Value::Null,
            },
        );
    }
    Ok(serde_json::json!({ "turnProviderSessionIds": map }))
}

fn is_turn_end_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "turn.completed" | "turn.reconciled-after-restart" | "turn.interrupted-by-restart"
    )
}

fn activity_turn_id<'a>(
    event_type: &str,
    detail: &'a serde_json::Value,
    active_turn_id: Option<&'a str>,
) -> Option<&'a str> {
    if is_turn_end_event(event_type)
        && let Some(tid) = detail.get("turnId").and_then(|v| v.as_str())
    {
        return Some(tid);
    }
    active_turn_id
}

#[allow(clippy::too_many_arguments)]
fn insert_activity_and_turn_updates(
    conn: &Connection,
    thread_id: &str,
    event_type: &str,
    detail: &serde_json::Value,
    active_turn_id: Option<&str>,
    sequence: i64,
    activity_id: &str,
    ts: &str,
) -> StorageResult<()> {
    if let Some(active_tid) = active_turn_id {
        conn.execute(
            "INSERT INTO projection_turns
               (turn_id, thread_id, provider_session_id, status, started_at, completed_at, last_event_sequence)
             VALUES (?1, ?2, (
               SELECT id FROM projection_provider_sessions
               WHERE thread_id = ?2 AND state = 'active' ORDER BY created_at DESC LIMIT 1
             ), 'running', ?3, NULL, ?4)
             ON CONFLICT(turn_id) DO UPDATE SET status = 'running', last_event_sequence = excluded.last_event_sequence
             WHERE projection_turns.completed_at IS NULL",
            params![active_tid, thread_id, ts, sequence],
        )?;
    }
    if is_turn_end_event(event_type)
        && let Some(turn_id) = detail.get("turnId").and_then(|v| v.as_str())
    {
        let status = detail
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or(if event_type == "turn.interrupted-by-restart" {
                "interrupted"
            } else {
                "completed"
            });
        conn.execute(
            "UPDATE projection_turns
             SET status = ?1, completed_at = ?2, last_event_sequence = ?3
             WHERE thread_id = ?4 AND turn_id = ?5",
            params![status, ts, sequence, thread_id, turn_id],
        )?;
    }
    if event_type == "turn.interrupted-by-restart" {
        conn.execute(
            "UPDATE projection_turns
             SET status = 'interrupted', completed_at = COALESCE(completed_at, ?1), last_event_sequence = ?2
             WHERE thread_id = ?3 AND status = 'running'",
            params![ts, sequence, thread_id],
        )?;
    }
    conn.execute(
        "INSERT INTO projection_thread_activities
           (activity_id, thread_id, turn_id, activity_type, payload_json, created_at, last_event_sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            activity_id,
            thread_id,
            activity_turn_id(event_type, detail, active_turn_id),
            event_type,
            serde_json::to_string(detail)?,
            ts,
            sequence
        ],
    )?;
    Ok(())
}

pub fn without_queue_positions(message: &ConversationMessage) -> ConversationMessage {
    if message.delivery.is_none() && message.exchange.is_none() {
        return message.clone();
    }
    let mut copy = message.clone();
    if let Some(del) = &mut copy.delivery {
        del.position = None;
    }
    if let Some(ex) = &mut copy.exchange {
        for d in &mut ex.deliveries {
            d.position = None;
        }
    }
    copy
}

fn merge_live_message(
    existing: &ConversationMessage,
    updated: &ConversationMessage,
) -> ConversationMessage {
    let mut merged = updated.clone();
    merged.id = existing.id.clone();
    merged.author = existing.author;
    merged.created_at = if updated.delivery.is_some() {
        updated.created_at.clone()
    } else {
        existing.created_at.clone()
    };
    merged.turn_id = updated
        .turn_id
        .clone()
        .or_else(|| existing.turn_id.clone());
    merged
}

fn merge_imported_message(
    existing: &ConversationMessage,
    imported: &ConversationMessage,
) -> ConversationMessage {
    let mut merged = imported.clone();
    merged.id = existing.id.clone();
    merged.author = existing.author;
    merged.created_at = existing.created_at.clone();
    merged.turn_id = existing
        .turn_id
        .clone()
        .or_else(|| imported.turn_id.clone());

    if existing.attachments.is_some() || imported.attachments.is_some() {
        let mut by_id: Vec<AttachmentSummary> =
            existing.attachments.clone().unwrap_or_default();
        for att in imported.attachments.as_deref().unwrap_or(&[]) {
            if let Some(slot) = by_id.iter_mut().find(|a| a.id == att.id) {
                *slot = att.clone();
            } else {
                by_id.push(att.clone());
            }
        }
        merged.attachments = Some(by_id);
    }
    if existing.reactions.is_some() || imported.reactions.is_some() {
        let mut list = existing.reactions.clone().unwrap_or_default();
        for r in imported.reactions.as_deref().unwrap_or(&[]) {
            if !list.contains(r) {
                list.push(r.clone());
            }
        }
        merged.reactions = Some(list);
    }
    if merged.delivery.is_none() {
        merged.delivery = existing.delivery.clone();
    }
    if merged.exchange.is_none() {
        merged.exchange = existing.exchange.clone();
    }
    if merged.sender_member.is_none() {
        merged.sender_member = existing.sender_member.clone();
    }
    merged.image_generation =
        merge_image_generation(existing.image_generation.as_ref(), imported.image_generation.as_ref());

    if existing.item_type.as_deref() == Some("image_generation")
        && matches!(
            existing.status,
            ConversationMessageStatus::Failed | ConversationMessageStatus::Interrupted
        )
        && existing.attachments.as_deref().unwrap_or(&[]).is_empty()
        && imported.attachments.as_deref().unwrap_or(&[]).is_empty()
    {
        merged.status = existing.status;
        if let Some(stored_ig) = &existing.image_generation
            && let Some(err) = &stored_ig.error
        {
            let mut ig = merged
                .image_generation
                .clone()
                .unwrap_or_else(|| stored_ig.clone());
            ig.error = Some(err.clone());
            merged.image_generation = Some(ig);
        }
    }
    merged
}

fn merge_image_generation(
    existing: Option<&ImageGenerationInfo>,
    imported: Option<&ImageGenerationInfo>,
) -> Option<ImageGenerationInfo> {
    match (existing, imported) {
        (Some(e), Some(i)) => Some(ImageGenerationInfo {
            prompt: i.prompt.clone().or_else(|| e.prompt.clone()),
            resolution: i.resolution.clone(),
            aspect_ratio: i.aspect_ratio,
            error: i.error.clone().or_else(|| e.error.clone()),
        }),
        (None, Some(i)) => Some(i.clone()),
        (Some(e), None) => Some(e.clone()),
        (None, None) => None,
    }
}

fn supersede_streamed_message_event(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
    event_type: &str,
    retained_seq: i64,
    remembered_seq: Option<i64>,
) -> StorageResult<()> {
    let mut prev: Option<(i64, String)> = conn
        .prepare_cached(
            "SELECT e.sequence, e.command_id FROM projection_thread_messages m
             JOIN orchestration_events e ON e.sequence = m.last_event_sequence
             WHERE m.thread_id = ?1 AND m.message_id = ?2
               AND e.aggregate_type = 'thread' AND e.aggregate_id = ?1
               AND e.event_type = ?3 AND e.sequence < ?4
               AND json_extract(e.payload_json, '$.streamedMessage.id') = ?2",
        )?
        .query_row(
            params![thread_id, message_id, event_type, retained_seq],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if prev.is_none()
        && let Some(rem) = remembered_seq
    {
        prev = conn
            .prepare_cached(
                "SELECT sequence, command_id FROM orchestration_events
                 WHERE sequence = ?1 AND aggregate_type = 'thread' AND aggregate_id = ?2
                   AND event_type = ?3 AND sequence < ?4
                   AND json_extract(payload_json, '$.streamedMessage.id') = ?5",
            )?
            .query_row(
                params![rem, thread_id, event_type, retained_seq, message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
    }
    if let Some((seq, cmd)) = prev {
        conn.execute(
            "DELETE FROM orchestration_events WHERE sequence = ?1",
            params![seq],
        )?;
        delete_receipts_of(conn, &[cmd])?;
    }
    Ok(())
}

fn supersede_previous_conversation_update(
    conn: &Connection,
    thread_id: &str,
    sequence: i64,
    changed_messages: &[ConversationMessage],
    active_turn_id: Option<&str>,
) -> StorageResult<()> {
    let prev_opt: Option<(i64, String, String, String)> = conn
        .prepare_cached(
            "SELECT sequence, command_id, event_type, payload_json FROM orchestration_events
             WHERE aggregate_type = 'thread' AND aggregate_id = ?1 AND sequence < ?2
             ORDER BY sequence DESC LIMIT 1",
        )?
        .query_row(params![thread_id, sequence], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .optional()?;
    let Some((prev_seq, prev_cmd, prev_type, prev_json)) = prev_opt else {
        return Ok(());
    };
    if prev_type != CONVERSATION_UPDATED_EVENT {
        return Ok(());
    }
    let Some(uuid_part) = prev_cmd.strip_prefix("conversation:conversation.snapshot-updated:")
    else {
        return Ok(());
    };
    if Uuid::parse_str(uuid_part).is_err() {
        return Ok(());
    }
    let Ok(prev_val) = serde_json::from_str::<serde_json::Value>(&prev_json) else {
        return Ok(());
    };
    if !replaced_conversation_update(&prev_val, changed_messages, active_turn_id) {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM projection_thread_activities WHERE activity_id = ?1",
        params![format!("{CONVERSATION_UPDATED_EVENT}:{prev_seq}")],
    )?;
    conn.execute(
        "DELETE FROM orchestration_events WHERE sequence = ?1",
        params![prev_seq],
    )?;
    delete_receipts_of(conn, &[prev_cmd])
}

fn replaced_conversation_update(
    previous: &serde_json::Value,
    changed_messages: &[ConversationMessage],
    active_turn_id: Option<&str>,
) -> bool {
    let Some(obj) = previous.as_object() else {
        return false;
    };
    let prev_turn = obj.get("activeTurnId").and_then(|v| v.as_str());
    if prev_turn != active_turn_id {
        return false;
    }
    if !obj
        .get("removedMessageIds")
        .and_then(|v| v.as_array())
        .is_some_and(Vec::is_empty)
    {
        return false;
    }
    let Some(prev_msgs) = obj.get("changedMessages").and_then(|v| v.as_array()) else {
        return false;
    };
    if prev_msgs.len() > changed_messages.len() {
        return false;
    }
    for (idx, prev_msg_val) in prev_msgs.iter().enumerate() {
        let Some(prev_msg_obj) = prev_msg_val.as_object() else {
            return false;
        };
        let next = &changed_messages[idx];
        let Ok(serde_json::Value::Object(next_obj)) = serde_json::to_value(next) else {
            return false;
        };
        if prev_msg_obj.get("id") != next_obj.get("id")
            || prev_msg_obj.get("author") != next_obj.get("author")
        {
            return false;
        }
        if next.delivery.is_none() && prev_msg_obj.get("createdAt") != next_obj.get("createdAt") {
            return false;
        }
        if !prev_msg_obj.keys().all(|k| next_obj.contains_key(k)) {
            return false;
        }
    }
    true
}

/// Prunes older snapshot and streamedMessage events for `thread_id` (`conversation-writer.ts:L1081-L1098`).
pub fn prune_conversation_snapshots(
    conn: &Connection,
    thread_id: &str,
    retained_sequence: i64,
) -> StorageResult<()> {
    conn.execute(
        "DELETE FROM projection_thread_activities
         WHERE thread_id = ?1 AND last_event_sequence IN (
           SELECT sequence FROM orchestration_events
           WHERE aggregate_type = 'thread' AND aggregate_id = ?1 AND sequence < ?2
             AND (json_type(payload_json, '$.snapshot') = 'object'
               OR json_type(payload_json, '$.streamedMessage') = 'object')
         )",
        params![thread_id, retained_sequence],
    )?;
    let command_ids: Vec<String> = {
        let mut stmt = conn.prepare(
            "DELETE FROM orchestration_events
             WHERE aggregate_type = 'thread' AND aggregate_id = ?1 AND sequence < ?2
               AND (json_type(payload_json, '$.snapshot') = 'object'
                 OR json_type(payload_json, '$.streamedMessage') = 'object')
             RETURNING command_id",
        )?;
        let rows = stmt.query_map(params![thread_id, retained_sequence], |r| r.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    delete_receipts_of(conn, &command_ids)
}

fn replay_thread_events_in_tx(
    conn: &Connection,
    agent_id: &str,
    thread_id: &str,
) -> StorageResult<()> {
    let events: Vec<(i64, String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT sequence, event_type, occurred_at, payload_json
             FROM orchestration_events
             WHERE aggregate_type = 'thread' AND aggregate_id = ?1
             ORDER BY sequence",
        )?;
        let rows = stmt.query_map(params![thread_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut latest: Option<ConversationSnapshot> = None;
    let mut latest_seq: i64 = 0;
    let mut session_order: Vec<String> = Vec::new();
    let mut sessions: HashMap<String, (ProviderSession, i64)> = HashMap::new();
    let mut summaries: Vec<(StoredThreadSummary, i64)> = Vec::new();
    let mut turn_sessions: HashMap<String, Option<String>> = HashMap::new();

    for (seq, event_type, _occurred_at, payload_json) in &events {
        let payload: serde_json::Value = serde_json::from_str(payload_json)?;
        let obj = payload.as_object();

        if event_type == "provider-session.bound" {
            if let Ok(sess) = serde_json::from_value::<ProviderSession>(payload.clone()) {
                for (existing, _) in sessions.values_mut() {
                    existing.state = ProviderSessionState::Inactive;
                }
                if !session_order.contains(&sess.id) {
                    session_order.push(sess.id.clone());
                }
                sessions.insert(sess.id.clone(), (sess, *seq));
            }
        } else if event_type == "provider-session.deactivated" {
            if let Some(ids) = obj.and_then(|o| o.get("sessionIds")).and_then(|v| v.as_array()) {
                for id_val in ids {
                    if let Some(sid) = id_val.as_str()
                        && let Some((sess, _)) = sessions.get_mut(sid)
                    {
                        sess.state = ProviderSessionState::Inactive;
                    }
                }
            }
        } else if event_type == "provider-session.config-updated" {
            if let Some(sid) = obj.and_then(|o| o.get("sessionId")).and_then(|v| v.as_str())
                && let Some((sess, s_seq)) = sessions.get_mut(sid)
            {
                if let Some(m) = obj.and_then(|o| o.get("model")).and_then(|v| v.as_str()) {
                    sess.model = m.to_string();
                }
                if let Some(e) = obj.and_then(|o| o.get("effort")).and_then(|v| v.as_str()) {
                    sess.effort = e.to_string();
                }
                *s_seq = *seq;
            }
        } else if event_type == "thread.summary-created"
            && let Ok(sum) = serde_json::from_value::<StoredThreadSummary>(payload.clone())
        {
            summaries.push((sum, *seq));
        }

        if let Some(snap_val) = obj.and_then(|o| o.get("snapshot"))
            && let Ok(snap) = serde_json::from_value::<ConversationSnapshot>(snap_val.clone())
        {
            if let Some(rec_map) = obj
                .and_then(|o| o.get("recovery"))
                .and_then(|r| r.get("turnProviderSessionIds"))
                .and_then(|m| m.as_object())
            {
                for (tid, sid_val) in rec_map {
                    turn_sessions.insert(tid.clone(), sid_val.as_str().map(str::to_string));
                }
            }
            if let Some(active_tid) = &snap.active_turn_id
                && !turn_sessions.contains_key(active_tid)
            {
                let active_sess = session_order
                    .iter()
                    .filter_map(|id| sessions.get(id))
                    .find(|(s, _)| s.state == ProviderSessionState::Active)
                    .map(|(s, _)| s.id.clone());
                turn_sessions.insert(active_tid.clone(), active_sess);
            }
            latest = Some(snap);
            latest_seq = *seq;
        } else if let Some(sm_val) = obj.and_then(|o| o.get("streamedMessage"))
            && latest.is_some()
            && let Ok(sm) = serde_json::from_value::<ConversationMessage>(sm_val.clone())
        {
            let snap = latest.as_mut().expect("checked Some");
            if let Some(slot) = snap.messages.iter_mut().find(|m| m.id == sm.id) {
                *slot = sm;
            } else {
                snap.messages.push(sm);
            }
            apply_active_turn_field(obj, snap);
            latest_seq = *seq;
        } else if let Some(msg_val) = obj
            .and_then(|o| o.get("appendedMessage").or_else(|| o.get("importedMessage")))
            && let Ok(msg) = serde_json::from_value::<ConversationMessage>(msg_val.clone())
        {
            let is_imported = obj.is_some_and(|o| o.contains_key("importedMessage") && !o.contains_key("appendedMessage"));
            let snap = latest.get_or_insert_with(|| ConversationSnapshot {
                agent_id: agent_id.to_string(),
                thread_id: Some(thread_id.to_string()),
                active_turn_id: None,
                revision: 0,
                messages: Vec::new(),
            });
            if let Some(slot) = snap.messages.iter_mut().find(|m| m.id == msg.id) {
                if is_imported {
                    *slot = merge_imported_message(slot, &msg);
                }
            } else {
                snap.messages.push(msg);
            }
            apply_active_turn_field(obj, snap);
            latest_seq = *seq;
        } else if obj.is_some_and(|o| {
            o.contains_key("changedMessages")
                || o.contains_key("removedMessageIds")
                || o.contains_key("activeTurnId")
        }) {
            let snap = latest.get_or_insert_with(|| ConversationSnapshot {
                agent_id: agent_id.to_string(),
                thread_id: Some(thread_id.to_string()),
                active_turn_id: None,
                revision: 0,
                messages: Vec::new(),
            });
            let removed: HashSet<String> = obj
                .and_then(|o| o.get("removedMessageIds"))
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let changed: Vec<ConversationMessage> = obj
                .and_then(|o| o.get("changedMessages"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| serde_json::from_value::<ConversationMessage>(x.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            let changed_by_id: HashMap<&str, &ConversationMessage> =
                changed.iter().map(|m| (m.id.as_str(), m)).collect();
            snap.messages.retain(|m| !removed.contains(&m.id));
            for m in &mut snap.messages {
                if let Some(upd) = changed_by_id.get(m.id.as_str()) {
                    *m = merge_live_message(m, upd);
                }
            }
            let shown: HashSet<String> = snap.messages.iter().map(|m| m.id.clone()).collect();
            for c in changed {
                if !shown.contains(&c.id) {
                    snap.messages.push(c);
                }
            }
            apply_active_turn_field(obj, snap);
            latest_seq = *seq;
        }
    }

    let mut rebuilt = latest.ok_or_else(|| {
        StorageError::NotFound(format!("Thread {thread_id} has no conversation events to replay."))
    })?;
    rebuilt.messages = without_deleted_messages(conn, thread_id, &rebuilt.messages)?;

    conn.execute("DELETE FROM projection_thread_activities WHERE thread_id = ?1", params![thread_id])?;
    conn.execute("DELETE FROM projection_turns WHERE thread_id = ?1", params![thread_id])?;
    conn.execute("DELETE FROM projection_thread_messages WHERE thread_id = ?1", params![thread_id])?;
    conn.execute("DELETE FROM projection_thread_summaries WHERE thread_id = ?1", params![thread_id])?;
    conn.execute("DELETE FROM projection_provider_sessions WHERE thread_id = ?1", params![thread_id])?;
    delete_thread_attachments(conn, thread_id)?;

    for sid in &session_order {
        if let Some((s, s_seq)) = sessions.get(sid) {
            conn.execute(
                "INSERT INTO projection_provider_sessions (
                   id, thread_id, provider, external_session_id, model, effort, state,
                   created_at, updated_at, resume_cursor, last_event_sequence
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    s.id, thread_id, s.provider.as_str(), s.external_session_id, s.model, s.effort,
                    s.state.as_str(), s.created_at, s.updated_at, s.resume_cursor, s_seq
                ],
            )?;
        }
    }
    for (ordinal, msg) in rebuilt.messages.iter().enumerate() {
        insert_thread_message_row(conn, thread_id, msg, ordinal as i64, latest_seq)?;
    }
    refresh_thread_order(conn, thread_id)?;
    write_replayed_turns_summaries_activities(
        conn,
        thread_id,
        &rebuilt,
        latest_seq,
        &turn_sessions,
        &summaries,
        &events,
    )
}

fn apply_active_turn_field(
    obj: Option<&serde_json::Map<String, serde_json::Value>>,
    snap: &mut ConversationSnapshot,
) {
    if let Some(at_val) = obj.and_then(|o| o.get("activeTurnId")) {
        if at_val.is_null() {
            snap.active_turn_id = None;
        } else if let Some(s) = at_val.as_str() {
            snap.active_turn_id = Some(s.to_string());
        }
    }
}

fn write_replayed_turns_summaries_activities(
    conn: &Connection,
    thread_id: &str,
    rebuilt: &ConversationSnapshot,
    latest_seq: i64,
    turn_sessions: &HashMap<String, Option<String>>,
    summaries: &[(StoredThreadSummary, i64)],
    events: &[(i64, String, String, String)],
) -> StorageResult<()> {
    let mut by_turn: BTreeMap<String, Vec<&ConversationMessage>> = BTreeMap::new();
    for msg in &rebuilt.messages {
        if let Some(tid) = &msg.turn_id {
            by_turn.entry(tid.clone()).or_default().push(msg);
        }
    }
    for (tid, msgs) in by_turn {
        let running = rebuilt.active_turn_id.as_deref() == Some(tid.as_str());
        let status = if running {
            "running"
        } else if msgs.iter().any(|m| m.status == ConversationMessageStatus::Failed) {
            "failed"
        } else if msgs
            .iter()
            .any(|m| m.status == ConversationMessageStatus::Interrupted)
        {
            "interrupted"
        } else {
            "completed"
        };
        let started = msgs
            .first()
            .map(|m| m.created_at.clone())
            .unwrap_or_else(now_iso);
        let completed = (!running).then(|| {
            msgs.last()
                .map(|m| m.created_at.clone())
                .unwrap_or_else(now_iso)
        });
        let sess_id = turn_sessions.get(&tid).cloned().flatten();
        conn.execute(
            "INSERT INTO projection_turns
               (turn_id, thread_id, provider_session_id, status, started_at, completed_at, last_event_sequence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(turn_id) DO NOTHING",
            params![tid, thread_id, sess_id, status, started, completed, latest_seq],
        )?;
    }
    for (sum, s_seq) in summaries {
        conn.execute(
            "INSERT INTO projection_thread_summaries
               (summary_id, thread_id, through_message_id, summary_text, estimated_tokens, created_at, last_event_sequence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                sum.id,
                thread_id,
                sum.through_message_id,
                sum.text,
                sum.estimated_tokens,
                sum.created_at,
                s_seq
            ],
        )?;
    }
    for (seq, event_type, occurred_at, payload_json) in events {
        let payload: serde_json::Value = serde_json::from_str(payload_json)?;
        let obj = payload.as_object();
        let has_envelope = obj.is_some_and(|o| {
            o.contains_key("snapshot")
                || o.contains_key("appendedMessage")
                || o.contains_key("importedMessage")
                || o.contains_key("changedMessages")
                || o.contains_key("removedMessageIds")
        });
        let act_payload = if has_envelope {
            obj.and_then(|o| o.get("detail"))
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            payload.clone()
        };
        let msg_turn = obj
            .and_then(|o| {
                o.get("appendedMessage")
                    .or_else(|| o.get("importedMessage"))
                    .or_else(|| o.get("streamedMessage"))
            })
            .and_then(|m| m.get("turnId"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let fallback_active = obj
            .and_then(|o| {
                o.get("snapshot")
                    .and_then(|s| s.get("activeTurnId"))
                    .or_else(|| o.get("activeTurnId"))
            })
            .and_then(|v| v.as_str());
        let detail_ref = obj
            .and_then(|o| o.get("detail"))
            .unwrap_or(&serde_json::Value::Null);
        let resolved_turn = msg_turn.or_else(|| {
            activity_turn_id(event_type, detail_ref, fallback_active).map(str::to_string)
        });
        conn.execute(
            "INSERT INTO projection_thread_activities
               (activity_id, thread_id, turn_id, activity_type, payload_json, created_at, last_event_sequence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                Uuid::new_v4().to_string(),
                thread_id,
                resolved_turn,
                event_type,
                serde_json::to_string(&act_payload)?,
                occurred_at,
                seq
            ],
        )?;
    }
    conn.execute(
        "UPDATE projection_threads
         SET active_turn_id = ?1, updated_at = ?2, last_event_sequence = ?3
         WHERE thread_id = ?4",
        params![rebuilt.active_turn_id, now_iso(), latest_seq, thread_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_store::AgentStore;
    use openbot_contracts::agents::CreateAgentInput;

    struct ScopedTestDir(PathBuf);

    impl ScopedTestDir {
        fn new(prefix: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("{prefix}_{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
    }

    impl Drop for ScopedTestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    use std::path::PathBuf;

    #[test]
    fn thread_store_enforces_zero_based_ordinal_tombstones_pins_sessions_waits_and_branches() {
        let tmp = ScopedTestDir::new("openbot_thread_store");
        let db = OpenBotDatabase::open(tmp.0.join("data").join("openbot.db")).unwrap();
        let agents = AgentStore::open(db.clone(), tmp.0.join("OpenBot")).unwrap();
        let agent = agents
            .create_agent(&CreateAgentInput {
                name: "Coder".to_string(),
                description: String::new(),
                provider: None,
                model: None,
                reasoning_effort: None,
                provider_account_id: None,
                avatar_seed: "coder".to_string(),
                avatar_hue: None,
                avatar_color: None,
                initial_message: "Hello".to_string(),
                operation_id: None,
            })
            .unwrap();
        let tid = agents.ensure_thread_id(&agent.id).unwrap();
        let store = ThreadStore::new(db.clone());

        let mut m0 = ConversationMessage::new_simple(
            "m0",
            ConversationMessageAuthor::User,
            "Hello",
            "2026-11-25T00:00:01.000Z",
            ConversationMessageStatus::Completed,
        );
        m0.turn_id = Some("t1".to_string());
        let mut m1 = ConversationMessage::new_simple(
            "m1",
            ConversationMessageAuthor::Assistant,
            "Hi there!",
            "2026-11-25T00:00:02.000Z",
            ConversationMessageStatus::Completed,
        );
        m1.turn_id = Some("t1".to_string());

        let snap = ConversationSnapshot {
            agent_id: agent.id.clone(),
            thread_id: Some(tid.clone()),
            active_turn_id: None,
            revision: 0,
            messages: vec![m0.clone(), m1.clone()],
        };
        store
            .persist_conversation(&snap, "conversation.updated", &serde_json::json!({}), None)
            .unwrap();

        let ordinals: Vec<i64> = db
            .with_conn(|conn| {
                let mut s = conn.prepare(
                    "SELECT ordinal FROM projection_thread_messages WHERE thread_id = ?1 ORDER BY ordinal",
                )?;
                let rows = s.query_map(params![tid], |r| r.get(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        assert_eq!(ordinals, vec![0, 1]);

        let pinned_ok = store
            .pins()
            .set_by_person(&agent.id, None, "m1", true, None)
            .unwrap();
        assert!(pinned_ok.is_ok());
        let agent_unpin = store.pins().set_by_agent(&agent.id, &tid, "m1", false).unwrap();
        assert_eq!(agent_unpin, Err(PinRefusalReason::PinnedByPerson));

        let branch_msgs = store.agent_threads().branch_source(&tid, "m1").unwrap().unwrap();
        let copied = copy_branch_messages(&branch_msgs, &HashMap::new());
        let branched = store
            .agent_threads()
            .branch(&AgentThreadBranchInput {
                agent_id: agent.id.clone(),
                request_id: "req-1".to_string(),
                thread_id: "openbot-thread-branch-1".to_string(),
                title: "Branch".to_string(),
                source_thread_id: tid.clone(),
                source_message_id: "m1".to_string(),
                messages: copied,
            })
            .unwrap();
        assert!(branched.branch.is_some());

        store
            .reads()
            .mark_read_for_thread("member-1", Some(&tid), Some("m1"), Default::default())
            .unwrap();
        store
            .persist_conversation_changes(&PersistConversationChangesInput {
                agent_id: agent.id.clone(),
                thread_id: tid.clone(),
                active_turn_id: None,
                changed_messages: Vec::new(),
                removed_message_ids: Vec::new(),
                deleted_message_ids: vec!["m1".to_string()],
                event_type: "conversation.message-deleted".to_string(),
                detail: serde_json::json!({}),
                command_id: None,
            })
            .unwrap();
        assert_eq!(store.pins().list(&agent.id).unwrap().pins.len(), 0);
        let read_after = store
            .reads()
            .read_state_for_thread("member-1", Some(&tid), Default::default())
            .unwrap();
        assert_eq!(read_after.through_message_id.as_deref(), Some("m0"));

        let s1 = store
            .provider_sessions()
            .bind_provider_session(BindProviderSessionInput {
                thread_id: tid.clone(),
                provider: AgentProviderId::Codex,
                external_session_id: "ext-1".to_string(),
                model: "gpt-5.4".to_string(),
                effort: "medium".to_string(),
                resume_cursor: None,
            })
            .unwrap();
        let s2 = store
            .provider_sessions()
            .bind_provider_session(BindProviderSessionInput {
                thread_id: tid.clone(),
                provider: AgentProviderId::Codex,
                external_session_id: "ext-2".to_string(),
                model: "gpt-5.4".to_string(),
                effort: "high".to_string(),
                resume_cursor: None,
            })
            .unwrap();
        let all_sessions = store.provider_sessions().list_provider_sessions(&tid).unwrap();
        assert_eq!(all_sessions.len(), 2);
        assert_eq!(all_sessions[0].id, s1.id);
        assert_eq!(all_sessions[0].state, ProviderSessionState::Inactive);
        assert_eq!(all_sessions[1].id, s2.id);
        assert_eq!(all_sessions[1].state, ProviderSessionState::Active);

        for idx in 0..INPUT_LIMITS.agent_waits {
            store
                .waits()
                .open(OpenAgentWaitInput {
                    agent_id: agent.id.clone(),
                    request_message_id: format!("req-{idx}"),
                    request_text: "wait".to_string(),
                    note: "waiting".to_string(),
                    routine_id: None,
                })
                .unwrap();
        }
        let overflow_wait = store.waits().open(OpenAgentWaitInput {
            agent_id: agent.id.clone(),
            request_message_id: "req-over".to_string(),
            request_text: "wait".to_string(),
            note: "waiting".to_string(),
            routine_id: None,
        });
        assert!(matches!(overflow_wait, Err(StorageError::LimitExceeded(_))));
    }
}
