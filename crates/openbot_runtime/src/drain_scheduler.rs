//! Multi-lane `DrainScheduler`, queue controls, branching, side-questions, reactions, pins,
//! bot-to-bot `Mailbox`, `ChannelRequestReturns`, `interrupt_agent`, and 1:1 `TeamChatStore` DMs (`OB-F07`).
//!
//! Mirrors `src/backend/agent/{drain-scheduler,queue-controls,side-question,message-pins,agent-interrupt-tool,boot-recovery}.ts`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use openbot_contracts::agents::{
    AGENT_TASK_LIMIT_DEFAULT, AGENT_TASK_LIMIT_EXTENDED_MAX, AgentProviderId, AgentSummary,
    AgentTask, AgentTaskBranch, AgentTaskOrigin, AgentTaskQueuedMessage, AgentTaskState,
    AgentTasksSnapshot, AgentUsageLimitReturn, agent_task_limit,
};
use openbot_contracts::channels::{
    CHANNEL_HANDOFF_AUTHOR_PREFIX, ChannelMessage, ChannelMessageAuthor, ChannelMessageAuthorKind,
    ChannelTask, ChannelTaskState, TASK_EXPECTED_RESULT,
};
use openbot_contracts::conversation::{
    AGENT_WELCOME_ITEM_TYPE, CHANNEL_RESUME_ITEM_TYPE, CONVERSATION_PLAN_ITEM_TYPE,
    CONVERSATION_SUBAGENTS_ITEM_TYPE, ConversationMessage, ConversationMessageAuthor,
    ConversationMessageSender, ConversationMessageSource, ConversationMessageStatus,
    ConversationReactionActor, ConversationSnapshot, MESSAGE_PINS_LIMIT, MessagePinner,
    PinnedMessagesSnapshot, ROUTINE_QUIET_ANSWER_ITEM_TYPE, is_message_reaction,
};
use openbot_contracts::limits::{
    ATTACHMENT_LIMITS, INPUT_LIMITS, clip_utf16, is_bounded_string, is_identifier, utf16_len,
};
use openbot_contracts::providers::{AgentProviderRef, provider_steers_turns};
use openbot_contracts::queue::{
    AttachmentSummary, QUEUE_EDIT_HOLD_MS, QueueDeliveryStatus, QueueEditRequest, QueueSender,
    QueueSnapshot, QueueSteerFallback, routine_runs_turn,
};
use openbot_storage::{
    AgentRoster, AgentThreadBranchInput, AgentThreadInput, AgentThreadRecord,
    AgentThreadTitleSource, AppendConversationMessageInput, ChannelChange,
    ChannelRequestReturnsTracker, ChannelStore, DirectConversationReadState,
    DirectConversationSnapshot, DirectMessage, DirectThreadSummary, EnqueueInput, InterruptRefusal,
    MailboxStore, OpenBotDatabase, ParsedTeammateReply, PersistConversationChangesInput,
    PinRefusalReason, QueueEditOutcome, QueuedDeliveryReceiptItem, QueuedMessageReceipt,
    SendAgentMessageInput, SendDirectMessageInput, TeamChatStore, ThreadStore, copy_branch_messages,
    now_rfc3339_millis, parse_teammate_reply, stable_thread_id,
};
use parking_lot::Mutex;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::attention_registry::AttentionRegistry;
use crate::context_compactor::{
    CompactionOutcome, CompactionReason, ContextCompactor, DEFAULT_CONTEXT_WINDOW,
    estimate_text_tokens,
};
use crate::error::{OpenBotRuntimeError, RuntimeResult};
use crate::providers::{ProviderRuntimeManager, StartTurnRequest, build_handoff_packet};

