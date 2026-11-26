//! Multi-Bot Shared Channel contracts (`Channel`, `ChannelDraft`, `ChannelTask`, `ChannelAssignment`,
//! `ChannelMessage`, `ChannelCommand`), asymmetric normalization/validation, Lead-Agent `ChannelRoutingDecision`,
//! 15 in-channel + 4 external channel tools, and `check_channel_reply` / failed-reply deadlock guards.
//! Mirrors `packages/contracts/src/ipc-chat-channels.ts` and `src/backend/channel-{replies,tools,agent-tools,store,service}.ts`.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::avatar::AvatarHue;
use crate::conversation::{ConversationMessage, ConversationMessageStatus};
use crate::limits::{
    ContractError, INPUT_LIMITS, double_option, is_bounded_string, is_identifier, non_null_option,
    utf16_len,
};
pub use crate::mentions::{
    CONTINUE_STOPPED_RE, FOLLOW_UP_PREFIX_RE, MentionableAgent, MentionableAgentInfo,
    NamedChannelAgent, agent_mention_rank, fold_mention_text, is_continue_stopped_message,
    is_follow_up_message, mid_text_typed_addresses, rank_agent_mentions,
    resolve_channel_recipients, typed_address, without_own_name_label,
};
use crate::routines::{RoutineLimitPolicy, RoutineMissedPolicy, RoutineSchedule};
pub use crate::tags::{
    ChatTagKind, ChatTagReference, EVERYONE_MENTION, FileQuoteLines, FileQuoteTarget,
    LeadingAddress, chat_tag_references, decode_chat_tag_component, encode_chat_tag_component,
    expand_chat_tag_references, file_quote_id, file_quote_target, leading_address,
    limited_quote_text, reference_name, serialize_chat_tag_reference,
};

pub const CHANNEL_CHATS_CAPABILITY: &str = "channel-chats-v1";
pub const CHANNEL_DELETE_CAPABILITY: &str = "channel-delete-v1";
pub const CHANNEL_TASK_CANCEL_CAPABILITY: &str = "channel-task-cancel-v1";
pub const SIGNED_OUT_CHANNEL_MEMBER_ID: &str = "local";
pub const CHANNEL_PARALLEL_LIMIT: usize = 2;
pub const CHANNEL_ASSIGNMENT_LIMIT: u64 = 8;
pub const CHANNEL_ASSIGNMENT_LIMIT_ERROR: &str =
    "The automatic assignment limit was reached. Continue or reassign this task.";
pub const CHANNEL_PREVIEW_LIMIT: usize = 160;
pub const CHANNEL_FOLDER_LIMIT: usize = 8;
pub const CHANNEL_REPLY_ID_PREFIX: &str = "channel-reply-";
pub const CHANNEL_REPLY_LIMIT: usize = 2;
pub const CHANNEL_REPLY_CHARACTERS: usize = 2_000;
pub const CHANNEL_HANDOFF_AUTHOR_PREFIX: &str = "agent:";
pub const CHANNEL_ROUTINE_AUTHOR_PREFIX: &str = "routine:";
pub const TASK_EXPECTED_RESULT: &str = "Complete the requested work and report the result.";
pub const ROUTING_RECENT_MESSAGES: usize = 5;
pub const ROUTING_TEXT_CHARACTERS: usize = 600;
pub const ROUTING_REQUEST_BYTES: usize = 20_000;
pub const ROUTING_PROMPT_CHARACTERS: usize = 120_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMember {
    pub agent_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelIconName {
    Megaphone,
    Chart,
    Lifebuoy,
    Search,
    Briefcase,
    Code,
    Globe,
    Mail,
    Calendar,
    Gift,
    Palette,
    Brain,
    Book,
    Sparkles,
    Star,
    Zap,
    Shield,
    Store,
    Workflow,
    Card,
}

impl ChannelIconName {
    pub const ALL: [Self; 20] = [
        Self::Megaphone,
        Self::Chart,
        Self::Lifebuoy,
        Self::Search,
        Self::Briefcase,
        Self::Code,
        Self::Globe,
        Self::Mail,
        Self::Calendar,
        Self::Gift,
        Self::Palette,
        Self::Brain,
        Self::Book,
        Self::Sparkles,
        Self::Star,
        Self::Zap,
        Self::Shield,
        Self::Store,
        Self::Workflow,
        Self::Card,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Megaphone => "megaphone",
            Self::Chart => "chart",
            Self::Lifebuoy => "lifebuoy",
            Self::Search => "search",
            Self::Briefcase => "briefcase",
            Self::Code => "code",
            Self::Globe => "globe",
            Self::Mail => "mail",
            Self::Calendar => "calendar",
            Self::Gift => "gift",
            Self::Palette => "palette",
            Self::Brain => "brain",
            Self::Book => "book",
            Self::Sparkles => "sparkles",
            Self::Star => "star",
            Self::Zap => "zap",
            Self::Shield => "shield",
            Self::Store => "store",
            Self::Workflow => "workflow",
            Self::Card => "card",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|i| i.as_str() == raw)
    }
}

