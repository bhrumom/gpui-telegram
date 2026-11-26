//! Dynamic Island contracts: preferences, geometry (`192x32` compact notch),
//! 7 `DynamicIslandPresentation` modes (priority `0..=6`), `interaction_locks_presentation`
//! critical prompt/approval lock, and 11 `DynamicIslandAction` variants.
//! Mirrors `packages/contracts/src/ipc-dynamic-island.ts`,
//! `packages/team-client/src/dynamic-island-presentation.ts`, and `DynamicIslandSurface.tsx`.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

use crate::avatar::{AvatarHue, is_avatar_color};
use crate::conversation::AgentPromptOption;
use crate::limits::{
    INPUT_LIMITS, RequestId, double_option, is_bounded_string, is_identifier, non_null_option,
    utf16_len,
};

pub const DEFAULT_NOTCH_WIDTH: u32 = 192;
pub const DYNAMIC_ISLAND_DEFAULT_COMPACT_HEIGHT: u32 = 32;
pub const DYNAMIC_ISLAND_SERVER_CHOICES_LIMIT: usize = 256;
pub const DYNAMIC_ISLAND_MAX_COUNT: u32 = 10_000;
pub const DYNAMIC_ISLAND_WORKING_LIMIT: usize = 3;

pub const WIDTH_PERCENT_MIN: u32 = 20;
pub const WIDTH_PERCENT_MAX: u32 = 130;
pub const HEIGHT_PERCENT_MIN: u32 = 75;
pub const HEIGHT_PERCENT_MAX: u32 = 125;
pub const SIZE_PERCENT_STEP: u32 = 5;