/// Maximum chat messages included in an ephemeral side-question context window.
pub const SIDE_QUESTION_CHAT_MESSAGES: usize = 60;
/// Character budget for chat transcript in a side-question prompt.
const SIDE_QUESTION_CHAT_CHARACTERS: usize = 60_000;
/// Character budget for earlier side-question turns in a side-question prompt.
const SIDE_QUESTION_EARLIER_CHARACTERS: usize = 60_000;
/// Prefix for chat-visual item types excluded from side-question transcripts.
const CHAT_VISUAL_ITEM_TYPE_PREFIX: &str = "chat-visual:";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SchedulerLane {
    Main,
    Messaging,
    Task {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Channel {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
}

impl SchedulerLane {
    pub fn key(&self, agent_id: &str) -> String {
        match self {
            Self::Main => agent_id.to_string(),
            Self::Messaging => format!("{agent_id}\0\u{0001}messaging"),
            Self::Task { task_id } => format!("{agent_id}\0{task_id}"),
            Self::Channel { channel_id } => format!("{agent_id}\0\u{0001}channel:{channel_id}"),
        }
    }

    pub fn mailbox_lane(&self) -> Option<String> {
        match self {
            Self::Main => None,
            Self::Messaging => Some("\u{0001}messaging".to_string()),
            Self::Task { task_id } => Some(task_id.clone()),
            Self::Channel { channel_id } => Some(format!("\u{0001}channel:{channel_id}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ActiveTurnOwner {
    User,
    Agent {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    Routine,
    Channel,
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActiveTurnOwnerHint {
    Caller,
    User,
    Channel,
    Shared,
}

impl ActiveTurnOwnerHint {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "caller" => Some(Self::Caller),
            "user" => Some(Self::User),
            "channel" => Some(Self::Channel),
            "shared" => Some(Self::Shared),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActiveLaneTurn {
    pub agent_id: String,
    pub lane: SchedulerLane,
    pub thread_id: String,
    pub turn_id: String,
    pub delivery_id: Option<String>,
    pub owner: ActiveTurnOwner,
    pub started_at: String,
    pub cancelled: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootRecoveryReport {
    pub recovered_deliveries: usize,
    pub requeued_deliveries: usize,
    pub interrupted_messages: usize,
    pub cleared_threads: usize,
    pub restored_channel_tasks: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnDispatchOutcome {
    pub agent_id: String,
    pub lane: SchedulerLane,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub delivery_id: String,
    pub turn_id: String,
    pub message_id: String,
    pub active_provider: AgentProviderId,
    pub active_model: String,
    pub answer_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit_return: Option<AgentUsageLimitReturn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskRunResult {
    pub task: AgentThreadRecord,
    pub delivery_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<TurnDispatchOutcome>,
}

#[derive(Debug, Clone, Default)]
pub struct SendChatMessageInput {
    pub agent_id: String,
    pub text: String,
    pub task_id: Option<String>,
    pub attachments: Vec<AttachmentSummary>,
    pub client_message_id: Option<String>,
    pub sender_member: Option<ConversationMessageSender>,
    pub simulate_primary_rate_limit: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendChatMessageOutcome {
    pub message_id: String,
    pub user_message_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub turn_id: String,
    pub active_provider: AgentProviderId,
    pub active_model: String,
    pub answer_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit_return: Option<AgentUsageLimitReturn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_switch_item_type: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct QueueAddInput {
    pub agent_id: String,
    pub text: String,
    pub task_id: Option<String>,
    pub attachments: Vec<AttachmentSummary>,
    pub welcome: bool,
    pub sender: Option<QueueSender>,
    pub sender_member: Option<ConversationMessageSender>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueAddOutcome {
    pub message_id: String,
    pub delivery_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    pub status: QueueDeliveryStatus,
    pub welcome: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueListItem {
    pub delivery_id: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    pub status: QueueDeliveryStatus,
    pub text: String,
    pub sender: QueueSender,
    pub welcome: bool,
    pub steered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editing: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_fallback: Option<QueueSteerFallback>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueListOutcome {
    pub agent_id: String,
    pub paused: bool,
    pub welcome: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<String>,
    pub items: Vec<QueueListItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuePauseOutcome {
    pub agent_id: String,
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueResumeOutcome {
    pub agent_id: String,
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_delivery_id: Option<String>,
    pub items: Vec<QueueListItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueEditResult {
    pub snapshot: QueueSnapshot,
    pub hold_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_expires_at_ms: Option<u64>,
    pub items: Vec<QueueListItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSteerInput {
    pub agent_id: String,
    pub delivery_id: String,
    pub expected_turn_id: String,
    pub routine_turn: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSteerOutcome {
    pub accepted: bool,
    pub steered: bool,
    pub steer_refused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_fallback: Option<QueueSteerFallback>,
    pub delivery_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct BranchThreadInput {
    pub agent_id: String,
    pub message_id: String,
    pub request_id: String,
    pub source_task_id: Option<String>,
    pub title: Option<String>,
    pub simulate_source_busy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchThreadOutcome {
    pub task_id: String,
    pub thread_id: String,
    pub source_thread_id: String,
    pub source_message_id: String,
    pub cloned_messages: usize,
    pub record: AgentThreadRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideQuestionTurn {
    pub question: String,
    pub answer: String,
}

#[derive(Debug, Clone, Default)]
pub struct SideQuestionInput {
    pub agent_id: String,
    pub question: String,
    pub task_id: Option<String>,
    pub earlier: Vec<SideQuestionTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideQuestionOutcome {
    pub agent_id: String,
    pub answer: String,
    pub earlier_count: usize,
}

#[derive(Debug, Clone)]
pub struct ReactMessageInput {
    pub agent_id: String,
    pub task_id: Option<String>,
    pub channel_id: Option<String>,
    pub message_id: String,
    pub emoji: String,
    pub actor: ConversationReactionActor,
    pub remove: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionSummaryItem {
    pub emoji: String,
    pub actor: ConversationReactionActor,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactMessageOutcome {
    pub agent_id: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    pub reactions: Vec<ReactionSummaryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinActor {
    Person {
        sender: Option<ConversationMessageSender>,
    },
    Agent,
}

#[derive(Debug, Clone)]
pub struct PinMessageInput {
    pub agent_id: String,
    pub task_id: Option<String>,
    pub channel_id: Option<String>,
    pub message_id: String,
    pub actor: PinActor,
    pub unpin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinMessageOutcome {
    pub agent_id: String,
    pub message_id: String,
    pub pinned: bool,
    pub pinned_by_kind: String,
    pub snapshot: PinnedMessagesSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxParsedReply {
    pub status: String,
    pub result: String,
    pub evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unblock: Option<String>,
}

impl From<ParsedTeammateReply> for MailboxParsedReply {
    fn from(value: ParsedTeammateReply) -> Self {
        Self {
            status: value.status.as_str().to_string(),
            result: value.result,
            evidence: value.evidence,
            unblock: value.unblock,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MailboxSendInput {
    pub sender_agent_id: String,
    pub recipient_agent_ids: Vec<String>,
    pub text: String,
    pub expects_reply: Option<bool>,
    pub reply_to_message_id: Option<String>,
    pub sender_task_id: Option<String>,
    pub requester_ids: Vec<String>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxSendOutcome {
    pub message_id: String,
    pub deliveries: Vec<QueuedDeliveryReceiptItem>,
    pub expects_reply: bool,
    pub answer_optional: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed_reply: Option<MailboxParsedReply>,
    pub barrier_complete: bool,
    pub pending_recipients: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SendToChannelInput {
    pub sender_agent_id: String,
    pub channel_id: String,
    pub text: String,
    pub sender_task_id: Option<String>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendToChannelOutcome {
    pub channel_id: String,
    pub message_id: String,
    pub task_id: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, Default)]
pub struct InterruptAgentInput {
    pub caller_agent_id: String,
    pub target_agent_id: String,
    pub reason: Option<String>,
    pub active_turn_owner: Option<ActiveTurnOwnerHint>,
    pub requester_ids: Vec<String>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptAgentOutcome {
    pub interrupted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_turn_id: Option<String>,
    pub cancelled_queued_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_message_id: Option<String>,
}

#[derive(Default)]
struct SchedulerState {
    active_lanes: HashMap<String, ActiveLaneTurn>,
    edit_hold_deadlines: HashMap<String, u64>,
}

/// Multi-lane execution scheduler and chat/mailbox/DM service coordinator (`OB-F07`).
pub struct DrainScheduler {
    database: OpenBotDatabase,
    mailbox: MailboxStore,
    thread_store: ThreadStore,
    team_chat_store: TeamChatStore,
    channel_store: ChannelStore,
    providers: Arc<ProviderRuntimeManager>,
    compactor: Arc<ContextCompactor>,
    attention: Arc<AttentionRegistry>,
    state: Mutex<SchedulerState>,
    channel_returns: Mutex<ChannelRequestReturnsTracker>,
}

impl DrainScheduler {
    pub fn new(
        database: OpenBotDatabase,
        providers: Arc<ProviderRuntimeManager>,
        compactor: Arc<ContextCompactor>,
        attention: Arc<AttentionRegistry>,
    ) -> RuntimeResult<Arc<Self>> {
        let mailbox = database.mailbox_store()?;
        let thread_store = database.thread_store();
        let team_chat_store = database.team_chat_store();
        let channel_store = database.channel_store();

        let mut tracker = ChannelRequestReturnsTracker::new();
        let mut all_tasks = Vec::new();
        if let Ok(channel_ids) = channel_store.ids() {
            for cid in channel_ids {
                if let Ok(tasks) = channel_store.tasks(&cid) {
                    all_tasks.extend(tasks);
                }
            }
        }
        tracker.restore(&all_tasks);

        Ok(Arc::new(Self {
            database,
            mailbox,
            thread_store,
            team_chat_store,
            channel_store,
            providers,
            compactor,
            attention,
            state: Mutex::new(SchedulerState::default()),
            channel_returns: Mutex::new(tracker),
        }))
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    pub fn mailbox(&self) -> &MailboxStore {
        &self.mailbox
    }

    pub fn thread_store(&self) -> &ThreadStore {
        &self.thread_store
    }

    pub fn team_chat_store(&self) -> &TeamChatStore {
        &self.team_chat_store
    }

    pub fn channel_store(&self) -> &ChannelStore {
        &self.channel_store
    }

    pub fn providers(&self) -> &Arc<ProviderRuntimeManager> {
        &self.providers
    }

    pub fn compactor(&self) -> &Arc<ContextCompactor> {
        &self.compactor
    }

    pub fn attention(&self) -> &Arc<AttentionRegistry> {
        &self.attention
    }

    /// Computes effective concurrent task lane limit (`1..=8`, clamped to `1` for
    /// single-turn providers like `acp`).
    pub fn effective_task_limit(&self, agent: &AgentSummary) -> u32 {
        agent_task_limit(agent.provider, agent.task_limit, AGENT_TASK_LIMIT_DEFAULT)
            .clamp(1, AGENT_TASK_LIMIT_EXTENDED_MAX)
    }

    pub fn active_lane_count(&self, agent_id: &str) -> usize {
        self.state
            .lock()
            .active_lanes
            .values()
            .filter(|turn| turn.agent_id == agent_id)
            .count()
    }

    pub fn can_start_lane(&self, agent: &AgentSummary, lane: &SchedulerLane) -> bool {
        let key = lane.key(&agent.id);
        let state = self.state.lock();
        if state.active_lanes.contains_key(&key) {
            return false;
        }
        let running = state
            .active_lanes
            .values()
            .filter(|turn| turn.agent_id == agent.id)
            .count();
        let limit = self.effective_task_limit(agent) as usize;
        if !agent.provider.descriptor().concurrent_turns {
            return running == 0;
        }
        running < limit
    }

    pub fn active_turns(&self, agent_id: &str) -> Vec<ActiveLaneTurn> {
        self.state
            .lock()
            .active_lanes
            .values()
            .filter(|t| t.agent_id == agent_id)
            .cloned()
            .collect()
    }

    /// Reconciles orphaned deliveries, stuck streaming messages, and channel request
    /// returns after an unclean crash or restart (`boot-recovery.ts`).
    pub fn recover_boot_state(&self) -> RuntimeResult<BootRecoveryReport> {
        let mut report = BootRecoveryReport::default();
        let roster = AgentRoster::new(self.database.clone());
        let existing_agents: HashSet<String> = roster
            .list_agents()?
            .into_iter()
            .map(|a| a.id)
            .collect();

        let unresolved = self.mailbox.unresolved_deliveries();
        for ctx in unresolved {
            let did = &ctx.delivery.id;
            let is_routine = matches!(ctx.delivery.sender, QueueSender::Routine { .. });
            if self.mailbox.requeue_after_restart(did, is_routine)? {
                report.requeued_deliveries += 1;
                continue;
            }
            self.mailbox.recover_as_interrupted(
                did,
                "OpenBot restarted before this delivery reached a confirmed terminal state.",
            )?;
            report.recovered_deliveries += 1;
            let agent_threads = self.thread_store.agent_threads();
            let _ = self.mailbox.answer_failed_request(
                did,
                |task_id| {
                    agent_threads
                        .task(task_id)
                        .ok()
                        .flatten()
                        .is_some_and(|rec| rec.closed_at.is_none())
                },
                |aid| existing_agents.contains(aid),
                true,
                false,
            )?;
        }

        let running_by_thread = self.thread_store.read_running_turn_ids()?;
        for (thread_id, turn_ids) in running_by_thread {
            let owner_opt: Option<String> = self.database.with_conn(|conn| {
                Ok(conn
                    .prepare_cached("SELECT agent_id FROM projection_threads WHERE thread_id = ?1")?
                    .query_row(params![thread_id], |r| r.get(0))
                    .optional()?)
            })?;
            let Some(agent_id) = owner_opt else {
                continue;
            };
            let active_turn = turn_ids.last().map(String::as_str);
            let recovery_msgs = self.thread_store.read_conversation_recovery_messages(
                &agent_id,
                Some(&thread_id),
                active_turn,
            )?;
            let mut changed = Vec::new();
            for mut msg in recovery_msgs {
                if msg.status == ConversationMessageStatus::Streaming {
                    msg.status = ConversationMessageStatus::Interrupted;
                    report.interrupted_messages += 1;
                    changed.push(msg);
                }
            }
            self.thread_store
                .persist_conversation_changes(&PersistConversationChangesInput {
                    agent_id,
                    thread_id,
                    active_turn_id: None,
                    changed_messages: changed,
                    removed_message_ids: Vec::new(),
                    deleted_message_ids: Vec::new(),
                    event_type: "recovery.interrupted".to_string(),
                    detail: serde_json::json!({ "recoveredAtBoot": true }),
                    command_id: None,
                })?;
            report.cleared_threads += 1;
        }

        let mut all_tasks = Vec::new();
        for cid in self.channel_store.ids()? {
            all_tasks.extend(self.channel_store.tasks(&cid)?);
        }
        report.restored_channel_tasks = all_tasks.len();
        self.channel_returns.lock().restore(&all_tasks);
        Ok(report)
    }

    /// Drains all runnable FIFO queued deliveries across `agent_id`'s main lane and open
    /// side-task lanes while respecting pause state, `10m` edit holds, fan-out barriers,
    /// and per-agent concurrency limits (`1..=8`).
    pub fn drain_agent_fifo(&self, agent_id: &str) -> RuntimeResult<Vec<TurnDispatchOutcome>> {
        let agent = self.require_agent(agent_id)?;
        if self.mailbox.is_queue_paused(agent_id) {
            return Ok(Vec::new());
        }
        let mut lanes = vec![SchedulerLane::Main];
        for task_id in self.thread_store.agent_threads().open_task_ids(agent_id)? {
            lanes.push(SchedulerLane::Task { task_id });
        }
        let mut dispatched = Vec::new();
        for lane in lanes {
            while let Some(outcome) = self.drain_lane_once(&agent.id, &lane)? {
                dispatched.push(outcome);
            }
        }
        Ok(dispatched)
    }

    /// Pops and executes the next eligible FIFO delivery (plus any sibling fan-out answers
    /// released together) on a single `SchedulerLane`.
    pub fn drain_lane_once(
        &self,
        agent_id: &str,
        lane: &SchedulerLane,
    ) -> RuntimeResult<Option<TurnDispatchOutcome>> {
        let agent = self.require_agent(agent_id)?;
        if self.mailbox.is_queue_paused(agent_id) || !self.can_start_lane(&agent, lane) {
            return Ok(None);
        }
        let task_id = match lane {
            SchedulerLane::Task { task_id } => Some(task_id.as_str()),
            _ => None,
        };
        let thread_id = self.ensure_thread_id(&agent, task_id)?;
        if self.compactor.is_held(&thread_id) {
            return Ok(None);
        }
        let mailbox_lane = lane.mailbox_lane();
        let Some(head) = self.mailbox.next_queued(agent_id, mailbox_lane.as_deref()) else {
            return Ok(None);
        };
        let mut batch = vec![head.clone()];
        batch.extend(self.mailbox.replies_to_start_with(&head.delivery.id));

        for ctx in &batch {
            self.mailbox.mark_starting(&ctx.delivery.id)?;
        }

        let combined_prompt = batch
            .iter()
            .map(|c| c.delivery.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let turn_owner = match &head.delivery.sender {
            QueueSender::User => ActiveTurnOwner::User,
            QueueSender::Agent { agent_id: aid } => ActiveTurnOwner::Agent {
                agent_id: aid.clone(),
            },
            QueueSender::Routine { .. } => ActiveTurnOwner::Routine,
        };

        let lane_key = lane.key(agent_id);
        let provisional_turn_id = format!("turn-{}", Uuid::new_v4());
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.state.lock().active_lanes.insert(
            lane_key.clone(),
            ActiveLaneTurn {
                agent_id: agent.id.clone(),
                lane: lane.clone(),
                thread_id: thread_id.clone(),
                turn_id: provisional_turn_id.clone(),
                delivery_id: Some(head.delivery.id.clone()),
                owner: turn_owner,
                started_at: now_rfc3339_millis(),
                cancelled: cancel_flag,
            },
        );

        let est_in = estimate_text_tokens(&combined_prompt);
        if self
            .compactor
            .expect_input(&thread_id, est_in, DEFAULT_CONTEXT_WINDOW)
        {
            let _ = self
                .compactor
                .compact_thread(&agent.id, &thread_id, CompactionReason::Threshold);
        }

        let exec_res = self.providers.execute_turn_with_failover(&StartTurnRequest {
            agent: agent.clone(),
            thread_id: thread_id.clone(),
            prompt: combined_prompt.clone(),
            handoff_prompt: None,
            simulate_primary_rate_limit: false,
        });
        self.state.lock().active_lanes.remove(&lane_key);

        let outcome = match exec_res {
            Ok(out) => out,
            Err(err) => {
                let reason = err.to_string();
                let roster = AgentRoster::new(self.database.clone());
                let existing_agents: HashSet<String> = roster
                    .list_agents()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| a.id)
                    .collect();
                for ctx in &batch {
                    let _ = self.mailbox.mark_terminal(
                        &ctx.delivery.id,
                        QueueDeliveryStatus::Failed,
                        Some(&reason),
                    );
                    let agent_threads = self.thread_store.agent_threads();
                    let _ = self.mailbox.answer_failed_request(
                        &ctx.delivery.id,
                        |tid| {
                            agent_threads
                                .task(tid)
                                .ok()
                                .flatten()
                                .is_some_and(|rec| rec.closed_at.is_none())
                        },
                        |aid| existing_agents.contains(aid),
                        false,
                        false,
                    );
                }
                return Err(err);
            }
        };

        for ctx in &batch {
            self.mailbox.mark_running(&ctx.delivery.id, &outcome.turn_id)?;
            self.mailbox
                .mark_terminal(&ctx.delivery.id, QueueDeliveryStatus::Completed, None)?;
        }

        let created_at = now_rfc3339_millis();
        let mut prompt_msg = ConversationMessage::new_simple(
            head.delivery.id.clone(),
            match &head.delivery.sender {
                QueueSender::User => ConversationMessageAuthor::User,
                QueueSender::Agent { .. } => ConversationMessageAuthor::Agent,
                QueueSender::Routine { .. } => ConversationMessageAuthor::User,
            },
            combined_prompt.clone(),
            created_at.clone(),
            ConversationMessageStatus::Completed,
        );
        prompt_msg.turn_id = Some(outcome.turn_id.clone());
        prompt_msg.source = Some(match &head.delivery.sender {
            QueueSender::User => ConversationMessageSource::User,
            QueueSender::Agent { .. } => ConversationMessageSource::Agent,
            QueueSender::Routine { .. } => ConversationMessageSource::Routine,
        });
        if let QueueSender::Agent { agent_id: sender_aid } = &head.delivery.sender {
            prompt_msg.sender_agent_id = Some(sender_aid.clone());
        }
        self.thread_store
            .append_conversation_message(&AppendConversationMessageInput {
                agent_id: agent.id.clone(),
                thread_id: thread_id.clone(),
                active_turn_id: Some(outcome.turn_id.clone()),
                message: prompt_msg,
                event_type: "delivery.started".to_string(),
                detail: serde_json::json!({ "deliveryId": head.delivery.id }),
                command_id: None,
            })?;

        let assistant_msg_id = format!("msg-{}", Uuid::new_v4());
        let mut answer_msg = ConversationMessage::new_simple(
            assistant_msg_id.clone(),
            ConversationMessageAuthor::Assistant,
            outcome.answer_text.clone(),
            now_rfc3339_millis(),
            ConversationMessageStatus::Completed,
        );
        answer_msg.turn_id = Some(outcome.turn_id.clone());
        answer_msg.source = Some(ConversationMessageSource::Assistant);
        self.thread_store
            .append_conversation_message(&AppendConversationMessageInput {
                agent_id: agent.id.clone(),
                thread_id: thread_id.clone(),
                active_turn_id: None,
                message: answer_msg,
                event_type: "turn.completed".to_string(),
                detail: serde_json::json!({
                    "deliveryId": head.delivery.id,
                    "provider": outcome.active_provider.as_str(),
                    "model": outcome.active_model,
                }),
                command_id: None,
            })?;

        let est_out = estimate_text_tokens(&outcome.answer_text);
        self.compactor
            .observe_usage(&thread_id, est_in, est_out, DEFAULT_CONTEXT_WINDOW);

        Ok(Some(TurnDispatchOutcome {
            agent_id: agent.id,
            lane: lane.clone(),
            thread_id,
            task_id: task_id.map(str::to_string),
            delivery_id: head.delivery.id,
            turn_id: outcome.turn_id,
            message_id: assistant_msg_id,
            active_provider: outcome.active_provider,
            active_model: outcome.active_model,
            answer_text: outcome.answer_text,
            usage_limit_return: outcome.usage_limit_return,
            handoff_prompt: outcome.handoff_prompt,
        }))
    }

    /// Executes a synchronous chat turn on `agent_id`'s main thread or branched side-task
    /// thread (`task_id`), enforcing `ATTACHMENT_LIMITS`, predictive compaction, and
    /// automatic backup-model failover.
    pub fn send_message(
        &self,
        input: SendChatMessageInput,
    ) -> RuntimeResult<SendChatMessageOutcome> {
        let agent = self.require_agent(&input.agent_id)?;
        validate_attachments(&input.attachments)?;
        let trimmed = input.text.trim();
        if trimmed.is_empty() && input.attachments.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Message text or at least one attachment is required.".to_string(),
            ));
        }
        if !is_bounded_string(trimmed, INPUT_LIMITS.message_text) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Message text exceeds {} characters.",
                INPUT_LIMITS.message_text
            )));
        }

        let thread_id = self.ensure_thread_id(&agent, input.task_id.as_deref())?;
        let snapshot = self
            .thread_store
            .read_conversation(&agent.id, Some(&thread_id))?;

        let est_in = estimate_text_tokens(trimmed);
        if self
            .compactor
            .expect_input(&thread_id, est_in, DEFAULT_CONTEXT_WINDOW)
        {
            let _ = self
                .compactor
                .compact_thread(&agent.id, &thread_id, CompactionReason::Threshold);
        }

        let pre_handoff = (input.simulate_primary_rate_limit || agent.usage_limit_return.is_some())
            .then(|| build_handoff_packet(&snapshot.messages, None, trimmed));

        let outcome = self.providers.execute_turn_with_failover(&StartTurnRequest {
            agent: agent.clone(),
            thread_id: thread_id.clone(),
            prompt: trimmed.to_string(),
            handoff_prompt: pre_handoff.clone(),
            simulate_primary_rate_limit: input.simulate_primary_rate_limit,
        })?;

        let user_msg_id = input
            .client_message_id
            .filter(|id| is_identifier(id))
            .unwrap_or_else(|| format!("msg-{}", Uuid::new_v4()));
        let mut user_msg = ConversationMessage::new_simple(
            user_msg_id.clone(),
            ConversationMessageAuthor::User,
            trimmed.to_string(),
            now_rfc3339_millis(),
            ConversationMessageStatus::Completed,
        );
        user_msg.turn_id = Some(outcome.turn_id.clone());
        user_msg.source = Some(ConversationMessageSource::User);
        user_msg.sender_member = input.sender_member;
        if !input.attachments.is_empty() {
            user_msg.attachments = Some(input.attachments);
        }
        self.thread_store
            .append_conversation_message(&AppendConversationMessageInput {
                agent_id: agent.id.clone(),
                thread_id: thread_id.clone(),
                active_turn_id: Some(outcome.turn_id.clone()),
                message: user_msg,
                event_type: "turn.started".to_string(),
                detail: serde_json::json!({ "turnId": outcome.turn_id }),
                command_id: None,
            })?;

        if let Some(ref switch_item) = outcome.model_switch_item_type {
            let mut switch_msg = ConversationMessage::new_simple(
                format!("switch-{}", Uuid::new_v4()),
                ConversationMessageAuthor::System,
                format!(
                    "Switched model to {} ({}).",
                    outcome.active_model,
                    outcome.active_provider.as_str()
                ),
                now_rfc3339_millis(),
                ConversationMessageStatus::Completed,
            );
            switch_msg.turn_id = Some(outcome.turn_id.clone());
            switch_msg.source = Some(ConversationMessageSource::System);
            switch_msg.item_type = Some(switch_item.clone());
            self.thread_store
                .append_conversation_message(&AppendConversationMessageInput {
                    agent_id: agent.id.clone(),
                    thread_id: thread_id.clone(),
                    active_turn_id: Some(outcome.turn_id.clone()),
                    message: switch_msg,
                    event_type: "model.switched".to_string(),
                    detail: serde_json::json!({
                        "provider": outcome.active_provider.as_str(),
                        "model": outcome.active_model,
                    }),
                    command_id: None,
                })?;
        }

        let assistant_msg_id = format!("msg-{}", Uuid::new_v4());
        let mut assistant_msg = ConversationMessage::new_simple(
            assistant_msg_id.clone(),
            ConversationMessageAuthor::Assistant,
            outcome.answer_text.clone(),
            now_rfc3339_millis(),
            ConversationMessageStatus::Completed,
        );
        assistant_msg.turn_id = Some(outcome.turn_id.clone());
        assistant_msg.source = Some(ConversationMessageSource::Assistant);
        self.thread_store
            .append_conversation_message(&AppendConversationMessageInput {
                agent_id: agent.id.clone(),
                thread_id: thread_id.clone(),
                active_turn_id: None,
                message: assistant_msg,
                event_type: "turn.completed".to_string(),
                detail: serde_json::json!({
                    "turnId": outcome.turn_id,
                    "provider": outcome.active_provider.as_str(),
                    "model": outcome.active_model,
                }),
                command_id: None,
            })?;

        if let Some(ref ul_ret) = outcome.usage_limit_return {
            self.persist_agent_failover_state(
                &agent.id,
                outcome.active_provider,
                &outcome.active_model,
                Some(ul_ret.clone()),
            )?;
        }

        let est_out = estimate_text_tokens(&outcome.answer_text);
        self.compactor
            .observe_usage(&thread_id, est_in, est_out, DEFAULT_CONTEXT_WINDOW);

        let handoff_prompt = outcome.handoff_prompt.or(pre_handoff);
        Ok(SendChatMessageOutcome {
            message_id: assistant_msg_id,
            user_message_id: user_msg_id,
            thread_id,
            task_id: input.task_id,
            turn_id: outcome.turn_id,
            active_provider: outcome.active_provider,
            active_model: outcome.active_model,
            answer_text: outcome.answer_text,
            usage_limit_return: outcome.usage_limit_return,
            handoff_prompt,
            model_switch_item_type: outcome.model_switch_item_type,
        })
    }

    pub fn queue_add(&self, input: QueueAddInput) -> RuntimeResult<QueueAddOutcome> {
        let agent = self.require_agent(&input.agent_id)?;
        validate_attachments(&input.attachments)?;
        let mut text = input.text.trim().to_string();
        if text.is_empty() && !input.attachments.is_empty() {
            text = input
                .attachments
                .iter()
                .map(|a| format!("[attachment: {}]", a.name))
                .collect::<Vec<_>>()
                .join(", ");
        }
        if text.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Queued message text cannot be empty.".to_string(),
            ));
        }
        if let Some(ref tid) = input.task_id {
            let _ = self.resolve_thread_id(&agent, Some(tid))?;
        }
        let receipt = self.mailbox.enqueue(EnqueueInput {
            task_id: input.task_id,
            sender: Some(input.sender.unwrap_or(QueueSender::User)),
            sender_member: input.sender_member,
            recipient_agent_ids: vec![agent.id],
            text,
            welcome: input.welcome,
            idempotency_key: input.idempotency_key,
            ..Default::default()
        })?;
        let item = receipt.deliveries.first().ok_or_else(|| {
            OpenBotRuntimeError::Execution("Mailbox enqueue returned zero deliveries.".to_string())
        })?;
        Ok(QueueAddOutcome {
            message_id: receipt.message_id,
            delivery_id: item.id.clone(),
            position: item.position,
            status: item.status,
            welcome: input.welcome,
        })
    }

    pub fn queue_list(&self, agent_id: &str) -> RuntimeResult<QueueListOutcome> {
        let agent = self.require_agent(agent_id)?;
        let snapshot = self.mailbox.list_queue(&agent.id);
        let paused = self.mailbox.is_queue_paused(&agent.id);
        let active_turn_id = self.active_turn_id_for_agent(&agent.id, None)?;
        let (welcome_msgs, steered_deliveries) = self.enrich_queue_metadata(&agent.id)?;
        let items: Vec<QueueListItem> = snapshot
            .deliveries
            .into_iter()
            .map(|d| QueueListItem {
                welcome: welcome_msgs.contains(&d.message_id),
                steered: steered_deliveries.contains(&d.id),
                delivery_id: d.id,
                message_id: d.message_id,
                position: d.position,
                status: d.status,
                text: d.text,
                sender: d.sender,
                editing: d.editing,
                turn_id: d.turn_id,
                steer_fallback: d.steer_fallback,
                created_at: d.created_at,
            })
            .collect();
        Ok(QueueListOutcome {
            agent_id: agent.id,
            paused,
            welcome: snapshot.welcome == Some(true) || items.iter().any(|i| i.welcome),
            active_turn_id,
            items,
        })
    }

    pub fn queue_reorder(
        &self,
        agent_id: &str,
        delivery_ids: &[String],
        task_id: Option<&str>,
    ) -> RuntimeResult<QueueListOutcome> {
        let agent = self.require_agent(agent_id)?;
        if delivery_ids.len() > INPUT_LIMITS.queue_reorder {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Queue reorder limit of {} exceeded.",
                INPUT_LIMITS.queue_reorder
            )));
        }
        if let Some(tid) = task_id {
            let lane_ids = self.mailbox.lane_queued_ids(&agent.id, Some(tid));
            let lane_set: HashSet<&str> = lane_ids.iter().map(String::as_str).collect();
            let req_set: HashSet<&str> = delivery_ids.iter().map(String::as_str).collect();
            if delivery_ids.len() != lane_ids.len()
                || req_set.len() != delivery_ids.len()
                || delivery_ids.iter().any(|id| !lane_set.contains(id.as_str()))
            {
                return Err(OpenBotRuntimeError::Conflict(
                    "Queue order is stale.".to_string(),
                ));
            }
            let full_ids: Vec<String> = self
                .mailbox
                .list_queue(&agent.id)
                .deliveries
                .into_iter()
                .filter(|d| d.status == QueueDeliveryStatus::Queued)
                .map(|d| d.id)
                .collect();
            let mut next_idx = 0usize;
            let merged: Vec<String> = full_ids
                .into_iter()
                .map(|id| {
                    if lane_set.contains(id.as_str()) {
                        let chosen = delivery_ids[next_idx].clone();
                        next_idx += 1;
                        chosen
                    } else {
                        id
                    }
                })
                .collect();
            self.mailbox.reorder_queue(&agent.id, &merged)?;
        } else {
            self.mailbox.reorder_queue(&agent.id, delivery_ids)?;
        }
        self.queue_list(&agent.id)
    }

    pub fn queue_pause(&self, agent_id: &str) -> RuntimeResult<QueuePauseOutcome> {
        let agent = self.require_agent(agent_id)?;
        self.mailbox.pause_queue(&agent.id)?;
        let active_turn_id = self.active_turn_id_for_agent(&agent.id, None)?;
        Ok(QueuePauseOutcome {
            agent_id: agent.id,
            paused: true,
            active_turn_id,
        })
    }

    /// Resumes `agent_id`'s queue and starts the next runnable FIFO delivery into an active
    /// turn so subsequent `queue_steer` calls can attach to `active_turn_id`.
    pub fn queue_resume(&self, agent_id: &str) -> RuntimeResult<QueueResumeOutcome> {
        let agent = self.require_agent(agent_id)?;
        self.mailbox.resume_queue(&agent.id)?;

        let mut active_turn_id = self.active_turn_id_for_agent(&agent.id, None)?;
        let mut started_delivery_id = None;

        if active_turn_id.is_none()
            && let Some(head) = self.mailbox.next_queued(&agent.id, None)
        {
            let turn_id = format!("turn-{}", Uuid::new_v4());
            let thread_id = self.ensure_thread_id(&agent, None)?;
            self.mailbox.mark_starting(&head.delivery.id)?;
            self.mailbox.mark_running(&head.delivery.id, &turn_id)?;

            let mut msg = ConversationMessage::new_simple(
                head.delivery.id.clone(),
                match &head.delivery.sender {
                    QueueSender::User | QueueSender::Routine { .. } => {
                        ConversationMessageAuthor::User
                    }
                    QueueSender::Agent { .. } => ConversationMessageAuthor::Agent,
                },
                head.delivery.text.clone(),
                now_rfc3339_millis(),
                ConversationMessageStatus::Completed,
            );
            msg.turn_id = Some(turn_id.clone());
            msg.source = Some(match &head.delivery.sender {
                QueueSender::User => ConversationMessageSource::User,
                QueueSender::Agent { .. } => ConversationMessageSource::Agent,
                QueueSender::Routine { .. } => ConversationMessageSource::Routine,
            });
            if let QueueSender::Agent { agent_id: sender_id } = &head.delivery.sender {
                msg.sender_agent_id = Some(sender_id.clone());
            }
            self.thread_store
                .append_conversation_message(&AppendConversationMessageInput {
                    agent_id: agent.id.clone(),
                    thread_id: thread_id.clone(),
                    active_turn_id: Some(turn_id.clone()),
                    message: msg,
                    event_type: "queue.resumed-turn".to_string(),
                    detail: serde_json::json!({
                        "deliveryId": head.delivery.id,
                        "turnId": turn_id,
                    }),
                    command_id: None,
                })?;

            let owner = match &head.delivery.sender {
                QueueSender::User => ActiveTurnOwner::User,
                QueueSender::Agent { agent_id: aid } => ActiveTurnOwner::Agent {
                    agent_id: aid.clone(),
                },
                QueueSender::Routine { .. } => ActiveTurnOwner::Routine,
            };
            self.state.lock().active_lanes.insert(
                SchedulerLane::Main.key(&agent.id),
                ActiveLaneTurn {
                    agent_id: agent.id.clone(),
                    lane: SchedulerLane::Main,
                    thread_id,
                    turn_id: turn_id.clone(),
                    delivery_id: Some(head.delivery.id.clone()),
                    owner,
                    started_at: now_rfc3339_millis(),
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            );
            started_delivery_id = Some(head.delivery.id);
            active_turn_id = Some(turn_id);
        }

        let listed = self.queue_list(&agent.id)?;
        Ok(QueueResumeOutcome {
            agent_id: agent.id,
            paused: false,
            active_turn_id,
            started_delivery_id,
            items: listed.items,
        })
    }

    pub fn queue_cancel(&self, agent_id: &str, delivery_id: &str) -> RuntimeResult<QueueListOutcome> {
        let agent = self.require_agent(agent_id)?;
        if self.channel_store.assignment_for_delivery(delivery_id)?.is_some() {
            return Err(OpenBotRuntimeError::Conflict(
                "Use channel task controls to cancel a channel delivery.".to_string(),
            ));
        }
        self.mailbox.cancel(&agent.id, delivery_id)?;
        self.queue_list(&agent.id)
    }

    /// Executes `QueueEditRequest` (`Begin`, `RetainAttachments`, `Save`, `Cancel`) with the
    /// `10m` (`QUEUE_EDIT_HOLD_MS = 600_000 ms`) hold and idempotent `saveHash` checks.
    pub fn queue_edit(
        &self,
        agent_id: &str,
        request: QueueEditRequest,
        sender: Option<ConversationMessageSender>,
    ) -> RuntimeResult<QueueEditResult> {
        let agent = self.require_agent(agent_id)?;
        request.validate()?;
        let now_ms = current_unix_ms();

        let (delivery_id, edit_id) = match &request {
            QueueEditRequest::Begin { delivery_id, edit_id }
            | QueueEditRequest::Cancel { delivery_id, edit_id }
            | QueueEditRequest::RetainAttachments { delivery_id, edit_id, .. }
            | QueueEditRequest::Save { delivery_id, edit_id, .. } => {
                (delivery_id.as_str(), edit_id.as_str())
            }
        };

        if let Some(finished) = self
            .mailbox
            .finished_queue_edit_action(&agent.id, delivery_id, edit_id)
        {
            match &request {
                QueueEditRequest::Begin { .. } | QueueEditRequest::RetainAttachments { .. } => {
                    return Err(OpenBotRuntimeError::Conflict(
                        "Queue edit session has already finished.".to_string(),
                    ));
                }
                QueueEditRequest::Cancel { .. } => {
                    if finished != QueueEditOutcome::Cancel {
                        return Err(OpenBotRuntimeError::Conflict(
                            "Queue edit was already saved.".to_string(),
                        ));
                    }
                }
                QueueEditRequest::Save {
                    text,
                    keep_attachment_ids,
                    attachment_draft_ids,
                    ..
                } => {
                    for draft_id in attachment_draft_ids {
                        let _ = self.mailbox.discard_draft(draft_id);
                    }
                    if finished != QueueEditOutcome::Save
                        || !self.mailbox.matches_finished_queue_save(
                            &agent.id,
                            delivery_id,
                            edit_id,
                            text,
                            keep_attachment_ids,
                            attachment_draft_ids,
                        )?
                    {
                        return Err(OpenBotRuntimeError::Conflict(
                            "Queue edit finished with a different outcome.".to_string(),
                        ));
                    }
                }
            }
            let snapshot = self.mailbox.list_queue(&agent.id);
            let items = self.queue_list(&agent.id)?.items;
            return Ok(QueueEditResult {
                snapshot,
                hold_ms: QUEUE_EDIT_HOLD_MS,
                hold_expires_at_ms: None,
                items,
            });
        }

        if self.channel_store.assignment_for_delivery(delivery_id)?.is_some() {
            return Err(OpenBotRuntimeError::Conflict(
                "Use channel task controls to edit a channel delivery.".to_string(),
            ));
        }

        let mut hold_expires_at_ms = None;
        match request {
            QueueEditRequest::Begin { delivery_id, edit_id } => {
                self.mailbox
                    .begin_queue_edit_at(&agent.id, &delivery_id, &edit_id, now_ms)?;
                let deadline = now_ms.saturating_add(QUEUE_EDIT_HOLD_MS);
                self.state
                    .lock()
                    .edit_hold_deadlines
                    .insert(delivery_id, deadline);
                hold_expires_at_ms = Some(deadline);
            }
            QueueEditRequest::RetainAttachments {
                delivery_id,
                edit_id,
                attachment_draft_ids,
            } => {
                self.mailbox.retain_queue_edit_attachments(
                    &agent.id,
                    &delivery_id,
                    &edit_id,
                    &attachment_draft_ids,
                )?;
            }
            QueueEditRequest::Save {
                delivery_id,
                edit_id,
                text,
                keep_attachment_ids,
                attachment_draft_ids,
            } => {
                self.mailbox.update_queued_message(
                    &agent.id,
                    &delivery_id,
                    &text,
                    &keep_attachment_ids,
                    &attachment_draft_ids,
                    Some(&edit_id),
                    sender,
                )?;
                self.state.lock().edit_hold_deadlines.remove(&delivery_id);
            }
            QueueEditRequest::Cancel { delivery_id, edit_id } => {
                self.mailbox
                    .finish_queue_edit(&agent.id, &delivery_id, &edit_id)?;
                self.state.lock().edit_hold_deadlines.remove(&delivery_id);
            }
        }

        let snapshot = self.mailbox.list_queue(&agent.id);
        let items = self.queue_list(&agent.id)?.items;
        Ok(QueueEditResult {
            snapshot,
            hold_ms: QUEUE_EDIT_HOLD_MS,
            hold_expires_at_ms,
            items,
        })
    }

    /// Steers a queued delivery into the currently active turn, refusing routine-owned turns
    /// (`steerRefused` / `routine_runs_turn`) and providers without native steer support.
    pub fn queue_steer(&self, input: QueueSteerInput) -> RuntimeResult<QueueSteerOutcome> {
        let agent = self.require_agent(&input.agent_id)?;
        if self
            .channel_store
            .assignment_for_delivery(&input.delivery_id)?
            .is_some()
        {
            return Err(OpenBotRuntimeError::Conflict(
                "Use channel task controls to steer a channel delivery.".to_string(),
            ));
        }
        let ctx = self.mailbox.get_delivery(&input.delivery_id).ok_or_else(|| {
            OpenBotRuntimeError::NotFound(format!("Queued delivery not found: {}", input.delivery_id))
        })?;
        if ctx.delivery.recipient_agent_id != agent.id
            || ctx.delivery.status != QueueDeliveryStatus::Queued
        {
            return Err(OpenBotRuntimeError::Conflict(
                "Only queued deliveries for this agent can be steered.".to_string(),
            ));
        }

        let snapshot = self.mailbox.list_queue(&agent.id);
        let is_routine = input.routine_turn
            || routine_runs_turn(&snapshot.deliveries, Some(&input.expected_turn_id))
            || self.is_active_turn_routine(&agent.id, &input.expected_turn_id);
        if is_routine {
            self.mailbox
                .mark_steer_fallback(&input.delivery_id, QueueSteerFallback::SteerFailed)?;
            return Ok(QueueSteerOutcome {
                accepted: false,
                steered: false,
                steer_refused: true,
                code: Some("steerRefused".to_string()),
                reason: Some(
                    "Cannot steer a routine turn (routine_runs_turn); message remains queued."
                        .to_string(),
                ),
                steer_fallback: Some(QueueSteerFallback::SteerFailed),
                delivery_id: input.delivery_id,
                turn_id: input.expected_turn_id,
            });
        }

        if !provider_steers_turns(&AgentProviderRef::Known(agent.provider)) {
            self.mailbox
                .mark_steer_fallback(&input.delivery_id, QueueSteerFallback::ProviderUnsupported)?;
            return Ok(QueueSteerOutcome {
                accepted: false,
                steered: false,
                steer_refused: true,
                code: Some("providerUnsupported".to_string()),
                reason: Some(format!(
                    "Provider '{}' does not support live turn steering.",
                    agent.provider.as_str()
                )),
                steer_fallback: Some(QueueSteerFallback::ProviderUnsupported),
                delivery_id: input.delivery_id,
                turn_id: input.expected_turn_id,
            });
        }

        let lane_task_id = self.mailbox.lane_id_for_delivery(&input.delivery_id);
        let active_turn = self.active_turn_id_for_agent(&agent.id, lane_task_id.as_deref())?;
        let matches_mailbox_turn = !self
            .mailbox
            .find_deliveries_by_turn(&agent.id, &input.expected_turn_id)
            .is_empty();
        if active_turn.as_deref() != Some(input.expected_turn_id.as_str()) && !matches_mailbox_turn {
            return Err(OpenBotRuntimeError::Conflict(
                "Active turn changed before the queued message could be steered.".to_string(),
            ));
        }

        self.mailbox
            .mark_steering(&input.delivery_id, &input.expected_turn_id)?;
        self.mailbox
            .mark_running(&input.delivery_id, &input.expected_turn_id)?;

        let thread_id = self.ensure_thread_id(&agent, lane_task_id.as_deref())?;
        let mut steered_msg = ConversationMessage::new_simple(
            input.delivery_id.clone(),
            match &ctx.delivery.sender {
                QueueSender::User | QueueSender::Routine { .. } => ConversationMessageAuthor::User,
                QueueSender::Agent { .. } => ConversationMessageAuthor::Agent,
            },
            ctx.delivery.text,
            now_rfc3339_millis(),
            ConversationMessageStatus::Completed,
        );
        steered_msg.turn_id = Some(input.expected_turn_id.clone());
        steered_msg.steered = Some(true);
        steered_msg.source = Some(match &ctx.delivery.sender {
            QueueSender::User => ConversationMessageSource::User,
            QueueSender::Agent { .. } => ConversationMessageSource::Agent,
            QueueSender::Routine { .. } => ConversationMessageSource::Routine,
        });
        self.thread_store
            .append_conversation_message(&AppendConversationMessageInput {
                agent_id: agent.id,
                thread_id,
                active_turn_id: Some(input.expected_turn_id.clone()),
                message: steered_msg,
                event_type: "queue.message-steered".to_string(),
                detail: serde_json::json!({ "deliveryId": input.delivery_id }),
                command_id: None,
            })?;

        Ok(QueueSteerOutcome {
            accepted: true,
            steered: true,
            steer_refused: false,
            code: None,
            reason: None,
            steer_fallback: None,
            delivery_id: input.delivery_id,
            turn_id: input.expected_turn_id,
        })
    }

    /// Creates an idempotent branched side-task thread (`openbot-thread-<uuid>`) cloned up
    /// to `message_id`, enforcing `branchSourceBusy` and the `50` open side-thread limit.
    pub fn branch_thread(&self, input: BranchThreadInput) -> RuntimeResult<BranchThreadOutcome> {
        let agent = self.require_agent(&input.agent_id)?;
        if !is_identifier(&input.request_id) {
            return Err(OpenBotRuntimeError::Validation(
                "A valid requestId is required to branch a thread.".to_string(),
            ));
        }
        if !is_identifier(&input.message_id) {
            return Err(OpenBotRuntimeError::Validation(
                "A valid messageId is required to branch a thread.".to_string(),
            ));
        }

        let agent_threads = self.thread_store.agent_threads();
        if let Some(existing) = agent_threads.branch_for(&agent.id, &input.request_id)? {
            let source_thread_id = existing
                .branch
                .as_ref()
                .map(|b| b.source_thread_id.clone())
                .unwrap_or_else(|| stable_thread_id(&agent.id));
            let cloned_messages = self
                .thread_store
                .read_conversation(&agent.id, Some(&existing.thread_id))?
                .messages
                .len();
            return Ok(BranchThreadOutcome {
                task_id: existing.task_id.clone(),
                thread_id: existing.thread_id.clone(),
                source_thread_id,
                source_message_id: input.message_id,
                cloned_messages,
                record: existing,
            });
        }

        let source_thread_id = self.resolve_thread_id(&agent, input.source_task_id.as_deref())?;
        let source_busy = input.simulate_source_busy
            || self
                .active_turn_id_for_agent(&agent.id, input.source_task_id.as_deref())?
                .is_some()
            || self
                .mailbox
                .lane_active(&agent.id, input.source_task_id.as_deref());
        if source_busy {
            return Err(OpenBotRuntimeError::Conflict(
                "branchSourceBusy: Cannot branch while the source thread has an active running turn."
                    .to_string(),
            ));
        }

        let source_msgs = match agent_threads.branch_source(&source_thread_id, &input.message_id)? {
            Some(msgs) => msgs,
            None => {
                let snap = self
                    .thread_store
                    .read_conversation(&agent.id, Some(&source_thread_id))?;
                let pos = snap
                    .messages
                    .iter()
                    .position(|m| m.id == input.message_id)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(format!(
                            "Source message not found for branch: {}",
                            input.message_id
                        ))
                    })?;
                if snap.messages[pos].status == ConversationMessageStatus::Streaming {
                    return Err(OpenBotRuntimeError::Conflict(
                        "branchSourceBusy: Cannot branch from a streaming message.".to_string(),
                    ));
                }
                snap.messages[..=pos].to_vec()
            }
        };

        let copied = copy_branch_messages(&source_msgs, &HashMap::new());
        let cloned_messages = copied.len();
        let new_thread_id = format!("openbot-thread-{}", Uuid::new_v4());
        let default_title = source_msgs
            .iter()
            .rev()
            .find(|m| !m.text.trim().is_empty())
            .map(|m| clip_utf16(m.text.trim(), INPUT_LIMITS.agent_task_title))
            .unwrap_or_else(|| "Branched thread".to_string());
        let title = input
            .title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| clip_utf16(s, INPUT_LIMITS.agent_task_title))
            .unwrap_or(default_title);

        let record = agent_threads.branch(&AgentThreadBranchInput {
            agent_id: agent.id,
            request_id: input.request_id,
            thread_id: new_thread_id,
            title,
            source_thread_id: source_thread_id.clone(),
            source_message_id: input.message_id.clone(),
            messages: copied,
        })?;

        Ok(BranchThreadOutcome {
            task_id: record.task_id.clone(),
            thread_id: record.thread_id.clone(),
            source_thread_id,
            source_message_id: input.message_id,
            cloned_messages,
            record,
        })
    }

    /// Ephemeral read-only side question (`<= 20` earlier turns, `<= 32,000`-char answer,
    /// zero database writes).
    pub fn ask_side_question(&self, input: SideQuestionInput) -> RuntimeResult<SideQuestionOutcome> {
        let agent = self.require_agent(&input.agent_id)?;
        let question = input.question.trim();
        if question.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Side question cannot be empty or blank.".to_string(),
            ));
        }
        if !is_bounded_string(question, INPUT_LIMITS.message_text) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Side question exceeds {} characters.",
                INPUT_LIMITS.message_text
            )));
        }
        if input.earlier.len() > INPUT_LIMITS.side_question_turns {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Too many earlier side-question turns (maximum {}).",
                INPUT_LIMITS.side_question_turns
            )));
        }

        let thread_id = self.resolve_thread_id(&agent, input.task_id.as_deref())?;
        let snapshot = self
            .thread_store
            .read_conversation(&agent.id, Some(&thread_id))?;
        let prompt = build_side_question_prompt(&agent, &snapshot.messages, question, &input.earlier);
        let raw_answer =
            self.providers
                .generate_ephemeral_text(agent.provider, &agent.model, &prompt, false)?;
        let answer = clip_utf16(raw_answer.trim(), INPUT_LIMITS.side_question_answer);
        if answer.is_empty() {
            return Err(OpenBotRuntimeError::Execution(
                "Provider returned an empty side-question answer.".to_string(),
            ));
        }
        Ok(SideQuestionOutcome {
            agent_id: agent.id,
            answer,
            earlier_count: input.earlier.len(),
        })
    }

    /// Sets or removes a single RGI emoji reaction on a 1:1 thread message, rejecting channel timelines.
    pub fn react_message(&self, input: ReactMessageInput) -> RuntimeResult<ReactMessageOutcome> {
        if input.channel_id.is_some() {
            return Err(OpenBotRuntimeError::Validation(
                "not-supported-in-channel: Reactions are not supported in channel timelines."
                    .to_string(),
            ));
        }
        let agent = self.require_agent(&input.agent_id)?;
        let thread_id = self.resolve_thread_id(&agent, input.task_id.as_deref())?;
        let found = self.thread_store.read_conversation_messages(
            &agent.id,
            Some(&thread_id),
            std::slice::from_ref(&input.message_id),
        )?;
        if found.is_empty() {
            return Err(OpenBotRuntimeError::NotFound(format!(
                "Message not found for reaction: {}",
                input.message_id
            )));
        }
        if !input.remove && !is_message_reaction(&input.emoji) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Invalid RGI emoji reaction: '{}'",
                input.emoji
            )));
        }
        let emoji_opt = (!input.remove).then_some(input.emoji.as_str());
        self.mailbox
            .set_reaction(&agent.id, &input.message_id, input.actor, emoji_opt)?;
        let mut by_msg = self.mailbox.reactions_for(&agent.id);
        let raw_list = by_msg.remove(&input.message_id).unwrap_or_default();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for item in &raw_list {
            *counts.entry(item.emoji.clone()).or_insert(0) += 1;
        }
        let reactions = raw_list
            .into_iter()
            .map(|r| {
                let count = counts.get(&r.emoji).copied().unwrap_or(1);
                ReactionSummaryItem {
                    emoji: r.emoji,
                    actor: r.actor,
                    count,
                }
            })
            .collect();
        Ok(ReactMessageOutcome {
            agent_id: agent.id,
            message_id: input.message_id,
            emoji: emoji_opt.map(str::to_string),
            reactions,
        })
    }

    /// Pins or unpins a thread message (`<= 50` per thread, rejecting system/streaming messages,
    /// rejecting channel timelines, and enforcing `"pinned-by-person"` human-pin ownership).
    pub fn pin_message(&self, input: PinMessageInput) -> RuntimeResult<PinMessageOutcome> {
        if input.channel_id.is_some() {
            return Err(OpenBotRuntimeError::Validation(
                "not-pinnable-chat: Channel timelines do not support 1:1 message pins.".to_string(),
            ));
        }
        let agent = self.require_agent(&input.agent_id)?;
        let pinned = !input.unpin;
        let pins = self.thread_store.pins();
        let (result, pinned_by_kind) = match input.actor {
            PinActor::Person { sender } => (
                pins.set_by_person(
                    &agent.id,
                    input.task_id.as_deref(),
                    &input.message_id,
                    pinned,
                    sender,
                )?,
                "user",
            ),
            PinActor::Agent => {
                let thread_id = self.resolve_thread_id(&agent, input.task_id.as_deref())?;
                (
                    pins.set_by_agent(&agent.id, &thread_id, &input.message_id, pinned)?,
                    "agent",
                )
            }
        };
        let snapshot = match result {
            Ok(snap) => snap,
            Err(PinRefusalReason::PinnedByPerson) => {
                return Err(OpenBotRuntimeError::Forbidden(
                    "pinned-by-person: An agent cannot unpin a message pinned by a person."
                        .to_string(),
                ));
            }
            Err(PinRefusalReason::Limit) => {
                return Err(OpenBotRuntimeError::LimitExceeded(format!(
                    "limit: Maximum of {MESSAGE_PINS_LIMIT} pinned messages reached for this thread."
                )));
            }
            Err(PinRefusalReason::NotFound) => {
                return Err(OpenBotRuntimeError::NotFound(
                    "not-found: Message was not found or cannot be pinned (system and streaming messages are not pinnable)."
                        .to_string(),
                ));
            }
            Err(PinRefusalReason::NotPinnableChat) => {
                return Err(OpenBotRuntimeError::Validation(
                    "not-pinnable-chat: Messages in this chat cannot be pinned.".to_string(),
                ));
            }
        };
        let actual_by_kind = snapshot
            .pins
            .iter()
            .find(|p| p.message_id == input.message_id)
            .map(|p| match p.pinned_by {
                MessagePinner::Person { .. } => "user",
                MessagePinner::Agent { .. } => "agent",
            })
            .unwrap_or(pinned_by_kind)
            .to_string();
        Ok(PinMessageOutcome {
            agent_id: agent.id,
            message_id: input.message_id,
            pinned,
            pinned_by_kind: actual_by_kind,
            snapshot,
        })
    }

    pub fn list_pins(&self, agent_id: &str) -> RuntimeResult<PinnedMessagesSnapshot> {
        let agent = self.require_agent(agent_id)?;
        Ok(self.thread_store.pins().list(&agent.id)?)
    }

    pub fn history(
        &self,
        agent_id: &str,
        task_id: Option<&str>,
        limit: Option<usize>,
    ) -> RuntimeResult<ConversationSnapshot> {
        let agent = self.require_agent(agent_id)?;
        let thread_id = self.resolve_thread_id(&agent, task_id)?;
        let mut snapshot = self
            .thread_store
            .read_conversation(&agent.id, Some(&thread_id))?;
        let reactions_map = self.mailbox.reactions_for(&agent.id);
        for msg in &mut snapshot.messages {
            if let Some(rxns) = reactions_map.get(&msg.id)
                && !rxns.is_empty()
            {
                msg.reactions = Some(rxns.clone());
            }
        }
        if let Some(lim) = limit
            && snapshot.messages.len() > lim
        {
            let keep_from = snapshot.messages.len() - lim;
            snapshot.messages.drain(..keep_from);
        }
        Ok(snapshot)
    }

    pub fn compact_context(
        &self,
        agent_id: &str,
        task_id: Option<&str>,
        simulated_usage: Option<&str>,
        reason: CompactionReason,
    ) -> RuntimeResult<CompactionOutcome> {
        let agent = self.require_agent(agent_id)?;
        let thread_id = self.ensure_thread_id(&agent, task_id)?;
        self.compactor
            .compact_thread_with_simulated_usage(&agent.id, &thread_id, simulated_usage, reason)
    }

    /// Promotes a queued user delivery onto a dedicated parallel `AgentTask` side thread.
    pub fn run_queued_as_task(
        &self,
        agent_id: &str,
        delivery_id: &str,
        title: Option<&str>,
    ) -> RuntimeResult<AgentTaskRunResult> {
        let agent = self.require_agent(agent_id)?;
        let ctx = self.mailbox.get_delivery(delivery_id).ok_or_else(|| {
            OpenBotRuntimeError::NotFound(format!("Queued delivery not found: {delivery_id}"))
        })?;
        let resolved_title = title
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| clip_utf16(s, INPUT_LIMITS.agent_task_title))
            .unwrap_or_else(|| {
                let fallback = ctx.delivery.text.trim();
                if fallback.is_empty() {
                    "Side task".to_string()
                } else {
                    clip_utf16(fallback, INPUT_LIMITS.agent_task_title)
                }
            });
        let record = self.thread_store.agent_threads().create(AgentThreadInput {
            agent_id: agent.id.clone(),
            origin: AgentTaskOrigin::User,
            title: resolved_title,
            title_source: AgentThreadTitleSource::Auto,
            routine_id: None,
        })?;
        self.mailbox
            .move_to_task(&agent.id, delivery_id, &record.task_id)?;
        let lane = SchedulerLane::Task {
            task_id: record.task_id.clone(),
        };
        let dispatch = if !self.mailbox.is_queue_paused(&agent.id) && self.can_start_lane(&agent, &lane) {
            self.drain_lane_once(&agent.id, &lane)?
        } else {
            None
        };
        Ok(AgentTaskRunResult {
            task: record,
            delivery_id: delivery_id.to_string(),
            dispatch,
        })
    }

    pub fn start_side_task(
        &self,
        agent_id: &str,
        title: &str,
        origin: AgentTaskOrigin,
        initial_prompt: Option<&str>,
    ) -> RuntimeResult<AgentThreadRecord> {
        let agent = self.require_agent(agent_id)?;
        let clean_title = clip_utf16(title.trim(), INPUT_LIMITS.agent_task_title);
        if clean_title.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Side-task title is required.".to_string(),
            ));
        }
        let record = match origin {
            AgentTaskOrigin::User => self.thread_store.agent_threads().create(AgentThreadInput {
                agent_id: agent.id.clone(),
                origin: AgentTaskOrigin::User,
                title: clean_title,
                title_source: AgentThreadTitleSource::User,
                routine_id: None,
            })?,
            AgentTaskOrigin::Agent => {
                let work = self.mailbox.task_work(&agent.id);
                let (created, _) = self.thread_store.agent_threads().start_agent_task_thread(
                    &agent.id,
                    &clean_title,
                    |rec| {
                        let tw = work.get(&rec.task_id);
                        let busy = tw.is_some_and(|w| w.active || !w.queued_messages.is_empty());
                        !busy && rec.outcome.is_some()
                    },
                )?;
                created
            }
        };
        if let Some(prompt) = initial_prompt.map(str::trim).filter(|s| !s.is_empty()) {
            self.mailbox.enqueue(EnqueueInput {
                task_id: Some(record.task_id.clone()),
                sender: Some(QueueSender::User),
                recipient_agent_ids: vec![agent.id],
                text: prompt.to_string(),
                ..Default::default()
            })?;
        }
        Ok(record)
    }

    pub fn list_side_tasks(&self, agent_id: &str) -> RuntimeResult<AgentTasksSnapshot> {
        let agent = self.require_agent(agent_id)?;
        let records = self.thread_store.agent_threads().list(&agent.id)?;
        let work_map = self.mailbox.task_work(&agent.id);
        let awaited_all = self.mailbox.awaited_thread_requests();
        let active_lanes = self.state.lock().active_lanes.clone();

        let tasks = records
            .into_iter()
            .map(|rec| {
                let tw = work_map.get(&rec.task_id);
                let lane_key = SchedulerLane::Task {
                    task_id: rec.task_id.clone(),
                }
                .key(&agent.id);
                let active_turn_id = active_lanes
                    .get(&lane_key)
                    .map(|l| l.turn_id.clone())
                    .or_else(|| {
                        self.thread_store
                            .read_active_turn_id(&agent.id, Some(&rec.thread_id))
                            .ok()
                            .flatten()
                    });
                let state = if rec.closed_at.is_some() {
                    AgentTaskState::Closed
                } else if active_turn_id.is_some() || tw.is_some_and(|w| w.active) {
                    AgentTaskState::Running
                } else if tw.is_some_and(|w| !w.queued_messages.is_empty()) {
                    AgentTaskState::Queued
                } else {
                    AgentTaskState::Idle
                };
                let queued_messages = tw
                    .map(|w| {
                        w.queued_messages
                            .iter()
                            .map(|(did, text, created_at)| AgentTaskQueuedMessage {
                                delivery_id: did.clone(),
                                text: clip_utf16(text, 500),
                                created_at: created_at.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let queue = tw.map(|w| w.queue.clone());
                let awaited_replies: Vec<_> = awaited_all
                    .iter()
                    .filter(|a| a.sender_agent_id == agent.id && a.task_id == rec.task_id)
                    .map(|a| a.reply.clone())
                    .collect();
                let branch = rec.branch.map(|b| {
                    Some(AgentTaskBranch {
                        source_task_id: None,
                        source_title: None,
                        source_message_id: b.source_message_id,
                        through_message_id: b.through_message_id,
                    })
                });
                AgentTask {
                    id: rec.task_id,
                    agent_id: rec.agent_id,
                    thread_id: rec.thread_id,
                    origin: rec.origin,
                    title: rec.title,
                    state,
                    outcome: rec.outcome,
                    hold: None,
                    active_turn_id,
                    progress: None,
                    error: rec.error,
                    queued_messages,
                    queue,
                    awaited_replies: (!awaited_replies.is_empty()).then_some(awaited_replies),
                    created_at: rec.created_at,
                    updated_at: rec.updated_at,
                    closed_at: rec.closed_at,
                    branch,
                    routine_id: rec.routine_id.map(Some),
                }
            })
            .collect();

        Ok(AgentTasksSnapshot {
            agent_id: agent.id.clone(),
            tasks,
            limit: self.effective_task_limit(&agent),
        })
    }

    pub fn close_side_task(&self, agent_id: &str, task_id: &str) -> RuntimeResult<()> {
        let agent = self.require_agent(agent_id)?;
        let Some(rec) = self.thread_store.agent_threads().task(task_id)? else {
            return Err(OpenBotRuntimeError::NotFound(format!(
                "Side task not found: {task_id}"
            )));
        };
        if rec.agent_id != agent.id {
            return Err(OpenBotRuntimeError::Forbidden(
                "Side task belongs to a different agent.".to_string(),
            ));
        }
        self.thread_store.agent_threads().close(task_id)?;
        Ok(())
    }

    /// Bot-to-bot mailbox fan-out (`1..=32` unique non-self recipients), `Status/Result/Evidence`
    /// reply parsing, `expectsReply=false` anti-ping-pong flag, and barrier status tracking.
    pub fn mailbox_send(&self, input: MailboxSendInput) -> RuntimeResult<MailboxSendOutcome> {
        let sender = self.require_agent(&input.sender_agent_id)?;
        if input.recipient_agent_ids.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "At least one recipient is required.".to_string(),
            ));
        }
        if input.recipient_agent_ids.len() > INPUT_LIMITS.message_recipients {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "At most {} recipients are allowed per mailbox message.",
                INPUT_LIMITS.message_recipients
            )));
        }
        let mut seen = HashSet::with_capacity(input.recipient_agent_ids.len());
        let roster = AgentRoster::new(self.database.clone());
        for rid in &input.recipient_agent_ids {
            if !seen.insert(rid.as_str()) {
                return Err(OpenBotRuntimeError::Validation(
                    "Duplicate recipients are not allowed.".to_string(),
                ));
            }
            if rid == &sender.id {
                return Err(OpenBotRuntimeError::Validation(
                    "An agent cannot message itself.".to_string(),
                ));
            }
            if roster.get_agent(rid)?.is_none() {
                return Err(OpenBotRuntimeError::NotFound(format!(
                    "Unknown recipient agent: {rid}"
                )));
            }
        }

        let trimmed = input.text.trim();
        if trimmed.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Mailbox message text is required.".to_string(),
            ));
        }
        let parsed_reply = parse_teammate_reply(trimmed).map(MailboxParsedReply::from);
        let expects_reply = input.expects_reply.unwrap_or(true);
        let answer_optional = !expects_reply;

        let receipt = self.mailbox.send_message(SendAgentMessageInput {
            sender_agent_id: sender.id.clone(),
            recipient_agent_ids: input.recipient_agent_ids,
            text: trimmed.to_string(),
            reply_to_message_id: input.reply_to_message_id.clone(),
            expects_reply: Some(expects_reply),
            sender_task_id: input.sender_task_id,
            requester_ids: input.requester_ids,
            source_paths: Vec::new(),
            idempotency_key: input.idempotency_key,
        })?;

        let pending_recipients = match &input.reply_to_message_id {
            Some(req_id) => self.mailbox.still_working_recipients(&sender.id, req_id),
            None => Vec::new(),
        };
        let barrier_complete = pending_recipients.is_empty();

        Ok(MailboxSendOutcome {
            message_id: receipt.message_id,
            deliveries: receipt.deliveries,
            expects_reply,
            answer_optional,
            reply_to_message_id: input.reply_to_message_id,
            parsed_reply,
            barrier_complete,
            pending_recipients,
        })
    }

    /// Posts an agent-authored handoff message into a shared channel and registers a root
    /// `ChannelTask` whose completion/pause/failure/cancellation routes back via
    /// `ChannelRequestReturnsTracker`.
    pub fn send_to_channel(&self, input: SendToChannelInput) -> RuntimeResult<SendToChannelOutcome> {
        let sender = self.require_agent(&input.sender_agent_id)?;
        let trimmed = input.text.trim();
        if trimmed.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Channel handoff text is required.".to_string(),
            ));
        }
        let channel = self.channel_store.get(&input.channel_id)?;
        if channel.archived {
            return Err(OpenBotRuntimeError::Forbidden(
                "Channel is archived (read_only).".to_string(),
            ));
        }
        let existing_msgs = self.channel_store.messages(&channel.id, None, None)?;
        let next_seq = existing_msgs
            .iter()
            .map(|m| m.sequence)
            .max()
            .unwrap_or(0)
            + 1;
        let msg_id = format!("ch-msg-{}", Uuid::new_v4());
        let task_id = format!("ch-task-{}", Uuid::new_v4());
        let created_at = now_rfc3339_millis();

        let mut inner_msg = ConversationMessage::new_simple(
            msg_id.clone(),
            ConversationMessageAuthor::Agent,
            trimmed.to_string(),
            created_at,
            ConversationMessageStatus::Completed,
        );
        inner_msg.sender_agent_id = Some(sender.id.clone());
        inner_msg.source = Some(ConversationMessageSource::Agent);

        let channel_msg = ChannelMessage {
            id: msg_id.clone(),
            channel_id: channel.id.clone(),
            sequence: next_seq,
            author: ChannelMessageAuthor {
                kind: ChannelMessageAuthorKind::Agent,
                id: format!("{CHANNEL_HANDOFF_AUTHOR_PREFIX}{}", sender.id),
                name: sender.name.clone(),
            },
            task_id: Some(task_id.clone()),
            superseded: false,
            message: inner_msg,
            deleted: None,
            sender_task_id: input.sender_task_id,
        };
        let owner_agent_id = channel
            .draft
            .lead_agent_id
            .clone()
            .or_else(|| channel.draft.members.first().map(|m| m.agent_id.clone()));
        let task = ChannelTask {
            id: task_id.clone(),
            channel_id: channel.id.clone(),
            parent_task_id: None,
            root_task_id: task_id.clone(),
            owner_agent_id,
            request_message_id: msg_id.clone(),
            instruction: trimmed.to_string(),
            attachment_draft_ids: Vec::new(),
            expected_result: TASK_EXPECTED_RESULT.to_string(),
            source_message_ids: vec![msg_id.clone()],
            dependencies: Vec::new(),
            resources: Vec::new(),
            state: ChannelTaskState::Queued,
            revision: 1,
            assignment_count: 0,
            error: None,
            seen_dependencies: None,
        };
        let op_id = input
            .idempotency_key
            .unwrap_or_else(|| format!("send-to-channel:{}:{}", channel.id, msg_id));
        self.channel_store.commit(
            &op_id,
            ChannelChange {
                channel: channel.clone(),
                messages: vec![channel_msg],
                tasks: vec![task],
                assignments: Vec::new(),
            },
        )?;
        Ok(SendToChannelOutcome {
            channel_id: channel.id,
            message_id: msg_id,
            task_id,
            sequence: next_seq,
        })
    }

    /// Reconciles completed, paused, failed, or cancelled `send_to_channel` root tasks back
    /// to their requesting agent's mailbox using idempotent `"channel-request-result:{taskId}:{revision}:{state}"` keys.
    pub fn reconcile_channel_returns(
        &self,
        channel_id: &str,
    ) -> RuntimeResult<Vec<QueuedMessageReceipt>> {
        let channel = self.channel_store.get(channel_id)?;
        let tasks = self.channel_store.tasks(&channel.id)?;
        let all_messages = self.channel_store.messages(&channel.id, None, None)?;
        let messages_by_id: HashMap<&str, ChannelMessage> = all_messages
            .iter()
            .map(|m| (m.id.as_str(), m.clone()))
            .collect();
        let roster = AgentRoster::new(self.database.clone());
        let existing_agents: HashSet<String> = roster
            .list_agents()?
            .into_iter()
            .map(|a| a.id)
            .collect();
        let mut open_tasks = HashSet::new();
        for aid in &existing_agents {
            for tid in self.thread_store.agent_threads().open_task_ids(aid)? {
                open_tasks.insert(tid);
            }
        }
        let mut tracker = self.channel_returns.lock();
        let receipts = tracker.reconcile_channel(
            &self.mailbox,
            &channel.id,
            &channel.draft.name,
            channel.draft.lead_agent_id.as_deref(),
            &tasks,
            |mid| messages_by_id.get(mid).cloned(),
            |task, _req| {
                all_messages
                    .iter()
                    .rev()
                    .find(|m| {
                        m.task_id.as_deref() == Some(task.id.as_str())
                            && m.id != task.request_message_id
                            && !m.superseded
                    })
                    .cloned()
            },
            |aid| existing_agents.contains(aid),
            |tid| open_tasks.contains(tid),
        )?;
        Ok(receipts)
    }

    /// Interrupts work owned solely by `caller_agent_id` on `target_agent_id`, cancelling
    /// queued deliveries from `caller_agent_id` and strictly refusing self-interrupts,
    /// user-owned turns (`interruptOtherWork`), shared turns (`interruptSharedWork`), and
    /// channel tasks (`useChannelTaskControlsWork`).
    pub fn interrupt_agent(
        &self,
        input: InterruptAgentInput,
    ) -> RuntimeResult<InterruptAgentOutcome> {
        if input.caller_agent_id == input.target_agent_id {
            return Err(OpenBotRuntimeError::Forbidden(
                "An agent cannot interrupt itself.".to_string(),
            ));
        }
        let caller = self.require_agent(&input.caller_agent_id)?;
        let target = self.require_agent(&input.target_agent_id)?;

        match input.active_turn_owner {
            Some(ActiveTurnOwnerHint::Channel) => {
                return Err(OpenBotRuntimeError::Forbidden(
                    "useChannelTaskControlsWork: Cannot interrupt channel task work; use channel task controls."
                        .to_string(),
                ));
            }
            Some(ActiveTurnOwnerHint::User) => {
                return Err(OpenBotRuntimeError::Forbidden(
                    "interruptOtherWork: Cannot interrupt a turn started by the user or another owner."
                        .to_string(),
                ));
            }
            Some(ActiveTurnOwnerHint::Shared) => {
                return Err(OpenBotRuntimeError::Forbidden(
                    "interruptSharedWork: Cannot interrupt shared turn work involving another sender."
                        .to_string(),
                ));
            }
            Some(ActiveTurnOwnerHint::Caller) | None => {}
        }

        let cancelled_queued_count = self
            .mailbox
            .cancel_queued_from_agent(&target.id, &caller.id)?;

        let active_turn = self.active_turn_id_for_agent(&target.id, None)?;
        let mut interrupted_turn_id = None;
        let mut notice_message_id = None;

        if let Some(turn_id) = active_turn {
            let mut is_channel = false;
            if let Some(lane_turn) = self
                .state
                .lock()
                .active_lanes
                .get(&SchedulerLane::Main.key(&target.id))
                && lane_turn.owner == ActiveTurnOwner::Channel
            {
                is_channel = true;
            }
            let interrupted_msg_id = if input.active_turn_owner == Some(ActiveTurnOwnerHint::Caller)
            {
                self.mailbox
                    .find_deliveries_by_turn(&target.id, &turn_id)
                    .first()
                    .map(|c| c.delivery.message_id.clone())
            } else {
                match self.mailbox.check_interrupt_ownership(
                    &target.id,
                    &caller.id,
                    &turn_id,
                    is_channel,
                ) {
                    Ok(mid) => mid,
                    Err(InterruptRefusal::SelfTarget) => {
                        return Err(OpenBotRuntimeError::Forbidden(
                            "An agent cannot interrupt itself.".to_string(),
                        ));
                    }
                    Err(InterruptRefusal::UseChannelTaskControlsWork) => {
                        return Err(OpenBotRuntimeError::Forbidden(
                            "useChannelTaskControlsWork: Cannot interrupt channel task work; use channel task controls."
                                .to_string(),
                        ));
                    }
                    Err(InterruptRefusal::InterruptSharedWork) => {
                        return Err(OpenBotRuntimeError::Forbidden(
                            "interruptSharedWork: Cannot interrupt shared turn work involving another sender."
                                .to_string(),
                        ));
                    }
                    Err(InterruptRefusal::InterruptOtherWork) => {
                        return Err(OpenBotRuntimeError::Forbidden(
                            "interruptOtherWork: Cannot interrupt a turn started by another owner."
                                .to_string(),
                        ));
                    }
                }
            };

            if let Some(active) = self
                .state
                .lock()
                .active_lanes
                .remove(&SchedulerLane::Main.key(&target.id))
            {
                active.cancelled.store(true, Ordering::SeqCst);
            }
            let reason_text = input
                .reason
                .as_deref()
                .unwrap_or("Interrupted by requesting agent.");
            for ctx in self.mailbox.find_deliveries_by_turn(&target.id, &turn_id) {
                self.mailbox.mark_terminal(
                    &ctx.delivery.id,
                    QueueDeliveryStatus::Interrupted,
                    Some(reason_text),
                )?;
            }
            let thread_id = self.resolve_thread_id(&target, None)?;
            self.clear_thread_active_turn(&target.id, &thread_id, &turn_id)?;

            let idem_key = input
                .idempotency_key
                .clone()
                .unwrap_or_else(|| format!("interrupt:{}:{}:{}", caller.id, target.id, turn_id));
            let receipt = self.mailbox.enqueue_interrupt_notice(
                &caller.id,
                input.requester_ids.clone(),
                &target.id,
                interrupted_msg_id.as_deref(),
                input.reason.as_deref(),
                idem_key,
            )?;
            interrupted_turn_id = Some(turn_id);
            notice_message_id = Some(receipt.message_id);
        } else if input.active_turn_owner == Some(ActiveTurnOwnerHint::Caller) {
            let idem_key = input
                .idempotency_key
                .unwrap_or_else(|| format!("interrupt:{}:{}:{}", caller.id, target.id, Uuid::new_v4()));
            let receipt = self.mailbox.enqueue_interrupt_notice(
                &caller.id,
                input.requester_ids,
                &target.id,
                None,
                input.reason.as_deref(),
                idem_key,
            )?;
            notice_message_id = Some(receipt.message_id);
        }

        let interrupted = interrupted_turn_id.is_some()
            || cancelled_queued_count > 0
            || input.active_turn_owner == Some(ActiveTurnOwnerHint::Caller);

        Ok(InterruptAgentOutcome {
            interrupted,
            interrupted_turn_id,
            cancelled_queued_count,
            notice_message_id,
        })
    }

    /// Sends a 1:1 member-to-member direct message with order-invariant `dm-<32hex>` SHA-256
    /// thread ID, rejecting self-DMs, blank payloads, and `> 20,000`-char payloads.
    pub fn dm_send(&self, mut input: SendDirectMessageInput) -> RuntimeResult<DirectMessage> {
        if input.sender_member_id.trim().is_empty() || input.recipient_member_id.trim().is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Direct message sender and recipient member IDs are required.".to_string(),
            ));
        }
        if input.client_message_id.trim().is_empty() {
            input.client_message_id = format!("dm-msg-{}", Uuid::new_v4());
        }
        Ok(self.team_chat_store.send_message(&input)?)
    }

    pub fn dm_list(&self, member_id: &str) -> RuntimeResult<Vec<DirectThreadSummary>> {
        if member_id.trim().is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Member ID is required to list direct messages.".to_string(),
            ));
        }
        Ok(self.team_chat_store.list_threads(member_id)?)
    }

    pub fn dm_read(
        &self,
        member_id: &str,
        other_member_id: &str,
    ) -> RuntimeResult<DirectConversationSnapshot> {
        Ok(self
            .team_chat_store
            .read_conversation(member_id, other_member_id)?)
    }

    pub fn dm_mark_read(
        &self,
        member_id: &str,
        other_member_id: &str,
        through_sequence: u64,
    ) -> RuntimeResult<DirectConversationReadState> {
        Ok(self
            .team_chat_store
            .mark_read(member_id, other_member_id, through_sequence)?)
    }

    fn require_agent(&self, agent_id: &str) -> RuntimeResult<AgentSummary> {
        let roster = AgentRoster::new(self.database.clone());
        roster.get_agent(agent_id)?.ok_or_else(|| {
            OpenBotRuntimeError::NotFound(format!("Unknown agent: {agent_id}"))
        })
    }

    fn ensure_thread_id(&self, agent: &AgentSummary, task_id: Option<&str>) -> RuntimeResult<String> {
        if task_id.is_some() || agent.thread_id.is_some() {
            return self.resolve_thread_id(agent, task_id);
        }
        let thread_id = stable_thread_id(&agent.id);
        let roster = AgentRoster::new(self.database.clone());
        let mut agents = roster.list_agents()?;
        let mut updated = false;
        for candidate in &mut agents {
            if candidate.id == agent.id && candidate.thread_id.is_none() {
                candidate.thread_id = Some(thread_id.clone());
                candidate.updated_at = Some(now_rfc3339_millis());
                updated = true;
                break;
            }
        }
        if updated {
            let cmd = format!("agent-thread-bind:{}:{thread_id}", agent.id);
            roster.replace_agents(&cmd, &agents, "agent.thread-bound", None)?;
        }
        Ok(thread_id)
    }

    fn resolve_thread_id(&self, agent: &AgentSummary, task_id: Option<&str>) -> RuntimeResult<String> {
        if let Some(tid) = task_id {
            let rec = self
                .thread_store
                .agent_threads()
                .task(tid)?
                .ok_or_else(|| {
                    OpenBotRuntimeError::NotFound(format!("Side-task thread not found: {tid}"))
                })?;
            if rec.agent_id != agent.id {
                return Err(OpenBotRuntimeError::Forbidden(format!(
                    "Side-task thread {tid} does not belong to agent {}.",
                    agent.id
                )));
            }
            return Ok(rec.thread_id);
        }
        Ok(agent
            .thread_id
            .clone()
            .unwrap_or_else(|| stable_thread_id(&agent.id)))
    }

    fn active_turn_id_for_agent(
        &self,
        agent_id: &str,
        task_id: Option<&str>,
    ) -> RuntimeResult<Option<String>> {
        let lane = match task_id {
            Some(tid) => SchedulerLane::Task {
                task_id: tid.to_string(),
            },
            None => SchedulerLane::Main,
        };
        if let Some(active) = self.state.lock().active_lanes.get(&lane.key(agent_id)) {
            return Ok(Some(active.turn_id.clone()));
        }
        if let Ok(agent) = self.require_agent(agent_id)
            && let Ok(thread_id) = self.resolve_thread_id(&agent, task_id)
            && let Some(tid) = self
                .thread_store
                .read_active_turn_id(agent_id, Some(&thread_id))?
        {
            return Ok(Some(tid));
        }
        let mailbox_lane = lane.mailbox_lane();
        for ctx in self.mailbox.unresolved_deliveries() {
            if ctx.delivery.recipient_agent_id == agent_id
                && self.mailbox.lane_id_for_delivery(&ctx.delivery.id) == mailbox_lane
                && let Some(tid) = ctx.delivery.turn_id
            {
                return Ok(Some(tid));
            }
        }
        Ok(None)
    }

    fn is_active_turn_routine(&self, agent_id: &str, turn_id: &str) -> bool {
        if self.state.lock().active_lanes.values().any(|l| {
            l.agent_id == agent_id && l.turn_id == turn_id && l.owner == ActiveTurnOwner::Routine
        }) {
            return true;
        }
        self.mailbox
            .find_deliveries_by_turn(agent_id, turn_id)
            .iter()
            .any(|ctx| matches!(ctx.delivery.sender, QueueSender::Routine { .. }))
    }

    fn enrich_queue_metadata(
        &self,
        agent_id: &str,
    ) -> RuntimeResult<(HashSet<String>, HashSet<String>)> {
        let mut welcome_msgs = HashSet::new();
        let mut steered_deliveries = HashSet::new();
        if let Some(state) = self.mailbox.projection().read_mailbox_state()? {
            for msg in &state.messages {
                if msg.welcome == Some(true) {
                    welcome_msgs.insert(msg.id.clone());
                }
            }
            for del in &state.deliveries {
                if del.recipient_agent_id == agent_id && del.steered_at.is_some() {
                    steered_deliveries.insert(del.id.clone());
                }
            }
        }
        Ok((welcome_msgs, steered_deliveries))
    }

    fn persist_agent_failover_state(
        &self,
        agent_id: &str,
        provider: AgentProviderId,
        model: &str,
        usage_limit_return: Option<AgentUsageLimitReturn>,
    ) -> RuntimeResult<()> {
        let roster = AgentRoster::new(self.database.clone());
        let mut agents = roster.list_agents()?;
        let mut updated = false;
        for ag in &mut agents {
            if ag.id == agent_id {
                ag.provider = provider;
                ag.model = model.to_string();
                ag.usage_limit_return = usage_limit_return.clone();
                ag.updated_at = Some(now_rfc3339_millis());
                updated = true;
                break;
            }
        }
        if updated {
            let cmd = format!("agent-failover:{agent_id}:{}", Uuid::new_v4());
            roster.replace_agents(&cmd, &agents, "agent.model-changed", None)?;
        }
        Ok(())
    }

    fn clear_thread_active_turn(
        &self,
        agent_id: &str,
        thread_id: &str,
        turn_id: &str,
    ) -> RuntimeResult<()> {
        let recovery_msgs = self.thread_store.read_conversation_recovery_messages(
            agent_id,
            Some(thread_id),
            Some(turn_id),
        )?;
        let mut changed = Vec::new();
        for mut msg in recovery_msgs {
            if msg.turn_id.as_deref() == Some(turn_id)
                && msg.status == ConversationMessageStatus::Streaming
            {
                msg.status = ConversationMessageStatus::Interrupted;
                changed.push(msg);
            }
        }
        self.thread_store
            .persist_conversation_changes(&PersistConversationChangesInput {
                agent_id: agent_id.to_string(),
                thread_id: thread_id.to_string(),
                active_turn_id: None,
                changed_messages: changed,
                removed_message_ids: Vec::new(),
                deleted_message_ids: Vec::new(),
                event_type: "turn.interrupted".to_string(),
                detail: serde_json::json!({ "turnId": turn_id }),
                command_id: None,
            })?;
        Ok(())
    }
}

