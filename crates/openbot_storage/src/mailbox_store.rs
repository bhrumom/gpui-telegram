//! `MailboxStore` & `MailboxProjection` — Multi-lane delivery queues (`projection_mailbox_messages`,
//! `projection_deliveries`, `projection_queue_state` with `__mailbox__` metadata row,
//! `projection_reactions`, `projection_attachments`, `file_deletion_outbox`), `10m`
//! `QUEUE_EDIT_HOLD_MS = 600_000` hold + `saveHash`, `1..=32` recipient fan-out barrier,
//! 3-line `Status / Result / Evidence` replies, `answer_failed_request`, `interrupt_agent`,
//! and `ChannelRequestReturns` (`"channel-request-result:{taskId}:{revision}:{state}"`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use openbot_contracts::agents::{AgentTaskAwaitedReply, AgentTaskAwaitedState};
use openbot_contracts::channels::{
    CHANNEL_HANDOFF_AUTHOR_PREFIX, ChannelMessage, ChannelTask, ChannelTaskState,
};
use openbot_contracts::conversation::{
    AGENT_EXCHANGE_ITEM_TYPE, AGENT_WELCOME_ITEM_TYPE, AgentExchangeDeliverySummary,
    AgentExchangeDirection, AgentExchangeSummary, ConversationDeliverySummary, ConversationMessage,
    ConversationMessageAuthor, ConversationMessageSender, ConversationMessageSource,
    ConversationMessageStatus, ConversationReaction, ConversationReactionActor,
    ConversationRoutineOrigin, is_message_reaction,
};
use openbot_contracts::limits::{ATTACHMENT_LIMITS, INPUT_LIMITS, is_bounded_string, is_identifier};
use openbot_contracts::queue::{
    AttachmentKind, AttachmentPreviewKind, AttachmentSummary, DraftAttachment, QUEUE_EDIT_HOLD_MS,
    QueueDelivery, QueueDeliveryStatus, QueueSender, QueueSnapshot, QueueSteerFallback,
};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::agent_store::{create_dir_0700, delete_receipts_of, hex_lower, now_iso, slice_utf16};
use crate::database::{OpenBotDatabase, OrchestrationEventInput};
use crate::error::{StorageError, StorageResult};

pub const MAILBOX_METADATA_ROW: &str = "__mailbox__";
pub const CHANNEL_LANE_PREFIX: &str = "\u{0001}channel:";
pub const MESSAGING_LANE: &str = "\u{0001}messaging";
pub const MAILBOX_CONVERSATION_MESSAGE_LIMIT: usize = 100;
pub const FAILED_REQUEST_KEY_PREFIX: &str = "failed-request:";
pub const AUTO_RESULT_KEY_PREFIX: &str = "auto-result:";
pub const RESTARTED_RESULT_KEY_PREFIX: &str = "restarted-result:";
pub const RESTARTED_TURN_KEY_PREFIX: &str = "restarted-turn:";
pub const CHANNEL_REQUEST_RETURN_KEY_PREFIX: &str = "channel-request-result:";
const FAILED_REQUEST_REASON_LIMIT: usize = 2_000;
const CHANNEL_RETURN_ANSWER_CHARS: usize = 90_000;
const CHANNEL_RETURN_ERROR_CHARS: usize = 2_000;

pub fn channel_lane(channel_id: &str) -> String {
    format!("{CHANNEL_LANE_PREFIX}{channel_id}")
}

pub fn lane_channel_id(lane: Option<&str>) -> Option<&str> {
    lane?.strip_prefix(CHANNEL_LANE_PREFIX)
}

/// Deterministic idempotency key `"channel-request-result:{taskId}:{revision}:{state}"`
/// (`channel-request-returns.ts:L170-L172`).
pub fn channel_request_return_key(
    task_id: &str,
    revision: u64,
    state: ChannelTaskState,
) -> String {
    let state_str = match state {
        ChannelTaskState::Queued => "queued",
        ChannelTaskState::Running => "running",
        ChannelTaskState::Waiting => "waiting",
        ChannelTaskState::Paused => "paused",
        ChannelTaskState::Completed => "completed",
        ChannelTaskState::Failed => "failed",
        ChannelTaskState::Cancelled => "cancelled",
    };
    format!("{CHANNEL_REQUEST_RETURN_KEY_PREFIX}{task_id}:{revision}:{state_str}")
}