pub const CHANNEL_ICON_NAMES: [&str; 20] = [
    "megaphone", "chart", "lifebuoy", "search", "briefcase", "code", "globe", "mail", "calendar",
    "gift", "palette", "brain", "book", "sparkles", "star", "zap", "shield", "store", "workflow",
    "card",
];

static CHANNEL_ICON_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9-]{0,39}$").expect("valid channel icon regex"));

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelIcon {
    pub name: String,
    pub hue: u16,
}

impl ChannelIcon {
    pub fn is_valid(&self) -> bool {
        CHANNEL_ICON_NAME_RE.is_match(&self.name) && self.hue < 360
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ChannelBrowser {
    #[default]
    Members,
    None,
    Member {
        agent_id: String,
    },
}

impl Serialize for ChannelBrowser {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Members => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("kind", "members")?;
                map.end()
            }
            Self::None => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("kind", "none")?;
                map.end()
            }
            Self::Member { agent_id } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "member")?;
                map.serialize_entry("agentId", agent_id)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for ChannelBrowser {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(deserializer)?;
        let Some(obj) = val.as_object() else {
            return Err(serde::de::Error::custom("ChannelBrowser must be an object"));
        };
        match obj.get("kind").and_then(serde_json::Value::as_str) {
            Some("members") if obj.len() == 1 => Ok(Self::Members),
            Some("none") if obj.len() == 1 => Ok(Self::None),
            Some("member") if obj.len() == 2 => {
                let Some(agent_id) = obj.get("agentId").and_then(serde_json::Value::as_str) else {
                    return Err(serde::de::Error::custom(
                        "ChannelBrowser.agentId must be a string",
                    ));
                };
                if !is_identifier(agent_id) {
                    return Err(serde::de::Error::custom(
                        "ChannelBrowser.agentId must be a valid identifier",
                    ));
                }
                Ok(Self::Member {
                    agent_id: agent_id.to_string(),
                })
            }
            _ => Err(serde::de::Error::custom("Invalid ChannelBrowser object")),
        }
    }
}