fn validate_attachments(attachments: &[AttachmentSummary]) -> RuntimeResult<()> {
    if attachments.len() > INPUT_LIMITS.attachments {
        return Err(OpenBotRuntimeError::Validation(format!(
            "Too many attachments (maximum {} per message).",
            INPUT_LIMITS.attachments
        )));
    }
    let mut total_bytes = 0u64;
    for att in attachments {
        if !att.is_valid() {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Invalid attachment metadata for '{}'.",
                att.name
            )));
        }
        if att.size > ATTACHMENT_LIMITS.file_bytes as u64 {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Attachment '{}' exceeds the {} MiB per-file limit.",
                att.name,
                ATTACHMENT_LIMITS.file_bytes / (1024 * 1024)
            )));
        }
        total_bytes = total_bytes.saturating_add(att.size);
        if total_bytes > ATTACHMENT_LIMITS.total_bytes as u64 {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Total attachment size exceeds the {} MiB per-message limit.",
                ATTACHMENT_LIMITS.total_bytes / (1024 * 1024)
            )));
        }
    }
    Ok(())
}

fn is_side_question_chat_message(msg: &ConversationMessage) -> bool {
    matches!(
        msg.author,
        ConversationMessageAuthor::User
            | ConversationMessageAuthor::Assistant
            | ConversationMessageAuthor::Agent
    ) && !msg.text.trim().is_empty()
        && !matches!(
            msg.item_type.as_deref(),
            Some(
                "commentary"
                    | "question_prompt"
                    | "agent_attachment"
                    | AGENT_WELCOME_ITEM_TYPE
                    | CHANNEL_RESUME_ITEM_TYPE
                    | CONVERSATION_PLAN_ITEM_TYPE
                    | CONVERSATION_SUBAGENTS_ITEM_TYPE
                    | ROUTINE_QUIET_ANSWER_ITEM_TYPE
            )
        )
        && !msg
            .item_type
            .as_deref()
            .is_some_and(|t| t.starts_with(CHAT_VISUAL_ITEM_TYPE_PREFIX))
}