pub fn dynamic_island_compact_height(notch_height: Option<u32>, height_percent: u32) -> u32 {
    let base = f64::from(notch_height.unwrap_or(DYNAMIC_ISLAND_DEFAULT_COMPACT_HEIGHT));
    let scaled = ((base * f64::from(height_percent)) / 100.0).round() as u32;
    notch_height.map_or(scaled, |nh| nh.max(scaled))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandPreference {
    pub enabled: bool,
    pub haptics_enabled: bool,
    pub idle_visible: bool,
    pub additional_displays_enabled: bool,
    pub width_percent: u32,
    pub height_percent: u32,
    pub server_visibility: BTreeMap<String, bool>,
    pub new_servers_visible: bool,
}

impl Default for DynamicIslandPreference {
    fn default() -> Self {
        Self {
            enabled: true,
            haptics_enabled: true,
            idle_visible: true,
            additional_displays_enabled: true,
            width_percent: 100,
            height_percent: 100,
            server_visibility: BTreeMap::new(),
            new_servers_visible: true,
        }
    }
}

impl DynamicIslandPreference {
    pub fn is_valid(&self) -> bool {
        (WIDTH_PERCENT_MIN..=WIDTH_PERCENT_MAX).contains(&self.width_percent)
            && self.width_percent.is_multiple_of(SIZE_PERCENT_STEP)
            && (HEIGHT_PERCENT_MIN..=HEIGHT_PERCENT_MAX).contains(&self.height_percent)
            && self.height_percent.is_multiple_of(SIZE_PERCENT_STEP)
            && self.server_visibility.len() <= DYNAMIC_ISLAND_SERVER_CHOICES_LIMIT
            && self.server_visibility.keys().all(|k| is_identifier(k))
    }

    pub fn is_server_visible(&self, server_id: &str) -> bool {
        self.server_visibility
            .get(server_id)
            .copied()
            .unwrap_or(self.new_servers_visible)
    }
}

pub fn is_dynamic_island_server_visible(pref: &DynamicIslandPreference, server_id: &str) -> bool {
    pref.is_server_visible(server_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DynamicIslandViewState {
    Compact,
    Expanded,
    CriticalPromptLock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DynamicIslandMode {
    Question,
    Approval,
    Takeover,
    Failed,
    Working,
    Message,
    Idle,
}

impl DynamicIslandMode {
    pub const ALL: [Self; 7] = [
        Self::Question,
        Self::Approval,
        Self::Takeover,
        Self::Failed,
        Self::Working,
        Self::Message,
        Self::Idle,
    ];

    /// Priority `0..=6` (`0` = highest urgency `Question`, `6` = lowest `Idle`).
    pub const fn priority(self) -> u8 {
        match self {
            Self::Question => 0,
            Self::Approval => 1,
            Self::Takeover => 2,
            Self::Failed => 3,
            Self::Working => 4,
            Self::Message => 5,
            Self::Idle => 6,
        }
    }

    pub const fn is_critical(self) -> bool {
        self.priority() <= 3
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Approval => "approval",
            Self::Takeover => "takeover",
            Self::Failed => "failed",
            Self::Working => "working",
            Self::Message => "message",
            Self::Idle => "idle",
        }
    }
}

fn deserialize_required_nullable_hue<'de, D>(deserializer: D) -> Result<Option<AvatarHue>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<AvatarHue>::deserialize(deserializer)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandAgentIdentity {
    pub id: String,
    pub name: String,
    pub avatar_seed: String,
    #[serde(deserialize_with = "deserialize_required_nullable_hue")]
    pub avatar_hue: Option<AvatarHue>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub avatar_color: Option<Option<String>>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub avatar_url: Option<String>,
}

#[inline]
fn is_short_string(s: &str, max: usize) -> bool {
    !s.is_empty() && is_bounded_string(s, max)
}

#[inline]
fn is_nullable_short_string(s: Option<&str>, max: usize) -> bool {
    s.is_none_or(|v| is_short_string(v, max))
}

#[inline]
fn is_short_string_list(list: &[String]) -> bool {
    list.len() <= 3 && list.iter().all(|item| is_short_string(item, 600))
}

#[inline]
fn is_dynamic_island_request_id(req: &RequestId) -> bool {
    match req {
        RequestId::String(s) => is_short_string(s, 160),
        RequestId::Number(n) => (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(n),
    }
}

impl DynamicIslandAgentIdentity {
    pub fn is_valid(&self) -> bool {
        is_short_string(&self.id, 160)
            && is_short_string(&self.name, 120)
            && is_short_string(&self.avatar_seed, 160)
            && self
                .avatar_color
                .as_ref()
                .is_none_or(|opt| opt.as_deref().is_none_or(is_avatar_color))
            && self
                .avatar_url
                .as_deref()
                .is_none_or(|u| is_short_string(u, INPUT_LIMITS.avatar_url))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandWorkingItem {
    pub agent: DynamicIslandAgentIdentity,
    pub task: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandMessageItem {
    pub agent: DynamicIslandAgentIdentity,
    pub message_id: String,
    pub text: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandQuestionItem {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub detail: Option<String>,
    pub is_secret: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub options: Option<Vec<AgentPromptOption>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandPromptItem {
    pub request_id: RequestId,
    pub agent: DynamicIslandAgentIdentity,
    pub title: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub detail: Option<String>,
    pub questions: Vec<DynamicIslandQuestionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicIslandFileSystemPermissions {
    pub read: Vec<String>,
    pub write: Vec<String>,
}

impl DynamicIslandFileSystemPermissions {
    pub fn is_valid(&self) -> bool {
        is_short_string_list(&self.read) && is_short_string_list(&self.write)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandApprovalPermissions {
    pub network: bool,
    pub file_system: DynamicIslandFileSystemPermissions,
}

impl DynamicIslandApprovalPermissions {
    pub fn is_valid(&self) -> bool {
        self.file_system.is_valid()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandApprovalDetails {
    pub kind: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub command: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub cwd: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub reason: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub grant_root: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub permissions: Option<DynamicIslandApprovalPermissions>,
}

impl DynamicIslandApprovalDetails {
    pub fn is_valid(&self) -> bool {
        matches!(self.kind.as_str(), "command" | "file-change" | "permissions")
            && is_nullable_short_string(self.command.as_deref(), 600)
            && is_nullable_short_string(self.cwd.as_deref(), 600)
            && is_nullable_short_string(self.reason.as_deref(), 600)
            && is_nullable_short_string(self.grant_root.as_deref(), 600)
            && self
                .permissions
                .as_ref()
                .is_none_or(DynamicIslandApprovalPermissions::is_valid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandApprovalItem {
    pub request_id: RequestId,
    pub agent: DynamicIslandAgentIdentity,
    pub title: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub detail: Option<String>,
    pub truncated: bool,
    pub approval: DynamicIslandApprovalDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandTakeoverItem {
    pub request_id: RequestId,
    pub agent: DynamicIslandAgentIdentity,
    pub title: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandFailureItem {
    pub turn_id: String,
    pub agent: DynamicIslandAgentIdentity,
    pub title: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub detail: Option<String>,
}

/// All 7 `DynamicIslandPresentation` modes (`ipc-dynamic-island.ts:L128-L144`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum DynamicIslandPresentation {
    Idle {
        #[serde(rename = "serverId")]
        server_id: String,
    },
    Working {
        #[serde(rename = "serverId")]
        server_id: String,
        working: Vec<DynamicIslandWorkingItem>,
    },
    Message {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "unreadCount")]
        unread_count: u32,
        message: DynamicIslandMessageItem,
    },
    Question {
        #[serde(rename = "serverId")]
        server_id: String,
        item: DynamicIslandPromptItem,
        #[serde(rename = "remainingCount")]
        remaining_count: u32,
    },
    Approval {
        #[serde(rename = "serverId")]
        server_id: String,
        item: DynamicIslandApprovalItem,
        #[serde(rename = "remainingCount")]
        remaining_count: u32,
    },
    Takeover {
        #[serde(rename = "serverId")]
        server_id: String,
        item: DynamicIslandTakeoverItem,
    },
    Failed {
        #[serde(rename = "serverId")]
        server_id: String,
        item: DynamicIslandFailureItem,
    },
}

impl DynamicIslandPresentation {
    pub fn idle_local() -> Self {
        Self::Idle {
            server_id: "local".to_string(),
        }
    }

    pub const fn mode(&self) -> DynamicIslandMode {
        match self {
            Self::Idle { .. } => DynamicIslandMode::Idle,
            Self::Working { .. } => DynamicIslandMode::Working,
            Self::Message { .. } => DynamicIslandMode::Message,
            Self::Question { .. } => DynamicIslandMode::Question,
            Self::Approval { .. } => DynamicIslandMode::Approval,
            Self::Takeover { .. } => DynamicIslandMode::Takeover,
            Self::Failed { .. } => DynamicIslandMode::Failed,
        }
    }

    pub fn server_id(&self) -> &str {
        match self {
            Self::Idle { server_id }
            | Self::Working { server_id, .. }
            | Self::Message { server_id, .. }
            | Self::Question { server_id, .. }
            | Self::Approval { server_id, .. }
            | Self::Takeover { server_id, .. }
            | Self::Failed { server_id, .. } => server_id,
        }
    }

    pub fn is_valid(&self) -> bool {
        let sid = self.server_id();
        if !is_short_string(sid, 160) {
            return false;
        }
        match self {
            Self::Working { working, .. } => {
                working.len() <= DYNAMIC_ISLAND_WORKING_LIMIT
                    && working.iter().all(|w| {
                        w.agent.is_valid()
                            && is_short_string(&w.task, 240)
                            && is_nullable_short_string(w.turn_id.as_deref(), 160)
                    })
            }
            Self::Message { unread_count, message, .. } => {
                *unread_count <= DYNAMIC_ISLAND_MAX_COUNT
                    && message.agent.is_valid()
                    && is_short_string(&message.message_id, 160)
                    && is_short_string(&message.text, 600)
                    && is_short_string(&message.created_at, 80)
            }
            Self::Question { item, remaining_count, .. } => {
                *remaining_count <= DYNAMIC_ISLAND_MAX_COUNT
                    && is_dynamic_island_request_id(&item.request_id)
                    && item.agent.is_valid()
                    && is_short_string(&item.title, 180)
                    && is_nullable_short_string(item.detail.as_deref(), 600)
                    && item.questions.len() <= INPUT_LIMITS.prompt_questions
                    && item.questions.iter().all(|q| {
                        is_short_string(&q.id, INPUT_LIMITS.identifier)
                            && is_short_string(&q.header, INPUT_LIMITS.prompt_header)
                            && is_short_string(&q.question, INPUT_LIMITS.prompt_question)
                            && q.detail
                                .as_deref()
                                .is_none_or(|d| is_short_string(d, INPUT_LIMITS.prompt_question))
                            && q.options.as_deref().is_none_or(|opts| {
                                opts.len() <= INPUT_LIMITS.prompt_options
                                    && opts.iter().all(|o| {
                                        is_short_string(&o.label, INPUT_LIMITS.prompt_option_label)
                                            && is_short_string(
                                                &o.description,
                                                INPUT_LIMITS.prompt_option_description,
                                            )
                                    })
                            })
                    })
            }
            Self::Approval { item, remaining_count, .. } => {
                *remaining_count <= DYNAMIC_ISLAND_MAX_COUNT
                    && is_dynamic_island_request_id(&item.request_id)
                    && item.agent.is_valid()
                    && is_short_string(&item.title, 180)
                    && is_nullable_short_string(item.detail.as_deref(), 600)
                    && item.approval.is_valid()
            }
            Self::Takeover { item, .. } => {
                is_dynamic_island_request_id(&item.request_id)
                    && item.agent.is_valid()
                    && is_short_string(&item.title, 180)
                    && is_nullable_short_string(item.detail.as_deref(), 600)
            }
            Self::Failed { item, .. } => {
                is_short_string(&item.turn_id, 160)
                    && item.agent.is_valid()
                    && is_short_string(&item.title, 180)
                    && is_nullable_short_string(item.detail.as_deref(), 600)
            }
            Self::Idle { .. } => true,
        }
    }
}

pub const fn is_critical_presentation(p: &DynamicIslandPresentation) -> bool {
    p.mode().is_critical()
}

pub const fn held_request_ended(
    current: &DynamicIslandPresentation,
    next: &DynamicIslandPresentation,
) -> bool {
    next.mode().priority() > current.mode().priority()
}

pub fn presentation_identity(p: &DynamicIslandPresentation) -> String {
    let sid = p.server_id();
    let mode = p.mode().as_str();
    match p {
        DynamicIslandPresentation::Question { item, .. } => {
            format!("{sid}:{mode}:{}", item.request_id.display_key())
        }
        DynamicIslandPresentation::Approval { item, .. } => {
            format!("{sid}:{mode}:{}", item.request_id.display_key())
        }
        DynamicIslandPresentation::Takeover { item, .. } => {
            format!("{sid}:{mode}:{}", item.request_id.display_key())
        }
        DynamicIslandPresentation::Failed { item, .. } => format!("{sid}:failed:{}", item.turn_id),
        DynamicIslandPresentation::Message { message, .. } => {
            format!("{sid}:message:{}", message.message_id)
        }
        DynamicIslandPresentation::Working { .. } | DynamicIslandPresentation::Idle { .. } => {
            format!("{sid}:{mode}")
        }
    }
}

/// Exact critical prompt/approval lock (`DynamicIslandSurface.tsx:L348-L383`):
/// holds critical item during user interaction, unlocking immediately once resolved (`held_request_ended`).
pub fn interaction_locks_presentation(
    current: &DynamicIslandPresentation,
    next: &DynamicIslandPresentation,
    interacting: bool,
    typing: bool,
) -> bool {
    if !interacting || !(typing || is_critical_presentation(current)) {
        return false;
    }
    if is_critical_presentation(current) && held_request_ended(current, next) {
        return false;
    }
    presentation_identity(current) != presentation_identity(next)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Accept,
    Decline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DynamicIslandAction {
    OpenApp,
    OpenAgent {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    OpenMessage {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "messageId")]
        message_id: String,
    },
    SendMessage {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        text: String,
        #[serde(rename = "clientMessageId")]
        client_message_id: String,
    },
    StopAgent {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
    },
    OpenFailure {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
    },
    DismissFailure {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
    },
    ReviewAttention {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "requestId")]
        request_id: RequestId,
    },
    OpenBrowserTakeover {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "tabId")]
        tab_id: String,
    },
    AnswerPrompt {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "requestId")]
        request_id: RequestId,
        answers: BTreeMap<String, Vec<String>>,
    },
    RespondApproval {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "requestId")]
        request_id: RequestId,
        decision: ApprovalDecision,
    },
}

impl DynamicIslandAction {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::OpenApp => true,
            Self::OpenAgent { server_id, agent_id } => {
                is_short_string(server_id, 160) && is_short_string(agent_id, 160)
            }
            Self::OpenMessage {
                server_id,
                agent_id,
                message_id,
            } => {
                is_short_string(server_id, 160)
                    && is_short_string(agent_id, 160)
                    && is_short_string(message_id, 160)
            }
            Self::SendMessage {
                server_id,
                agent_id,
                text,
                client_message_id,
            } => {
                is_short_string(server_id, 160)
                    && is_short_string(agent_id, 160)
                    && is_short_string(text, INPUT_LIMITS.direct_message_text)
                    && !text.trim().is_empty()
                    && is_short_string(client_message_id, INPUT_LIMITS.identifier)
            }
            Self::StopAgent {
                server_id,
                agent_id,
                turn_id,
            }
            | Self::OpenFailure {
                server_id,
                agent_id,
                turn_id,
            }
            | Self::DismissFailure {
                server_id,
                agent_id,
                turn_id,
            } => {
                is_short_string(server_id, 160)
                    && is_short_string(agent_id, 160)
                    && is_short_string(turn_id, 160)
            }
            Self::ReviewAttention {
                server_id,
                agent_id,
                request_id,
            }
            | Self::RespondApproval {
                server_id,
                agent_id,
                request_id,
                ..
            } => {
                is_short_string(server_id, 160)
                    && is_short_string(agent_id, 160)
                    && is_dynamic_island_request_id(request_id)
            }
            Self::OpenBrowserTakeover {
                server_id,
                agent_id,
                tab_id,
            } => {
                is_short_string(server_id, 160)
                    && is_short_string(agent_id, 160)
                    && is_short_string(tab_id, 160)
            }
            Self::AnswerPrompt {
                server_id,
                agent_id,
                request_id,
                answers,
            } => {
                if !is_short_string(server_id, 160)
                    || !is_short_string(agent_id, 160)
                    || !is_dynamic_island_request_id(request_id)
                    || answers.is_empty()
                    || answers.len() > INPUT_LIMITS.prompt_questions
                {
                    return false;
                }
                let mut total_len = 0usize;
                for (qid, ans_list) in answers {
                    if !is_short_string(qid, INPUT_LIMITS.identifier) || ans_list.len() != 1 {
                        return false;
                    }
                    let ans = &ans_list[0];
                    if !is_short_string(ans, INPUT_LIMITS.direct_message_text) {
                        return false;
                    }
                    total_len += utf16_len(ans);
                }
                total_len <= INPUT_LIMITS.prompt_answers_total_text
            }
        }
    }
}

fn deserialize_positive_finite<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let val = f64::deserialize(deserializer)?;
    if !val.is_finite() || val <= 0.0 {
        return Err(serde::de::Error::custom(
            "DynamicIslandNotchSize dimensions must be positive finite numbers",
        ));
    }
    Ok(val)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DynamicIslandNotchSize {
    #[serde(deserialize_with = "deserialize_positive_finite")]
    pub width: f64,
    #[serde(deserialize_with = "deserialize_positive_finite")]
    pub height: f64,
}

impl DynamicIslandNotchSize {
    pub fn is_valid(&self) -> bool {
        self.width.is_finite() && self.width > 0.0 && self.height.is_finite() && self.height > 0.0
    }
}

pub type DynamicIslandGeometry = Option<DynamicIslandNotchSize>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetDynamicIslandInteractiveInput {
    pub interactive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub keyboard: Option<bool>,
}
