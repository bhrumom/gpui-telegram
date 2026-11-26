//! `ConversationMessage`, preset/RGI reactions, `PinnedMessage` (`<=50`/`<=200`),
//! `ConversationQuestionPrompt`, plans, subagents, turn traces, memory changes, and `itemType` markers.
//! Mirrors `packages/contracts/src/ipc-conversation-{messages,events,plan,subagents,trace,memory-changes}.ts`,
//! `ipc-conversations.ts`, and `ipc-message-pins.ts`.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::limits::{
    INPUT_LIMITS, RequestId, double_option, is_bounded_string, is_identifier,
    is_nullable_bounded_string, non_null_option,
};
use crate::queue::{AttachmentSummary, QueueDeliveryStatus};
pub use crate::queue::{QueueDelivery, QueueItem};
pub use crate::ui_blocks::{ConversationUiBlock, UiBlockSpec};

pub const MCP_ELICITATION_DECISION_QUESTION_ID: &str = "mcp-elicitation-decision";
pub const ROUTINE_QUIET_ANSWER_ITEM_TYPE: &str = "routine_quiet_answer";
pub const AGENT_WELCOME_ITEM_TYPE: &str = "agent_welcome";
pub const CHANNEL_RESUME_ITEM_TYPE: &str = "channel_resume";
pub const CONVERSATION_PLAN_ITEM_TYPE: &str = "plan";
pub const CONVERSATION_SUBAGENTS_ITEM_TYPE: &str = "subagents";
pub const CONVERSATION_MEMORY_CHANGES_ITEM_TYPE: &str = "memory-changes";
pub const CONTEXT_RESET_ITEM_TYPE: &str = "context-reset";
pub const CHANNEL_CHANGE_ITEM_TYPE: &str = "channel-change";
pub const VAULT_CHANGE_ITEM_TYPE: &str = "vault-change";
pub const PLAN_CONTINUATION_ITEM_TYPE: &str = "plan-continuation";
pub const NO_ANSWER_NOTICE_ITEM_TYPE: &str = "no-answer-notice";
pub const AGENT_EXCHANGE_ITEM_TYPE: &str = "agent-exchange";
pub const INSTRUCTIONS_CHANGE_ITEM_TYPE: &str = "instructions-change";

pub const ROUTINE_EVENT_ITEM_PREFIX: &str = "routine-event:";
pub const ROUTINE_RUN_EVENT_ITEM_PREFIX: &str = "routine-run-event:";
pub const HOSTED_SITE_EVENT_ITEM_PREFIX: &str = "hosted-site-event:";
pub const CHANNEL_ROUTING_EVENT_ITEM_PREFIX: &str = "channel-routing-event:";
pub const MODEL_SWITCH_ITEM_PREFIX: &str = "model-switch:";
pub const SIGN_IN_FAILURE_ITEM_PREFIX: &str = "sign-in-failure:";
pub const SKILL_SUGGESTION_ITEM_PREFIX: &str = "skill-suggestion:";

pub const MESSAGE_REACTIONS: [&str; 6] = ["👍", "👎", "❤️", "😂", "🎉", "😮"];
pub const MORE_MESSAGE_REACTIONS: [&str; 8] = ["🔥", "👏", "🙏", "🤔", "👀", "✅", "🚀", "💯"];

pub const MESSAGE_PINS_LIMIT: usize = 50;
pub const MESSAGE_PINS_LIST_LIMIT: usize = 200;
pub const MESSAGE_PIN_TEXT_LIMIT: usize = 1_000;
pub const MESSAGE_PIN_THREAD_TITLE_LIMIT: usize = 120;