fn build_side_question_prompt(
    agent: &AgentSummary,
    messages: &[ConversationMessage],
    question: &str,
    earlier: &[SideQuestionTurn],
) -> String {
    let mut chat_entries = Vec::new();
    let mut chat_chars = 0usize;
    for msg in messages
        .iter()
        .filter(|m| is_side_question_chat_message(m))
        .rev()
        .take(SIDE_QUESTION_CHAT_MESSAGES)
    {
        let text = clip_utf16(&msg.text, INPUT_LIMITS.side_question_earlier_text);
        let len = utf16_len(&text);
        if chat_chars + len > SIDE_QUESTION_CHAT_CHARACTERS {
            break;
        }
        chat_chars += len;
        let from = match msg.author {
            ConversationMessageAuthor::User => msg
                .sender_member
                .as_ref()
                .map(|s| s.name.as_str())
                .unwrap_or("User"),
            _ if msg.sender_agent_id.is_some() => "Another agent",
            _ => agent.name.as_str(),
        };
        chat_entries.push(serde_json::json!({ "from": from, "text": text }));
    }
    chat_entries.reverse();

    let mut earlier_entries = Vec::new();
    let mut earlier_chars = 0usize;
    for turn in earlier.iter().rev() {
        let q = clip_utf16(&turn.question, INPUT_LIMITS.side_question_earlier_text);
        let a = clip_utf16(&turn.answer, INPUT_LIMITS.side_question_earlier_text);
        let len = utf16_len(&q) + utf16_len(&a);
        if earlier_chars + len > SIDE_QUESTION_EARLIER_CHARACTERS {
            break;
        }
        earlier_chars += len;
        earlier_entries.push(serde_json::json!({ "question": q, "answer": a }));
    }
    earlier_entries.reverse();

    let profile = serde_json::json!({
        "name": agent.name,
        "title": if agent.title.trim().is_empty() { "General assistant" } else { agent.title.trim() },
        "description": if agent.description.trim().is_empty() {
            "No additional description configured."
        } else {
            agent.description.trim()
        },
    });
    let mut sections = vec![
        "You are the agent in this profile. The user asks a side question about your chat with them. The question and your answer stay out of the chat, and you cannot use tools here. Answer from the chat and from what you know. Give the shortest complete answer in plain text or Markdown.".to_string(),
        "<agent_profile>".to_string(),
        serde_json::to_string(&profile).unwrap_or_default(),
        "</agent_profile>".to_string(),
        "<chat>".to_string(),
        serde_json::to_string(&chat_entries).unwrap_or_default(),
        "</chat>".to_string(),
    ];
    if !earlier_entries.is_empty() {
        sections.push("<earlier_side_questions>".to_string());
        sections.push(serde_json::to_string(&earlier_entries).unwrap_or_default());
        sections.push("</earlier_side_questions>".to_string());
    }
    sections.push("<side_question>".to_string());
    sections.push(serde_json::to_string(question).unwrap_or_else(|_| question.to_string()));
    sections.push("</side_question>".to_string());
    sections.join("\n")
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