/// SHA-256 hex digest of `JSON.stringify([text, keepAttachmentIds, attachmentDraftIds])`
/// (`mailbox-store.ts:L4308-L4312`).
pub fn queue_save_hash(
    text: &str,
    keep_attachment_ids: &[String],
    attachment_draft_ids: &[String],
) -> StorageResult<String> {
    let payload = serde_json::to_string(&(text, keep_attachment_ids, attachment_draft_ids))?;
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    Ok(hex_lower(&hasher.finalize()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueEditOutcome {
    Save,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredFinishedEdit {
    pub action: QueueEditOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitContinuation {
    pub turn_id: String,
    pub work_steps: String,
    pub previous_turn_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanContinuationMarker {
    pub turn_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagingOriginReplyQuote {
    pub author_name: String,
    pub text: String,
    pub sent_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagingOrigin {
    pub link_id: String,
    pub author_id: String,
    pub author_name: String,
    pub platform_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<MessagingOriginReplyQuote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAttachment {
    #[serde(flatten)]
    pub summary: AttachmentSummary,
    pub path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredDraft {
    #[serde(flatten)]
    pub attachment: StoredAttachment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_edit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve_on_restart: Option<bool>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredGeneratedAttachment {
    #[serde(flatten)]
    pub attachment: StoredAttachment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_thread_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMailboxMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging: Option<MessagingOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging_return: Option<MessagingOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_request_ids: Option<Vec<String>>,
    pub sender: QueueSender,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_member: Option<ConversationMessageSender>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester_ids: Option<Vec<String>>,
    pub text: String,
    pub attachments: Vec<StoredAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_recording: Option<StoredAttachment>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub reply_to_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expects_reply: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_setup: Option<bool>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMailboxDelivery {
    pub id: String,
    pub message_id: String,
    pub recipient_agent_id: String,
    #[serde(default)]
    pub queue_order: i64,
    pub status: QueueDeliveryStatus,
    #[serde(deserialize_with = "Option::deserialize")]
    pub turn_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub error: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_edit_outcomes: Option<BTreeMap<String, StoredFinishedEdit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steered_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_fallback: Option<QueueSteerFallback>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_after_usage_limit: Option<UsageLimitContinuation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_continuation: Option<PlanContinuationMarker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredReaction {
    pub agent_id: String,
    pub message_id: String,
    pub emoji: String,
    pub actor: ConversationReactionActor,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxProjectionState {
    #[serde(default = "default_mailbox_version")]
    pub version: u8,
    pub messages: Vec<StoredMailboxMessage>,
    pub deliveries: Vec<StoredMailboxDelivery>,
    pub drafts: Vec<StoredDraft>,
    #[serde(default)]
    pub generated_attachments: Vec<StoredGeneratedAttachment>,
    pub paused_agent_ids: Vec<String>,
    pub idempotency: BTreeMap<String, String>,
    pub reactions: Vec<StoredReaction>,
}

const fn default_mailbox_version() -> u8 {
    3
}

impl Default for MailboxProjectionState {
    fn default() -> Self {
        Self {
            version: 3,
            messages: Vec::new(),
            deliveries: Vec::new(),
            drafts: Vec::new(),
            generated_attachments: Vec::new(),
            paused_agent_ids: Vec::new(),
            idempotency: BTreeMap::new(),
            reactions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MailboxProjectionChanges {
    pub message_ids: HashSet<String>,
    pub delivery_ids: HashSet<String>,
    pub draft_ids: HashSet<String>,
    pub generated_attachment_ids: HashSet<String>,
    pub reaction_keys: HashSet<String>,
}

impl MailboxProjectionChanges {
    pub fn clear(&mut self) {
        self.message_ids.clear();
        self.delivery_ids.clear();
        self.draft_ids.clear();
        self.generated_attachment_ids.clear();
        self.reaction_keys.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedDeliveryReceiptItem {
    pub id: String,
    pub recipient_agent_id: String,
    pub status: QueueDeliveryStatus,
    pub position: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessageReceipt {
    pub message_id: String,
    pub deliveries: Vec<QueuedDeliveryReceiptItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAttachment {
    #[serde(flatten)]
    pub summary: AttachmentSummary,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryContext {
    pub delivery: QueueDelivery,
    pub managed_attachments: Vec<ManagedAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_after_usage_limit: Option<UsageLimitContinuation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_continuation: Option<PlanContinuationMarker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restarted_result: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restarted_turn_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskWork {
    pub queued_messages: Vec<(String, String, String)>,
    pub queue: Vec<QueueDelivery>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwaitedThreadRequest {
    pub delivery_id: String,
    pub sender_agent_id: String,
    pub task_id: String,
    pub reply: AgentTaskAwaitedReply,
}

#[derive(Debug, Clone, Default)]
pub struct EnqueueInput {
    pub channel_id: Option<String>,
    pub messaging: Option<MessagingOrigin>,
    pub messaging_return: Option<MessagingOrigin>,
    pub task_id: Option<String>,
    pub sender_task_id: Option<String>,
    pub held_request_ids: Vec<String>,
    pub sender: Option<QueueSender>,
    pub sender_member: Option<ConversationMessageSender>,
    pub requester_ids: Vec<String>,
    pub recipient_agent_ids: Vec<String>,
    pub text: String,
    pub reply_to_message_id: Option<String>,
    pub expects_reply: Option<bool>,
    pub welcome: bool,
    pub agent_setup: bool,
    pub draft_ids: Vec<String>,
    pub source_paths: Vec<PathBuf>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SendAgentMessageInput {
    pub sender_agent_id: String,
    pub recipient_agent_ids: Vec<String>,
    pub text: String,
    pub reply_to_message_id: Option<String>,
    pub expects_reply: Option<bool>,
    pub sender_task_id: Option<String>,
    pub requester_ids: Vec<String>,
    pub source_paths: Vec<PathBuf>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeammateReplyStatus {
    Done,
    Partial,
    Blocked,
}

impl TeammateReplyStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Partial => "partial",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "done" => Some(Self::Done),
            "partial" => Some(Self::Partial),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTeammateReply {
    pub status: TeammateReplyStatus,
    pub result: String,
    pub evidence: String,
    pub unblock: Option<String>,
}

pub fn format_teammate_reply(reply: &ParsedTeammateReply) -> String {
    let mut lines = vec![
        format!("Status: {}", reply.status.as_str()),
        format!("Result: {}", reply.result),
        format!("Evidence: {}", reply.evidence),
    ];
    if let Some(unblock) = &reply.unblock {
        lines.push(format!("Unblock: {unblock}"));
    }
    lines.join("\n")
}

/// Parses 3-line (`Status: done|partial|blocked` / `Result: ...` / `Evidence: ...` [+ optional `Unblock: ...`])
/// teammate replies (`openbot-tool-router.ts`).
pub fn parse_teammate_reply(text: &str) -> Option<ParsedTeammateReply> {
    let mut lines = text.lines();
    let status_line = lines.next()?.trim();
    let result_line = lines.next()?.trim();
    let evidence_line = lines.next()?.trim();
    let status = TeammateReplyStatus::parse(status_line.strip_prefix("Status:")?)?;
    let result = result_line.strip_prefix("Result:")?.trim().to_string();
    let evidence = evidence_line.strip_prefix("Evidence:")?.trim().to_string();
    if result.is_empty() || evidence.is_empty() {
        return None;
    }
    let unblock = lines
        .next()
        .and_then(|l| l.trim().strip_prefix("Unblock:"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some(ParsedTeammateReply {
        status,
        result,
        evidence,
        unblock,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptRefusal {
    SelfTarget,
    UseChannelTaskControlsWork,
    InterruptSharedWork,
    InterruptOtherWork,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxPendingFileDeletion {
    pub id: String,
    pub path: String,
}

pub fn mailbox_reaction_row_key(
    agent_id: &str,
    message_id: &str,
    actor: &ConversationReactionActor,
) -> StorageResult<String> {
    let (actor_kind, actor_agent_id) = match actor {
        ConversationReactionActor::User => ("user", ""),
        ConversationReactionActor::Agent { agent_id } => ("agent", agent_id.as_str()),
    };
    Ok(serde_json::to_string(&[
        agent_id,
        message_id,
        actor_kind,
        actor_agent_id,
    ])?)
}

/// Incremental SQLite projection writer over `projection_mailbox_messages`, `projection_deliveries`,
/// `projection_queue_state` (`__mailbox__`), `projection_reactions`, `projection_attachments`,
/// and `file_deletion_outbox` (`src/backend/database/mailbox-projection.ts`).
#[derive(Clone)]
pub struct MailboxProjection {
    database: OpenBotDatabase,
}

impl MailboxProjection {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn replace_mailbox_state(
        &self,
        command_id: &str,
        state: &MailboxProjectionState,
        event_type: &str,
        file_deletions: &[String],
    ) -> StorageResult<bool> {
        self.write_projection(command_id, state, event_type, file_deletions, |conn, seq| {
            let kept_deliveries: HashSet<&str> =
                state.deliveries.iter().map(|d| d.id.as_str()).collect();
            delete_single_key_not_in(conn, "projection_deliveries", "delivery_id", &kept_deliveries, "1=1")?;

            let kept_messages: HashSet<&str> =
                state.messages.iter().map(|m| m.id.as_str()).collect();
            delete_single_key_not_in(conn, "projection_mailbox_messages", "message_id", &kept_messages, "1=1")?;

            let kept_reactions: HashSet<String> = state
                .reactions
                .iter()
                .map(|r| mailbox_reaction_row_key(&r.agent_id, &r.message_id, &r.actor))
                .collect::<StorageResult<HashSet<_>>>()?;
            delete_reactions_not_in(conn, &kept_reactions)?;

            let att_rows = all_mailbox_attachment_rows(state)?;
            let kept_atts: HashSet<&str> = att_rows.iter().map(|r| r.0.as_str()).collect();
            delete_single_key_not_in(
                conn,
                "projection_attachments",
                "attachment_id",
                &kept_atts,
                "owner_kind IN ('mailbox-message', 'draft', 'generated')",
            )?;

            for msg in &state.messages {
                upsert_mailbox_message_row(conn, msg, seq)?;
            }
            for del in &state.deliveries {
                upsert_mailbox_delivery_row(conn, del, seq)?;
            }
            for rxn in &state.reactions {
                upsert_mailbox_reaction_row(conn, rxn, seq)?;
            }
            for row in &att_rows {
                upsert_mailbox_attachment_row(conn, row, seq)?;
            }
            Ok(())
        })
    }

    pub fn apply_mailbox_changes(
        &self,
        command_id: &str,
        state: &MailboxProjectionState,
        changes: &MailboxProjectionChanges,
        event_type: &str,
        file_deletions: &[String],
    ) -> StorageResult<bool> {
        self.write_projection(command_id, state, event_type, file_deletions, |conn, seq| {
            let msgs_by_id: HashMap<&str, &StoredMailboxMessage> = state
                .messages
                .iter()
                .filter(|m| changes.message_ids.contains(&m.id))
                .map(|m| (m.id.as_str(), m))
                .collect();
            let dels_by_id: HashMap<&str, &StoredMailboxDelivery> = state
                .deliveries
                .iter()
                .filter(|d| changes.delivery_ids.contains(&d.id))
                .map(|d| (d.id.as_str(), d))
                .collect();
            let mut rxns_by_key: HashMap<String, &StoredReaction> = HashMap::new();
            for r in &state.reactions {
                let k = mailbox_reaction_row_key(&r.agent_id, &r.message_id, &r.actor)?;
                if changes.reaction_keys.contains(&k) {
                    rxns_by_key.insert(k, r);
                }
            }
            let drafts_by_id: HashMap<&str, &StoredDraft> = state
                .drafts
                .iter()
                .filter(|d| changes.draft_ids.contains(&d.attachment.summary.id))
                .map(|d| (d.attachment.summary.id.as_str(), d))
                .collect();
            let gen_by_id: HashMap<&str, &StoredGeneratedAttachment> = state
                .generated_attachments
                .iter()
                .filter(|g| changes.generated_attachment_ids.contains(&g.attachment.summary.id))
                .map(|g| (g.attachment.summary.id.as_str(), g))
                .collect();

            for did in &changes.delivery_ids {
                if !dels_by_id.contains_key(did.as_str()) {
                    conn.execute(
                        "DELETE FROM projection_deliveries WHERE delivery_id = ?1",
                        params![did],
                    )?;
                }
            }
            for mid in &changes.message_ids {
                let kept_msg = msgs_by_id.get(mid.as_str()).copied();
                let kept_att_ids: HashSet<&str> = kept_msg
                    .map(|m| {
                        m.attachments
                            .iter()
                            .map(|a| a.summary.id.as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                for old_att_id in stored_mailbox_message_attachment_ids(conn, mid)? {
                    if !kept_att_ids.contains(old_att_id.as_str()) {
                        conn.execute(
                            "DELETE FROM projection_attachments
                             WHERE attachment_id = ?1 AND owner_kind = 'mailbox-message' AND owner_id = ?2",
                            params![old_att_id, mid],
                        )?;
                    }
                }
                if kept_msg.is_none() {
                    conn.execute(
                        "DELETE FROM projection_mailbox_messages WHERE message_id = ?1",
                        params![mid],
                    )?;
                }
            }
            for rkey in &changes.reaction_keys {
                if !rxns_by_key.contains_key(rkey) {
                    let parts: [String; 4] = serde_json::from_str(rkey)?;
                    conn.execute(
                        "DELETE FROM projection_reactions
                         WHERE agent_id = ?1 AND message_id = ?2 AND actor_kind = ?3 AND actor_agent_id = ?4",
                        params![parts[0], parts[1], parts[2], parts[3]],
                    )?;
                }
            }
            for draft_id in &changes.draft_ids {
                if !drafts_by_id.contains_key(draft_id.as_str()) {
                    conn.execute(
                        "DELETE FROM projection_attachments
                         WHERE attachment_id = ?1 AND owner_kind = 'draft' AND owner_id = ?1",
                        params![draft_id],
                    )?;
                }
            }
            for gen_id in &changes.generated_attachment_ids {
                if !gen_by_id.contains_key(gen_id.as_str()) {
                    conn.execute(
                        "DELETE FROM projection_attachments
                         WHERE attachment_id = ?1 AND owner_kind = 'generated' AND owner_id = ?1",
                        params![gen_id],
                    )?;
                }
            }

            for msg in msgs_by_id.values() {
                upsert_mailbox_message_row(conn, msg, seq)?;
                for row in message_attachment_rows(msg)? {
                    upsert_mailbox_attachment_row(conn, &row, seq)?;
                }
            }
            for del in dels_by_id.values() {
                upsert_mailbox_delivery_row(conn, del, seq)?;
            }
            for rxn in rxns_by_key.values() {
                upsert_mailbox_reaction_row(conn, rxn, seq)?;
            }
            for draft in drafts_by_id.values() {
                upsert_mailbox_attachment_row(conn, &draft_attachment_row(draft)?, seq)?;
            }
            for g in gen_by_id.values() {
                upsert_mailbox_attachment_row(conn, &generated_attachment_row(g)?, seq)?;
            }
            Ok(())
        })
    }

    fn write_projection<F>(
        &self,
        command_id: &str,
        state: &MailboxProjectionState,
        event_type: &str,
        file_deletions: &[String],
        write_rows: F,
    ) -> StorageResult<bool>
    where
        F: FnOnce(&Connection, i64) -> StorageResult<()>,
    {
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "mailbox".to_string(),
            aggregate_id: "mailbox".to_string(),
            event_type: event_type.to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({
                "messages": state.messages.len(),
                "deliveries": state.deliveries.len(),
            }),
        }];
        let mut wrote = false;
        let _ = self.database.dispatch_with_projection(
            command_id,
            None,
            &events,
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(0);
                let older_cmds: Vec<String> = {
                    let mut stmt = conn.prepare(
                        "DELETE FROM orchestration_events
                         WHERE aggregate_type = 'mailbox' AND aggregate_id = 'mailbox' AND sequence < ?1
                         RETURNING command_id",
                    )?;
                    let rows = stmt.query_map(params![seq], |r| r.get(0))?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                delete_receipts_of(conn, &older_cmds)?;
                write_rows(conn, seq)?;

                let mut keep_queue = Vec::with_capacity(1 + state.paused_agent_ids.len());
                keep_queue.push(MAILBOX_METADATA_ROW.to_string());
                keep_queue.extend(state.paused_agent_ids.iter().cloned());
                conn.execute(
                    "DELETE FROM projection_queue_state
                     WHERE agent_id NOT IN (SELECT value FROM json_each(?1))",
                    params![serde_json::to_string(&keep_queue)?],
                )?;
                let meta_json = serde_json::to_string(&serde_json::json!({
                    "idempotency": state.idempotency,
                }))?;
                upsert_queue_state_row(conn, MAILBOX_METADATA_ROW, 0, &meta_json, seq)?;
                for aid in &state.paused_agent_ids {
                    upsert_queue_state_row(conn, aid, 1, "{}", seq)?;
                }
                let outbox_ts = now_iso();
                for path in file_deletions {
                    OpenBotDatabase::enqueue_file_deletion(
                        conn,
                        &Uuid::new_v4().to_string(),
                        path,
                        event_type,
                        &outbox_ts,
                    )?;
                }
                wrote = true;
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(wrote)
    }

    pub fn read_mailbox_state(&self) -> StorageResult<Option<MailboxProjectionState>> {
        self.database.with_conn(|conn| {
            let marker_opt: Option<String> = conn
                .prepare_cached(
                    "SELECT metadata_json FROM projection_queue_state WHERE agent_id = ?1",
                )?
                .query_row(params![MAILBOX_METADATA_ROW], |r| r.get(0))
                .optional()?;
            let Some(marker_raw) = marker_opt else {
                return Ok(None);
            };
            let meta_val: serde_json::Value = serde_json::from_str(&marker_raw)?;
            let mut idempotency = BTreeMap::new();
            if let Some(idem_obj) = meta_val.get("idempotency").and_then(|v| v.as_object()) {
                for (k, v) in idem_obj {
                    if let Some(s) = v.as_str() {
                        idempotency.insert(k.clone(), s.to_string());
                    }
                }
            }

            let messages = query_json_vec::<StoredMailboxMessage>(
                conn,
                "SELECT message_json FROM projection_mailbox_messages ORDER BY created_at, message_id",
            )?;
            let deliveries = query_json_vec::<StoredMailboxDelivery>(
                conn,
                "SELECT delivery_json FROM projection_deliveries ORDER BY created_at, delivery_id",
            )?;
            let drafts = query_json_vec::<StoredDraft>(
                conn,
                "SELECT metadata_json FROM projection_attachments WHERE owner_kind = 'draft'",
            )?;
            let generated_attachments = query_json_vec::<StoredGeneratedAttachment>(
                conn,
                "SELECT metadata_json FROM projection_attachments WHERE owner_kind = 'generated'",
            )?;
            let paused_agent_ids: Vec<String> = {
                let mut stmt = conn
                    .prepare_cached("SELECT agent_id FROM projection_queue_state WHERE paused = 1")?;
                let rows = stmt.query_map([], |r| r.get(0))?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            let reactions: Vec<StoredReaction> = {
                let mut stmt = conn.prepare_cached(
                    "SELECT agent_id, message_id, emoji, actor_kind, actor_agent_id, updated_at
                     FROM projection_reactions",
                )?;
                let rows = stmt.query_map([], |r| {
                    let agent_id: String = r.get(0)?;
                    let message_id: String = r.get(1)?;
                    let emoji: String = r.get(2)?;
                    let actor_kind: String = r.get(3)?;
                    let actor_agent_id: String = r.get(4)?;
                    let updated_at: String = r.get(5)?;
                    let actor = if actor_kind == "agent" {
                        ConversationReactionActor::Agent {
                            agent_id: actor_agent_id,
                        }
                    } else {
                        ConversationReactionActor::User
                    };
                    Ok(StoredReaction {
                        agent_id,
                        message_id,
                        emoji,
                        actor,
                        updated_at,
                    })
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };

            Ok(Some(normalize_mailbox_state(MailboxProjectionState {
                version: 3,
                messages,
                deliveries,
                drafts,
                generated_attachments,
                paused_agent_ids,
                idempotency,
                reactions,
            })))
        })
    }

    pub fn pending_file_deletions(&self) -> StorageResult<Vec<MailboxPendingFileDeletion>> {
        self.database.with_conn(|conn| {
            let mut stmt =
                conn.prepare_cached("SELECT id, path FROM file_deletion_outbox ORDER BY created_at")?;
            let rows = stmt.query_map([], |r| {
                Ok(MailboxPendingFileDeletion {
                    id: r.get(0)?,
                    path: r.get(1)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn complete_file_deletion(&self, id: &str) -> StorageResult<()> {
        self.database.with_conn(|conn| {
            conn.execute("DELETE FROM file_deletion_outbox WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn fail_file_deletion(&self, id: &str, error: &str) -> StorageResult<()> {
        let clipped = slice_utf16(error, 2_000);
        self.database.with_conn(|conn| {
            conn.execute(
                "UPDATE file_deletion_outbox SET attempts = attempts + 1, last_error = ?1 WHERE id = ?2",
                params![clipped, id],
            )?;
            Ok(())
        })
    }
}

type MailboxAttachmentTuple = (String, &'static str, String, String, String, String, String);

fn query_json_vec<T: for<'de> Deserialize<'de>>(
    conn: &Connection,
    sql: &str,
) -> StorageResult<Vec<T>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for raw in rows {
        if let Ok(parsed) = serde_json::from_str::<T>(&raw?) {
            out.push(parsed);
        }
    }
    Ok(out)
}

fn normalize_mailbox_state(mut state: MailboxProjectionState) -> MailboxProjectionState {
    let mut next_order_by_agent: HashMap<String, i64> = HashMap::new();
    for del in &mut state.deliveries {
        let fallback = next_order_by_agent
            .get(&del.recipient_agent_id)
            .copied()
            .unwrap_or(0);
        next_order_by_agent.insert(
            del.recipient_agent_id.clone(),
            fallback.max(del.queue_order + 1),
        );
    }
    state.version = 3;
    state
}

fn sender_kind_and_agent(sender: &QueueSender) -> (&'static str, Option<&str>) {
    match sender {
        QueueSender::User => ("user", None),
        QueueSender::Agent { agent_id } => ("agent", Some(agent_id.as_str())),
        QueueSender::Routine { .. } => ("routine", None),
    }
}

fn delivery_status_str(status: QueueDeliveryStatus) -> &'static str {
    match status {
        QueueDeliveryStatus::Queued => "queued",
        QueueDeliveryStatus::Starting => "starting",
        QueueDeliveryStatus::Running => "running",
        QueueDeliveryStatus::Completed => "completed",
        QueueDeliveryStatus::Failed => "failed",
        QueueDeliveryStatus::Interrupted => "interrupted",
        QueueDeliveryStatus::Cancelled => "cancelled",
    }
}

fn upsert_mailbox_message_row(
    conn: &Connection,
    msg: &StoredMailboxMessage,
    seq: i64,
) -> StorageResult<()> {
    let (sender_kind, sender_agent_id) = sender_kind_and_agent(&msg.sender);
    let json = serde_json::to_string(msg)?;
    conn.execute(
        "INSERT INTO projection_mailbox_messages
           (message_id, sender_kind, sender_agent_id, text, reply_to_message_id, created_at, message_json, last_event_sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(message_id) DO UPDATE SET
           sender_kind = excluded.sender_kind,
           sender_agent_id = excluded.sender_agent_id,
           text = excluded.text,
           reply_to_message_id = excluded.reply_to_message_id,
           created_at = excluded.created_at,
           message_json = excluded.message_json,
           last_event_sequence = excluded.last_event_sequence
         WHERE projection_mailbox_messages.message_json IS NOT excluded.message_json",
        params![
            msg.id,
            sender_kind,
            sender_agent_id,
            msg.text,
            msg.reply_to_message_id,
            msg.created_at,
            json,
            seq
        ],
    )?;
    Ok(())
}

fn upsert_mailbox_delivery_row(
    conn: &Connection,
    del: &StoredMailboxDelivery,
    seq: i64,
) -> StorageResult<()> {
    let json = serde_json::to_string(del)?;
    conn.execute(
        "INSERT INTO projection_deliveries
           (delivery_id, message_id, recipient_agent_id, status, turn_id, error, created_at, delivery_json, last_event_sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(delivery_id) DO UPDATE SET
           message_id = excluded.message_id,
           recipient_agent_id = excluded.recipient_agent_id,
           status = excluded.status,
           turn_id = excluded.turn_id,
           error = excluded.error,
           created_at = excluded.created_at,
           delivery_json = excluded.delivery_json,
           last_event_sequence = excluded.last_event_sequence
         WHERE projection_deliveries.delivery_json IS NOT excluded.delivery_json",
        params![
            del.id,
            del.message_id,
            del.recipient_agent_id,
            delivery_status_str(del.status),
            del.turn_id,
            del.error,
            del.created_at,
            json,
            seq
        ],
    )?;
    Ok(())
}

fn upsert_queue_state_row(
    conn: &Connection,
    agent_id: &str,
    paused: i64,
    metadata_json: &str,
    seq: i64,
) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO projection_queue_state
           (agent_id, paused, metadata_json, last_event_sequence)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(agent_id) DO UPDATE SET
           paused = excluded.paused,
           metadata_json = excluded.metadata_json,
           last_event_sequence = excluded.last_event_sequence
         WHERE projection_queue_state.paused IS NOT excluded.paused
            OR projection_queue_state.metadata_json IS NOT excluded.metadata_json",
        params![agent_id, paused, metadata_json, seq],
    )?;
    Ok(())
}

fn upsert_mailbox_reaction_row(
    conn: &Connection,
    rxn: &StoredReaction,
    seq: i64,
) -> StorageResult<()> {
    let (actor_kind, actor_agent_id) = match &rxn.actor {
        ConversationReactionActor::User => ("user", ""),
        ConversationReactionActor::Agent { agent_id } => ("agent", agent_id.as_str()),
    };
    conn.execute(
        "INSERT INTO projection_reactions
           (agent_id, message_id, actor_kind, actor_agent_id, emoji, updated_at, last_event_sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(agent_id, message_id, actor_kind, actor_agent_id) DO UPDATE SET
           emoji = excluded.emoji,
           updated_at = excluded.updated_at,
           last_event_sequence = excluded.last_event_sequence
         WHERE projection_reactions.emoji IS NOT excluded.emoji
            OR projection_reactions.updated_at IS NOT excluded.updated_at",
        params![
            rxn.agent_id,
            rxn.message_id,
            actor_kind,
            actor_agent_id,
            rxn.emoji,
            rxn.updated_at,
            seq
        ],
    )?;
    Ok(())
}

fn upsert_mailbox_attachment_row(
    conn: &Connection,
    row: &MailboxAttachmentTuple,
    seq: i64,
) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO projection_attachments
           (attachment_id, owner_kind, owner_id, name, path, metadata_json, created_at, last_event_sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(attachment_id) DO UPDATE SET
           owner_kind = excluded.owner_kind,
           owner_id = excluded.owner_id,
           name = excluded.name,
           path = excluded.path,
           metadata_json = excluded.metadata_json,
           created_at = excluded.created_at,
           last_event_sequence = excluded.last_event_sequence
         WHERE projection_attachments.owner_kind IS NOT excluded.owner_kind
            OR projection_attachments.owner_id IS NOT excluded.owner_id
            OR projection_attachments.name IS NOT excluded.name
            OR projection_attachments.path IS NOT excluded.path
            OR projection_attachments.metadata_json IS NOT excluded.metadata_json",
        params![row.0, row.1, row.2, row.3, row.4, row.5, row.6, seq],
    )?;
    Ok(())
}

fn delete_single_key_not_in(
    conn: &Connection,
    table: &str,
    col: &str,
    kept: &HashSet<&str>,
    where_clause: &str,
) -> StorageResult<()> {
    let existing: Vec<String> = {
        let mut stmt = conn.prepare(&format!("SELECT {col} FROM {table} WHERE {where_clause}"))?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut del = conn.prepare(&format!("DELETE FROM {table} WHERE {col} = ?1"))?;
    for key in existing {
        if !kept.contains(key.as_str()) {
            del.execute(params![key])?;
        }
    }
    Ok(())
}

fn delete_reactions_not_in(conn: &Connection, kept: &HashSet<String>) -> StorageResult<()> {
    let existing: Vec<(String, String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT agent_id, message_id, actor_kind, actor_agent_id FROM projection_reactions",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut del = conn.prepare_cached(
        "DELETE FROM projection_reactions
         WHERE agent_id = ?1 AND message_id = ?2 AND actor_kind = ?3 AND actor_agent_id = ?4",
    )?;
    for (aid, mid, ak, aaid) in existing {
        let k = serde_json::to_string(&[aid.as_str(), mid.as_str(), ak.as_str(), aaid.as_str()])?;
        if !kept.contains(&k) {
            del.execute(params![aid, mid, ak, aaid])?;
        }
    }
    Ok(())
}

fn stored_mailbox_message_attachment_ids(
    conn: &Connection,
    message_id: &str,
) -> StorageResult<Vec<String>> {
    let raw_opt: Option<String> = conn
        .prepare_cached("SELECT message_json FROM projection_mailbox_messages WHERE message_id = ?1")?
        .query_row(params![message_id], |r| r.get(0))
        .optional()?;
    let Some(raw) = raw_opt else {
        return Ok(Vec::new());
    };
    let val: serde_json::Value = serde_json::from_str(&raw)?;
    Ok(val
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("id").and_then(|x| x.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

fn all_mailbox_attachment_rows(
    state: &MailboxProjectionState,
) -> StorageResult<Vec<MailboxAttachmentTuple>> {
    let mut rows = Vec::new();
    for msg in &state.messages {
        rows.extend(message_attachment_rows(msg)?);
    }
    for draft in &state.drafts {
        rows.push(draft_attachment_row(draft)?);
    }
    for g in &state.generated_attachments {
        rows.push(generated_attachment_row(g)?);
    }
    Ok(rows)
}

fn message_attachment_rows(msg: &StoredMailboxMessage) -> StorageResult<Vec<MailboxAttachmentTuple>> {
    msg.attachments
        .iter()
        .map(|a| {
            Ok((
                a.summary.id.clone(),
                "mailbox-message",
                msg.id.clone(),
                a.summary.name.clone(),
                a.path.clone(),
                serde_json::to_string(a)?,
                msg.created_at.clone(),
            ))
        })
        .collect()
}

fn draft_attachment_row(draft: &StoredDraft) -> StorageResult<MailboxAttachmentTuple> {
    Ok((
        draft.attachment.summary.id.clone(),
        "draft",
        draft.attachment.summary.id.clone(),
        draft.attachment.summary.name.clone(),
        draft.attachment.path.clone(),
        serde_json::to_string(draft)?,
        draft.created_at.clone(),
    ))
}

fn generated_attachment_row(
    g: &StoredGeneratedAttachment,
) -> StorageResult<MailboxAttachmentTuple> {
    Ok((
        g.attachment.summary.id.clone(),
        "generated",
        g.attachment.summary.id.clone(),
        g.attachment.summary.name.clone(),
        g.attachment.path.clone(),
        serde_json::to_string(g)?,
        now_iso(),
    ))
}

#[derive(Debug, Clone)]
struct EditHoldEntry {
    edit_id: String,
    started_at_ms: u64,
}

struct MailboxMutable {
    state: MailboxProjectionState,
    changes: MailboxProjectionChanges,
    edit_holds: HashMap<String, EditHoldEntry>,
}

struct MailboxStoreInner {
    database: OpenBotDatabase,
    projection: MailboxProjection,
    drafts_root: PathBuf,
    transfers_root: PathBuf,
    mutable: Mutex<MailboxMutable>,
}

/// High-level `MailboxStore` over `OpenBotDatabase` (`src/backend/mailbox-store.ts`).
#[derive(Clone)]
pub struct MailboxStore {
    inner: Arc<MailboxStoreInner>,
}

impl MailboxStore {
    pub fn open(
        database: OpenBotDatabase,
        shared_root: impl AsRef<Path>,
    ) -> StorageResult<Self> {
        let user_data = database.data_dir().to_path_buf();
        Self::open_with_roots(user_data, shared_root, database)
    }

    pub fn open_with_roots(
        user_data_path: impl AsRef<Path>,
        shared_root: impl AsRef<Path>,
        database: OpenBotDatabase,
    ) -> StorageResult<Self> {
        let drafts_root = user_data_path.as_ref().join("attachment-drafts");
        let transfers_root = shared_root.as_ref().join("Transfers");
        create_dir_0700(&drafts_root)?;
        create_dir_0700(&transfers_root)?;

        let projection = MailboxProjection::new(database.clone());
        let mut state = projection.read_mailbox_state()?.unwrap_or_default();
        let mut changes = MailboxProjectionChanges::default();
        let now_ms = current_unix_ms();
        let mut edit_holds = HashMap::new();
        for del in &state.deliveries {
            if del.status == QueueDeliveryStatus::Queued
                && let Some(eid) = &del.edit_id
            {
                edit_holds.insert(
                    del.id.clone(),
                    EditHoldEntry {
                        edit_id: eid.clone(),
                        started_at_ms: now_ms,
                    },
                );
            }
        }
        let active_edits: HashSet<&str> =
            edit_holds.values().map(|h| h.edit_id.as_str()).collect();
        let prev_draft_len = state.drafts.len();
        let mut removed_draft_paths = Vec::new();
        let mut kept_drafts = Vec::new();
        for draft in state.drafts.drain(..) {
            let keep = draft.preserve_on_restart == Some(true)
                || draft
                    .owner_edit_id
                    .as_deref()
                    .is_some_and(|eid| active_edits.contains(eid));
            if keep {
                kept_drafts.push(draft);
            } else {
                changes.draft_ids.insert(draft.attachment.summary.id.clone());
                removed_draft_paths.push(draft.attachment.path.clone());
            }
        }
        state.drafts = kept_drafts;
        if state.drafts.len() != prev_draft_len {
            let cmd = format!("mailbox:mailbox.drafts-cleared:{}", Uuid::new_v4());
            projection.apply_mailbox_changes(&cmd, &state, &changes, "mailbox.drafts-cleared", &[])?;
            changes.clear();
            for p in removed_draft_paths {
                if let Some(dir) = Path::new(&p).parent() {
                    let _ = fs::remove_dir_all(dir);
                }
            }
        }

        let store = Self {
            inner: Arc::new(MailboxStoreInner {
                database,
                projection,
                drafts_root,
                transfers_root,
                mutable: Mutex::new(MailboxMutable {
                    state,
                    changes,
                    edit_holds,
                }),
            }),
        };
        store.drain_file_deletion_outbox()?;
        Ok(store)
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.inner.database
    }

    pub fn projection(&self) -> &MailboxProjection {
        &self.inner.projection
    }

    pub fn drain_file_deletion_outbox(&self) -> StorageResult<()> {
        for item in self.inner.projection.pending_file_deletions()? {
            let p = Path::new(&item.path);
            let remove_res = if p.is_dir() {
                fs::remove_dir_all(p)
            } else if p.exists() {
                fs::remove_file(p)
            } else {
                Ok(())
            };
            match remove_res {
                Ok(()) => self.inner.projection.complete_file_deletion(&item.id)?,
                Err(err) => {
                    self.inner
                        .projection
                        .fail_file_deletion(&item.id, &err.to_string())?;
                }
            }
        }
        Ok(())
    }

    pub fn prepare_attachment_drafts(&self, paths: &[PathBuf]) -> StorageResult<Vec<DraftAttachment>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        if paths.len() > INPUT_LIMITS.attachments {
            return Err(StorageError::LimitExceeded(format!(
                "Too many attachments (max {}).",
                INPUT_LIMITS.attachments
            )));
        }
        let mut guard = self.inner.mutable.lock();
        if guard.state.drafts.len() + paths.len() > INPUT_LIMITS.draft_attachments {
            return Err(StorageError::LimitExceeded(format!(
                "Draft attachment limit of {} reached.",
                INPUT_LIMITS.draft_attachments
            )));
        }
        let mut total_bytes = 0usize;
        let mut created = Vec::with_capacity(paths.len());
        let mut created_dirs = Vec::with_capacity(paths.len());

        let prep_res = (|| -> StorageResult<()> {
            for src in paths {
                let bytes = fs::read(src)?;
                if bytes.is_empty() || bytes.len() > ATTACHMENT_LIMITS.file_bytes {
                    return Err(StorageError::Validation(
                        "Attachment size is invalid.".to_string(),
                    ));
                }
                total_bytes = total_bytes.saturating_add(bytes.len());
                if total_bytes > ATTACHMENT_LIMITS.total_bytes {
                    return Err(StorageError::LimitExceeded(
                        "Total attachment size limit exceeded.".to_string(),
                    ));
                }
                let id = Uuid::new_v4().to_string();
                let dir = self.inner.drafts_root.join(&id);
                create_dir_0700(&dir)?;
                created_dirs.push(dir.clone());
                let name = sanitize_file_name(src);
                let target = dir.join(&name);
                fs::write(&target, &bytes)?;
                let stored = build_stored_attachment(&id, &name, &target, &bytes);
                created.push(StoredDraft {
                    attachment: stored,
                    owner_edit_id: None,
                    preserve_on_restart: None,
                    created_at: now_iso(),
                });
            }
            Ok(())
        })();
        if let Err(err) = prep_res {
            for d in created_dirs {
                let _ = fs::remove_dir_all(d);
            }
            return Err(err);
        }

        let summaries: Vec<DraftAttachment> =
            created.iter().map(|d| d.attachment.summary.clone()).collect();
        let added_ids: HashSet<String> =
            created.iter().map(|d| d.attachment.summary.id.clone()).collect();
        for id in &added_ids {
            guard.changes.draft_ids.insert(id.clone());
        }
        guard.state.drafts.extend(created);
        if let Err(err) = self.persist_locked(&mut guard, "attachments.prepared", None, &[], false) {
            guard
                .state
                .drafts
                .retain(|d| !added_ids.contains(&d.attachment.summary.id));
            for d in created_dirs {
                let _ = fs::remove_dir_all(d);
            }
            return Err(err);
        }
        Ok(summaries)
    }

    pub fn discard_draft(&self, draft_id: &str) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let Some(pos) = guard
            .state
            .drafts
            .iter()
            .position(|d| d.attachment.summary.id == draft_id)
        else {
            return Ok(());
        };
        let removed = guard.state.drafts.remove(pos);
        guard.changes.draft_ids.insert(draft_id.to_string());
        if let Err(err) =
            self.persist_locked(&mut guard, "attachment-draft.discarded", None, &[], false)
        {
            guard.state.drafts.insert(pos, removed);
            return Err(err);
        }
        if let Some(dir) = Path::new(&removed.attachment.path).parent() {
            let _ = fs::remove_dir_all(dir);
        }
        Ok(())
    }

    pub fn store_generated_attachment(
        &self,
        name: &str,
        bytes: &[u8],
        owner_agent_id: Option<&str>,
        owner_thread_id: Option<&str>,
    ) -> StorageResult<AttachmentSummary> {
        if bytes.is_empty() || bytes.len() > ATTACHMENT_LIMITS.file_bytes {
            return Err(StorageError::Validation(
                "Generated attachment size is invalid.".to_string(),
            ));
        }
        let id = Uuid::new_v4().to_string();
        let dir = self.inner.transfers_root.join(format!("generated-{id}"));
        create_dir_0700(&dir)?;
        let safe_name = sanitize_name_str(name);
        let target = dir.join(&safe_name);
        fs::write(&target, bytes)?;
        let stored = StoredGeneratedAttachment {
            attachment: build_stored_attachment(&id, &safe_name, &target, bytes),
            owner_agent_id: owner_agent_id.map(str::to_string),
            owner_thread_id: owner_thread_id.map(str::to_string),
        };
        let summary = stored.attachment.summary.clone();
        let mut guard = self.inner.mutable.lock();
        guard.changes.generated_attachment_ids.insert(id.clone());
        guard.state.generated_attachments.push(stored);
        if let Err(err) = self.persist_locked(&mut guard, "attachment.generated", None, &[], false) {
            guard
                .state
                .generated_attachments
                .retain(|g| g.attachment.summary.id != id);
            let _ = fs::remove_dir_all(&dir);
            return Err(err);
        }
        Ok(summary)
    }

    pub fn pause_queue(&self, agent_id: &str) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        if guard.state.paused_agent_ids.iter().any(|a| a == agent_id) {
            return Ok(());
        }
        guard.state.paused_agent_ids.push(agent_id.to_string());
        self.persist_locked(&mut guard, "queue.paused", None, &[], false)
    }

    pub fn resume_queue(&self, agent_id: &str) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let before = guard.state.paused_agent_ids.len();
        guard.state.paused_agent_ids.retain(|a| a != agent_id);
        if guard.state.paused_agent_ids.len() == before {
            return Ok(());
        }
        self.persist_locked(&mut guard, "queue.resumed", None, &[], false)
    }

    pub fn is_queue_paused(&self, agent_id: &str) -> bool {
        self.inner
            .mutable
            .lock()
            .state
            .paused_agent_ids
            .iter()
            .any(|a| a == agent_id)
    }

    /// Strict agent-to-agent `send_message` fan-out (`openbot-tool-router.ts:L3796-L3803`):
    /// enforces `1 <= recipients <= 32`, rejects duplicate recipients, and rejects self-send.
    pub fn send_message(&self, input: SendAgentMessageInput) -> StorageResult<QueuedMessageReceipt> {
        if input.recipient_agent_ids.is_empty() {
            return Err(StorageError::Validation(
                "At least one recipient is required.".to_string(),
            ));
        }
        if input.recipient_agent_ids.len() > INPUT_LIMITS.message_recipients {
            return Err(StorageError::LimitExceeded(format!(
                "Message recipient limit of {} exceeded.",
                INPUT_LIMITS.message_recipients
            )));
        }
        let mut seen = HashSet::with_capacity(input.recipient_agent_ids.len());
        for rid in &input.recipient_agent_ids {
            if !seen.insert(rid.as_str()) {
                return Err(StorageError::Validation(
                    "Duplicate recipients are not allowed.".to_string(),
                ));
            }
            if rid == &input.sender_agent_id {
                return Err(StorageError::Validation(
                    "An agent cannot message itself.".to_string(),
                ));
            }
        }
        self.enqueue(EnqueueInput {
            sender: Some(QueueSender::Agent {
                agent_id: input.sender_agent_id,
            }),
            recipient_agent_ids: input.recipient_agent_ids,
            text: input.text,
            reply_to_message_id: input.reply_to_message_id,
            expects_reply: input.expects_reply,
            sender_task_id: input.sender_task_id,
            requester_ids: input.requester_ids,
            source_paths: input.source_paths,
            idempotency_key: input.idempotency_key,
            ..Default::default()
        })
    }

    pub fn enqueue(&self, input: EnqueueInput) -> StorageResult<QueuedMessageReceipt> {
        let mut guard = self.inner.mutable.lock();
        if let Some(key) = &input.idempotency_key
            && let Some(existing_msg_id) = guard.state.idempotency.get(key).cloned()
        {
            return Ok(build_receipt(&guard.state, &existing_msg_id));
        }

        let mut recipients = Vec::new();
        let mut seen_recips = HashSet::new();
        for rid in &input.recipient_agent_ids {
            if seen_recips.insert(rid.as_str()) {
                recipients.push(rid.clone());
            }
        }
        if recipients.is_empty() {
            return Err(StorageError::Validation(
                "At least one recipient is required.".to_string(),
            ));
        }
        if input.task_id.is_some() && recipients.len() != 1 {
            return Err(StorageError::Validation(
                "A task message has one recipient.".to_string(),
            ));
        }
        if recipients.len() > INPUT_LIMITS.message_recipients {
            return Err(StorageError::LimitExceeded(format!(
                "Message recipient limit of {} exceeded.",
                INPUT_LIMITS.message_recipients
            )));
        }
        if recipients.iter().any(|r| !is_identifier(r)) {
            return Err(StorageError::Validation(
                "A message recipient is invalid.".to_string(),
            ));
        }
        if let Some(key) = &input.idempotency_key
            && !is_bounded_string(key, INPUT_LIMITS.identifier)
        {
            return Err(StorageError::Validation(
                "The idempotency key is too long.".to_string(),
            ));
        }

        let trimmed_text = input.text.trim();
        if !is_bounded_string(trimmed_text, INPUT_LIMITS.message_text) {
            return Err(StorageError::Validation("Message text is too long.".to_string()));
        }

        let mut draft_seen = HashSet::new();
        let mut drafts = Vec::with_capacity(input.draft_ids.len());
        for did in &input.draft_ids {
            if !draft_seen.insert(did.as_str()) {
                return Err(StorageError::Validation(
                    "Duplicate attachment draft.".to_string(),
                ));
            }
            let draft = guard
                .state
                .drafts
                .iter()
                .find(|d| &d.attachment.summary.id == did)
                .cloned()
                .ok_or_else(|| {
                    StorageError::NotFound(format!("Attachment draft not found: {did}"))
                })?;
            if draft.owner_edit_id.is_some() {
                return Err(StorageError::Conflict(
                    "Attachment draft is currently held by a queue edit.".to_string(),
                ));
            }
            drafts.push(draft);
        }

        let total_files = drafts.len() + input.source_paths.len();
        if trimmed_text.is_empty() && total_files == 0 {
            return Err(StorageError::Validation("Message cannot be empty.".to_string()));
        }
        if total_files > INPUT_LIMITS.attachments {
            return Err(StorageError::LimitExceeded(format!(
                "Attachment limit of {} exceeded.",
                INPUT_LIMITS.attachments
            )));
        }

        let created_at = now_iso();
        let message_id = Uuid::new_v4().to_string();
        let mut source_paths: Vec<PathBuf> =
            drafts.iter().map(|d| PathBuf::from(&d.attachment.path)).collect();
        source_paths.extend(input.source_paths.iter().cloned());
        let transfer_dir = self.inner.transfers_root.join(&message_id);
        let committed_attachments = if source_paths.is_empty() {
            Vec::new()
        } else {
            commit_transfer_files(&transfer_dir, &source_paths)?
        };

        let draft_remap: HashMap<&str, (&str, &str)> = drafts
            .iter()
            .zip(committed_attachments.iter())
            .map(|(d, c)| {
                (
                    d.attachment.summary.id.as_str(),
                    (c.summary.id.as_str(), c.summary.name.as_str()),
                )
            })
            .collect();
        let rewritten_text = rewrite_attachment_references(trimmed_text, |att_id| {
            draft_remap.get(att_id).copied()
        });

        let sender = normalize_sender(input.sender.unwrap_or(QueueSender::User));
        let answer_return = match (&sender, &input.reply_to_message_id) {
            (QueueSender::Agent { .. }, Some(reply_id)) if recipients.len() == 1 => guard
                .state
                .messages
                .iter()
                .find(|m| &m.id == reply_id)
                .filter(|m| {
                    matches!(&m.sender, QueueSender::Agent { agent_id } if agent_id == &recipients[0])
                })
                .and_then(|m| m.messaging_return.clone()),
            _ => None,
        };
        let messaging = input.messaging.or(answer_return);

        let message = StoredMailboxMessage {
            id: message_id.clone(),
            channel_id: input.channel_id,
            messaging,
            messaging_return: input.messaging_return,
            task_id: input.task_id.clone(),
            sender_task_id: input.sender_task_id.clone(),
            held_request_ids: ((input.task_id.is_some() || input.sender_task_id.is_some())
                && !input.held_request_ids.is_empty())
            .then_some(input.held_request_ids),
            sender_member: matches!(sender, QueueSender::User).then_some(input.sender_member).flatten(),
            requester_ids: (matches!(sender, QueueSender::Agent { .. })
                && !input.requester_ids.is_empty())
            .then_some(input.requester_ids),
            sender,
            text: rewritten_text,
            attachments: committed_attachments,
            voice_recording: None,
            reply_to_message_id: input.reply_to_message_id,
            expects_reply: (input.expects_reply == Some(false)).then_some(false),
            welcome: input.welcome.then_some(true),
            agent_setup: input.agent_setup.then_some(true),
            created_at: created_at.clone(),
        };

        let mut deliveries = Vec::with_capacity(recipients.len());
        for recipient_agent_id in recipients {
            let order = next_queue_order(&guard.state.deliveries, &recipient_agent_id)
                + deliveries
                    .iter()
                    .filter(|d: &&StoredMailboxDelivery| d.recipient_agent_id == recipient_agent_id)
                    .count() as i64;
            deliveries.push(StoredMailboxDelivery {
                id: Uuid::new_v4().to_string(),
                message_id: message_id.clone(),
                recipient_agent_id,
                queue_order: order,
                status: QueueDeliveryStatus::Queued,
                turn_id: None,
                error: None,
                created_at: created_at.clone(),
                edit_id: None,
                finished_edit_outcomes: None,
                steered_at: None,
                steer_fallback: None,
                resume_after_usage_limit: None,
                plan_continuation: None,
            });
        }

        let used_draft_ids: HashSet<String> =
            drafts.iter().map(|d| d.attachment.summary.id.clone()).collect();
        let prev_state = guard.state.clone();
        guard.state.messages.push(message);
        for d in &deliveries {
            guard.changes.delivery_ids.insert(d.id.clone());
        }
        guard.state.deliveries.extend(deliveries);
        if let Some(key) = &input.idempotency_key {
            guard.state.idempotency.insert(key.clone(), message_id.clone());
        }
        guard
            .state
            .drafts
            .retain(|d| !used_draft_ids.contains(&d.attachment.summary.id));
        guard.changes.message_ids.insert(message_id.clone());
        for did in &used_draft_ids {
            guard.changes.draft_ids.insert(did.clone());
        }

        if let Err(err) = self.persist_locked(&mut guard, "mailbox.updated", None, &[], false) {
            guard.state = prev_state;
            let _ = fs::remove_dir_all(&transfer_dir);
            return Err(err);
        }
        for d in drafts {
            if let Some(dir) = Path::new(&d.attachment.path).parent() {
                let _ = fs::remove_dir_all(dir);
            }
        }
        Ok(build_receipt(&guard.state, &message_id))
    }

    pub fn receipt_for_key(&self, key: &str) -> Option<QueuedMessageReceipt> {
        let guard = self.inner.mutable.lock();
        let mid = guard.state.idempotency.get(key)?;
        Some(build_receipt(&guard.state, mid))
    }

    pub fn forget_idempotency_key(&self, key: &str) {
        self.inner.mutable.lock().state.idempotency.remove(key);
    }

    pub fn delivery_for_key(&self, key: &str) -> Option<DeliveryContext> {
        let guard = self.inner.mutable.lock();
        let mid = guard.state.idempotency.get(key)?;
        let del = guard.state.deliveries.iter().find(|d| &d.message_id == mid)?;
        let positions = compute_queued_positions(&guard.state.deliveries);
        build_delivery_context(&guard.state, del, &positions)
    }

    pub fn get_delivery(&self, delivery_id: &str) -> Option<DeliveryContext> {
        let guard = self.inner.mutable.lock();
        let del = guard.state.deliveries.iter().find(|d| d.id == delivery_id)?;
        let positions = compute_queued_positions(&guard.state.deliveries);
        build_delivery_context(&guard.state, del, &positions)
    }

    pub fn delivery_for_message(
        &self,
        message_id: &str,
        recipient_agent_id: &str,
    ) -> Option<DeliveryContext> {
        let guard = self.inner.mutable.lock();
        let del = guard.state.deliveries.iter().find(|d| {
            d.message_id == message_id && d.recipient_agent_id == recipient_agent_id
        })?;
        let positions = compute_queued_positions(&guard.state.deliveries);
        build_delivery_context(&guard.state, del, &positions)
    }

    pub fn find_deliveries_by_turn(&self, agent_id: &str, turn_id: &str) -> Vec<DeliveryContext> {
        let guard = self.inner.mutable.lock();
        let positions = compute_queued_positions(&guard.state.deliveries);
        guard
            .state
            .deliveries
            .iter()
            .filter(|d| {
                d.recipient_agent_id == agent_id
                    && d.turn_id.as_deref() == Some(turn_id)
                    && matches!(
                        d.status,
                        QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
                    )
            })
            .filter_map(|d| build_delivery_context(&guard.state, d, &positions))
            .collect()
    }

    pub fn completed_deliveries_of_turn(
        &self,
        agent_id: &str,
        turn_id: &str,
    ) -> Vec<DeliveryContext> {
        let guard = self.inner.mutable.lock();
        let positions = compute_queued_positions(&guard.state.deliveries);
        guard
            .state
            .deliveries
            .iter()
            .filter(|d| {
                d.recipient_agent_id == agent_id
                    && d.turn_id.as_deref() == Some(turn_id)
                    && d.status == QueueDeliveryStatus::Completed
            })
            .filter_map(|d| build_delivery_context(&guard.state, d, &positions))
            .collect()
    }

    pub fn starting_delivery_for_agent(
        &self,
        agent_id: &str,
        lane: Option<Option<&str>>,
    ) -> Option<DeliveryContext> {
        let guard = self.inner.mutable.lock();
        let msgs = messages_by_id(&guard.state.messages);
        let del = guard.state.deliveries.iter().find(|d| {
            d.recipient_agent_id == agent_id
                && d.status == QueueDeliveryStatus::Starting
                && d.turn_id.is_none()
                && lane.is_none_or(|expected| lane_of_delivery(d, &msgs).as_deref() == expected)
        })?;
        let positions = compute_queued_positions(&guard.state.deliveries);
        build_delivery_context(&guard.state, del, &positions)
    }

    pub fn list_queue(&self, agent_id: &str) -> QueueSnapshot {
        let guard = self.inner.mutable.lock();
        let exec_ids = execution_message_ids(&guard.state.messages);
        let positions = compute_queued_positions(&guard.state.deliveries);
        let msgs = messages_by_id(&guard.state.messages);
        let welcome = guard.state.deliveries.iter().any(|d| {
            d.recipient_agent_id == agent_id
                && is_active_delivery(d)
                && msgs.get(d.message_id.as_str()).is_some_and(|m| m.welcome == Some(true))
        });
        let mut selected: Vec<&StoredMailboxDelivery> = guard
            .state
            .deliveries
            .iter()
            .filter(|d| d.recipient_agent_id == agent_id && !exec_ids.contains(&d.message_id))
            .collect();
        selected.sort_by(|a, b| compare_queue_order(a, b));
        let deliveries = selected
            .into_iter()
            .filter_map(|d| {
                let m = msgs.get(d.message_id.as_str())?;
                Some(to_public_delivery(d, &positions, m))
            })
            .collect();
        QueueSnapshot {
            agent_id: agent_id.to_string(),
            hold: None,
            welcome: welcome.then_some(true),
            deliveries,
        }
    }

    pub fn list_active_queues(
        &self,
        agent_ids: &[String],
        failed_turns: &HashMap<String, String>,
    ) -> Vec<QueueSnapshot> {
        let guard = self.inner.mutable.lock();
        let exec_ids = execution_message_ids(&guard.state.messages);
        let msgs = messages_by_id(&guard.state.messages);
        let positions = compute_queued_positions(&guard.state.deliveries);
        let targets: HashSet<&str> = agent_ids.iter().map(String::as_str).collect();
        let mut welcomes: HashSet<&str> = HashSet::new();
        let mut open: HashMap<&str, Vec<&StoredMailboxDelivery>> = HashMap::new();

        for d in &guard.state.deliveries {
            if !targets.contains(d.recipient_agent_id.as_str()) {
                continue;
            }
            if is_active_delivery(d)
                && msgs.get(d.message_id.as_str()).is_some_and(|m| m.welcome == Some(true))
            {
                welcomes.insert(&d.recipient_agent_id);
            }
            if exec_ids.contains(&d.message_id) {
                continue;
            }
            let is_failed = d.status == QueueDeliveryStatus::Failed
                && d.turn_id.as_deref() == failed_turns.get(&d.recipient_agent_id).map(String::as_str);
            if !is_active_delivery(d) && !is_failed {
                continue;
            }
            open.entry(&d.recipient_agent_id).or_default().push(d);
        }

        agent_ids
            .iter()
            .map(|aid| {
                let mut list = open.remove(aid.as_str()).unwrap_or_default();
                list.sort_by(|a, b| compare_queue_order(a, b));
                QueueSnapshot {
                    agent_id: aid.clone(),
                    hold: None,
                    welcome: welcomes.contains(aid.as_str()).then_some(true),
                    deliveries: list
                        .into_iter()
                        .filter_map(|d| Some(to_public_delivery(d, &positions, msgs.get(d.message_id.as_str())?)))
                        .collect(),
                }
            })
            .collect()
    }

    pub fn queued_positions(&self) -> HashMap<String, u32> {
        let guard = self.inner.mutable.lock();
        compute_queued_positions(&guard.state.deliveries)
    }

    pub fn lane_id_for_delivery(&self, delivery_id: &str) -> Option<String> {
        let guard = self.inner.mutable.lock();
        let msgs = messages_by_id(&guard.state.messages);
        let del = guard.state.deliveries.iter().find(|d| d.id == delivery_id)?;
        lane_of_delivery(del, &msgs)
    }

    pub fn lane_active(&self, agent_id: &str, lane: Option<&str>) -> bool {
        let guard = self.inner.mutable.lock();
        let msgs = messages_by_id(&guard.state.messages);
        guard.state.deliveries.iter().any(|d| {
            d.recipient_agent_id == agent_id
                && matches!(
                    d.status,
                    QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
                )
                && lane_of_delivery(d, &msgs).as_deref() == lane
        })
    }

    pub fn lane_queued_ids(&self, agent_id: &str, lane: Option<&str>) -> Vec<String> {
        let guard = self.inner.mutable.lock();
        let msgs = messages_by_id(&guard.state.messages);
        let mut list: Vec<&StoredMailboxDelivery> = guard
            .state
            .deliveries
            .iter()
            .filter(|d| {
                d.recipient_agent_id == agent_id
                    && d.status == QueueDeliveryStatus::Queued
                    && !waits_for_plan_continuation(d)
                    && lane_of_delivery(d, &msgs).as_deref() == lane
            })
            .collect();
        list.sort_by(|a, b| compare_queue_order(a, b));
        list.into_iter().map(|d| d.id.clone()).collect()
    }

    pub fn task_work(&self, agent_id: &str) -> HashMap<String, TaskWork> {
        let guard = self.inner.mutable.lock();
        let msgs = messages_by_id(&guard.state.messages);
        let mut works: HashMap<String, TaskWork> = HashMap::new();
        let mut queued_by_task: HashMap<String, Vec<(&StoredMailboxDelivery, &StoredMailboxMessage)>> =
            HashMap::new();

        for d in &guard.state.deliveries {
            if d.recipient_agent_id != agent_id {
                continue;
            }
            let Some(m) = msgs.get(d.message_id.as_str()) else {
                continue;
            };
            let Some(tid) = &m.task_id else {
                continue;
            };
            if d.status == QueueDeliveryStatus::Queued {
                if !waits_for_plan_continuation(d) {
                    queued_by_task.entry(tid.clone()).or_default().push((d, m));
                }
            } else if matches!(
                d.status,
                QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
            ) {
                works.entry(tid.clone()).or_default().active = true;
            }
        }
        for (tid, mut items) in queued_by_task {
            items.sort_by(|(a, _), (b, _)| compare_queue_order(a, b));
            let pos_map: HashMap<String, u32> = items
                .iter()
                .enumerate()
                .map(|(idx, (d, _))| (d.id.clone(), (idx + 1) as u32))
                .collect();
            let entry = works.entry(tid).or_default();
            for (d, m) in items {
                entry
                    .queued_messages
                    .push((d.id.clone(), m.text.clone(), m.created_at.clone()));
                entry.queue.push(to_public_delivery(d, &pos_map, m));
            }
        }
        works
    }

    pub fn awaited_thread_requests(&self) -> Vec<AwaitedThreadRequest> {
        let guard = self.inner.mutable.lock();
        awaited_thread_requests_from_state(&guard.state)
    }

    pub fn next_queued(&self, agent_id: &str, lane: Option<&str>) -> Option<DeliveryContext> {
        self.next_queued_at(agent_id, lane, current_unix_ms())
    }

    /// Selects the next runnable queued delivery in `lane`, skipping deliveries held by a live
    /// `10m` queue edit or held by a multi-recipient fan-out reply barrier (`mailbox-store.ts:L1955-L1962`).
    pub fn next_queued_at(
        &self,
        agent_id: &str,
        lane: Option<&str>,
        now_ms: u64,
    ) -> Option<DeliveryContext> {
        let guard = self.inner.mutable.lock();
        let msgs = messages_by_id(&guard.state.messages);
        if guard.state.deliveries.iter().any(|d| {
            d.recipient_agent_id == agent_id
                && matches!(
                    d.status,
                    QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
                )
                && lane_of_delivery(d, &msgs).as_deref() == lane
        }) {
            return None;
        }
        let mut queued: Vec<&StoredMailboxDelivery> = guard
            .state
            .deliveries
            .iter()
            .filter(|d| d.recipient_agent_id == agent_id && d.status == QueueDeliveryStatus::Queued)
            .collect();
        queued.sort_by(|a, b| compare_queue_order(a, b));
        let holders = build_request_holders(&guard.state);
        let chosen = queued.into_iter().find(|d| {
            lane_of_delivery(d, &msgs).as_deref() == lane
                && !is_held_by_edit_map(&guard.edit_holds, d, now_ms)
                && !is_held_reply_in_state(&guard.state, d, &msgs, &holders)
        })?;
        let positions = compute_queued_positions(&guard.state.deliveries);
        build_delivery_context(&guard.state, chosen, &positions)
    }

    pub fn replies_to_start_with(&self, delivery_id: &str) -> Vec<DeliveryContext> {
        self.replies_to_start_with_at(delivery_id, current_unix_ms())
    }

    /// Collects all sibling answers to the same `request_id` in the same lane so all fan-out
    /// answers release together in a single combined turn (`mailbox-store.ts:L1964-L1980`).
    pub fn replies_to_start_with_at(
        &self,
        delivery_id: &str,
        now_ms: u64,
    ) -> Vec<DeliveryContext> {
        let guard = self.inner.mutable.lock();
        let msgs = messages_by_id(&guard.state.messages);
        let Some(next) = guard.state.deliveries.iter().find(|d| d.id == delivery_id) else {
            return Vec::new();
        };
        let Some(first_msg) = msgs.get(next.message_id.as_str()) else {
            return Vec::new();
        };
        if !is_answer_message(first_msg) {
            return Vec::new();
        }
        let Some(req_id) = first_msg.reply_to_message_id.as_deref() else {
            return Vec::new();
        };
        let lane = lane_of_delivery(next, &msgs);
        let mut candidates: Vec<&StoredMailboxDelivery> = guard
            .state
            .deliveries
            .iter()
            .filter(|d| {
                d.recipient_agent_id == next.recipient_agent_id
                    && d.status == QueueDeliveryStatus::Queued
                    && d.id != next.id
                    && !is_held_by_edit_map(&guard.edit_holds, d, now_ms)
                    && lane_of_delivery(d, &msgs) == lane
                    && msgs
                        .get(d.message_id.as_str())
                        .is_some_and(|rm| is_answer_message(rm) && rm.reply_to_message_id.as_deref() == Some(req_id))
            })
            .collect();
        candidates.sort_by(|a, b| compare_queue_order(a, b));
        let positions = compute_queued_positions(&guard.state.deliveries);
        candidates
            .into_iter()
            .filter_map(|d| build_delivery_context(&guard.state, d, &positions))
            .collect()
    }

    pub fn user_messages_to_start_with(
        &self,
        delivery_id: &str,
        now_ms: u64,
    ) -> Vec<DeliveryContext> {
        let guard = self.inner.mutable.lock();
        let msgs = messages_by_id(&guard.state.messages);
        let Some(next) = guard.state.deliveries.iter().find(|d| d.id == delivery_id) else {
            return Vec::new();
        };
        let Some(first) = msgs.get(next.message_id.as_str()) else {
            return Vec::new();
        };
        if !is_chat_message_from_person(first) {
            return Vec::new();
        }
        let lane = lane_of_delivery(next, &msgs);
        let holders = build_request_holders(&guard.state);
        let mut queued: Vec<&StoredMailboxDelivery> = guard
            .state
            .deliveries
            .iter()
            .filter(|d| {
                d.recipient_agent_id == next.recipient_agent_id
                    && d.status == QueueDeliveryStatus::Queued
                    && lane_of_delivery(d, &msgs) == lane
                    && !is_held_reply_in_state(&guard.state, d, &msgs, &holders)
            })
            .collect();
        queued.sort_by(|a, b| compare_queue_order(a, b));
        let Some(start) = queued.iter().position(|d| d.id == next.id) else {
            return Vec::new();
        };
        let first_member_id = first.sender_member.as_ref().map(|s| s.id.as_str());
        let next_ul_turn = next.resume_after_usage_limit.as_ref().map(|u| u.turn_id.as_str());
        let mut run = Vec::new();
        for cand in &queued[start + 1..] {
            let Some(m) = msgs.get(cand.message_id.as_str()) else {
                break;
            };
            if is_held_by_edit_map(&guard.edit_holds, cand, now_ms)
                || cand.resume_after_usage_limit.as_ref().map(|u| u.turn_id.as_str()) != next_ul_turn
                || !is_chat_message_from_person(m)
                || m.sender_member.as_ref().map(|s| s.id.as_str()) != first_member_id
            {
                break;
            }
            run.push(*cand);
        }
        let positions = compute_queued_positions(&guard.state.deliveries);
        run.into_iter()
            .filter_map(|d| build_delivery_context(&guard.state, d, &positions))
            .collect()
    }

    pub fn unanswered_recipients(&self, request_id: &str) -> Vec<String> {
        let guard = self.inner.mutable.lock();
        guard
            .state
            .deliveries
            .iter()
            .filter(|d| {
                d.message_id == request_id
                    && matches!(
                        d.status,
                        QueueDeliveryStatus::Failed
                            | QueueDeliveryStatus::Interrupted
                            | QueueDeliveryStatus::Cancelled
                    )
                    && !has_answered(&guard.state.messages, request_id, &d.recipient_agent_id)
            })
            .map(|d| d.recipient_agent_id.clone())
            .collect()
    }

    pub fn still_working_recipients(&self, agent_id: &str, request_id: &str) -> Vec<String> {
        let guard = self.inner.mutable.lock();
        let msgs = messages_by_id(&guard.state.messages);
        if answered_request_of(&msgs, agent_id, request_id).is_none() {
            return Vec::new();
        }
        let holders = build_request_holders(&guard.state);
        guard
            .state
            .deliveries
            .iter()
            .filter(|d| {
                d.message_id == request_id
                    && still_works_on_in_state(&guard.state, d, request_id, &msgs, &holders)
                    && !has_answered(&guard.state.messages, request_id, &d.recipient_agent_id)
            })
            .map(|d| d.recipient_agent_id.clone())
            .collect()
    }

    pub fn joins_running_turn(&self, delivery_id: &str) -> bool {
        let guard = self.inner.mutable.lock();
        let Some(del) = guard.state.deliveries.iter().find(|d| d.id == delivery_id) else {
            return false;
        };
        if del.status != QueueDeliveryStatus::Queued {
            return false;
        }
        let Some(msg) = guard.state.messages.iter().find(|m| m.id == del.message_id) else {
            return false;
        };
        if msg.channel_id.is_some() || msg.messaging.is_some() {
            return false;
        }
        if msg.reply_to_message_id.is_some() && msg.expects_reply != Some(false) {
            return false;
        }
        if !is_answer_message(msg) {
            return true;
        }
        guard
            .state
            .deliveries
            .iter()
            .filter(|c| c.message_id.as_str() == msg.reply_to_message_id.as_deref().unwrap_or(""))
            .count()
            < 2
    }

    pub fn chain_origin_agent_id(&self, message_id: &str) -> Option<String> {
        let guard = self.inner.mutable.lock();
        chain_origin_in_messages(&guard.state.messages, message_id)
    }

    pub fn reply_reference_message_id(&self, id: &str) -> Option<String> {
        let guard = self.inner.mutable.lock();
        if guard.state.messages.iter().any(|m| m.id == id) {
            return Some(id.to_string());
        }
        guard
            .state
            .deliveries
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.message_id.clone())
    }

    pub fn has_reply_from(&self, agent_id: &str, message_id: &str) -> bool {
        let guard = self.inner.mutable.lock();
        has_reply_from_messages(&guard.state.messages, agent_id, message_id)
    }

    pub fn expects_reply(&self, message_id: &str) -> bool {
        let guard = self.inner.mutable.lock();
        guard
            .state
            .messages
            .iter()
            .find(|m| m.id == message_id)
            .is_none_or(|m| m.expects_reply != Some(false))
    }

    pub fn answer_task_id(
        &self,
        sender_agent_id: &str,
        reply_to_message_id: &str,
        recipient_agent_ids: &[String],
    ) -> Option<String> {
        if recipient_agent_ids.len() != 1 {
            return None;
        }
        let guard = self.inner.mutable.lock();
        answer_task_id_in_state(
            &guard.state,
            sender_agent_id,
            reply_to_message_id,
            &recipient_agent_ids[0],
        )
    }

    /// Synthesizes a 4-line `Status: blocked` note back to `requester_id` when a teammate delivery
    /// fails or is interrupted (`mailbox-store.ts:L2519-L2575`).
    pub fn answer_failed_request<FT, FA>(
        &self,
        delivery_id: &str,
        task_open: FT,
        agent_exists: FA,
        recovered: bool,
        temporary: bool,
    ) -> StorageResult<Option<String>>
    where
        FT: Fn(&str) -> bool,
        FA: Fn(&str) -> bool,
    {
        let enqueue_req = {
            let guard = self.inner.mutable.lock();
            let Some(del) = guard.state.deliveries.iter().find(|d| d.id == delivery_id) else {
                return Ok(None);
            };
            let Some(request) = guard.state.messages.iter().find(|m| m.id == del.message_id)
            else {
                return Ok(None);
            };
            let steered = del.status == QueueDeliveryStatus::Interrupted
                && del.steered_at.is_some()
                && del.turn_id.as_deref() != Some(del.id.as_str());
            let outcome_failed = del.status == QueueDeliveryStatus::Failed;
            let outcome_interrupted =
                del.status == QueueDeliveryStatus::Interrupted && (steered || recovered);
            if !outcome_failed && !outcome_interrupted {
                return Ok(None);
            }
            let QueueSender::Agent {
                agent_id: ref requester_id,
            } = request.sender
            else {
                return Ok(None);
            };
            if request.channel_id.is_some() {
                return Ok(None);
            }
            let teammate_id = &del.recipient_agent_id;
            let origin_id = chain_origin_in_messages(&guard.state.messages, &request.id);
            if requester_id == teammate_id
                || !agent_exists(requester_id)
                || origin_id.as_deref().is_none_or(|orig| orig == teammate_id)
                || request.expects_reply == Some(false)
                || has_reply_from_messages(&guard.state.messages, teammate_id, &request.id)
            {
                return Ok(None);
            }
            let ans_tid =
                answer_task_id_in_state(&guard.state, teammate_id, &request.id, requester_id)
                    .filter(|tid| task_open(tid));

            let reason = slice_utf16(
                del.error
                    .as_deref()
                    .unwrap_or("The provider gave no reason."),
                FAILED_REQUEST_REASON_LIMIT,
            );
            let result_line = if outcome_failed {
                format!(
                    "Result: OpenBot note, not written by this teammate: the request failed before the teammate answered. {reason}"
                )
            } else if steered {
                "Result: OpenBot note, not written by this teammate: the request went into a running turn of the teammate, and that turn ended before the teammate answered.".to_string()
            } else {
                "Result: OpenBot note, not written by this teammate: the teammate's turn stopped when OpenBot or its provider restarted, before the teammate answered. Part of the work can be done.".to_string()
            };
            let unblock_line = if outcome_failed && temporary {
                "Unblock: The failure can be temporary. Send the request again one time if you still need the result. If it fails again, tell the user."
            } else if outcome_failed {
                "Unblock: The user must fix the cause, then ask for the request again."
            } else if steered {
                "Unblock: Send the request again if you still need the result."
            } else {
                "Unblock: Check what the teammate already did, then send the request again if you still need the result."
            };
            let text = [
                "Status: blocked",
                result_line.as_str(),
                "Evidence: none",
                unblock_line,
            ]
            .join("\n");
            let req_id_out = requester_id.clone();
            (
                req_id_out.clone(),
                EnqueueInput {
                    sender: Some(QueueSender::Agent {
                        agent_id: teammate_id.clone(),
                    }),
                    requester_ids: request.requester_ids.clone().unwrap_or_default(),
                    recipient_agent_ids: vec![req_id_out],
                    text,
                    reply_to_message_id: Some(request.id.clone()),
                    task_id: ans_tid,
                    expects_reply: Some(false),
                    idempotency_key: Some(format!("{FAILED_REQUEST_KEY_PREFIX}{delivery_id}")),
                    ..Default::default()
                },
            )
        };
        let (requester_id, input) = enqueue_req;
        self.enqueue(input)?;
        Ok(Some(requester_id))
    }

    pub fn begin_queue_edit(
        &self,
        agent_id: &str,
        delivery_id: &str,
        edit_id: &str,
    ) -> StorageResult<()> {
        self.begin_queue_edit_at(agent_id, delivery_id, edit_id, current_unix_ms())
    }

    pub fn begin_queue_edit_at(
        &self,
        agent_id: &str,
        delivery_id: &str,
        edit_id: &str,
        now_ms: u64,
    ) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let idx = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id && d.recipient_agent_id == agent_id)
            .ok_or_else(|| StorageError::Conflict("Queued message is unavailable.".to_string()))?;
        if guard.state.deliveries[idx].status != QueueDeliveryStatus::Queued {
            return Err(StorageError::Conflict(
                "Queued message is unavailable.".to_string(),
            ));
        }
        if let Some(existing_edit) = &guard.state.deliveries[idx].edit_id
            && existing_edit != edit_id
        {
            return Err(StorageError::Conflict(
                "This queued message is being edited on another device.".to_string(),
            ));
        }
        let prev = guard.state.deliveries[idx].edit_id.clone();
        guard.state.deliveries[idx].edit_id = Some(edit_id.to_string());
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        if let Err(err) = self.persist_locked(&mut guard, "delivery.edit-started", None, &[], false)
        {
            guard.state.deliveries[idx].edit_id = prev;
            return Err(err);
        }
        guard.edit_holds.insert(
            delivery_id.to_string(),
            EditHoldEntry {
                edit_id: edit_id.to_string(),
                started_at_ms: now_ms,
            },
        );
        Ok(())
    }

    pub fn retain_queue_edit_attachments(
        &self,
        agent_id: &str,
        delivery_id: &str,
        edit_id: &str,
        draft_ids: &[String],
    ) -> StorageResult<()> {
        if draft_ids.len() > INPUT_LIMITS.attachments {
            return Err(StorageError::LimitExceeded(
                "Invalid edit attachment count.".to_string(),
            ));
        }
        let unique: HashSet<&str> = draft_ids.iter().map(String::as_str).collect();
        if unique.len() != draft_ids.len() {
            return Err(StorageError::Validation(
                "Duplicate edit attachment draft.".to_string(),
            ));
        }
        let mut guard = self.inner.mutable.lock();
        let valid_edit = guard.state.deliveries.iter().any(|d| {
            d.id == delivery_id
                && d.recipient_agent_id == agent_id
                && d.status == QueueDeliveryStatus::Queued
                && d.edit_id.as_deref() == Some(edit_id)
        });
        if !valid_edit {
            return Err(StorageError::Conflict("Edit session is unavailable.".to_string()));
        }
        for did in draft_ids {
            let draft = guard
                .state
                .drafts
                .iter()
                .find(|d| &d.attachment.summary.id == did)
                .ok_or_else(|| StorageError::NotFound(format!("Draft not found: {did}")))?;
            if let Some(owner) = &draft.owner_edit_id
                && owner != edit_id
            {
                return Err(StorageError::Conflict(
                    "Attachment is retained by another edit.".to_string(),
                ));
            }
        }
        let prev_drafts = guard.state.drafts.clone();
        let g = &mut *guard;
        for draft in &mut g.state.drafts {
            if unique.contains(draft.attachment.summary.id.as_str()) {
                draft.owner_edit_id = Some(edit_id.to_string());
                g.changes
                    .draft_ids
                    .insert(draft.attachment.summary.id.clone());
            }
        }
        if let Err(err) = self.persist_locked(
            &mut guard,
            "delivery.edit-attachments-retained",
            None,
            &[],
            false,
        ) {
            guard.state.drafts = prev_drafts;
            return Err(err);
        }
        Ok(())
    }

    pub fn finished_queue_edit_action(
        &self,
        agent_id: &str,
        delivery_id: &str,
        edit_id: &str,
    ) -> Option<QueueEditOutcome> {
        let guard = self.inner.mutable.lock();
        guard
            .state
            .deliveries
            .iter()
            .find(|d| d.id == delivery_id && d.recipient_agent_id == agent_id)
            .and_then(|d| d.finished_edit_outcomes.as_ref())
            .and_then(|m| m.get(edit_id))
            .map(|o| o.action)
    }

    pub fn matches_finished_queue_save(
        &self,
        agent_id: &str,
        delivery_id: &str,
        edit_id: &str,
        text: &str,
        keep_attachment_ids: &[String],
        attachment_draft_ids: &[String],
    ) -> StorageResult<bool> {
        let expected_hash = queue_save_hash(text, keep_attachment_ids, attachment_draft_ids)?;
        let guard = self.inner.mutable.lock();
        let matched = guard
            .state
            .deliveries
            .iter()
            .find(|d| d.id == delivery_id && d.recipient_agent_id == agent_id)
            .and_then(|d| d.finished_edit_outcomes.as_ref())
            .and_then(|m| m.get(edit_id))
            .is_some_and(|o| {
                o.action == QueueEditOutcome::Save && o.save_hash.as_deref() == Some(&expected_hash)
            });
        Ok(matched)
    }

    pub fn finish_queue_edit(
        &self,
        agent_id: &str,
        delivery_id: &str,
        edit_id: &str,
    ) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let valid = guard.state.deliveries.iter().any(|d| {
            d.id == delivery_id
                && d.recipient_agent_id == agent_id
                && d.edit_id.as_deref() == Some(edit_id)
        });
        if !valid {
            return Err(StorageError::Conflict("Edit session is unavailable.".to_string()));
        }
        self.finish_cancellation_locked(&mut guard, delivery_id, false)
    }

    pub fn agents_with_edit_holds(&self, now_ms: u64) -> Vec<String> {
        let guard = self.inner.mutable.lock();
        let mut set = HashSet::new();
        for d in &guard.state.deliveries {
            if d.status == QueueDeliveryStatus::Queued
                && is_held_by_edit_map(&guard.edit_holds, d, now_ms)
            {
                set.insert(d.recipient_agent_id.clone());
            }
        }
        set.into_iter().collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_queued_message(
        &self,
        agent_id: &str,
        delivery_id: &str,
        text: &str,
        keep_attachment_ids: &[String],
        attachment_draft_ids: &[String],
        edit_id: Option<&str>,
        sender_member: Option<ConversationMessageSender>,
    ) -> StorageResult<()> {
        if let Some(eid) = edit_id
            && self.matches_finished_queue_save(
                agent_id,
                delivery_id,
                eid,
                text,
                keep_attachment_ids,
                attachment_draft_ids,
            )?
        {
            return Ok(());
        }

        let mut guard = self.inner.mutable.lock();
        let del_idx = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id && d.recipient_agent_id == agent_id)
            .ok_or_else(|| StorageError::NotFound("Queued message not found.".to_string()))?;
        if guard.state.deliveries[del_idx].status != QueueDeliveryStatus::Queued {
            return Err(StorageError::Conflict(
                "Only queued messages can be edited.".to_string(),
            ));
        }
        if guard.state.deliveries[del_idx].edit_id.as_deref() != edit_id {
            return Err(StorageError::Conflict(
                "This queued message is being edited on another device.".to_string(),
            ));
        }
        let msg_id = guard.state.deliveries[del_idx].message_id.clone();
        let msg_idx = guard
            .state
            .messages
            .iter()
            .position(|m| m.id == msg_id)
            .ok_or_else(|| StorageError::NotFound(format!("Missing message: {msg_id}")))?;

        let keep_set: HashSet<&str> = keep_attachment_ids.iter().map(String::as_str).collect();
        if keep_set.len() != keep_attachment_ids.len() {
            return Err(StorageError::Validation("Duplicate attachments.".to_string()));
        }
        for kid in keep_attachment_ids {
            if !guard.state.messages[msg_idx]
                .attachments
                .iter()
                .any(|a| &a.summary.id == kid)
            {
                return Err(StorageError::NotFound(
                    "Attachment is not part of the message.".to_string(),
                ));
            }
        }
        let draft_set: HashSet<&str> = attachment_draft_ids.iter().map(String::as_str).collect();
        if draft_set.len() != attachment_draft_ids.len() {
            return Err(StorageError::Validation(
                "Duplicate attachment drafts.".to_string(),
            ));
        }
        let mut drafts = Vec::with_capacity(attachment_draft_ids.len());
        for did in attachment_draft_ids {
            let d = guard
                .state
                .drafts
                .iter()
                .find(|d| &d.attachment.summary.id == did)
                .cloned()
                .ok_or_else(|| StorageError::NotFound(format!("Draft not found: {did}")))?;
            if let Some(owner) = &d.owner_edit_id
                && Some(owner.as_str()) != edit_id
            {
                return Err(StorageError::Conflict(
                    "Attachment is retained by another edit.".to_string(),
                ));
            }
            drafts.push(d);
        }
        if keep_attachment_ids.len() + drafts.len() > INPUT_LIMITS.attachments {
            return Err(StorageError::LimitExceeded(format!(
                "Attachment limit of {} exceeded.",
                INPUT_LIMITS.attachments
            )));
        }
        let normalized_text = text.trim();
        if normalized_text.is_empty() && keep_attachment_ids.is_empty() && drafts.is_empty() {
            return Err(StorageError::Validation("Message cannot be empty.".to_string()));
        }

        let transfer_dir = self
            .inner
            .transfers_root
            .join(format!("{msg_id}-edit-{}", Uuid::new_v4()));
        let draft_paths: Vec<PathBuf> =
            drafts.iter().map(|d| PathBuf::from(&d.attachment.path)).collect();
        let committed = if draft_paths.is_empty() {
            Vec::new()
        } else {
            commit_transfer_files(&transfer_dir, &draft_paths)?
        };

        let kept_attachments: Vec<StoredAttachment> = keep_attachment_ids
            .iter()
            .filter_map(|kid| {
                guard.state.messages[msg_idx]
                    .attachments
                    .iter()
                    .find(|a| &a.summary.id == kid)
                    .cloned()
            })
            .collect();
        let old_removed_paths: Vec<String> = guard.state.messages[msg_idx]
            .attachments
            .iter()
            .filter(|a| !keep_set.contains(a.summary.id.as_str()))
            .map(|a| a.path.clone())
            .collect();

        let mut replacement = kept_attachments.clone();
        replacement.extend(committed.iter().cloned());
        let mut ref_map: HashMap<&str, (&str, &str)> = kept_attachments
            .iter()
            .map(|a| (a.summary.id.as_str(), (a.summary.id.as_str(), a.summary.name.as_str())))
            .collect();
        for (d, c) in drafts.iter().zip(committed.iter()) {
            ref_map.insert(
                d.attachment.summary.id.as_str(),
                (c.summary.id.as_str(), c.summary.name.as_str()),
            );
        }
        let rewritten =
            rewrite_attachment_references(normalized_text, |att_id| ref_map.get(att_id).copied());

        let save_hash = queue_save_hash(text, keep_attachment_ids, attachment_draft_ids)?;
        let prev_state = guard.state.clone();

        guard.state.messages[msg_idx].text = rewritten;
        guard.state.messages[msg_idx].attachments = replacement;
        if guard.state.messages[msg_idx].sender == QueueSender::User {
            guard.state.messages[msg_idx].sender_member = sender_member;
        }
        if let Some(eid) = edit_id {
            guard.state.deliveries[del_idx].edit_id = None;
            let outcomes = guard.state.deliveries[del_idx]
                .finished_edit_outcomes
                .get_or_insert_with(BTreeMap::new);
            outcomes.insert(
                eid.to_string(),
                StoredFinishedEdit {
                    action: QueueEditOutcome::Save,
                    save_hash: Some(save_hash),
                },
            );
        }
        guard
            .state
            .drafts
            .retain(|d| !draft_set.contains(d.attachment.summary.id.as_str()));
        let mut released_draft_ids = Vec::new();
        if let Some(eid) = edit_id {
            for d in &mut guard.state.drafts {
                if d.owner_edit_id.as_deref() == Some(eid) {
                    d.owner_edit_id = None;
                    released_draft_ids.push(d.attachment.summary.id.clone());
                }
            }
        }
        guard.changes.message_ids.insert(msg_id);
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        for did in attachment_draft_ids {
            guard.changes.draft_ids.insert(did.clone());
        }
        for did in released_draft_ids {
            guard.changes.draft_ids.insert(did);
        }

        let cmd = format!("mailbox:message-updated:{delivery_id}:{}", Uuid::new_v4());
        if let Err(err) = self.persist_locked(
            &mut guard,
            "message.updated",
            Some(&cmd),
            &old_removed_paths,
            false,
        ) {
            guard.state = prev_state;
            let _ = fs::remove_dir_all(&transfer_dir);
            return Err(err);
        }
        guard.edit_holds.remove(delivery_id);
        for d in drafts {
            if let Some(dir) = Path::new(&d.attachment.path).parent() {
                let _ = fs::remove_dir_all(dir);
            }
        }
        drop(guard);
        self.drain_file_deletion_outbox()
    }

    pub fn reorder_queue(&self, agent_id: &str, delivery_ids: &[String]) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let mut all_queued: Vec<StoredMailboxDelivery> = guard
            .state
            .deliveries
            .iter()
            .filter(|d| d.recipient_agent_id == agent_id && d.status == QueueDeliveryStatus::Queued)
            .cloned()
            .collect();
        all_queued.sort_by(compare_queue_order);

        let is_held = |d: &StoredMailboxDelivery| {
            d.edit_id.is_some() || waits_for_plan_continuation(d)
        };
        let held_ids: HashSet<&str> = all_queued
            .iter()
            .filter(|d| is_held(d))
            .map(|d| d.id.as_str())
            .collect();
        let requested: Vec<&str> = delivery_ids
            .iter()
            .map(String::as_str)
            .filter(|id| !held_ids.contains(id))
            .collect();
        let expected: HashSet<&str> = all_queued
            .iter()
            .filter(|d| !is_held(d))
            .map(|d| d.id.as_str())
            .collect();
        let req_unique: HashSet<&str> = requested.iter().copied().collect();
        if requested.len() != expected.len()
            || req_unique.len() != requested.len()
            || requested.iter().any(|id| !expected.contains(id))
        {
            return Err(StorageError::Conflict(
                "Queue order is stale.".to_string(),
            ));
        }
        let mut next_visible = 0usize;
        let mut new_orders: HashMap<String, i64> = HashMap::with_capacity(all_queued.len());
        for (slot, d) in all_queued.iter().enumerate() {
            let chosen_id = if is_held(d) {
                d.id.as_str()
            } else {
                let id = requested[next_visible];
                next_visible += 1;
                id
            };
            new_orders.insert(chosen_id.to_string(), slot as i64);
        }
        let prev = guard.state.deliveries.clone();
        let g = &mut *guard;
        for d in &mut g.state.deliveries {
            if let Some(&ord) = new_orders.get(&d.id) {
                d.queue_order = ord;
                g.changes.delivery_ids.insert(d.id.clone());
            }
        }
        if let Err(err) = self.persist_locked(&mut guard, "queue.reordered", None, &[], false) {
            guard.state.deliveries = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn mark_starting(&self, delivery_id: &str) -> StorageResult<()> {
        self.mark_starting_at(delivery_id, current_unix_ms())
    }

    /// Transitions `Queued -> Starting`. If `edit_id` is set and the `10m` hold has expired,
    /// auto-cancels the stale edit first; if the `10m` hold is still active, returns `Conflict`.
    pub fn mark_starting_at(&self, delivery_id: &str, now_ms: u64) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let expired_edit = guard
            .state
            .deliveries
            .iter()
            .find(|d| d.id == delivery_id)
            .is_some_and(|d| {
                d.status == QueueDeliveryStatus::Queued
                    && d.edit_id.is_some()
                    && !is_held_by_edit_map(&guard.edit_holds, d, now_ms)
            });
        if expired_edit {
            self.finish_cancellation_locked(&mut guard, delivery_id, false)?;
        }
        let Some(idx) = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id)
        else {
            return Err(StorageError::NotFound(format!("Unknown delivery: {delivery_id}")));
        };
        if guard.state.deliveries[idx].edit_id.is_some() {
            return Err(StorageError::Conflict(
                "Message is currently being edited.".to_string(),
            ));
        }
        if guard.state.deliveries[idx].status != QueueDeliveryStatus::Queued {
            return Ok(());
        }
        let prev = guard.state.deliveries[idx].clone();
        guard.state.deliveries[idx].steer_fallback = None;
        guard.state.deliveries[idx].status = QueueDeliveryStatus::Starting;
        guard.state.deliveries[idx].error = None;
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        if let Err(err) = self.persist_locked(&mut guard, "delivery.updated", None, &[], false) {
            guard.state.deliveries[idx] = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn mark_running(&self, delivery_id: &str, turn_id: &str) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let idx = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id)
            .ok_or_else(|| StorageError::NotFound(format!("Unknown delivery: {delivery_id}")))?;
        if !matches!(
            guard.state.deliveries[idx].status,
            QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
        ) {
            return Ok(());
        }
        let prev = guard.state.deliveries[idx].clone();
        guard.state.deliveries[idx].status = QueueDeliveryStatus::Running;
        guard.state.deliveries[idx].turn_id = Some(turn_id.to_string());
        guard.state.deliveries[idx].error = None;
        if let Some(ul) = &mut guard.state.deliveries[idx].resume_after_usage_limit
            && !ul.previous_turn_ids.iter().any(|t| t == turn_id)
        {
            ul.previous_turn_ids.push(turn_id.to_string());
        }
        if let Some(pc) = &mut guard.state.deliveries[idx].plan_continuation {
            if !pc.turn_ids.iter().any(|t| t == turn_id) {
                pc.turn_ids.push(turn_id.to_string());
            }
            pc.queued = None;
        }
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        if let Err(err) = self.persist_locked(&mut guard, "delivery.updated", None, &[], false) {
            guard.state.deliveries[idx] = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn mark_terminal(
        &self,
        delivery_id: &str,
        status: QueueDeliveryStatus,
        error: Option<&str>,
    ) -> StorageResult<()> {
        if !matches!(
            status,
            QueueDeliveryStatus::Completed
                | QueueDeliveryStatus::Failed
                | QueueDeliveryStatus::Interrupted
        ) {
            return Err(StorageError::Validation(
                "Invalid terminal delivery status.".to_string(),
            ));
        }
        let mut guard = self.inner.mutable.lock();
        let idx = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id)
            .ok_or_else(|| StorageError::NotFound(format!("Unknown delivery: {delivery_id}")))?;
        if !matches!(
            guard.state.deliveries[idx].status,
            QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
        ) {
            return Ok(());
        }
        let answered = status != QueueDeliveryStatus::Completed
            && answered_request_in_state(&guard.state, delivery_id);
        let prev = guard.state.deliveries[idx].clone();
        guard.state.deliveries[idx].status = if answered {
            QueueDeliveryStatus::Completed
        } else {
            status
        };
        guard.state.deliveries[idx].error = if answered {
            None
        } else {
            error.map(str::to_string)
        };
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        if let Err(err) = self.persist_locked(&mut guard, "delivery.updated", None, &[], false) {
            guard.state.deliveries[idx] = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn cancel(&self, agent_id: &str, delivery_id: &str) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let d = guard
            .state
            .deliveries
            .iter()
            .find(|d| d.id == delivery_id && d.recipient_agent_id == agent_id)
            .ok_or_else(|| StorageError::NotFound("Queued message not found.".to_string()))?;
        if d.status != QueueDeliveryStatus::Queued {
            return Err(StorageError::Conflict(
                "Only queued messages can be cancelled.".to_string(),
            ));
        }
        self.finish_cancellation_locked(&mut guard, delivery_id, true)
    }

    pub fn mark_steering(&self, delivery_id: &str, turn_id: &str) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let idx = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id)
            .ok_or_else(|| StorageError::NotFound(format!("Unknown delivery: {delivery_id}")))?;
        if guard.state.deliveries[idx].edit_id.is_some() {
            return Err(StorageError::Conflict(
                "Message is currently being edited.".to_string(),
            ));
        }
        if guard.state.deliveries[idx].status != QueueDeliveryStatus::Queued {
            return Ok(());
        }
        let prev = guard.state.deliveries[idx].clone();
        guard.state.deliveries[idx].steer_fallback = None;
        guard.state.deliveries[idx].status = QueueDeliveryStatus::Starting;
        guard.state.deliveries[idx].turn_id = Some(turn_id.to_string());
        guard.state.deliveries[idx].steered_at = Some(now_iso());
        guard.state.deliveries[idx].error = None;
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        if let Err(err) = self.persist_locked(&mut guard, "delivery.updated", None, &[], false) {
            guard.state.deliveries[idx] = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn restore_unsteered(&self, delivery_id: &str, turn_id: &str) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let Some(idx) = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id)
        else {
            return Ok(());
        };
        if guard.state.deliveries[idx].turn_id.as_deref() != Some(turn_id)
            || matches!(
                guard.state.deliveries[idx].status,
                QueueDeliveryStatus::Queued | QueueDeliveryStatus::Cancelled
            )
        {
            return Ok(());
        }
        let prev = guard.state.deliveries[idx].clone();
        guard.state.deliveries[idx].status = QueueDeliveryStatus::Queued;
        guard.state.deliveries[idx].turn_id = None;
        guard.state.deliveries[idx].error = None;
        guard.state.deliveries[idx].steered_at = None;
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        if let Err(err) = self.persist_locked(&mut guard, "delivery.updated", None, &[], false) {
            guard.state.deliveries[idx] = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn move_steer_to_turn(&self, agent_id: &str, turn_id: &str) -> StorageResult<bool> {
        let mut guard = self.inner.mutable.lock();
        let Some(idx) = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == turn_id)
        else {
            return Ok(false);
        };
        let d = &guard.state.deliveries[idx];
        if d.recipient_agent_id != agent_id
            || d.steered_at.is_none()
            || d.turn_id.is_none()
            || d.turn_id.as_deref() == Some(turn_id)
            || matches!(
                d.status,
                QueueDeliveryStatus::Queued | QueueDeliveryStatus::Cancelled
            )
        {
            return Ok(false);
        }
        let prev = guard.state.deliveries[idx].turn_id.clone();
        guard.state.deliveries[idx].turn_id = Some(turn_id.to_string());
        guard.changes.delivery_ids.insert(turn_id.to_string());
        if let Err(err) = self.persist_locked(&mut guard, "delivery.updated", None, &[], false) {
            guard.state.deliveries[idx].turn_id = prev;
            return Err(err);
        }
        Ok(true)
    }

    pub fn mark_steer_fallback(
        &self,
        delivery_id: &str,
        fallback: QueueSteerFallback,
    ) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let idx = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id)
            .ok_or_else(|| StorageError::NotFound(format!("Unknown delivery: {delivery_id}")))?;
        if guard.state.deliveries[idx].status != QueueDeliveryStatus::Queued {
            return Ok(());
        }
        let prev = guard.state.deliveries[idx].steer_fallback;
        guard.state.deliveries[idx].steer_fallback = Some(fallback);
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        if let Err(err) = self.persist_locked(&mut guard, "delivery.updated", None, &[], false) {
            guard.state.deliveries[idx].steer_fallback = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn restore_queued(&self, delivery_id: &str) -> StorageResult<()> {
        self.requeue_if_in(
            delivery_id,
            &[QueueDeliveryStatus::Starting],
            None,
        )
    }

    pub fn requeue_refused(
        &self,
        delivery_id: &str,
        resume_after_usage_limit: Option<UsageLimitContinuation>,
        finished: bool,
    ) -> StorageResult<()> {
        let allowed: &[QueueDeliveryStatus] = if finished {
            &[QueueDeliveryStatus::Completed]
        } else {
            &[QueueDeliveryStatus::Starting, QueueDeliveryStatus::Running]
        };
        self.requeue_if_in(delivery_id, allowed, resume_after_usage_limit)
    }

    pub fn requeue_after_restart(
        &self,
        delivery_id: &str,
        routine_retry: bool,
    ) -> StorageResult<bool> {
        let should_requeue = {
            let mut guard = self.inner.mutable.lock();
            let Some(del) = guard
                .state
                .deliveries
                .iter()
                .find(|d| d.id == delivery_id)
                .cloned()
            else {
                return Ok(false);
            };
            let Some(msg) = guard
                .state
                .messages
                .iter()
                .find(|m| m.id == del.message_id)
                .cloned()
            else {
                return Ok(false);
            };
            let key = format!("{RESTARTED_RESULT_KEY_PREFIX}{delivery_id}");
            let is_teammate_res = is_answer_message(&msg) && msg.channel_id.is_none();
            let eligible_kind = if routine_retry {
                matches!(msg.sender, QueueSender::Routine { .. })
            } else {
                is_teammate_res
            };
            if !matches!(
                del.status,
                QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
            ) || !eligible_kind
                || guard.state.idempotency.contains_key(&key)
            {
                return Ok(false);
            }
            guard.state.idempotency.insert(key, msg.id.clone());
            if del.status == QueueDeliveryStatus::Running
                && let Some(tid) = &del.turn_id
            {
                guard.state.idempotency.insert(
                    format!("{RESTARTED_TURN_KEY_PREFIX}{delivery_id}:{tid}"),
                    msg.id,
                );
            }
            true
        };
        if should_requeue {
            self.requeue_refused(delivery_id, None, false)?;
        }
        Ok(should_requeue)
    }

    pub fn requeue_for_plan(&self, delivery_id: &str, turn_id: &str) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let idx = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id)
            .ok_or_else(|| StorageError::NotFound(format!("Unknown delivery: {delivery_id}")))?;
        if !matches!(
            guard.state.deliveries[idx].status,
            QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
        ) {
            return Ok(());
        }
        let prev = guard.state.deliveries[idx].clone();
        let mut turn_ids = prev
            .plan_continuation
            .as_ref()
            .map(|p| p.turn_ids.clone())
            .unwrap_or_default();
        if !turn_ids.iter().any(|t| t == turn_id) {
            turn_ids.push(turn_id.to_string());
        }
        guard.state.deliveries[idx].status = QueueDeliveryStatus::Queued;
        guard.state.deliveries[idx].turn_id = None;
        guard.state.deliveries[idx].error = None;
        guard.state.deliveries[idx].steered_at = None;
        guard.state.deliveries[idx].plan_continuation = Some(PlanContinuationMarker {
            turn_ids,
            queued: Some(true),
        });
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        if let Err(err) = self.persist_locked(&mut guard, "delivery.updated", None, &[], false) {
            guard.state.deliveries[idx] = prev;
            return Err(err);
        }
        Ok(())
    }

    pub fn settle_plan_continuations(
        &self,
        agent_id: &str,
        lane: Option<Option<&str>>,
    ) -> StorageResult<Vec<String>> {
        let mut guard = self.inner.mutable.lock();
        let prev = guard.state.deliveries.clone();
        let mut settled = Vec::new();
        {
            let g = &mut *guard;
            let state = &mut g.state;
            let msgs = messages_by_id(&state.messages);
            for d in &mut state.deliveries {
                if d.recipient_agent_id != agent_id || !waits_for_plan_continuation(d) {
                    continue;
                }
                if let Some(expected_lane) = lane
                    && lane_of_delivery(d, &msgs).as_deref() != expected_lane
                {
                    continue;
                }
                let tids = d
                    .plan_continuation
                    .as_ref()
                    .map(|p| p.turn_ids.clone())
                    .unwrap_or_default();
                d.status = QueueDeliveryStatus::Completed;
                d.turn_id = tids.last().cloned();
                d.error = None;
                d.plan_continuation = Some(PlanContinuationMarker {
                    turn_ids: tids,
                    queued: None,
                });
                g.changes.delivery_ids.insert(d.id.clone());
                settled.push(d.id.clone());
            }
        }
        if !settled.is_empty()
            && let Err(err) = self.persist_locked(&mut guard, "delivery.updated", None, &[], false)
        {
            guard.state.deliveries = prev;
            return Err(err);
        }
        Ok(settled)
    }

    pub fn recover_as_interrupted(&self, delivery_id: &str, reason: &str) -> StorageResult<()> {
        self.mark_terminal(delivery_id, QueueDeliveryStatus::Interrupted, Some(reason))
    }

    pub fn unresolved_deliveries(&self) -> Vec<DeliveryContext> {
        let guard = self.inner.mutable.lock();
        let positions = compute_queued_positions(&guard.state.deliveries);
        guard
            .state
            .deliveries
            .iter()
            .filter(|d| {
                matches!(
                    d.status,
                    QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
                )
            })
            .filter_map(|d| build_delivery_context(&guard.state, d, &positions))
            .collect()
    }

    /// Cancels all `"queued"` main-queue deliveries to `target_agent_id` sent by `caller_agent_id`
    /// that are NOT completed-answer notices (`agent-interrupt-tool.ts:L156-L173`).
    pub fn cancel_queued_from_agent(
        &self,
        target_agent_id: &str,
        caller_agent_id: &str,
    ) -> StorageResult<usize> {
        if target_agent_id == caller_agent_id {
            return Err(StorageError::Validation(
                "An agent cannot interrupt itself.".to_string(),
            ));
        }
        let ids: Vec<String> = self
            .list_queue(target_agent_id)
            .deliveries
            .into_iter()
            .filter(|d| {
                d.status == QueueDeliveryStatus::Queued
                    && matches!(&d.sender, QueueSender::Agent { agent_id } if agent_id == caller_agent_id)
                    && !(d.reply_to_message_id.is_some() && d.expects_reply == Some(false))
            })
            .map(|d| d.id)
            .collect();
        for did in &ids {
            self.cancel(target_agent_id, did)?;
        }
        Ok(ids.len())
    }

    /// Validates whether `caller_agent_id` owns all active deliveries of `turn_id` on `target_agent_id`
    /// (`agent-interrupt-tool.ts:L84-L104`, `L209-L215`).
    pub fn check_interrupt_ownership(
        &self,
        target_agent_id: &str,
        caller_agent_id: &str,
        turn_id: &str,
        is_execution_or_channel: bool,
    ) -> Result<Option<String>, InterruptRefusal> {
        if target_agent_id == caller_agent_id {
            return Err(InterruptRefusal::SelfTarget);
        }
        let deliveries = self.find_deliveries_by_turn(target_agent_id, turn_id);
        if is_execution_or_channel || deliveries.iter().any(|c| c.channel_id.is_some()) {
            return Err(InterruptRefusal::UseChannelTaskControlsWork);
        }
        let sent_by_caller = |c: &DeliveryContext| {
            matches!(&c.delivery.sender, QueueSender::Agent { agent_id } if agent_id == caller_agent_id)
        };
        if deliveries.is_empty() || !deliveries.iter().all(sent_by_caller) {
            return if deliveries.iter().any(sent_by_caller) {
                Err(InterruptRefusal::InterruptSharedWork)
            } else {
                Err(InterruptRefusal::InterruptOtherWork)
            };
        }
        Ok(deliveries.first().map(|c| c.delivery.message_id.clone()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn enqueue_interrupt_notice(
        &self,
        caller_agent_id: &str,
        requester_ids: Vec<String>,
        target_agent_id: &str,
        interrupted_message_id: Option<&str>,
        reason: Option<&str>,
        idempotency_key: String,
    ) -> StorageResult<QueuedMessageReceipt> {
        if target_agent_id == caller_agent_id {
            return Err(StorageError::Validation(
                "An agent cannot interrupt itself.".to_string(),
            ));
        }
        let mut lines = vec![match interrupted_message_id {
            Some(mid) => format!("I interrupted your turn for my message {mid}."),
            None => "I interrupted the turn that you ran for my message.".to_string(),
        }];
        if let Some(r) = reason.map(str::trim).filter(|s| !s.is_empty()) {
            lines.push(format!("Reason: {r}"));
        }
        lines.push(
            "Do not resume that work unless I send you a new request. I do not need a result for it, and I do not need a reply to this notice."
                .to_string(),
        );
        self.enqueue(EnqueueInput {
            sender: Some(QueueSender::Agent {
                agent_id: caller_agent_id.to_string(),
            }),
            requester_ids,
            recipient_agent_ids: vec![target_agent_id.to_string()],
            text: lines.join("\n"),
            reply_to_message_id: None,
            expects_reply: Some(false),
            idempotency_key: Some(idempotency_key),
            ..Default::default()
        })
    }

    pub fn reaction_for(
        &self,
        agent_id: &str,
        message_id: &str,
        actor: &ConversationReactionActor,
    ) -> Option<String> {
        let guard = self.inner.mutable.lock();
        guard
            .state
            .reactions
            .iter()
            .find(|r| r.agent_id == agent_id && r.message_id == message_id && &r.actor == actor)
            .map(|r| r.emoji.clone())
    }

    pub fn reactions_for(&self, agent_id: &str) -> HashMap<String, Vec<ConversationReaction>> {
        let guard = self.inner.mutable.lock();
        let mut map: HashMap<String, Vec<ConversationReaction>> = HashMap::new();
        for r in &guard.state.reactions {
            if r.agent_id != agent_id {
                continue;
            }
            map.entry(r.message_id.clone())
                .or_default()
                .push(ConversationReaction {
                    emoji: r.emoji.clone(),
                    actor: r.actor.clone(),
                });
        }
        for list in map.values_mut() {
            list.sort_by(compare_reaction_actors);
        }
        map
    }

    pub fn set_reaction(
        &self,
        agent_id: &str,
        message_id: &str,
        actor: ConversationReactionActor,
        emoji: Option<&str>,
    ) -> StorageResult<()> {
        if let Some(em) = emoji
            && !is_message_reaction(em)
        {
            return Err(StorageError::Validation("Invalid message reaction.".to_string()));
        }
        let mut guard = self.inner.mutable.lock();
        let prev = guard.state.reactions.clone();
        let pos = guard
            .state
            .reactions
            .iter()
            .position(|r| r.agent_id == agent_id && r.message_id == message_id && r.actor == actor);
        match emoji {
            None => {
                let Some(idx) = pos else {
                    return Ok(());
                };
                guard.state.reactions.remove(idx);
            }
            Some(em) => {
                let entry = StoredReaction {
                    agent_id: agent_id.to_string(),
                    message_id: message_id.to_string(),
                    emoji: em.to_string(),
                    actor: actor.clone(),
                    updated_at: now_iso(),
                };
                if let Some(idx) = pos {
                    guard.state.reactions[idx] = entry;
                } else {
                    guard.state.reactions.push(entry);
                }
            }
        }
        let key = mailbox_reaction_row_key(agent_id, message_id, &actor)?;
        guard.changes.reaction_keys.insert(key);
        if let Err(err) = self.persist_locked(&mut guard, "reaction.updated", None, &[], false) {
            guard.state.reactions = prev;
            return Err(err);
        }
        Ok(())
    }

    /// Projects mailbox exchanges into `ConversationMessage` rows (`outbox-<message.id>` + incoming
    /// `${delivery.id}`) (`mailbox-store.ts:L1670-L1803`).
    pub fn conversation_messages(
        &self,
        agent_id: &str,
        from_created_at: Option<&str>,
        limit: Option<usize>,
    ) -> Vec<ConversationMessage> {
        let cap = limit
            .unwrap_or(MAILBOX_CONVERSATION_MESSAGE_LIMIT)
            .clamp(1, MAILBOX_CONVERSATION_MESSAGE_LIMIT);
        let guard = self.inner.mutable.lock();
        let positions = compute_queued_positions(&guard.state.deliveries);
        let source_turns = compute_source_turn_ids(&guard.state.idempotency);
        let holders = build_request_holders(&guard.state);
        let msgs_map = messages_by_id(&guard.state.messages);

        let mut dels_by_msg: HashMap<&str, Vec<&StoredMailboxDelivery>> = HashMap::new();
        for d in &guard.state.deliveries {
            dels_by_msg.entry(d.message_id.as_str()).or_default().push(d);
        }

        let mut selected = Vec::new();
        let mut completed_count = 0usize;
        for msg in guard.state.messages.iter().rev() {
            if msg.channel_id.is_some() || msg.messaging.is_some() {
                continue;
            }
            let own_msg = matches!(&msg.sender, QueueSender::Agent { agent_id: aid } if aid == agent_id);
            let dels = dels_by_msg
                .get(msg.id.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if msg.task_id.is_some()
                && (!own_msg || !dels.iter().any(|d| d.recipient_agent_id != agent_id))
            {
                continue;
            }
            if (msg.messaging_return.is_some() || msg.sender_task_id.is_some()) && own_msg {
                continue;
            }
            let mut relevant = own_msg;
            let mut active = false;
            for d in dels {
                if !own_msg && d.recipient_agent_id != agent_id {
                    continue;
                }
                relevant = true;
                if is_active_delivery(d) {
                    active = true;
                }
            }
            if !relevant {
                continue;
            }
            let conv_time = if own_msg {
                msg.created_at.as_str()
            } else {
                dels.iter()
                    .find(|d| d.recipient_agent_id == agent_id)
                    .and_then(|d| d.steered_at.as_deref())
                    .unwrap_or(msg.created_at.as_str())
            };
            if !active
                && let Some(cutoff) = from_created_at
                && conv_time < cutoff
            {
                continue;
            }
            if !active && completed_count >= cap {
                continue;
            }
            selected.push(msg);
            if !active {
                completed_count += 1;
            }
        }
        selected.reverse();

        let mut out = Vec::new();
        for msg in selected {
            let dels = dels_by_msg
                .get(msg.id.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let only_self = !dels.is_empty() && dels.iter().all(|d| d.recipient_agent_id == agent_id);
            if matches!(&msg.sender, QueueSender::Agent { agent_id: aid } if aid == agent_id)
                && !only_self
            {
                let mut row = ConversationMessage::new_simple(
                    format!("outbox-{}", msg.id),
                    ConversationMessageAuthor::System,
                    msg.text.clone(),
                    msg.created_at.clone(),
                    ConversationMessageStatus::Completed,
                );
                row.turn_id = source_turns.get(&msg.id).cloned();
                row.source = Some(ConversationMessageSource::System);
                row.attachments =
                    Some(msg.attachments.iter().map(|a| a.summary.clone()).collect());
                row.reply_to_message_id = Some(msg.reply_to_message_id.clone());
                row.item_type = Some(AGENT_EXCHANGE_ITEM_TYPE.to_string());
                row.exchange = Some(AgentExchangeSummary {
                    direction: AgentExchangeDirection::Outgoing,
                    message_id: msg.id.clone(),
                    sender_agent_id: agent_id.to_string(),
                    recipient_agent_ids: dels.iter().map(|d| d.recipient_agent_id.clone()).collect(),
                    reply_to_message_id: msg.reply_to_message_id.clone(),
                    expects_reply: (msg.expects_reply == Some(false)).then_some(false),
                    sender_task_id: None,
                    deliveries: dels
                        .iter()
                        .map(|item| {
                            let pub_d = to_public_delivery(item, &positions, msg);
                            let held = pub_d.status == QueueDeliveryStatus::Completed
                                && msg.expects_reply != Some(false)
                                && held_by_task_in_state(
                                    &guard.state,
                                    &item.recipient_agent_id,
                                    &msg.id,
                                    &msgs_map,
                                    &holders,
                                );
                            AgentExchangeDeliverySummary {
                                id: pub_d.id,
                                recipient_agent_id: pub_d.recipient_agent_id,
                                status: if held {
                                    QueueDeliveryStatus::Running
                                } else {
                                    pub_d.status
                                },
                                position: pub_d.position,
                                error: pub_d.error,
                            }
                        })
                        .collect(),
                });
                out.push(row);
            }

            for sd in dels {
                if sd.recipient_agent_id != agent_id {
                    continue;
                }
                let pub_d = to_public_delivery(sd, &positions, msg);
                let (author, source, sender_aid, item_type, routine) = match &msg.sender {
                    QueueSender::Agent { agent_id: aid } => (
                        ConversationMessageAuthor::Agent,
                        ConversationMessageSource::Agent,
                        Some(aid.clone()),
                        Some(AGENT_EXCHANGE_ITEM_TYPE.to_string()),
                        None,
                    ),
                    QueueSender::Routine {
                        routine_id,
                        run_id,
                        routine_name,
                        scheduled_for,
                    } => (
                        ConversationMessageAuthor::User,
                        ConversationMessageSource::Routine,
                        None,
                        Some("routine".to_string()),
                        Some(ConversationRoutineOrigin {
                            routine_id: routine_id.clone(),
                            run_id: run_id.clone(),
                            name: routine_name.clone(),
                            scheduled_for: scheduled_for.clone(),
                        }),
                    ),
                    QueueSender::User => (
                        ConversationMessageAuthor::User,
                        ConversationMessageSource::User,
                        None,
                        (msg.welcome == Some(true)).then(|| AGENT_WELCOME_ITEM_TYPE.to_string()),
                        None,
                    ),
                };
                let mut row = ConversationMessage::new_simple(
                    pub_d.id.clone(),
                    author,
                    msg.text.clone(),
                    sd.steered_at.clone().unwrap_or_else(|| msg.created_at.clone()),
                    if pub_d.status == QueueDeliveryStatus::Failed {
                        ConversationMessageStatus::Failed
                    } else {
                        ConversationMessageStatus::Completed
                    },
                );
                row.turn_id = sd
                    .plan_continuation
                    .as_ref()
                    .and_then(|p| p.turn_ids.first().cloned())
                    .or_else(|| {
                        sd.resume_after_usage_limit
                            .as_ref()
                            .and_then(|u| u.previous_turn_ids.first().cloned())
                    })
                    .or_else(|| sd.turn_id.clone());
                row.source = Some(source);
                row.sender_agent_id = sender_aid.clone();
                row.sender_member = msg.sender_member.clone();
                row.attachments =
                    Some(msg.attachments.iter().map(|a| a.summary.clone()).collect());
                row.voice_recording = msg
                    .voice_recording
                    .as_ref()
                    .filter(|v| v.deleted_at.is_none())
                    .map(|v| v.summary.clone());
                row.reply_to_message_id = Some(msg.reply_to_message_id.clone());
                row.delivery = Some(ConversationDeliverySummary {
                    id: pub_d.id.clone(),
                    status: pub_d.status,
                    position: pub_d.position,
                });
                row.exchange = sender_aid.map(|aid| AgentExchangeSummary {
                    direction: AgentExchangeDirection::Incoming,
                    message_id: msg.id.clone(),
                    sender_agent_id: aid,
                    recipient_agent_ids: dels.iter().map(|d| d.recipient_agent_id.clone()).collect(),
                    reply_to_message_id: msg.reply_to_message_id.clone(),
                    expects_reply: (msg.expects_reply == Some(false)).then_some(false),
                    sender_task_id: msg.sender_task_id.clone(),
                    deliveries: dels
                        .iter()
                        .map(|item| {
                            let pi = to_public_delivery(item, &positions, msg);
                            AgentExchangeDeliverySummary {
                                id: pi.id,
                                recipient_agent_id: pi.recipient_agent_id,
                                status: pi.status,
                                position: pi.position,
                                error: pi.error,
                            }
                        })
                        .collect(),
                });
                row.routine = routine;
                row.steered = sd.steered_at.is_some().then_some(true);
                row.item_type = item_type;
                out.push(row);
            }
        }
        out
    }

    pub fn delete_agent_data(
        &self,
        agent_id: &str,
        channel_thread_ids: &[String],
    ) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let prev = guard.state.clone();
        let ch_threads: HashSet<&str> = channel_thread_ids.iter().map(String::as_str).collect();
        let mut outbox_paths = HashSet::new();

        let mut kept_gen = Vec::new();
        for g in guard.state.generated_attachments.drain(..) {
            let remove = g.owner_agent_id.as_deref() == Some(agent_id)
                && !g
                    .owner_thread_id
                    .as_deref()
                    .is_some_and(|tid| ch_threads.contains(tid));
            if remove {
                if let Some(dir) = Path::new(&g.attachment.path).parent() {
                    outbox_paths.insert(dir.to_string_lossy().into_owned());
                }
            } else {
                kept_gen.push(g);
            }
        }
        guard.state.generated_attachments = kept_gen;
        guard
            .state
            .deliveries
            .retain(|d| d.recipient_agent_id != agent_id);
        let remaining_msg_ids: HashSet<String> = guard
            .state
            .deliveries
            .iter()
            .map(|d| d.message_id.clone())
            .collect();
        let mut removed_msg_ids = HashSet::new();
        guard.state.messages.retain(|m| {
            let keep = remaining_msg_ids.contains(&m.id) || m.channel_id.is_some();
            if !keep {
                removed_msg_ids.insert(m.id.clone());
                outbox_paths.insert(
                    self.inner
                        .transfers_root
                        .join(&m.id)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            keep
        });
        guard.state.paused_agent_ids.retain(|id| id != agent_id);
        guard
            .state
            .reactions
            .retain(|r| r.agent_id != agent_id && !removed_msg_ids.contains(&r.message_id));
        guard
            .state
            .idempotency
            .retain(|_, mid| !removed_msg_ids.contains(mid));

        let deletions: Vec<String> = outbox_paths.into_iter().collect();
        let cmd = format!("mailbox:hard-delete:{}", Uuid::new_v4());
        if let Err(err) = self.persist_locked(
            &mut guard,
            "mailbox.agent-data-deleted",
            Some(&cmd),
            &deletions,
            true,
        ) {
            guard.state = prev;
            return Err(err);
        }
        drop(guard);
        self.drain_file_deletion_outbox()
    }

    pub fn delete_channel_data(
        &self,
        channel_id: &str,
        thread_ids: &[String],
    ) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let prev = guard.state.clone();
        let removed_threads: HashSet<&str> = thread_ids.iter().map(String::as_str).collect();
        let mut removed_msg_ids = HashSet::new();
        let mut outbox_paths = HashSet::new();

        guard.state.messages.retain(|m| {
            if m.channel_id.as_deref() == Some(channel_id) {
                removed_msg_ids.insert(m.id.clone());
                outbox_paths.insert(
                    self.inner
                        .transfers_root
                        .join(&m.id)
                        .to_string_lossy()
                        .into_owned(),
                );
                false
            } else {
                true
            }
        });
        guard
            .state
            .deliveries
            .retain(|d| !removed_msg_ids.contains(&d.message_id));
        guard
            .state
            .reactions
            .retain(|r| !removed_msg_ids.contains(&r.message_id));
        guard
            .state
            .idempotency
            .retain(|_, mid| !removed_msg_ids.contains(mid));
        guard.state.generated_attachments.retain(|g| {
            let remove = g
                .owner_thread_id
                .as_deref()
                .is_some_and(|tid| removed_threads.contains(tid));
            if remove && let Some(dir) = Path::new(&g.attachment.path).parent() {
                outbox_paths.insert(dir.to_string_lossy().into_owned());
            }
            !remove
        });

        let deletions: Vec<String> = outbox_paths.into_iter().collect();
        let cmd = format!("mailbox:channel-delete:{}", Uuid::new_v4());
        if let Err(err) = self.persist_locked(
            &mut guard,
            "mailbox.channel-data-deleted",
            Some(&cmd),
            &deletions,
            true,
        ) {
            guard.state = prev;
            return Err(err);
        }
        drop(guard);
        self.drain_file_deletion_outbox()
    }

    pub fn move_to_task(
        &self,
        agent_id: &str,
        delivery_id: &str,
        task_id: &str,
    ) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let del = guard
            .state
            .deliveries
            .iter()
            .find(|d| d.id == delivery_id && d.recipient_agent_id == agent_id)
            .cloned()
            .ok_or_else(|| StorageError::Conflict("Queued message is unavailable.".to_string()))?;
        if del.status != QueueDeliveryStatus::Queued || del.edit_id.is_some() {
            return Err(StorageError::Conflict(
                "Queued message is unavailable.".to_string(),
            ));
        }
        let recipient_count = guard
            .state
            .deliveries
            .iter()
            .filter(|d| d.message_id == del.message_id)
            .count();
        let msg_idx = guard
            .state
            .messages
            .iter()
            .position(|m| m.id == del.message_id)
            .ok_or_else(|| StorageError::NotFound("Message missing.".to_string()))?;
        let m = &guard.state.messages[msg_idx];
        if m.sender != QueueSender::User
            || m.channel_id.is_some()
            || m.messaging.is_some()
            || m.task_id.is_some()
            || recipient_count != 1
        {
            return Err(StorageError::Conflict(
                "Message cannot be moved to a side thread.".to_string(),
            ));
        }
        guard.state.messages[msg_idx].task_id = Some(task_id.to_string());
        guard.changes.message_ids.insert(del.message_id);
        let cmd = format!("mailbox:task-assigned:{delivery_id}");
        if let Err(err) =
            self.persist_locked(&mut guard, "mailbox.task-assigned", Some(&cmd), &[], false)
        {
            guard.state.messages[msg_idx].task_id = None;
            return Err(err);
        }
        Ok(())
    }

    fn finish_cancellation_locked(
        &self,
        guard: &mut MailboxMutable,
        delivery_id: &str,
        cancel_delivery: bool,
    ) -> StorageResult<()> {
        let idx = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id)
            .ok_or_else(|| StorageError::NotFound(format!("Unknown delivery: {delivery_id}")))?;
        let prev_state = guard.state.clone();
        let edit_id = guard.state.deliveries[idx].edit_id.take();
        if cancel_delivery {
            guard.state.deliveries[idx].status = QueueDeliveryStatus::Cancelled;
        }
        if let Some(ref eid) = edit_id {
            let outcomes = guard.state.deliveries[idx]
                .finished_edit_outcomes
                .get_or_insert_with(BTreeMap::new);
            outcomes.insert(
                eid.clone(),
                StoredFinishedEdit {
                    action: QueueEditOutcome::Cancel,
                    save_hash: None,
                },
            );
            for d in &mut guard.state.drafts {
                if d.owner_edit_id.as_deref() == Some(eid.as_str()) {
                    d.owner_edit_id = None;
                    d.preserve_on_restart = Some(true);
                    guard
                        .changes
                        .draft_ids
                        .insert(d.attachment.summary.id.clone());
                }
            }
        }
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        let event_type = if cancel_delivery {
            "delivery.cancelled"
        } else {
            "delivery.edit-finished"
        };
        if let Err(err) = self.persist_locked(guard, event_type, None, &[], false) {
            guard.state = prev_state;
            return Err(err);
        }
        guard.edit_holds.remove(delivery_id);
        Ok(())
    }

    fn requeue_if_in(
        &self,
        delivery_id: &str,
        allowed: &[QueueDeliveryStatus],
        resume_after_usage_limit: Option<UsageLimitContinuation>,
    ) -> StorageResult<()> {
        let mut guard = self.inner.mutable.lock();
        let idx = guard
            .state
            .deliveries
            .iter()
            .position(|d| d.id == delivery_id)
            .ok_or_else(|| StorageError::NotFound(format!("Unknown delivery: {delivery_id}")))?;
        if !allowed.contains(&guard.state.deliveries[idx].status) {
            return Ok(());
        }
        let prev = guard.state.deliveries[idx].clone();
        guard.state.deliveries[idx].status = QueueDeliveryStatus::Queued;
        guard.state.deliveries[idx].turn_id = None;
        guard.state.deliveries[idx].error = None;
        guard.state.deliveries[idx].steered_at = None;
        if let Some(ul) = resume_after_usage_limit {
            guard.state.deliveries[idx].resume_after_usage_limit = Some(ul);
        }
        guard.changes.delivery_ids.insert(delivery_id.to_string());
        if let Err(err) = self.persist_locked(&mut guard, "delivery.updated", None, &[], false) {
            guard.state.deliveries[idx] = prev;
            return Err(err);
        }
        Ok(())
    }

    fn persist_locked(
        &self,
        guard: &mut MailboxMutable,
        event_type: &str,
        command_id: Option<&str>,
        file_deletions: &[String],
        replace_all: bool,
    ) -> StorageResult<()> {
        let resolved_cmd = command_id
            .map(str::to_string)
            .unwrap_or_else(|| format!("mailbox:{event_type}:{}", Uuid::new_v4()));
        let wrote = if replace_all {
            self.inner.projection.replace_mailbox_state(
                &resolved_cmd,
                &guard.state,
                event_type,
                file_deletions,
            )?
        } else {
            self.inner.projection.apply_mailbox_changes(
                &resolved_cmd,
                &guard.state,
                &guard.changes,
                event_type,
                file_deletions,
            )?
        };
        if wrote {
            guard.changes.clear();
        }
        Ok(())
    }
}

pub fn is_returned_channel_task(task: &ChannelTask) -> bool {
    task.parent_task_id.is_none()
        && matches!(
            task.state,
            ChannelTaskState::Completed
                | ChannelTaskState::Failed
                | ChannelTaskState::Cancelled
                | ChannelTaskState::Paused
        )
}

pub fn moved_to_open_task(task: &ChannelTask, routed_message: &ChannelMessage) -> bool {
    task.state == ChannelTaskState::Cancelled
        && task.owner_agent_id.is_none()
        && routed_message
            .task_id
            .as_deref()
            .is_some_and(|tid| tid != task.id)
}

pub fn channel_handoff_sender_agent_id(message: &ChannelMessage) -> Option<&str> {
    message
        .author
        .id
        .strip_prefix(CHANNEL_HANDOFF_AUTHOR_PREFIX)
        .filter(|id| is_identifier(id))
}

/// Formats the channel request return text (`channel-request-returns.ts:L174-L195`).
pub fn format_channel_request_return_text(
    task: &ChannelTask,
    channel_id: &str,
    channel_name: &str,
    answer: Option<&ChannelMessage>,
) -> String {
    let origin = format!(
        "Channel: {channel_name} ({channel_id}). This answers your openbot.send_to_channel request."
    );
    if task.state == ChannelTaskState::Completed {
        let trimmed = answer.map(|a| a.message.text.trim()).unwrap_or("");
        let body = if trimmed.is_empty() {
            "The member finished without a written result.".to_string()
        } else {
            slice_utf16(trimmed, CHANNEL_RETURN_ANSWER_CHARS)
        };
        return format!("{origin}\n{body}");
    }
    let reason = slice_utf16(
        task.error.as_deref().unwrap_or("").trim(),
        CHANNEL_RETURN_ERROR_CHARS,
    );
    if task.state == ChannelTaskState::Paused {
        let res_line = format!(
            "Result: OpenBot note, not written by this member: the channel task stopped before it finished. {reason}"
        );
        return [
            origin.as_str(),
            "Status: paused",
            res_line.trim(),
            "Next: Do not send the request again. A person can continue the task in the channel, and its result then comes back here.",
        ]
        .join("\n");
    }
    let verb = if task.state == ChannelTaskState::Failed {
        "failed"
    } else {
        "was cancelled"
    };
    let res_line = format!(
        "Result: OpenBot note, not written by this member: the channel task {verb} before it finished. {reason}"
    );
    [
        origin.as_str(),
        "Status: blocked",
        res_line.trim(),
        "Unblock: Read the channel, then send the request again if you still need the result.",
    ]
    .join("\n")
}

/// Stateful return reconciler for `send_to_channel` root tasks (`channel-request-returns.ts:L32-L158`).
#[derive(Default)]
pub struct ChannelRequestReturnsTracker {
    handled: HashSet<String>,
    noted: HashSet<String>,
    restored: bool,
}

impl ChannelRequestReturnsTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn restore(&mut self, all_tasks: &[ChannelTask]) {
        for task in all_tasks {
            if is_returned_channel_task(task) {
                self.handled
                    .insert(channel_request_return_key(&task.id, task.revision, task.state));
                self.noted.insert(task.id.clone());
            }
        }
        self.restored = true;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn reconcile_channel<FA, FT>(
        &mut self,
        mailbox: &MailboxStore,
        channel_id: &str,
        channel_name: &str,
        lead_agent_id: Option<&str>,
        tasks: &[ChannelTask],
        messages_by_id_fn: impl Fn(&str) -> Option<ChannelMessage>,
        answer_for_task: impl Fn(&ChannelTask, &ChannelMessage) -> Option<ChannelMessage>,
        agent_exists: FA,
        task_open: FT,
    ) -> StorageResult<Vec<QueuedMessageReceipt>>
    where
        FA: Fn(&str) -> bool,
        FT: Fn(&str) -> bool,
    {
        if !self.restored {
            return Ok(Vec::new());
        }
        let mut receipts = Vec::new();
        for task in tasks {
            if !is_returned_channel_task(task) {
                self.noted.remove(&task.id);
                continue;
            }
            let key = channel_request_return_key(&task.id, task.revision, task.state);
            if !self.handled.insert(key.clone()) {
                continue;
            }
            if task.state == ChannelTaskState::Paused && !self.noted.insert(task.id.clone()) {
                continue;
            }
            self.noted.insert(task.id.clone());
            if let Some(routed) = messages_by_id_fn(&task.request_message_id)
                && moved_to_open_task(task, &routed)
            {
                continue;
            }
            let mut candidate_ids = vec![task.request_message_id.as_str()];
            for sid in task.source_message_ids.iter().rev() {
                if !candidate_ids.contains(&sid.as_str()) {
                    candidate_ids.push(sid.as_str());
                }
            }
            let Some((req_msg, requester_id)) = candidate_ids.into_iter().find_map(|id| {
                let m = messages_by_id_fn(id)?;
                let req_aid = channel_handoff_sender_agent_id(&m)?.to_string();
                Some((m, req_aid))
            }) else {
                continue;
            };
            let Some(sender_agent_id) = task.owner_agent_id.as_deref().or(lead_agent_id) else {
                continue;
            };
            if !agent_exists(sender_agent_id) || !agent_exists(&requester_id) {
                continue;
            }
            let answer = (task.state == ChannelTaskState::Completed)
                .then(|| answer_for_task(task, &req_msg))
                .flatten();
            let requester_tid = req_msg
                .sender_task_id
                .clone()
                .filter(|tid| task_open(tid));
            let receipt = mailbox.enqueue(EnqueueInput {
                sender: Some(QueueSender::Agent {
                    agent_id: sender_agent_id.to_string(),
                }),
                recipient_agent_ids: vec![requester_id],
                text: format_channel_request_return_text(
                    task,
                    channel_id,
                    channel_name,
                    answer.as_ref(),
                ),
                reply_to_message_id: Some(req_msg.id),
                task_id: requester_tid,
                expects_reply: Some(false),
                idempotency_key: Some(key),
                ..Default::default()
            })?;
            receipts.push(receipt);
        }
        Ok(receipts)
    }
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn waits_for_plan_continuation(d: &StoredMailboxDelivery) -> bool {
    d.status == QueueDeliveryStatus::Queued
        && d.plan_continuation
            .as_ref()
            .is_some_and(|p| p.queued == Some(true))
}

fn is_active_delivery(d: &StoredMailboxDelivery) -> bool {
    matches!(
        d.status,
        QueueDeliveryStatus::Queued | QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
    )
}

fn compare_queue_order(a: &StoredMailboxDelivery, b: &StoredMailboxDelivery) -> std::cmp::Ordering {
    a.queue_order
        .cmp(&b.queue_order)
        .then_with(|| a.created_at.cmp(&b.created_at))
        .then_with(|| a.id.cmp(&b.id))
}

fn next_queue_order(deliveries: &[StoredMailboxDelivery], agent_id: &str) -> i64 {
    deliveries
        .iter()
        .filter(|d| d.recipient_agent_id == agent_id)
        .map(|d| d.queue_order)
        .fold(-1i64, i64::max)
        + 1
}

/// Computes 1-based queue `position` per `recipient_agent_id` (`mailbox-store.ts:L3811-L3823`).
fn compute_queued_positions(deliveries: &[StoredMailboxDelivery]) -> HashMap<String, u32> {
    let mut queued: Vec<&StoredMailboxDelivery> = deliveries
        .iter()
        .filter(|d| d.status == QueueDeliveryStatus::Queued && !waits_for_plan_continuation(d))
        .collect();
    queued.sort_by(|a, b| compare_queue_order(a, b));
    let mut counts: HashMap<&str, u32> = HashMap::new();
    let mut positions = HashMap::with_capacity(queued.len());
    for d in queued {
        let next = counts
            .get(d.recipient_agent_id.as_str())
            .copied()
            .unwrap_or(0)
            + 1;
        counts.insert(&d.recipient_agent_id, next);
        positions.insert(d.id.clone(), next);
    }
    positions
}

fn to_public_delivery(
    d: &StoredMailboxDelivery,
    positions: &HashMap<String, u32>,
    msg: &StoredMailboxMessage,
) -> QueueDelivery {
    let waiting_plan = waits_for_plan_continuation(d);
    let status = if waiting_plan {
        QueueDeliveryStatus::Running
    } else {
        d.status
    };
    let turn_id = if waiting_plan {
        d.plan_continuation
            .as_ref()
            .and_then(|p| p.turn_ids.last().cloned())
    } else {
        d.turn_id.clone()
    };
    let position = if status == QueueDeliveryStatus::Queued {
        positions.get(&d.id).copied()
    } else {
        None
    };
    QueueDelivery {
        id: d.id.clone(),
        message_id: d.message_id.clone(),
        recipient_agent_id: d.recipient_agent_id.clone(),
        sender: msg.sender.clone(),
        text: msg.text.clone(),
        attachments: msg.attachments.iter().map(|a| a.summary.clone()).collect(),
        reply_to_message_id: msg.reply_to_message_id.clone(),
        expects_reply: (msg.expects_reply == Some(false)).then_some(false),
        status,
        position,
        editing: Some(d.edit_id.is_some()),
        steer_fallback: (d.status == QueueDeliveryStatus::Queued)
            .then_some(d.steer_fallback)
            .flatten(),
        turn_id,
        error: d.error.clone(),
        created_at: d.created_at.clone(),
    }
}

fn build_receipt(state: &MailboxProjectionState, message_id: &str) -> QueuedMessageReceipt {
    let positions = compute_queued_positions(&state.deliveries);
    let Some(msg) = state.messages.iter().find(|m| m.id == message_id) else {
        return QueuedMessageReceipt {
            message_id: message_id.to_string(),
            deliveries: Vec::new(),
        };
    };
    let deliveries = state
        .deliveries
        .iter()
        .filter(|d| d.message_id == message_id)
        .map(|d| {
            let pub_d = to_public_delivery(d, &positions, msg);
            QueuedDeliveryReceiptItem {
                id: pub_d.id,
                recipient_agent_id: pub_d.recipient_agent_id,
                status: pub_d.status,
                position: pub_d.position,
            }
        })
        .collect();
    QueuedMessageReceipt {
        message_id: message_id.to_string(),
        deliveries,
    }
}

fn build_delivery_context(
    state: &MailboxProjectionState,
    d: &StoredMailboxDelivery,
    positions: &HashMap<String, u32>,
) -> Option<DeliveryContext> {
    let msg = state.messages.iter().find(|m| m.id == d.message_id)?;
    let restarted = state
        .idempotency
        .contains_key(&format!("{RESTARTED_RESULT_KEY_PREFIX}{}", d.id))
        && d.resume_after_usage_limit.is_none()
        && d.plan_continuation.is_none();
    let turn_prefix = format!("{RESTARTED_TURN_KEY_PREFIX}{}:", d.id);
    let restarted_turn_id = restarted.then(|| {
        state
            .idempotency
            .keys()
            .find_map(|k| k.strip_prefix(&turn_prefix).map(str::to_string))
    }).flatten();
    Some(DeliveryContext {
        delivery: to_public_delivery(d, positions, msg),
        managed_attachments: msg
            .attachments
            .iter()
            .map(|a| ManagedAttachment {
                summary: a.summary.clone(),
                path: a.path.clone(),
            })
            .collect(),
        resume_after_usage_limit: d.resume_after_usage_limit.clone(),
        plan_continuation: d.plan_continuation.clone(),
        channel_id: msg.channel_id.clone(),
        sender_task_id: msg.sender_task_id.clone(),
        restarted_result: restarted.then_some(true),
        restarted_turn_id,
    })
}

fn messages_by_id(messages: &[StoredMailboxMessage]) -> HashMap<&str, &StoredMailboxMessage> {
    messages.iter().map(|m| (m.id.as_str(), m)).collect()
}

fn execution_message_ids(messages: &[StoredMailboxMessage]) -> HashSet<String> {
    messages
        .iter()
        .filter(|m| {
            m.channel_id.is_some()
                || m.messaging.is_some()
                || m.task_id.is_some()
                || m.welcome == Some(true)
        })
        .map(|m| m.id.clone())
        .collect()
}

fn lane_of_delivery(
    d: &StoredMailboxDelivery,
    msgs: &HashMap<&str, &StoredMailboxMessage>,
) -> Option<String> {
    let msg = msgs.get(d.message_id.as_str())?;
    if let Some(tid) = &msg.task_id {
        return Some(tid.clone());
    }
    if let Some(cid) = &msg.channel_id {
        return Some(channel_lane(cid));
    }
    if msg.messaging.is_some() {
        return Some(MESSAGING_LANE.to_string());
    }
    None
}

fn is_held_by_edit_map(
    holds: &HashMap<String, EditHoldEntry>,
    d: &StoredMailboxDelivery,
    now_ms: u64,
) -> bool {
    let Some(ref eid) = d.edit_id else {
        return false;
    };
    let Some(hold) = holds.get(&d.id) else {
        return false;
    };
    &hold.edit_id == eid && now_ms.saturating_sub(hold.started_at_ms) < QUEUE_EDIT_HOLD_MS
}

fn is_answer_message(m: &StoredMailboxMessage) -> bool {
    matches!(m.sender, QueueSender::Agent { .. })
        && m.reply_to_message_id.is_some()
        && m.expects_reply == Some(false)
}

fn is_chat_message_from_person(m: &StoredMailboxMessage) -> bool {
    m.sender == QueueSender::User
        && m.welcome != Some(true)
        && m.channel_id.is_none()
        && m.messaging.is_none()
        && m.messaging_return.is_none()
}

fn has_answered(messages: &[StoredMailboxMessage], request_id: &str, agent_id: &str) -> bool {
    messages.iter().any(|m| {
        is_answer_message(m)
            && matches!(&m.sender, QueueSender::Agent { agent_id: aid } if aid == agent_id)
            && m.reply_to_message_id.as_deref() == Some(request_id)
    })
}

fn has_reply_from_messages(
    messages: &[StoredMailboxMessage],
    agent_id: &str,
    message_id: &str,
) -> bool {
    messages.iter().any(|m| {
        matches!(&m.sender, QueueSender::Agent { agent_id: aid } if aid == agent_id)
            && m.reply_to_message_id.as_deref() == Some(message_id)
    })
}

fn answered_request_of<'a>(
    msgs: &'a HashMap<&str, &StoredMailboxMessage>,
    agent_id: &str,
    request_id: &str,
) -> Option<&'a StoredMailboxMessage> {
    let req = *msgs.get(request_id)?;
    if matches!(&req.sender, QueueSender::Agent { agent_id: aid } if aid == agent_id)
        && req.expects_reply != Some(false)
    {
        Some(req)
    } else {
        None
    }
}

fn answered_request_in_state(state: &MailboxProjectionState, delivery_id: &str) -> bool {
    let Some(del) = state.deliveries.iter().find(|d| d.id == delivery_id) else {
        return false;
    };
    let Some(req) = state.messages.iter().find(|m| m.id == del.message_id) else {
        return false;
    };
    if !matches!(req.sender, QueueSender::Agent { .. }) || req.channel_id.is_some() {
        return false;
    }
    has_answered(&state.messages, &req.id, &del.recipient_agent_id)
}

fn build_request_holders(state: &MailboxProjectionState) -> HashMap<String, HashSet<String>> {
    let mut holders: HashMap<String, HashSet<String>> = HashMap::new();
    for msg in &state.messages {
        let Some(tid) = msg.task_id.as_ref().or(msg.sender_task_id.as_ref()) else {
            continue;
        };
        let QueueSender::Agent { ref agent_id } = msg.sender else {
            continue;
        };
        for req_id in msg.held_request_ids.as_deref().unwrap_or(&[]) {
            holders
                .entry(format!("{agent_id}:{req_id}"))
                .or_default()
                .insert(tid.clone());
        }
    }
    holders
}

fn task_has_own_work(
    state: &MailboxProjectionState,
    agent_id: &str,
    task_id: &str,
    msgs: &HashMap<&str, &StoredMailboxMessage>,
) -> bool {
    state.deliveries.iter().any(|d| {
        if d.recipient_agent_id != agent_id || !is_active_delivery(d) {
            return false;
        }
        let Some(m) = msgs.get(d.message_id.as_str()) else {
            return false;
        };
        m.task_id.as_deref() == Some(task_id)
            || (m.sender_task_id.as_deref() == Some(task_id)
                && matches!(&m.sender, QueueSender::Agent { agent_id: aid } if aid == agent_id))
    })
}

fn held_by_task_in_state(
    state: &MailboxProjectionState,
    agent_id: &str,
    request_id: &str,
    msgs: &HashMap<&str, &StoredMailboxMessage>,
    holders: &HashMap<String, HashSet<String>>,
) -> bool {
    let Some(tids) = holders.get(&format!("{agent_id}:{request_id}")) else {
        return false;
    };
    if tids.is_empty() || has_reply_from_messages(&state.messages, agent_id, request_id) {
        return false;
    }
    let awaited = awaited_thread_requests_from_state(state);
    tids.iter().any(|tid| {
        task_has_own_work(state, agent_id, tid, msgs)
            || awaited
                .iter()
                .any(|r| r.task_id == *tid && r.sender_agent_id == agent_id)
    })
}

fn still_works_on_in_state(
    state: &MailboxProjectionState,
    d: &StoredMailboxDelivery,
    request_id: &str,
    msgs: &HashMap<&str, &StoredMailboxMessage>,
    holders: &HashMap<String, HashSet<String>>,
) -> bool {
    is_active_delivery(d)
        || (d.status == QueueDeliveryStatus::Completed
            && held_by_task_in_state(state, &d.recipient_agent_id, request_id, msgs, holders))
}

/// Multi-recipient fan-out barrier (`mailbox-store.ts:L2078-L2116`): holds an answer while any
/// sibling recipient still works on `request_id` unless a subsequent turn in the same lane ended.
fn is_held_reply_in_state(
    state: &MailboxProjectionState,
    d: &StoredMailboxDelivery,
    msgs: &HashMap<&str, &StoredMailboxMessage>,
    holders: &HashMap<String, HashSet<String>>,
) -> bool {
    let Some(reply) = msgs.get(d.message_id.as_str()) else {
        return false;
    };
    if !is_answer_message(reply) {
        return false;
    }
    let QueueSender::Agent {
        agent_id: ref answered_by,
    } = reply.sender
    else {
        return false;
    };
    let Some(req_id) = reply.reply_to_message_id.as_deref() else {
        return false;
    };
    let Some(request) = answered_request_of(msgs, &d.recipient_agent_id, req_id) else {
        return false;
    };
    let sibling_working = state.deliveries.iter().any(|c| {
        c.message_id == request.id
            && &c.recipient_agent_id != answered_by
            && still_works_on_in_state(state, c, &request.id, msgs, holders)
    });
    if !sibling_working {
        return false;
    }
    let lane = lane_of_delivery(d, msgs);
    let turn_after_ended = state.deliveries.iter().any(|c| {
        c.recipient_agent_id == d.recipient_agent_id
            && c.turn_id.is_some()
            && c.steered_at.is_none()
            && !is_active_delivery(c)
            && c.created_at > request.created_at
            && lane_of_delivery(c, msgs) == lane
    });
    !turn_after_ended
}

fn awaited_thread_requests_from_state(
    state: &MailboxProjectionState,
) -> Vec<AwaitedThreadRequest> {
    let msgs = messages_by_id(&state.messages);
    let holders = build_request_holders(state);
    let answered: HashSet<String> = state
        .messages
        .iter()
        .filter(|m| is_answer_message(m))
        .filter_map(|m| {
            let rid = m.reply_to_message_id.as_deref()?;
            let QueueSender::Agent { ref agent_id } = m.sender else {
                return None;
            };
            Some(format!("{rid}:{agent_id}"))
        })
        .collect();

    state
        .deliveries
        .iter()
        .filter_map(|d| {
            let completed = d.status == QueueDeliveryStatus::Completed;
            let awaited_state = match d.status {
                QueueDeliveryStatus::Completed | QueueDeliveryStatus::Running => {
                    AgentTaskAwaitedState::Running
                }
                QueueDeliveryStatus::Queued => AgentTaskAwaitedState::Queued,
                QueueDeliveryStatus::Starting => AgentTaskAwaitedState::Starting,
                _ => return None,
            };
            let m = msgs.get(d.message_id.as_str())?;
            let sender_tid = m.sender_task_id.as_ref()?;
            let QueueSender::Agent {
                agent_id: ref sender_aid,
            } = m.sender
            else {
                return None;
            };
            if m.expects_reply == Some(false)
                || answered.contains(&format!("{}:{}", m.id, d.recipient_agent_id))
            {
                return None;
            }
            if completed {
                let held_own = holders
                    .get(&format!("{}:{}", d.recipient_agent_id, m.id))
                    .is_some_and(|tids| {
                        tids.iter()
                            .any(|tid| task_has_own_work(state, &d.recipient_agent_id, tid, &msgs))
                    });
                if !held_own {
                    return None;
                }
            }
            Some(AwaitedThreadRequest {
                delivery_id: d.id.clone(),
                sender_agent_id: sender_aid.clone(),
                task_id: sender_tid.clone(),
                reply: AgentTaskAwaitedReply {
                    message_id: m.id.clone(),
                    recipient_agent_id: d.recipient_agent_id.clone(),
                    status: awaited_state,
                },
            })
        })
        .collect()
}

fn chain_origin_in_messages(messages: &[StoredMailboxMessage], message_id: &str) -> Option<String> {
    let mut visited = HashSet::new();
    let mut current = messages.iter().find(|m| m.id == message_id);
    while let Some(msg) = current {
        if !visited.insert(msg.id.as_str()) {
            return None;
        }
        let parent = msg
            .reply_to_message_id
            .as_deref()
            .and_then(|pid| messages.iter().find(|m| m.id == pid));
        match parent {
            None => {
                return match &msg.sender {
                    QueueSender::Agent { agent_id } => Some(agent_id.clone()),
                    _ => None,
                };
            }
            Some(p) => current = Some(p),
        }
    }
    None
}

fn answer_task_id_in_state(
    state: &MailboxProjectionState,
    sender_agent_id: &str,
    reply_to_message_id: &str,
    recipient_agent_id: &str,
) -> Option<String> {
    let req = state.messages.iter().find(|m| m.id == reply_to_message_id)?;
    let sender_tid = req.sender_task_id.as_ref()?;
    if !matches!(&req.sender, QueueSender::Agent { agent_id } if agent_id == recipient_agent_id) {
        return None;
    }
    let received = state
        .deliveries
        .iter()
        .any(|d| d.message_id == req.id && d.recipient_agent_id == sender_agent_id);
    received.then(|| sender_tid.clone())
}

fn compare_reaction_actors(
    left: &ConversationReaction,
    right: &ConversationReaction,
) -> std::cmp::Ordering {
    match (&left.actor, &right.actor) {
        (ConversationReactionActor::User, ConversationReactionActor::User) => {
            std::cmp::Ordering::Equal
        }
        (ConversationReactionActor::User, ConversationReactionActor::Agent { .. }) => {
            std::cmp::Ordering::Less
        }
        (ConversationReactionActor::Agent { .. }, ConversationReactionActor::User) => {
            std::cmp::Ordering::Greater
        }
        (
            ConversationReactionActor::Agent { agent_id: la },
            ConversationReactionActor::Agent { agent_id: ra },
        ) => la.cmp(ra),
    }
}

fn compute_source_turn_ids(idempotency: &BTreeMap<String, String>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (key, mid) in idempotency {
        if map.contains_key(mid) {
            continue;
        }
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() >= 3 {
            let turn_id = parts[parts.len() - 2];
            if !turn_id.is_empty() {
                map.insert(mid.clone(), turn_id.to_string());
            }
        }
    }
    map
}

fn normalize_sender(sender: QueueSender) -> QueueSender {
    match sender {
        QueueSender::Routine {
            routine_id,
            run_id,
            routine_name,
            scheduled_for,
        } => QueueSender::Routine {
            routine_id,
            run_id,
            routine_name: slice_utf16(&routine_name, INPUT_LIMITS.routine_name),
            scheduled_for,
        },
        other => other,
    }
}

/// Rewrites `@[name](attachment:id)` markers (`packages/contracts/src/attachment-references.ts:L38-L55`).
pub fn rewrite_attachment_references<'a, F>(text: &str, resolve: F) -> String
where
    F: Fn(&str) -> Option<(&'a str, &'a str)>,
{
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("@[") {
        out.push_str(&rest[..start]);
        let after_at = &rest[start + 2..];
        let Some(close_bracket) = after_at.find("](attachment:") else {
            out.push_str("@[");
            rest = after_at;
            continue;
        };
        let raw_name = &after_at[..close_bracket];
        if raw_name.is_empty() || raw_name.contains(['\r', '\n', ']']) {
            out.push_str("@[");
            rest = after_at;
            continue;
        }
        let after_prefix = &after_at[close_bracket + "](attachment:".len()..];
        let Some(close_paren) = after_prefix.find(')') else {
            out.push_str("@[");
            rest = after_at;
            continue;
        };
        let raw_id = &after_prefix[..close_paren];
        if raw_id.is_empty() || raw_id.contains(['\r', '\n', ')']) {
            out.push_str("@[");
            rest = after_at;
            continue;
        }
        match resolve(raw_id) {
            Some((new_id, new_name)) => {
                let safe_name: String = new_name
                    .chars()
                    .map(|c| if matches!(c, ']' | '\r' | '\n') { ' ' } else { c })
                    .collect();
                let clean_name = safe_name.trim();
                let final_name = if clean_name.is_empty() { "file" } else { clean_name };
                let clean_id: String = new_id
                    .chars()
                    .filter(|c| !matches!(c, ')' | '\r' | '\n'))
                    .collect();
                out.push_str(&format!("@[{final_name}](attachment:{})", clean_id.trim()));
            }
            None => out.push_str(raw_name),
        }
        rest = &after_prefix[close_paren + 1..];
    }
    out.push_str(rest);
    out
}

fn commit_transfer_files(
    transfer_dir: &Path,
    sources: &[PathBuf],
) -> StorageResult<Vec<StoredAttachment>> {
    create_dir_0700(transfer_dir)?;
    let mut committed = Vec::with_capacity(sources.len());
    for (idx, src) in sources.iter().enumerate() {
        let bytes = fs::read(src)?;
        let id = Uuid::new_v4().to_string();
        let name = sanitize_file_name(src);
        let sub = transfer_dir.join(format!("{idx}-{id}"));
        create_dir_0700(&sub)?;
        let dst = sub.join(&name);
        fs::write(&dst, &bytes)?;
        committed.push(build_stored_attachment(&id, &name, &dst, &bytes));
    }
    Ok(committed)
}

fn build_stored_attachment(
    id: &str,
    name: &str,
    path: &Path,
    bytes: &[u8],
) -> StoredAttachment {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let sha256 = hex_lower(&hasher.finalize());
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (kind, mime_type, preview_kind) = match ext.as_str() {
        "png" => (AttachmentKind::Image, "image/png", AttachmentPreviewKind::Image),
        "jpg" | "jpeg" => (AttachmentKind::Image, "image/jpeg", AttachmentPreviewKind::Image),
        "webp" => (AttachmentKind::Image, "image/webp", AttachmentPreviewKind::Image),
        "gif" => (AttachmentKind::Image, "image/gif", AttachmentPreviewKind::Image),
        "pdf" => (AttachmentKind::File, "application/pdf", AttachmentPreviewKind::Pdf),
        "txt" | "md" | "json" | "rs" | "ts" | "py" | "toml" | "yaml" | "yml" | "csv" => {
            (AttachmentKind::File, "text/plain", AttachmentPreviewKind::Text)
        }
        _ => (
            AttachmentKind::File,
            "application/octet-stream",
            AttachmentPreviewKind::None,
        ),
    };
    StoredAttachment {
        summary: AttachmentSummary {
            id: id.to_string(),
            name: name.to_string(),
            size: bytes.len() as u64,
            kind,
            mime_type: mime_type.to_string(),
            preview_kind,
            preview_url: None,
        },
        path: path.to_string_lossy().into_owned(),
        sha256,
        deleted_at: None,
    }
}

fn sanitize_file_name(path: &Path) -> String {
    let raw = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("attachment.bin");
    sanitize_name_str(raw)
}

fn sanitize_name_str(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | '\0') { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "attachment.bin".to_string()
    } else {
        slice_utf16(trimmed, INPUT_LIMITS.attachment_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScopedTestDir(PathBuf);

    impl ScopedTestDir {
        fn new(prefix: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("{prefix}_{}", Uuid::new_v4()));
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
    }

    impl Drop for ScopedTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn mailbox_store_enforces_positions_edit_hold_fanout_barrier_and_failed_reply_synthesis() {
        let tmp = ScopedTestDir::new("openbot_mailbox_store");
        let db = OpenBotDatabase::open(tmp.0.join("data").join("openbot.db")).unwrap();
        let store = MailboxStore::open(db.clone(), tmp.0.join("OpenBot").join("Shared")).unwrap();

        let r1 = store
            .enqueue(EnqueueInput {
                recipient_agent_ids: vec!["agent-a".to_string()],
                text: "First user message".to_string(),
                ..Default::default()
            })
            .unwrap();
        let r2 = store
            .enqueue(EnqueueInput {
                recipient_agent_ids: vec!["agent-a".to_string()],
                text: "Second user message".to_string(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r1.deliveries[0].position, Some(1));
        assert_eq!(r2.deliveries[0].position, Some(2));

        let d1_id = r1.deliveries[0].id.clone();
        store
            .begin_queue_edit_at("agent-a", &d1_id, "edit-1", 1_000)
            .unwrap();
        let next_while_held = store.next_queued_at("agent-a", None, 2_000).unwrap();
        assert_eq!(next_while_held.delivery.id, r2.deliveries[0].id);
        assert!(store.mark_starting_at(&d1_id, 2_000).is_err());

        store
            .mark_starting_at(&d1_id, 1_000 + QUEUE_EDIT_HOLD_MS + 1)
            .unwrap();
        assert_eq!(
            store.finished_queue_edit_action("agent-a", &d1_id, "edit-1"),
            Some(QueueEditOutcome::Cancel)
        );
        assert_eq!(store.get_delivery(&d1_id).unwrap().delivery.position, None);
        store.mark_running(&d1_id, "turn-a1").unwrap();
        store
            .mark_terminal(&d1_id, QueueDeliveryStatus::Completed, None)
            .unwrap();
        store.cancel("agent-a", &r2.deliveries[0].id).unwrap();

        assert!(store
            .send_message(SendAgentMessageInput {
                sender_agent_id: "orch".to_string(),
                recipient_agent_ids: vec!["orch".to_string()],
                text: "self".to_string(),
                reply_to_message_id: None,
                expects_reply: None,
                sender_task_id: None,
                requester_ids: Vec::new(),
                source_paths: Vec::new(),
                idempotency_key: None,
            })
            .is_err());

        let fanout = store
            .send_message(SendAgentMessageInput {
                sender_agent_id: "orch".to_string(),
                recipient_agent_ids: vec!["w1".to_string(), "w2".to_string()],
                text: "Investigate module".to_string(),
                reply_to_message_id: None,
                expects_reply: Some(true),
                sender_task_id: None,
                requester_ids: Vec::new(),
                source_paths: Vec::new(),
                idempotency_key: Some("fanout-1".to_string()),
            })
            .unwrap();
        let w1_del = fanout.deliveries[0].id.clone();
        let w2_del = fanout.deliveries[1].id.clone();
        store.mark_starting_at(&w1_del, 10_000).unwrap();
        store.mark_running(&w1_del, "turn-w1").unwrap();
        store.mark_starting_at(&w2_del, 10_000).unwrap();
        store.mark_running(&w2_del, "turn-w2").unwrap();

        let reply1_text = format_teammate_reply(&ParsedTeammateReply {
            status: TeammateReplyStatus::Done,
            result: "Checked w1".to_string(),
            evidence: "w1.rs:10".to_string(),
            unblock: None,
        });
        assert!(parse_teammate_reply(&reply1_text).is_some());
        let ans1 = store
            .send_message(SendAgentMessageInput {
                sender_agent_id: "w1".to_string(),
                recipient_agent_ids: vec!["orch".to_string()],
                text: reply1_text,
                reply_to_message_id: Some(fanout.message_id.clone()),
                expects_reply: Some(false),
                sender_task_id: None,
                requester_ids: Vec::new(),
                source_paths: Vec::new(),
                idempotency_key: None,
            })
            .unwrap();
        store
            .mark_terminal(&w1_del, QueueDeliveryStatus::Completed, None)
            .unwrap();

        assert!(store.next_queued_at("orch", None, 20_000).is_none());

        store
            .mark_terminal(&w2_del, QueueDeliveryStatus::Failed, Some("rate limit"))
            .unwrap();
        let notified = store
            .answer_failed_request(&w2_del, |_| false, |_| true, false, true)
            .unwrap();
        assert_eq!(notified.as_deref(), Some("orch"));

        let first_ready = store.next_queued_at("orch", None, 30_000).unwrap();
        assert_eq!(first_ready.delivery.id, ans1.deliveries[0].id);
        let companions = store.replies_to_start_with_at(&first_ready.delivery.id, 30_000);
        assert_eq!(companions.len(), 1);
        assert!(companions[0].delivery.text.starts_with("Status: blocked\n"));

        let mailbox_events: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM orchestration_events WHERE aggregate_type = 'mailbox'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(mailbox_events, 1);
    }
}