pub const CONVERSATION_PLAN_STEP_LIMIT: usize = 100;
pub const CONVERSATION_SUBAGENT_LIMIT: usize = 24;
pub const CONVERSATION_SUBAGENT_STEP_LIMIT: usize = 40;
pub const CONVERSATION_SUBAGENT_TITLE_LIMIT: usize = 300;
pub const CONVERSATION_MEMORY_CHANGE_LIMIT: usize = 50;
pub const CONVERSATION_TRACE_STEP_LIMIT: usize = 500;
pub const CONVERSATION_TRACE_TEXT_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationMessageAuthor {
    User,
    Assistant,
    Agent,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationMessageStatus {
    Streaming,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationMessageSource {
    User,
    Assistant,
    Agent,
    System,
    Routine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageGenerationAspectRatio {
    Square,
    Portrait,
    Landscape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePixelSize {
    pub width: u32,
    pub height: u32,
}

static PIXEL_SIZE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d{1,5})\s*[×xX]\s*(\d{1,5})\s*$").expect("valid pixel size regex"));

pub fn image_pixel_size(value: Option<&str>) -> Option<ImagePixelSize> {
    let caps = PIXEL_SIZE_RE.captures(value?)?;
    let width: u32 = caps.get(1)?.as_str().parse().ok()?;
    let height: u32 = caps.get(2)?.as_str().parse().ok()?;
    (width > 0 && height > 0).then_some(ImagePixelSize { width, height })
}

pub fn image_pixel_size_text(size: ImagePixelSize) -> String {
    format!("{} × {}", size.width, size.height)
}

pub fn image_aspect_ratio_of(size: ImagePixelSize) -> ImageGenerationAspectRatio {
    let ratio = f64::from(size.width) / f64::from(size.height);
    if ratio > 1.05 {
        ImageGenerationAspectRatio::Landscape
    } else if ratio < 1.0 / 1.05 {
        ImageGenerationAspectRatio::Portrait
    } else {
        ImageGenerationAspectRatio::Square
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationInfo {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub prompt: Option<String>,
    pub resolution: String,
    pub aspect_ratio: ImageGenerationAspectRatio,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoGenerationInfo {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub prompt: Option<String>,
    pub aspect_ratio: ImageGenerationAspectRatio,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub duration_seconds: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub resolution: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub error: Option<String>,
}

impl VideoGenerationInfo {
    pub fn is_valid(&self) -> bool {
        self.duration_seconds
            .is_none_or(|d| d.is_finite() && d > 0.0)
            && self
                .resolution
                .as_deref()
                .is_none_or(|r| is_bounded_string(r, 40))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPromptOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPromptQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub detail: Option<String>,
    pub is_secret: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub options: Option<Vec<AgentPromptOption>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub multi_select: Option<bool>,
}

impl AgentPromptQuestion {
    pub fn is_valid(&self) -> bool {
        is_bounded_string(&self.id, INPUT_LIMITS.identifier)
            && is_bounded_string(&self.header, INPUT_LIMITS.prompt_header)
            && is_bounded_string(&self.question, INPUT_LIMITS.prompt_question)
            && self
                .detail
                .as_deref()
                .is_none_or(|d| is_bounded_string(d, INPUT_LIMITS.prompt_question))
            && self.options.as_ref().is_none_or(|opts| {
                opts.len() <= INPUT_LIMITS.prompt_options
                    && opts.iter().all(|o| {
                        is_bounded_string(&o.label, INPUT_LIMITS.prompt_option_label)
                            && is_bounded_string(
                                &o.description,
                                INPUT_LIMITS.prompt_option_description,
                            )
                    })
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum AgentPromptQuestionResolution {
    Answered {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        answers: Option<Vec<String>>,
    },
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum AgentPromptResolution {
    Answered {
        responses: BTreeMap<String, AgentPromptQuestionResolution>,
    },
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationQuestionPrompt {
    pub request_id: RequestId,
    pub questions: Vec<AgentPromptQuestion>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub resolution: Option<AgentPromptResolution>,
}

impl ConversationQuestionPrompt {
    pub fn is_valid(&self) -> bool {
        self.request_id.is_valid()
            && self.questions.len() <= INPUT_LIMITS.prompt_questions
            && self.questions.iter().all(AgentPromptQuestion::is_valid)
    }
}

/// Validates a single RGI emoji grapheme sequence matching `isMessageReaction` (`ipc-conversation-messages.ts:L183-L189`).
pub fn is_message_reaction(value: &str) -> bool {
    crate::emoji_rgi::is_rgi_single_emoji(value)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ConversationReactionActor {
    User,
    Agent {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationReaction {
    pub emoji: String,
    pub actor: ConversationReactionActor,
}

impl ConversationReaction {
    pub fn is_valid(&self) -> bool {
        is_message_reaction(&self.emoji)
            && match &self.actor {
                ConversationReactionActor::User => true,
                ConversationReactionActor::Agent { agent_id } => !agent_id.is_empty(),
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationMessageSender {
    pub id: String,
    pub name: String,
}

impl ConversationMessageSender {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.id) && is_bounded_string(&self.name, INPUT_LIMITS.account_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MessagePinner {
    Person {
        #[serde(deserialize_with = "Option::deserialize")]
        sender: Option<ConversationMessageSender>,
    },
    Agent {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
}

impl MessagePinner {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Person { sender } => sender.as_ref().is_none_or(ConversationMessageSender::is_valid),
            Self::Agent { agent_id } => is_identifier(agent_id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinnedMessageAuthor {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedMessage {
    pub message_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub task_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub thread_title: Option<String>,
    pub pinned_at: String,
    pub pinned_by: MessagePinner,
    pub author: PinnedMessageAuthor,
    #[serde(deserialize_with = "Option::deserialize")]
    pub sender: Option<ConversationMessageSender>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub sender_agent_id: Option<String>,
    pub created_at: String,
    pub text: String,
}

impl PinnedMessage {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.message_id)
            && self.task_id.as_deref().is_none_or(is_identifier)
            && is_nullable_bounded_string(self.thread_title.as_deref(), MESSAGE_PIN_THREAD_TITLE_LIMIT)
            && is_bounded_string(&self.pinned_at, 160)
            && self.pinned_by.is_valid()
            && self.sender.as_ref().is_none_or(ConversationMessageSender::is_valid)
            && self.sender_agent_id.as_deref().is_none_or(is_identifier)
            && is_bounded_string(&self.created_at, 160)
            && is_bounded_string(&self.text, MESSAGE_PIN_TEXT_LIMIT)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedMessagesSnapshot {
    pub agent_id: String,
    pub pins: Vec<PinnedMessage>,
}

impl PinnedMessagesSnapshot {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.agent_id)
            && self.pins.len() <= MESSAGE_PINS_LIST_LIMIT
            && self.pins.iter().all(PinnedMessage::is_valid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDeliverySummary {
    pub id: String,
    pub status: QueueDeliveryStatus,
    #[serde(deserialize_with = "Option::deserialize")]
    pub position: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentExchangeDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExchangeDeliverySummary {
    pub id: String,
    pub recipient_agent_id: String,
    pub status: QueueDeliveryStatus,
    #[serde(deserialize_with = "Option::deserialize")]
    pub position: Option<u32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExchangeSummary {
    pub direction: AgentExchangeDirection,
    pub message_id: String,
    pub sender_agent_id: String,
    pub recipient_agent_ids: Vec<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub reply_to_message_id: Option<String>,
    pub deliveries: Vec<AgentExchangeDeliverySummary>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub expects_reply: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub sender_task_id: Option<String>,
}

impl AgentExchangeSummary {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.message_id)
            && is_identifier(&self.sender_agent_id)
            && self.recipient_agent_ids.len() <= INPUT_LIMITS.message_recipients
            && self.recipient_agent_ids.iter().all(|id| is_identifier(id))
            && self.reply_to_message_id.as_deref().is_none_or(is_identifier)
            && self.deliveries.len() <= INPUT_LIMITS.message_recipients
            && self.deliveries.iter().all(|d| {
                is_identifier(&d.id)
                    && is_identifier(&d.recipient_agent_id)
                    && d.position.is_none_or(|p| p >= 1)
                    && is_nullable_bounded_string(d.error.as_deref(), INPUT_LIMITS.message_text)
            })
            && self.sender_task_id.as_deref().is_none_or(is_identifier)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRoutineOrigin {
    pub routine_id: String,
    pub run_id: String,
    pub name: String,
    pub scheduled_for: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationPlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPlanStep {
    pub id: String,
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub active_text: Option<String>,
    pub status: ConversationPlanStepStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPlan {
    #[serde(deserialize_with = "Option::deserialize")]
    pub explanation: Option<String>,
    pub steps: Vec<ConversationPlanStep>,
}

impl ConversationPlan {
    pub fn is_valid(&self) -> bool {
        is_nullable_bounded_string(self.explanation.as_deref(), INPUT_LIMITS.prompt_question)
            && self.steps.len() <= CONVERSATION_PLAN_STEP_LIMIT
            && self.steps.iter().all(|s| {
                is_identifier(&s.id)
                    && is_bounded_string(&s.text, INPUT_LIMITS.prompt_question)
                    && s.active_text
                        .as_deref()
                        .is_none_or(|a| is_bounded_string(a, INPUT_LIMITS.prompt_question))
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationSubagentStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationSubagentStepKind {
    Thinking,
    Text,
    Search,
    Read,
    Edit,
    Command,
    Web,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSubagentStep {
    pub id: String,
    pub kind: ConversationSubagentStepKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSubagent {
    pub id: String,
    pub title: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub agent_type: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub prompt: Option<String>,
    pub status: ConversationSubagentStatus,
    pub started_at: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub ended_at: Option<String>,
    pub steps: Vec<ConversationSubagentStep>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub error: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub background: Option<bool>,
}

impl ConversationSubagent {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.id)
            && is_bounded_string(&self.title, CONVERSATION_SUBAGENT_TITLE_LIMIT)
            && is_nullable_bounded_string(self.agent_type.as_deref(), 80)
            && is_nullable_bounded_string(self.prompt.as_deref(), 2_000)
            && is_bounded_string(&self.started_at, 160)
            && is_nullable_bounded_string(self.ended_at.as_deref(), 160)
            && self.steps.len() <= CONVERSATION_SUBAGENT_STEP_LIMIT
            && self.steps.iter().all(|s| is_identifier(&s.id) && is_bounded_string(&s.text, 500))
            && is_nullable_bounded_string(self.error.as_deref(), 500)
            && self.background.is_none_or(|b| b)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationMemoryChangeAction {
    Saved,
    Updated,
    Forgotten,
    NotChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMemoryChange {
    pub memory_id: String,
    pub action: ConversationMemoryChangeAction,
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub previous_text: Option<String>,
}

impl ConversationMemoryChange {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.memory_id)
            && is_bounded_string(&self.text, INPUT_LIMITS.agent_memory_text)
            && self
                .previous_text
                .as_deref()
                .is_none_or(|p| is_bounded_string(p, INPUT_LIMITS.agent_memory_text))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInstructionsChange {
    pub before: String,
    pub after: String,
}

impl AgentInstructionsChange {
    pub fn is_valid(&self) -> bool {
        is_bounded_string(&self.before, INPUT_LIMITS.agent_instructions_file)
            && is_bounded_string(&self.after, INPUT_LIMITS.agent_instructions_file)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSkillSuggestion {
    pub name: String,
}

impl ConversationSkillSuggestion {
    pub fn is_valid(&self) -> bool {
        !self.name.trim().is_empty() && is_bounded_string(&self.name, 200)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationTraceStepKind {
    Reasoning,
    Tool,
    Message,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTraceStep {
    pub id: String,
    pub kind: ConversationTraceStepKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub text: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub command: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub recipient_agent_ids: Option<Vec<String>>,
    pub started_at: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub ended_at: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub failed: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub error: Option<String>,
}

const MAX_JS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTurnTrace {
    pub started_at: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub ended_at: Option<String>,
    pub steps: Vec<ConversationTraceStep>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub tokens: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub cached_input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub cost_usd: Option<Option<f64>>,
}

impl ConversationTurnTrace {
    pub fn is_valid(&self) -> bool {
        is_bounded_string(&self.started_at, 160)
            && is_nullable_bounded_string(self.ended_at.as_deref(), 160)
            && self.steps.len() <= CONVERSATION_TRACE_STEP_LIMIT
            && self.steps.iter().all(|s| {
                is_identifier(&s.id)
                    && s.text.as_deref().is_none_or(|t| is_bounded_string(t, CONVERSATION_TRACE_TEXT_LIMIT))
                    && s.command.as_deref().is_none_or(|c| is_bounded_string(c, CONVERSATION_TRACE_TEXT_LIMIT))
                    && s.error.as_deref().is_none_or(|e| is_bounded_string(e, CONVERSATION_TRACE_TEXT_LIMIT))
                    && s.recipient_agent_ids.as_ref().is_none_or(|ids| {
                        ids.len() <= INPUT_LIMITS.message_recipients && ids.iter().all(|id| is_identifier(id))
                    })
                    && is_bounded_string(&s.started_at, 160)
                    && is_nullable_bounded_string(s.ended_at.as_deref(), 160)
                    && s.failed.is_none_or(|f| f)
            })
            && self.tokens.is_none_or(|t| t <= MAX_JS_SAFE_INTEGER)
            && self.cached_input.is_none_or(|c| c <= MAX_JS_SAFE_INTEGER)
            && self.cost_usd.as_ref().is_none_or(|opt| opt.is_none_or(|c| c.is_finite() && c >= 0.0))
    }
}

/// Complete 28-field `ConversationMessage` (`ipc-conversation-messages.ts:L265-L302`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    pub id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub turn_id: Option<String>,
    pub author: ConversationMessageAuthor,
    pub text: String,
    pub created_at: String,
    pub status: ConversationMessageStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub item_type: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub source: Option<ConversationMessageSource>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub sender_agent_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub sender_member: Option<ConversationMessageSender>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub reply_to_message_id: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub attachments: Option<Vec<AttachmentSummary>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub voice_recording: Option<AttachmentSummary>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub image_generation: Option<ImageGenerationInfo>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub video_generation: Option<VideoGenerationInfo>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub delivery: Option<ConversationDeliverySummary>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub steered: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub exchange: Option<AgentExchangeSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub reaction: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub reactions: Option<Vec<ConversationReaction>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub routine: Option<ConversationRoutineOrigin>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub question_prompt: Option<ConversationQuestionPrompt>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub plan: Option<ConversationPlan>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub subagents: Option<Vec<ConversationSubagent>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub memory_changes: Option<Vec<ConversationMemoryChange>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub instructions_change: Option<AgentInstructionsChange>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub skill_suggestion: Option<ConversationSkillSuggestion>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub ui_block: Option<ConversationUiBlock>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub trace: Option<ConversationTurnTrace>,
}

impl ConversationMessage {
    pub fn new_simple(
        id: impl Into<String>,
        author: ConversationMessageAuthor,
        text: impl Into<String>,
        created_at: impl Into<String>,
        status: ConversationMessageStatus,
    ) -> Self {
        Self {
            id: id.into(),
            turn_id: None,
            author,
            text: text.into(),
            created_at: created_at.into(),
            status,
            item_type: None,
            source: None,
            sender_agent_id: None,
            sender_member: None,
            reply_to_message_id: None,
            attachments: None,
            voice_recording: None,
            image_generation: None,
            video_generation: None,
            delivery: None,
            steered: None,
            exchange: None,
            reaction: None,
            reactions: None,
            routine: None,
            question_prompt: None,
            plan: None,
            subagents: None,
            memory_changes: None,
            instructions_change: None,
            skill_suggestion: None,
            ui_block: None,
            trace: None,
        }
    }

    pub fn is_valid(&self) -> bool {
        is_identifier(&self.id)
            && is_bounded_string(&self.created_at, 160)
            && self.turn_id.as_deref().is_none_or(is_identifier)
            && self.item_type.as_deref().is_none_or(|t| is_bounded_string(t, INPUT_LIMITS.identifier))
            && self.sender_agent_id.as_deref().is_none_or(is_identifier)
            && self.sender_member.as_ref().is_none_or(ConversationMessageSender::is_valid)
            && self.reply_to_message_id.as_ref().is_none_or(|opt| opt.as_deref().is_none_or(is_identifier))
            && self.attachments.as_ref().is_none_or(|a| {
                a.len() <= INPUT_LIMITS.attachments && a.iter().all(AttachmentSummary::is_valid)
            })
            && self.voice_recording.as_ref().is_none_or(AttachmentSummary::is_valid)
            && self.delivery.as_ref().is_none_or(|d| is_identifier(&d.id) && d.position.is_none_or(|p| p >= 1))
            && self.steered.is_none_or(|s| s)
            && self.exchange.as_ref().is_none_or(AgentExchangeSummary::is_valid)
            && self.reaction.as_ref().is_none_or(|opt| opt.as_deref().is_none_or(is_message_reaction))
            && self.reactions.as_ref().is_none_or(|r| {
                r.len() <= INPUT_LIMITS.team_members && r.iter().all(ConversationReaction::is_valid)
            })
            && self.routine.as_ref().is_none_or(|r| {
                is_identifier(&r.routine_id)
                    && is_identifier(&r.run_id)
                    && is_bounded_string(&r.name, INPUT_LIMITS.routine_name)
                    && is_bounded_string(&r.scheduled_for, 160)
            })
            && self.video_generation.as_ref().is_none_or(VideoGenerationInfo::is_valid)
            && self.question_prompt.as_ref().is_none_or(ConversationQuestionPrompt::is_valid)
            && self.plan.as_ref().is_none_or(ConversationPlan::is_valid)
            && self.subagents.as_ref().is_none_or(|s| {
                s.len() <= CONVERSATION_SUBAGENT_LIMIT && s.iter().all(ConversationSubagent::is_valid)
            })
            && self.memory_changes.as_ref().is_none_or(|m| {
                m.len() <= CONVERSATION_MEMORY_CHANGE_LIMIT && m.iter().all(ConversationMemoryChange::is_valid)
            })
            && self.instructions_change.as_ref().is_none_or(AgentInstructionsChange::is_valid)
            && self.skill_suggestion.as_ref().is_none_or(ConversationSkillSuggestion::is_valid)
            && self.ui_block.as_ref().is_none_or(ConversationUiBlock::validate)
            && self.trace.as_ref().is_none_or(ConversationTurnTrace::is_valid)
    }

    pub fn is_agent_answer(&self) -> bool {
        self.author == ConversationMessageAuthor::Assistant
            && !matches!(
                self.item_type.as_deref(),
                Some(
                    "commentary"
                        | "question_prompt"
                        | CONVERSATION_PLAN_ITEM_TYPE
                        | CONVERSATION_SUBAGENTS_ITEM_TYPE
                        | ROUTINE_QUIET_ANSWER_ITEM_TYPE
                )
            )
            && !self.text.trim().is_empty()
    }
}

pub fn routine_quiet_steps_item_id(turn_id: &str) -> String {
    format!("{turn_id}:quiet-steps")
}

pub fn latest_turn_answer<'a>(
    messages: &'a [ConversationMessage],
    turn_id: &str,
) -> Option<&'a ConversationMessage> {
    messages
        .iter()
        .rev()
        .find(|m| m.turn_id.as_deref() == Some(turn_id) && m.is_agent_answer())
}

pub fn open_exchanges_outside(
    loaded: &[ConversationMessage],
    page_ids: &HashSet<String>,
) -> Vec<String> {
    loaded
        .iter()
        .filter_map(|m| {
            let ex = m.exchange.as_ref()?;
            let open = !page_ids.contains(&m.id)
                && ex.direction == AgentExchangeDirection::Outgoing
                && ex.expects_reply != Some(false)
                && ex.deliveries.iter().any(|d| {
                    matches!(
                        d.status,
                        QueueDeliveryStatus::Queued
                            | QueueDeliveryStatus::Starting
                            | QueueDeliveryStatus::Running
                    )
                });
            open.then(|| m.id.clone())
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSnapshot {
    pub agent_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub thread_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub active_turn_id: Option<String>,
    pub revision: u64,
    pub messages: Vec<ConversationMessage>,
}

impl ConversationSnapshot {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.agent_id)
            && self.thread_id.as_deref().is_none_or(is_identifier)
            && self.active_turn_id.as_deref().is_none_or(is_identifier)
            && self.messages.iter().all(ConversationMessage::is_valid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationReadState {
    pub unread_count: u64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub first_unread_message_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub through_message_id: Option<String>,
}

impl ConversationReadState {
    pub fn is_valid(&self) -> bool {
        self.first_unread_message_id.as_deref().is_none_or(is_identifier)
            && self.through_message_id.as_deref().is_none_or(is_identifier)
    }
}