pub fn channel_allows_browser(browser: Option<&ChannelBrowser>, agent_id: &str) -> bool {
    match browser.unwrap_or(&ChannelBrowser::Members) {
        ChannelBrowser::Members => true,
        ChannelBrowser::None => false,
        ChannelBrowser::Member { agent_id: allowed } => allowed == agent_id,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelDraft {
    pub name: String,
    pub title: String,
    pub instructions: String,
    pub members: Vec<ChannelMember>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub lead_agent_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub isolated: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub browser: Option<ChannelBrowser>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub icon: Option<Option<ChannelIcon>>,
}

pub fn with_browser_member(draft: &mut ChannelDraft) {
    if let Some(ChannelBrowser::Member { agent_id }) = &draft.browser
        && !draft.members.iter().any(|m| m.agent_id == *agent_id)
    {
        draft.browser = Some(ChannelBrowser::None);
    }
}

impl ChannelDraft {
    pub fn validate_with_limit(&self, instructions_limit: usize) -> Result<(), ContractError> {
        if self.name.trim().is_empty() || !is_bounded_string(&self.name, INPUT_LIMITS.agent_name) {
            return Err(ContractError::InvalidValue("Invalid channel name.".to_string()));
        }
        if !is_bounded_string(&self.title, INPUT_LIMITS.agent_title) {
            return Err(ContractError::InvalidValue("Invalid channel title.".to_string()));
        }
        if !is_bounded_string(&self.instructions, instructions_limit) {
            return Err(ContractError::InvalidValue("Channel instructions exceed limit.".to_string()));
        }
        if self.members.len() > INPUT_LIMITS.agents {
            return Err(ContractError::InvalidValue("Too many channel members.".to_string()));
        }
        let mut member_ids = HashSet::new();
        for m in &self.members {
            if !is_identifier(&m.agent_id) || !member_ids.insert(m.agent_id.as_str()) {
                return Err(ContractError::InvalidValue("Invalid or duplicate channel member.".to_string()));
            }
        }
        if let Some(lead) = &self.lead_agent_id
            && (!is_identifier(lead) || !member_ids.contains(lead.as_str()))
        {
            return Err(ContractError::InvalidValue(
                "Channel leadAgentId must be a channel member.".to_string(),
            ));
        }
        if let Some(ChannelBrowser::Member { agent_id }) = &self.browser
            && (!is_identifier(agent_id) || !member_ids.contains(agent_id.as_str()))
        {
            return Err(ContractError::InvalidValue(
                "Channel browser member must be a channel member.".to_string(),
            ));
        }
        if let Some(Some(icon)) = &self.icon
            && !icon.is_valid()
        {
            return Err(ContractError::InvalidValue("Invalid channel icon.".to_string()));
        }
        Ok(())
    }

    /// Strict write-side validator: enforces `agent_description = 2_000` on modified instructions,
    /// while allowing `stored_channel_instructions = 32_000` when instructions are unchanged (`channel-service.ts:L660-L664`).
    pub fn validate_strict(&self, existing_instructions: Option<&str>) -> Result<(), ContractError> {
        let limit = if existing_instructions == Some(self.instructions.as_str()) {
            INPUT_LIMITS.stored_channel_instructions
        } else {
            INPUT_LIMITS.agent_description
        };
        self.validate_with_limit(limit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    #[serde(flatten)]
    pub draft: ChannelDraft,
    pub archived: bool,
    pub revision: u64,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub image_url: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub folders: Option<Vec<String>>,
}

impl Channel {
    pub fn is_valid_stored(&self) -> bool {
        is_identifier(&self.id)
            && self
                .draft
                .validate_with_limit(INPUT_LIMITS.stored_channel_instructions)
                .is_ok()
            && is_bounded_string(&self.created_at, 80)
            && self
                .image_url
                .as_ref()
                .is_none_or(|opt| opt.as_deref().is_none_or(|u| is_bounded_string(u, 2_048)))
            && self.folders.as_ref().is_none_or(|f| {
                f.len() <= CHANNEL_FOLDER_LIMIT
                    && f.iter().all(|p| !p.is_empty() && is_bounded_string(p, INPUT_LIMITS.path))
            })
    }

    /// Lenient read-side decoder (`ipc-chat-channels.ts:decodeChannel` / `normalizeSavedChannel`, `L222-L248`).
    pub fn decode_normalized(raw: &serde_json::Value) -> Result<Self, ContractError> {
        let serde_json::Value::Object(mut map) = raw.clone() else {
            return Err(ContractError::InvalidValue("Invalid channel response.".to_string()));
        };
        if let Some(purpose_val) = map.remove("purpose") {
            let purpose_str = purpose_val.as_str().unwrap_or("").to_string();
            map.entry("title".to_string())
                .or_insert_with(|| serde_json::Value::String(String::new()));
            map.entry("instructions".to_string())
                .or_insert_with(|| serde_json::Value::String(purpose_str));
        }
        let valid_member_ids: Vec<String> = map
            .get("members")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("agentId")?.as_str())
                    .filter(|id| is_identifier(id))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(serde_json::Value::String(lead)) = map.get("leadAgentId")
            && is_identifier(lead)
            && !valid_member_ids.iter().any(|id| id == lead)
        {
            let replacement = valid_member_ids
                .first()
                .cloned()
                .map_or(serde_json::Value::Null, serde_json::Value::String);
            map.insert("leadAgentId".to_string(), replacement);
        }
        if let Some(browser_val) = map.get("browser") {
            let valid_browser = serde_json::from_value::<ChannelBrowser>(browser_val.clone())
                .ok()
                .is_some_and(|b| match b {
                    ChannelBrowser::Members | ChannelBrowser::None => {
                        browser_val.as_object().is_some_and(|o| o.len() == 1)
                    }
                    ChannelBrowser::Member { agent_id } => {
                        browser_val.as_object().is_some_and(|o| o.len() == 2)
                            && valid_member_ids.contains(&agent_id)
                    }
                });
            if !valid_browser {
                map.insert("browser".to_string(), serde_json::json!({ "kind": "none" }));
            }
        }
        if let Some(icon_val) = map.get("icon")
            && !icon_val.is_null()
        {
            let icon_ok = serde_json::from_value::<ChannelIcon>(icon_val.clone())
                .ok()
                .is_some_and(|ic| ic.is_valid());
            if !icon_ok {
                map.remove("icon");
            }
        }
        let channel: Self = serde_json::from_value(serde_json::Value::Object(map))
            .map_err(|e| ContractError::Json(e.to_string()))?;
        if !channel.is_valid_stored() {
            return Err(ContractError::InvalidValue("Invalid channel response.".to_string()));
        }
        Ok(channel)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelTaskState {
    Queued,
    Running,
    Waiting,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl ChannelTaskState {
    pub const ALL: [Self; 7] = [
        Self::Queued,
        Self::Running,
        Self::Waiting,
        Self::Paused,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelTask {
    pub id: String,
    pub channel_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub parent_task_id: Option<String>,
    pub root_task_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub owner_agent_id: Option<String>,
    pub request_message_id: String,
    pub instruction: String,
    pub attachment_draft_ids: Vec<String>,
    pub expected_result: String,
    pub source_message_ids: Vec<String>,
    pub dependencies: Vec<String>,
    pub resources: Vec<String>,
    pub state: ChannelTaskState,
    pub revision: u64,
    pub assignment_count: u64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub error: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub seen_dependencies: Option<Vec<String>>,
}

pub fn is_reply_task(task: &ChannelTask) -> bool {
    task.parent_task_id.is_some()
        && task.request_message_id.starts_with(CHANNEL_REPLY_ID_PREFIX)
        && task.source_message_ids.first() == Some(&task.request_message_id)
}

/// Failed reply subtask deadlock guard (`channel-service.ts:settled`, `L4038-L4040`).
pub fn is_task_settled(task: &ChannelTask, state: ChannelTaskState) -> bool {
    state == ChannelTaskState::Completed || (state == ChannelTaskState::Failed && is_reply_task(task))
}

pub fn released_parent_task(
    tasks: &[ChannelTask],
    task: &ChannelTask,
    next_state: ChannelTaskState,
) -> Option<ChannelTask> {
    let parent_id = task.parent_task_id.as_deref()?;
    let parent = tasks.iter().find(|t| t.id == parent_id)?;
    if parent.state != ChannelTaskState::Waiting {
        return None;
    }
    let all_settled = tasks
        .iter()
        .filter(|t| parent.dependencies.contains(&t.id))
        .all(|item| {
            if item.id == task.id {
                is_task_settled(task, next_state)
            } else {
                is_task_settled(item, item.state)
            }
        });
    all_settled.then(|| {
        let mut resumed = parent.clone();
        resumed.state = ChannelTaskState::Queued;
        resumed.revision += 1;
        resumed
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelAssignmentState {
    Queued,
    Starting,
    Running,
    Completed,
    Failed,
    Interrupted,
}

pub const ACTIVE_ASSIGNMENT_STATES: [ChannelAssignmentState; 3] = [
    ChannelAssignmentState::Starting,
    ChannelAssignmentState::Running,
    ChannelAssignmentState::Queued,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelAssignment {
    pub id: String,
    pub channel_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub task_revision: u64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub request_message_id: Option<String>,
    pub resources: Vec<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub delivery_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub turn_id: Option<String>,
    pub state: ChannelAssignmentState,
    pub through_sequence: u64,
    pub summary_version: u64,
    pub awaited_task_ids: Vec<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub pending_revision: Option<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub pending_outcome: Option<String>,
}

impl ChannelAssignment {
    pub fn is_active(&self) -> bool {
        ACTIVE_ASSIGNMENT_STATES.contains(&self.state)
    }

    pub fn result_message_id(&self) -> String {
        channel_result_id(self)
    }
}

pub fn channel_result_id(assignment: &ChannelAssignment) -> String {
    format!(
        "channel-result-{}-revision-{}",
        assignment.id, assignment.task_revision
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelContext {
    pub thread_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub session_id: Option<String>,
    pub through_sequence: u64,
    pub summary_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelHistorySummary {
    pub version: u64,
    pub through_sequence: u64,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelMessageAuthorKind {
    Member,
    Agent,
    Coordinator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelMessageAuthor {
    pub kind: ChannelMessageAuthorKind,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMessage {
    pub id: String,
    pub channel_id: String,
    pub sequence: u64,
    pub author: ChannelMessageAuthor,
    #[serde(deserialize_with = "Option::deserialize")]
    pub task_id: Option<String>,
    pub superseded: bool,
    pub message: ConversationMessage,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub deleted: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub sender_task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPreviewAuthor {
    pub kind: ChannelMessageAuthorKind,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelPreview {
    pub author_name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub author: Option<ChannelPreviewAuthor>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub status: Option<ConversationMessageStatus>,
    pub text: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelThread {
    pub agent_id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSummary {
    #[serde(flatten)]
    pub channel: Channel,
    pub unread_count: u64,
    pub active_tasks: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub open_tasks: Option<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub last_message: Option<ChannelPreview>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub threads: Option<Vec<ChannelThread>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum ChannelTaskWait {
    AgentBusy {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "channelName")]
        channel_name: String,
    },
    ChannelFull {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelPage {
    pub channel: Channel,
    pub messages: Vec<ChannelMessage>,
    pub tasks: Vec<ChannelTask>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub older_cursor: Option<u64>,
    pub through_sequence: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub older_count: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub oldest_at: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub waits: Option<Vec<ChannelTaskWait>>,
}

fn finished_channel_task(task: &ChannelTask) -> bool {
    matches!(
        task.state,
        ChannelTaskState::Completed | ChannelTaskState::Cancelled
    )
}

pub fn channel_page_tasks(tasks: &[ChannelTask], messages: &[ChannelMessage]) -> Vec<ChannelTask> {
    let open_runs: HashSet<&str> = tasks
        .iter()
        .filter(|t| !finished_channel_task(t))
        .map(|t| t.root_task_id.as_str())
        .collect();
    let msg_task_ids: HashSet<&str> = messages
        .iter()
        .filter_map(|m| m.task_id.as_deref())
        .collect();
    let msg_ids: HashSet<&str> = messages.iter().map(|m| m.id.as_str()).collect();
    tasks
        .iter()
        .filter(|t| {
            open_runs.contains(t.root_task_id.as_str())
                || msg_task_ids.contains(t.id.as_str())
                || msg_ids.contains(t.request_message_id.as_str())
        })
        .cloned()
        .collect()
}

pub fn merge_channel_tasks(
    newer: &[ChannelTask],
    older: &[ChannelTask],
    messages: &[ChannelMessage],
) -> Vec<ChannelTask> {
    let newer_ids: HashSet<&str> = newer.iter().map(|t| t.id.as_str()).collect();
    let msg_task_ids: HashSet<&str> = messages
        .iter()
        .filter_map(|m| m.task_id.as_deref())
        .collect();
    let msg_ids: HashSet<&str> = messages.iter().map(|m| m.id.as_str()).collect();
    let mut kept: Vec<ChannelTask> = older
        .iter()
        .filter(|t| {
            !newer_ids.contains(t.id.as_str())
                && finished_channel_task(t)
                && (msg_task_ids.contains(t.id.as_str())
                    || msg_ids.contains(t.request_message_id.as_str()))
        })
        .cloned()
        .collect();
    kept.extend_from_slice(newer);
    kept
}

pub fn closed_channel_task_sequences(
    newer: &[ChannelTask],
    older: &[ChannelTask],
    messages: &[ChannelMessage],
) -> Vec<u64> {
    let newer_ids: HashSet<&str> = newer.iter().map(|t| t.id.as_str()).collect();
    let closed: Vec<&ChannelTask> = older
        .iter()
        .filter(|t| !newer_ids.contains(t.id.as_str()) && !finished_channel_task(t))
        .collect();
    if closed.is_empty() {
        return Vec::new();
    }
    let mut seqs: Vec<u64> = messages
        .iter()
        .filter(|m| {
            closed
                .iter()
                .any(|t| m.task_id.as_deref() == Some(t.id.as_str()) || m.id == t.request_message_id)
        })
        .map(|m| m.sequence)
        .collect();
    seqs.sort_unstable_by(|a, b| b.cmp(a));
    seqs.dedup();
    seqs
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ChannelRoutineOrigin {
    Routine {
        #[serde(rename = "routineId")]
        routine_id: String,
        #[serde(rename = "routineName")]
        routine_name: String,
        #[serde(rename = "runId")]
        run_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ChannelCommand {
    Save {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        draft: ChannelDraft,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        update: Option<bool>,
    },
    Restore {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    Archive {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    Send {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        text: String,
        #[serde(rename = "recipientAgentId", deserialize_with = "Option::deserialize")]
        recipient_agent_id: Option<String>,
        #[serde(rename = "replyToMessageId", deserialize_with = "Option::deserialize")]
        reply_to_message_id: Option<String>,
        #[serde(rename = "attachmentDraftIds")]
        attachment_draft_ids: Vec<String>,
    },
    Request {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        text: String,
        #[serde(rename = "recipientAgentId", deserialize_with = "Option::deserialize")]
        recipient_agent_id: Option<String>,
        #[serde(rename = "requestMessageId")]
        request_message_id: String,
        origin: ChannelRoutineOrigin,
    },
    Stop {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "recipientAgentId", deserialize_with = "Option::deserialize")]
        recipient_agent_id: Option<String>,
    },
    Resume {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "recipientAgentId", deserialize_with = "Option::deserialize")]
        recipient_agent_id: Option<String>,
    },
    Reassign {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "recipientAgentId", deserialize_with = "Option::deserialize")]
        recipient_agent_id: Option<String>,
    },
    Cancel {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "recipientAgentId", deserialize_with = "Option::deserialize")]
        recipient_agent_id: Option<String>,
    },
    Read {
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "throughSequence")]
        through_sequence: u64,
    },
}

/// 5-shape Lead-Agent router decision (`channel-service.ts:L141-L156`), rejecting excess keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelRoutingDecision {
    AssignAgents { agent_ids: Vec<String> },
    ResumeTask { task_id: String },
    Idle,
    AskLead { agent_id: String },
    ClarifyQuestion { question: String },
}

impl Serialize for ChannelRoutingDecision {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            Self::AssignAgents { agent_ids } => map.serialize_entry("agentIds", agent_ids)?,
            Self::ResumeTask { task_id } => map.serialize_entry("taskId", task_id)?,
            Self::Idle => map.serialize_entry("idle", &true)?,
            Self::AskLead { agent_id } => map.serialize_entry("agentId", agent_id)?,
            Self::ClarifyQuestion { question } => map.serialize_entry("question", question)?,
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ChannelRoutingDecision {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(deserializer)?;
        Self::decode_strict(&val, None).map_err(serde::de::Error::custom)
    }
}

/// Builds `(agent_id -> mK, mK -> agent_id)` deduplicating repeated `member_ids` and skipping any `mK` that collides with a literal `agent_id`.
pub fn build_member_alias_map(
    member_ids: &[String],
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let literal_set: HashSet<&str> = member_ids.iter().map(String::as_str).collect();
    let mut agent_to_alias = BTreeMap::new();
    let mut alias_to_agent = BTreeMap::new();
    let mut k = 1usize;
    for member_id in member_ids {
        if agent_to_alias.contains_key(member_id.as_str()) {
            continue;
        }
        loop {
            let candidate = format!("m{k}");
            k += 1;
            if !literal_set.contains(candidate.as_str()) {
                agent_to_alias.insert(member_id.clone(), candidate.clone());
                alias_to_agent.insert(candidate, member_id.clone());
                break;
            }
        }
    }
    (agent_to_alias, alias_to_agent)
}

impl ChannelRoutingDecision {
    pub fn decode_strict(
        value: &serde_json::Value,
        alias_to_agent: Option<&BTreeMap<String, String>>,
    ) -> Result<Self, ContractError> {
        let serde_json::Value::Object(map) = value else {
            return Err(ContractError::InvalidValue("Routing decision must be a JSON object.".to_string()));
        };
        if map.len() != 1 {
            return Err(ContractError::InvalidValue(
                "Routing decision must contain exactly 1 key (excess properties rejected).".to_string(),
            ));
        }
        let resolve = |raw_id: &str| -> String {
            alias_to_agent
                .and_then(|m| m.get(raw_id))
                .cloned()
                .unwrap_or_else(|| raw_id.to_string())
        };
        if let Some(val) = map.get("agentIds") {
            let Some(arr) = val.as_array() else {
                return Err(ContractError::InvalidValue("agentIds must be an array.".to_string()));
            };
            if arr.is_empty() || arr.len() > INPUT_LIMITS.agents {
                return Err(ContractError::InvalidValue("agentIds must have 1..=100 items.".to_string()));
            }
            let mut ids = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(s) = item.as_str().filter(|s| !s.is_empty()) else {
                    return Err(ContractError::InvalidValue("agentIds item must be non-empty string.".to_string()));
                };
                ids.push(resolve(s));
            }
            return Ok(Self::AssignAgents { agent_ids: ids });
        }
        if let Some(val) = map.get("taskId") {
            let Some(s) = val.as_str().filter(|s| !s.is_empty()) else {
                return Err(ContractError::InvalidValue("taskId must be non-empty string.".to_string()));
            };
            return Ok(Self::ResumeTask {
                task_id: s.to_string(),
            });
        }
        if let Some(val) = map.get("idle") {
            if val.as_bool() == Some(true) {
                return Ok(Self::Idle);
            }
            return Err(ContractError::InvalidValue("idle must be literal true.".to_string()));
        }
        if let Some(val) = map.get("agentId") {
            let Some(s) = val.as_str().filter(|s| !s.is_empty()) else {
                return Err(ContractError::InvalidValue("agentId must be non-empty string.".to_string()));
            };
            return Ok(Self::AskLead {
                agent_id: resolve(s),
            });
        }
        if let Some(val) = map.get("question") {
            let Some(s) = val.as_str() else {
                return Err(ContractError::InvalidValue("question must be a string.".to_string()));
            };
            return Ok(Self::ClarifyQuestion {
                question: s.to_string(),
            });
        }
        Err(ContractError::InvalidValue("Unknown routing decision shape.".to_string()))
    }

    /// Parses raw Lead-Router JSON text, falling back to `AskLead { agent_id: lead_agent_id }` on error.
    pub fn decode_or_lead_fallback(
        raw_json: &str,
        alias_to_agent: Option<&BTreeMap<String, String>>,
        lead_agent_id: &str,
    ) -> Self {
        serde_json::from_str::<serde_json::Value>(raw_json)
            .ok()
            .and_then(|v| Self::decode_strict(&v, alias_to_agent).ok())
            .unwrap_or_else(|| Self::AskLead {
                agent_id: lead_agent_id.to_string(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelHandoffToolInput {
    pub recipient_agent_id: String,
    pub task: String,
    pub expected_result: String,
    pub source_message_ids: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub resources: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub dependencies: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SendToChannelToolInput {
    pub channel_id: String,
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub recipient_agent_ids: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub paths: Option<Vec<String>>,
}

/// Core 6 `ChannelToolCall` variants (`PROJECT.md § Interface Contracts § 1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ChannelToolCall {
    Assign(ChannelHandoffToolInput),
    Transfer(ChannelHandoffToolInput),
    Reply {
        #[serde(rename = "messageId")]
        message_id: String,
        text: String,
    },
    Result {
        text: String,
    },
    Cancel {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    SendToChannel(SendToChannelToolInput),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelToolDefinition {
    pub name: &'static str,
    pub person_only: bool,
}

/// Complete 15 in-channel `channel_*` tools (`src/backend/channel-tools.ts:L29-L134`).
pub const CHANNEL_TOOL_DEFINITIONS: [ChannelToolDefinition; 15] = [
    ChannelToolDefinition { name: "channel_history", person_only: false },
    ChannelToolDefinition { name: "channel_assign", person_only: false },
    ChannelToolDefinition { name: "channel_transfer", person_only: false },
    ChannelToolDefinition { name: "channel_result", person_only: false },
    ChannelToolDefinition { name: "channel_reply", person_only: false },
    ChannelToolDefinition { name: "channel_cancel", person_only: true },
    ChannelToolDefinition { name: "channel_remember", person_only: false },
    ChannelToolDefinition { name: "channel_forget_memory", person_only: false },
    ChannelToolDefinition { name: "channel_add_member", person_only: true },
    ChannelToolDefinition { name: "channel_remove_member", person_only: true },
    ChannelToolDefinition { name: "channel_set_instructions", person_only: true },
    ChannelToolDefinition { name: "channel_list_routines", person_only: true },
    ChannelToolDefinition { name: "channel_create_routine", person_only: true },
    ChannelToolDefinition { name: "channel_update_routine", person_only: true },
    ChannelToolDefinition { name: "channel_delete_routine", person_only: true },
];

pub const EXTERNAL_CHANNEL_TOOL_NAMES: [&str; 4] = [
    "list_channels",
    "send_to_channel",
    "create_channel",
    "update_channel",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelHistoryToolInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub before_sequence: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub attachment_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub message_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ChannelCreateRoutineToolInput {
    pub name: String,
    pub instruction: String,
    pub schedule: RoutineSchedule,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub active: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub timezone: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub limit_policy: Option<RoutineLimitPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub missed_policy: Option<RoutineMissedPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ChannelUpdateRoutineToolInput {
    pub routine_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub instruction: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub schedule: Option<RoutineSchedule>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub active: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub limit_policy: Option<RoutineLimitPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub missed_policy: Option<RoutineMissedPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateChannelToolIcon {
    pub name: ChannelIconName,
    pub hue: AvatarHue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateChannelToolInput {
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub title: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub instructions: Option<String>,
    pub member_agent_ids: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub lead_agent_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub isolated: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub icon: Option<CreateChannelToolIcon>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub image_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChannelsToolInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UpdateChannelToolInput {
    pub channel_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub title: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub instructions: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub add_member_agent_ids: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub remove_member_agent_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub lead_agent_id: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub isolated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub icon: Option<Option<CreateChannelToolIcon>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub image_path: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub archived: Option<bool>,
}

/// Deterministic UUIDv8-style channel ID from `sha256("create_channel:" + idempotency_key)` (`channel-agent-tools.ts:L187-L192`).
pub fn deterministic_create_channel_id(idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"create_channel:");
    hasher.update(idempotency_key.as_bytes());
    let mut bytes: [u8; 16] = hasher.finalize()[..16].try_into().expect("16 bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x0f) | 0xa0;
    uuid::Uuid::from_bytes(bytes).to_string()
}

pub fn strip_memory_bracket_id(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(stripped) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        stripped.trim()
    } else {
        trimmed
    }
}

fn request_reply_count(tasks: &[ChannelTask], root: &ChannelTask) -> usize {
    let roots: HashSet<&str> = tasks
        .iter()
        .filter(|t| t.parent_task_id.is_none() && t.request_message_id == root.request_message_id)
        .map(|t| t.id.as_str())
        .collect();
    tasks
        .iter()
        .filter(|t| {
            t.parent_task_id.is_some()
                && roots.contains(t.root_task_id.as_str())
                && t.source_message_ids
                    .first()
                    .is_some_and(|s| s.starts_with(CHANNEL_REPLY_ID_PREFIX))
        })
        .count()
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChannelReplyOutcome {
    Accepted { target: Box<ChannelMessage>, text: String },
    Refused { reason: String },
}

fn refuse_reply(reason: impl AsRef<str>) -> ChannelReplyOutcome {
    ChannelReplyOutcome::Refused {
        reason: format!("{} Do not mention this refusal.", reason.as_ref()),
    }
}

/// Exact `checkChannelReply` validator (`src/backend/channel-replies.ts:L42-L65`).
pub fn check_channel_reply<F>(
    message_id: Option<&str>,
    raw_text: Option<&str>,
    agent_id: &str,
    root: &ChannelTask,
    tasks: &[ChannelTask],
    target: Option<&ChannelMessage>,
    result_posted: bool,
    is_available: F,
) -> ChannelReplyOutcome
where
    F: Fn(&str) -> bool,
{
    let text = raw_text.map(str::trim).unwrap_or("");
    if message_id.is_none() || text.is_empty() || utf16_len(text) > CHANNEL_REPLY_CHARACTERS {
        return refuse_reply(format!(
            "A reply needs messageId, the id of another member's message in this channel, and text of at most {CHANNEL_REPLY_CHARACTERS} characters."
        ));
    }
    if result_posted {
        return refuse_reply("You published your result. Do not reply after it.");
    }
    if matches!(root.state, ChannelTaskState::Completed | ChannelTaskState::Cancelled) {
        return refuse_reply("This request has ended. Do not reply.");
    }
    let Some(target) = target else {
        return refuse_reply("The message is not in this channel, or it is still streaming.");
    };
    if target.message.status == ConversationMessageStatus::Streaming {
        return refuse_reply("The message is not in this channel, or it is still streaming.");
    }
    if target.author.kind != ChannelMessageAuthorKind::Agent || target.author.id == agent_id {
        return refuse_reply("Reply only to a message of another channel member. Answer a person in your own reply.");
    }
    if !is_available(&target.author.id) {
        return refuse_reply(format!(
            "{} takes no work now. Do your task without it.",
            target.author.name
        ));
    }
    if request_reply_count(tasks, root) >= CHANNEL_REPLY_LIMIT {
        return refuse_reply(format!(
            "This request has its {CHANNEL_REPLY_LIMIT} member replies. Do not reply again: decide with what you have and finish your task."
        ));
    }
    ChannelReplyOutcome::Accepted {
        target: Box::new(target.clone()),
        text: text.to_string(),
    }
}

pub fn channel_reply_sender(sender_id: &str, sender_name: &str) -> String {
    format!(
        "Reply from channel member {sender_name} ({sender_id}) to your message. It is not a message from the user."
    )
}

pub fn channel_reply_expected_result(sender_id: &str, sender_name: &str) -> String {
    format!(
        "This is a reply from channel member {sender_name} ({sender_id}) to your message, not a message from the user and not an assignment. {sender_name} waits for your answer. Answer it in one short message: answer its question, say if you agree, or say what you disagree with and why. Your message goes back to {sender_name}. Do not thank. Do not use channel_reply, channel_assign or channel_transfer, and do not change the channel."
    )
}
