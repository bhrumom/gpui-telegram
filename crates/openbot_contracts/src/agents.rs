//! Agent lifecycle, 39-field `AgentSummary`, `UpdateAgentInput` tri-state optionals,
//! `AgentBackupModels` (`<= 8`), session overrides, profile drafts, welcome prompt, and side-thread tasks.
//! Mirrors `packages/contracts/src/{ipc-agents,ipc-agent-identity,ipc-agent-profile,agent-welcome,ipc-agent-session-settings,ipc-agent-tasks,ipc-agent-folders,ipc-hidden-agents}.ts`.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize};

use crate::avatar::{AvatarHue, AvatarMimeType, is_avatar_color, is_avatar_seed};
use crate::limits::{
    ContractError, INPUT_LIMITS, double_option, forbid_field, is_bounded_string, is_identifier,
    is_iso_timestamp, is_uuid_v4, non_null_option,
};
pub use crate::providers::{
    AgentProviderId, AgentProviderRef, CustomProviderEndpoint, ForeignProviderId, is_agent_model,
};
use crate::queue::QueueDelivery;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentReasoningEffort {
    None,
    Minimal,
    #[default]
    Low,
    Medium,
    High,
    XHigh,
    Max,
    Ultra,
}

impl AgentReasoningEffort {
    pub const ALL: [Self; 8] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
        Self::Ultra,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|e| e.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LegacyAgentReasoningEffort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

pub const fn legacy_reasoning_effort(effort: AgentReasoningEffort) -> LegacyAgentReasoningEffort {
    match effort {
        AgentReasoningEffort::None | AgentReasoningEffort::Minimal | AgentReasoningEffort::Low => {
            LegacyAgentReasoningEffort::Low
        }
        AgentReasoningEffort::Medium => LegacyAgentReasoningEffort::Medium,
        AgentReasoningEffort::High => LegacyAgentReasoningEffort::High,
        AgentReasoningEffort::XHigh => LegacyAgentReasoningEffort::XHigh,
        AgentReasoningEffort::Max | AgentReasoningEffort::Ultra => LegacyAgentReasoningEffort::Max,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentModelListing {
    New,
    Older,
    Kept,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelOptionRecord<P> {
    pub provider: P,
    pub id: String,
    pub name: String,
    pub description: String,
    pub default_reasoning_effort: AgentReasoningEffort,
    pub supported_reasoning_efforts: Vec<AgentReasoningEffort>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub reasoning_effort_configurable: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub fast_service_tier: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub ultrafast_service_tier: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub listing: Option<AgentModelListing>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub steers_turns: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub workspace_only_unavailable: Option<bool>,
}

impl<P> AgentModelOptionRecord<P> {
    pub fn is_valid(&self) -> bool {
        is_agent_model(&self.id)
            && is_bounded_string(&self.name, INPUT_LIMITS.model_name)
            && is_bounded_string(&self.description, INPUT_LIMITS.agent_description)
            && self
                .fast_service_tier
                .as_deref()
                .is_none_or(|tier| is_bounded_string(tier, INPUT_LIMITS.service_tier))
            && self
                .ultrafast_service_tier
                .as_deref()
                .is_none_or(|tier| is_bounded_string(tier, INPUT_LIMITS.service_tier))
    }
}

pub type AgentModelOption = AgentModelOptionRecord<AgentProviderId>;
pub type ClientAgentModelOption = AgentModelOptionRecord<AgentProviderRef>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentAccess {
    #[default]
    Full,
    Workspace,
}

pub const DEFAULT_AGENT_ACCESS: AgentAccess = AgentAccess::Full;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BusyMessageMode {
    Queue,
    #[default]
    Steer,
}

pub const DEFAULT_BUSY_MESSAGE_MODE: BusyMessageMode = BusyMessageMode::Steer;

pub const OPENBOT_COMMIT_TRAILER: &str =
    "Co-authored-by: OpenBot <335604065+openbotgit[bot]@users.noreply.github.com>";
pub const DEFAULT_PLAN_START_AFTER_MS: u64 = 10_000;
pub const MIN_PLAN_START_AFTER_MS: u64 = 5_000;
pub const MAX_PLAN_START_AFTER_MS: u64 = 2_147_483_647;
pub const DEFAULT_AUTOMATION_RUNS_PER_HOUR: u32 = 30;
pub const MIN_AUTOMATION_RUNS_PER_HOUR: u32 = 1;
pub const MAX_AUTOMATION_RUNS_PER_HOUR: u32 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefaults {
    pub allow_automation: bool,
    pub allow_new_chat: bool,
    pub quick_acknowledgement: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub plan_start_after_ms: Option<u64>,
    pub automation_runs_per_hour: u32,
    pub commit_co_author: bool,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        DEFAULT_AGENT_DEFAULTS
    }
}

pub const DEFAULT_AGENT_DEFAULTS: AgentDefaults = AgentDefaults {
    allow_automation: false,
    allow_new_chat: false,
    quick_acknowledgement: false,
    plan_start_after_ms: Some(DEFAULT_PLAN_START_AFTER_MS),
    automation_runs_per_hour: DEFAULT_AUTOMATION_RUNS_PER_HOUR,
    commit_co_author: true,
};

pub fn is_plan_start_after_ms(value: Option<u64>) -> bool {
    value.is_none_or(|ms| {
        ms.is_multiple_of(1_000) && (MIN_PLAN_START_AFTER_MS..=MAX_PLAN_START_AFTER_MS).contains(&ms)
    })
}

pub fn is_automation_runs_per_hour(value: u32) -> bool {
    (MIN_AUTOMATION_RUNS_PER_HOUR..=MAX_AUTOMATION_RUNS_PER_HOUR).contains(&value)
}

impl AgentDefaults {
    pub fn is_valid(&self) -> bool {
        is_plan_start_after_ms(self.plan_start_after_ms)
            && is_automation_runs_per_hour(self.automation_runs_per_hour)
    }
}

pub const DEFAULT_PROVIDER_ACCOUNT_ID: &str = "default";
pub const AGENT_BACKUP_MODEL_LIMIT: usize = 8;
pub const AGENT_TASK_LIMIT_MAX: u32 = 4;
pub const AGENT_TASK_LIMIT_EXTENDED_MAX: u32 = 8;
pub const AGENT_TASK_LIMIT_DEFAULT: u32 = 4;
pub const HOST_AGENT_TASK_LIMIT_MAX: u32 = 64;
pub const AGENT_FOLDER_LIMIT: usize = 32;
pub const DEFAULT_AGENT_PREVIEW: &str = "No messages yet";

pub fn is_provider_account_id(value: &str) -> bool {
    value == DEFAULT_PROVIDER_ACCOUNT_ID
        || (value.len() == 17
            && value.starts_with('a')
            && value.as_bytes()[1..]
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)))
}

pub fn is_strict_agent_task_limit(value: u32) -> bool {
    (1..=AGENT_TASK_LIMIT_MAX).contains(&value)
}

pub fn is_agent_task_limit(value: u32) -> bool {
    (1..=AGENT_TASK_LIMIT_EXTENDED_MAX).contains(&value)
}

pub fn is_host_agent_task_limit(value: u32) -> bool {
    (1..=HOST_AGENT_TASK_LIMIT_MAX).contains(&value)
}

pub fn agent_task_limit(
    provider: AgentProviderId,
    task_limit: Option<u32>,
    host_default: u32,
) -> u32 {
    if !provider.descriptor().concurrent_turns {
        return 1;
    }
    task_limit.unwrap_or(host_default)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelChoice {
    pub provider: AgentProviderId,
    pub model: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub provider_account_id: Option<String>,
}

/// Alias matching `PROJECT.md § Interface Contracts § 1` (`BackupModelSpec`).
pub type BackupModelSpec = AgentModelChoice;

impl AgentModelChoice {
    pub fn is_valid(&self) -> bool {
        is_agent_model(&self.model)
            && self
                .provider_account_id
                .as_deref()
                .is_none_or(is_provider_account_id)
    }
}

pub fn same_agent_model_choice(left: &AgentModelChoice, right: &AgentModelChoice) -> bool {
    left.provider == right.provider
        && left.model == right.model
        && left
            .provider_account_id
            .as_deref()
            .unwrap_or(DEFAULT_PROVIDER_ACCOUNT_ID)
            == right
                .provider_account_id
                .as_deref()
                .unwrap_or(DEFAULT_PROVIDER_ACCOUNT_ID)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackupModels {
    pub automatic: bool,
    pub order: Vec<AgentModelChoice>,
}

impl AgentBackupModels {
    pub fn is_valid(&self) -> bool {
        self.order.len() <= AGENT_BACKUP_MODEL_LIMIT
            && self.order.iter().all(AgentModelChoice::is_valid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageLimitReturn {
    pub provider: AgentProviderId,
    pub model: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub provider_account_id: Option<String>,
    pub reasoning_effort: AgentReasoningEffort,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub cause: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub fast_mode: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub ultrafast: Option<bool>,
}

impl AgentUsageLimitReturn {
    pub fn is_valid(&self) -> bool {
        is_agent_model(&self.model)
            && self
                .provider_account_id
                .as_deref()
                .is_none_or(is_provider_account_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPendingRuntime {
    pub provider: AgentProviderId,
    pub model: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub provider_account_id: Option<String>,
    pub reasoning_effort: AgentReasoningEffort,
}

impl AgentPendingRuntime {
    pub fn is_valid(&self) -> bool {
        is_agent_model(&self.model)
            && self
                .provider_account_id
                .as_deref()
                .is_none_or(is_provider_account_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMarketplaceSource {
    pub listing_id: String,
    pub version_id: String,
    pub version: i64,
    pub skill_ids: Vec<String>,
    pub routine_ids: Vec<String>,
}

impl AgentMarketplaceSource {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.listing_id)
            && is_identifier(&self.version_id)
            && self.skill_ids.len() <= INPUT_LIMITS.agents
            && self.skill_ids.iter().all(|s| is_identifier(s))
            && self.routine_ids.len() <= INPUT_LIMITS.agent_routines
            && self.routine_ids.iter().all(|r| is_identifier(r))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentSessionSettingValue {
    Bool(bool),
    String(String),
}

pub type AgentSessionSettingOverrides =
    BTreeMap<String, BTreeMap<String, AgentSessionSettingValue>>;

pub fn is_agent_session_setting_overrides(overrides: &AgentSessionSettingOverrides) -> bool {
    overrides.len() <= 32
        && overrides.iter().all(|(identity, values)| {
            !identity.is_empty()
                && is_bounded_string(identity, 160)
                && values.len() <= 64
                && values.iter().all(|(key, entry)| {
                    !key.is_empty()
                        && is_bounded_string(key, 256)
                        && match entry {
                            AgentSessionSettingValue::Bool(_) => true,
                            AgentSessionSettingValue::String(s) => is_bounded_string(s, 4_096),
                        }
                })
        })
}

pub fn is_agent_session_setting_resets(resets: &[String]) -> bool {
    resets.len() <= 32
        && resets
            .iter()
            .all(|id| !id.is_empty() && is_bounded_string(id, 160))
}

pub fn is_agent_additional_folders(folders: &[String]) -> bool {
    folders.len() <= AGENT_FOLDER_LIMIT
        && folders
            .iter()
            .all(|p| !p.is_empty() && is_bounded_string(p, INPUT_LIMITS.path))
}

/// Matches `isBrowserProfileId` (`ipc-browser-profiles.ts:L8,L20-L22`): exact 32 lowercase hex digits.
pub fn is_browser_profile_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn folder_inside(root: &str, path: &str) -> bool {
    if path == root {
        return true;
    }
    let trimmed = root
        .strip_suffix('/')
        .or_else(|| root.strip_suffix('\\'))
        .unwrap_or(root);
    path.starts_with(trimmed)
        && path[trimmed.len()..]
            .as_bytes()
            .first()
            .is_some_and(|&b| b == b'/' || b == b'\\')
}

/// Complete 39-field OpenBot agent summary (`ipc-agents.ts:L218-L265`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummaryRecord<P> {
    pub id: String,
    pub name: String,
    pub title: String,
    pub description: String,
    pub notifications: bool,
    pub provider: P,
    pub model: String,
    pub reasoning_effort: AgentReasoningEffort,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub session_setting_overrides: Option<AgentSessionSettingOverrides>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub session_setting_resets: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub access: Option<AgentAccess>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub additional_folders: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub excluded_folders: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub computer_use: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub memory: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub allow_automation: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub allow_new_chat: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub quick_acknowledgement: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub own_browser_session: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub browser_profile_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub fast_mode: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub ultrafast: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub busy_message_mode: Option<BusyMessageMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub task_limit: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub provider_account_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub backup_models: Option<AgentBackupModels>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub usage_limit_return: Option<AgentUsageLimitReturn>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub pending_runtime: Option<AgentPendingRuntime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub pinned_account_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub hidden_at: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub thread_id: Option<String>,
    pub workspace_path: String,
    pub preview: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub updated_at: Option<String>,
    pub avatar_seed: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub avatar_hue: Option<AvatarHue>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub avatar_color: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub chat_color: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub avatar_url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub marketplace_source: Option<AgentMarketplaceSource>,
}

pub type AgentSummary = AgentSummaryRecord<AgentProviderId>;
pub type ClientAgentSummary = AgentSummaryRecord<AgentProviderRef>;

impl AgentSummary {
    pub fn new_default(id: impl Into<String>, name: impl Into<String>, workspace_path: impl Into<String>) -> Self {
        let id_str = id.into();
        let avatar_seed = id_str.to_ascii_lowercase();
        Self {
            id: id_str,
            name: name.into(),
            title: String::new(),
            description: String::new(),
            notifications: true,
            provider: AgentProviderId::Codex,
            model: "gpt-6-luna".to_string(),
            reasoning_effort: AgentReasoningEffort::Low,
            session_setting_overrides: None,
            session_setting_resets: None,
            access: None,
            additional_folders: None,
            excluded_folders: None,
            computer_use: None,
            memory: None,
            allow_automation: None,
            allow_new_chat: None,
            quick_acknowledgement: None,
            own_browser_session: None,
            browser_profile_id: None,
            fast_mode: None,
            ultrafast: None,
            busy_message_mode: None,
            task_limit: None,
            provider_account_id: None,
            backup_models: None,
            usage_limit_return: None,
            pending_runtime: None,
            pinned_account_id: None,
            hidden_at: None,
            thread_id: None,
            workspace_path: workspace_path.into(),
            preview: DEFAULT_AGENT_PREVIEW.to_string(),
            updated_at: None,
            avatar_seed,
            avatar_hue: None,
            avatar_color: None,
            chat_color: None,
            avatar_url: None,
            marketplace_source: None,
        }
    }
}

impl<P> AgentSummaryRecord<P> {
    pub fn computer_use_enabled(&self) -> bool {
        self.computer_use != Some(false)
    }

    pub fn memory_enabled(&self) -> bool {
        self.memory != Some(false)
    }

    pub fn validate(&self) -> bool {
        self.validate_with_task_limit_check(is_agent_task_limit)
    }

    pub fn validate_strict_wire(&self) -> bool {
        self.validate_with_task_limit_check(is_strict_agent_task_limit)
    }

    fn validate_with_task_limit_check(&self, check_limit: fn(u32) -> bool) -> bool {
        is_identifier(&self.id)
            && is_bounded_string(&self.name, INPUT_LIMITS.agent_name)
            && is_bounded_string(&self.title, INPUT_LIMITS.agent_title)
            && is_bounded_string(&self.description, INPUT_LIMITS.agent_description)
            && is_agent_model(&self.model)
            && self.session_setting_overrides.as_ref().is_none_or(is_agent_session_setting_overrides)
            && self.session_setting_resets.as_deref().is_none_or(is_agent_session_setting_resets)
            && self.additional_folders.as_deref().is_none_or(is_agent_additional_folders)
            && self.excluded_folders.as_deref().is_none_or(is_agent_additional_folders)
            && self.browser_profile_id.as_deref().is_none_or(is_browser_profile_id)
            && self.task_limit.is_none_or(check_limit)
            && self.provider_account_id.as_deref().is_none_or(is_provider_account_id)
            && self.backup_models.as_ref().is_none_or(AgentBackupModels::is_valid)
            && self.usage_limit_return.as_ref().is_none_or(AgentUsageLimitReturn::is_valid)
            && self.pending_runtime.as_ref().is_none_or(AgentPendingRuntime::is_valid)
            && self.pinned_account_id.as_deref().is_none_or(is_provider_account_id)
            && self.hidden_at.as_deref().is_none_or(|h| is_bounded_string(h, 64) && is_iso_timestamp(h))
            && self.thread_id.as_deref().is_none_or(is_identifier)
            && is_bounded_string(&self.workspace_path, INPUT_LIMITS.path)
            && is_bounded_string(&self.preview, INPUT_LIMITS.message_text)
            && self.updated_at.as_deref().is_none_or(|u| is_bounded_string(u, 160))
            && is_avatar_seed(&self.avatar_seed)
            && self.avatar_color.as_ref().is_none_or(|opt| opt.as_deref().is_none_or(is_avatar_color))
            && self.chat_color.as_deref().is_none_or(is_avatar_color)
            && self.avatar_url.as_deref().is_none_or(|u| is_bounded_string(u, INPUT_LIMITS.avatar_url))
            && self.marketplace_source.as_ref().is_none_or(AgentMarketplaceSource::is_valid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentInput {
    pub name: String,
    pub description: String,
    pub avatar_seed: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub avatar_hue: Option<AvatarHue>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub avatar_color: Option<Option<String>>,
    pub initial_message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub provider: Option<AgentProviderId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub reasoning_effort: Option<AgentReasoningEffort>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub provider_account_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub operation_id: Option<String>,
}

impl CreateAgentInput {
    pub fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
            && is_bounded_string(&self.name, INPUT_LIMITS.agent_name)
            && is_bounded_string(&self.description, INPUT_LIMITS.agent_description)
            && is_avatar_seed(&self.avatar_seed)
            && self
                .avatar_color
                .as_ref()
                .is_none_or(|opt| opt.as_deref().is_none_or(is_avatar_color))
            && !self.initial_message.trim().is_empty()
            && is_bounded_string(&self.initial_message, INPUT_LIMITS.message_text)
            && self.model.as_deref().is_none_or(is_agent_model)
            && self
                .provider_account_id
                .as_deref()
                .is_none_or(is_provider_account_id)
            && self
                .operation_id
                .as_deref()
                .is_none_or(|id| is_bounded_string(id, INPUT_LIMITS.identifier))
    }
}

/// Tri-state `UpdateAgentInput` (`ipc-agents.ts:L335-L361` & `agent-inputs.ts:L757-L877`):
/// `Option<Option<T>>` fields distinguish omitted (`None`) vs explicit `null` reset (`Some(None)`) vs value (`Some(Some(v))`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgentInput {
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProviderId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<AgentReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access: Option<AgentAccess>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computer_use: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<bool>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub allow_automation: Option<Option<bool>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub allow_new_chat: Option<Option<bool>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub quick_acknowledgement: Option<Option<bool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ultrafast: Option<bool>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub busy_message_mode: Option<Option<BusyMessageMode>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub task_limit: Option<Option<u32>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub provider_account_id: Option<Option<String>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub backup_models: Option<Option<AgentBackupModels>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leave_usage_limit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_seed: Option<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub avatar_hue: Option<Option<AvatarHue>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub avatar_color: Option<Option<String>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub chat_color: Option<Option<String>>,
}

impl<'de> Deserialize<'de> for UpdateAgentInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawUpdateAgentInput {
            agent_id: String,
            #[serde(default, deserialize_with = "non_null_option")]
            name: Option<String>,
            #[serde(default, deserialize_with = "non_null_option")]
            title: Option<String>,
            #[serde(default, deserialize_with = "non_null_option")]
            description: Option<String>,
            #[serde(default, deserialize_with = "non_null_option")]
            notifications: Option<bool>,
            #[serde(default, deserialize_with = "non_null_option")]
            provider: Option<AgentProviderId>,
            #[serde(default, deserialize_with = "non_null_option")]
            model: Option<String>,
            #[serde(default, deserialize_with = "non_null_option")]
            reasoning_effort: Option<AgentReasoningEffort>,
            #[serde(default, deserialize_with = "non_null_option")]
            access: Option<AgentAccess>,
            #[serde(default, deserialize_with = "non_null_option")]
            computer_use: Option<bool>,
            #[serde(default, deserialize_with = "non_null_option")]
            memory: Option<bool>,
            #[serde(default, with = "double_option")]
            allow_automation: Option<Option<bool>>,
            #[serde(default, with = "double_option")]
            allow_new_chat: Option<Option<bool>>,
            #[serde(default, with = "double_option")]
            quick_acknowledgement: Option<Option<bool>>,
            #[serde(default, deserialize_with = "non_null_option")]
            fast_mode: Option<bool>,
            #[serde(default, deserialize_with = "non_null_option")]
            ultrafast: Option<bool>,
            #[serde(default, with = "double_option")]
            busy_message_mode: Option<Option<BusyMessageMode>>,
            #[serde(default, with = "double_option")]
            task_limit: Option<Option<u32>>,
            #[serde(default, with = "double_option")]
            provider_account_id: Option<Option<String>>,
            #[serde(default, with = "double_option")]
            backup_models: Option<Option<AgentBackupModels>>,
            #[serde(default, deserialize_with = "non_null_option")]
            leave_usage_limit: Option<bool>,
            #[serde(default, deserialize_with = "non_null_option")]
            avatar_seed: Option<String>,
            #[serde(default, with = "double_option")]
            avatar_hue: Option<Option<AvatarHue>>,
            #[serde(default, with = "double_option")]
            avatar_color: Option<Option<String>>,
            #[serde(default, with = "double_option")]
            chat_color: Option<Option<String>>,
            #[serde(default, rename = "role", deserialize_with = "forbid_field")]
            _forbid_role: (),
        }

        let raw = RawUpdateAgentInput::deserialize(deserializer)?;
        Ok(Self {
            agent_id: raw.agent_id,
            name: raw.name,
            title: raw.title,
            description: raw.description,
            notifications: raw.notifications,
            provider: raw.provider,
            model: raw.model,
            reasoning_effort: raw.reasoning_effort,
            access: raw.access,
            computer_use: raw.computer_use,
            memory: raw.memory,
            allow_automation: raw.allow_automation,
            allow_new_chat: raw.allow_new_chat,
            quick_acknowledgement: raw.quick_acknowledgement,
            fast_mode: raw.fast_mode,
            ultrafast: raw.ultrafast,
            busy_message_mode: raw.busy_message_mode,
            task_limit: raw.task_limit,
            provider_account_id: raw.provider_account_id,
            backup_models: raw.backup_models,
            leave_usage_limit: raw.leave_usage_limit,
            avatar_seed: raw.avatar_seed,
            avatar_hue: raw.avatar_hue,
            avatar_color: raw.avatar_color,
            chat_color: raw.chat_color,
        })
    }
}

impl UpdateAgentInput {
    pub fn is_valid(&self) -> bool {
        !self.agent_id.trim().is_empty()
            && is_bounded_string(&self.agent_id, INPUT_LIMITS.identifier)
            && self
                .name
                .as_deref()
                .is_none_or(|n| is_bounded_string(n, INPUT_LIMITS.agent_name))
            && self
                .title
                .as_deref()
                .is_none_or(|t| is_bounded_string(t, INPUT_LIMITS.agent_title))
            && self
                .description
                .as_deref()
                .is_none_or(|d| is_bounded_string(d, INPUT_LIMITS.agent_description))
            && self.model.as_deref().is_none_or(is_agent_model)
            && self
                .task_limit
                .as_ref()
                .is_none_or(|opt| opt.is_none_or(is_strict_agent_task_limit))
            && self
                .provider_account_id
                .as_ref()
                .is_none_or(|opt| opt.as_deref().is_none_or(is_provider_account_id))
            && self
                .backup_models
                .as_ref()
                .is_none_or(|opt| opt.as_ref().is_none_or(AgentBackupModels::is_valid))
            && self.avatar_seed.as_deref().is_none_or(is_avatar_seed)
            && self
                .avatar_color
                .as_ref()
                .is_none_or(|opt| opt.as_deref().is_none_or(is_avatar_color))
            && self
                .chat_color
                .as_ref()
                .is_none_or(|opt| opt.as_deref().is_none_or(is_avatar_color))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarImageInput {
    pub mime_type: AvatarMimeType,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAgentAvatarInput {
    pub agent_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub image: Option<AvatarImageInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HiddenAgent {
    pub agent_id: String,
    pub hidden_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiddenAgentsSnapshot {
    pub agents: Vec<HiddenAgent>,
}

impl HiddenAgentsSnapshot {
    pub fn is_valid(&self) -> bool {
        if self.agents.len() > INPUT_LIMITS.agents {
            return false;
        }
        let mut seen = HashSet::new();
        self.agents.iter().all(|a| {
            is_identifier(&a.agent_id)
                && is_bounded_string(&a.hidden_at, 64)
                && is_iso_timestamp(&a.hidden_at)
                && seen.insert(a.agent_id.as_str())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileDraft {
    pub name: String,
    pub title: String,
    pub description: String,
    pub avatar_seed: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub avatar_hue: Option<AvatarHue>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub section_id: Option<String>,
}

impl AgentProfileDraft {
    pub fn is_bounded(&self) -> bool {
        is_bounded_string(&self.name, INPUT_LIMITS.agent_name)
            && is_bounded_string(&self.title, INPUT_LIMITS.agent_title)
            && is_bounded_string(&self.description, INPUT_LIMITS.agent_description)
            && is_avatar_seed(&self.avatar_seed)
            && self.section_id.as_deref().is_none_or(is_identifier)
    }

    pub fn is_valid(&self) -> bool {
        self.is_bounded() && !self.name.trim().is_empty() && !self.description.trim().is_empty()
    }

    pub fn normalized(&self) -> Result<Self, ContractError> {
        if !self.is_valid() {
            return Err(ContractError::InvalidValue(
                "The generated profile is invalid. Try revising your prompt.".to_string(),
            ));
        }
        Ok(Self {
            name: self.name.trim().to_string(),
            title: self.title.trim().to_string(),
            description: self.description.trim().to_string(),
            avatar_seed: self.avatar_seed.clone(),
            avatar_hue: self.avatar_hue,
            section_id: self.section_id.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateAgentProfileInput {
    pub prompt: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub agent_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub draft: Option<AgentProfileDraft>,
}

impl GenerateAgentProfileInput {
    pub fn validate(&self) -> Result<Self, ContractError> {
        if !is_bounded_string(&self.prompt, INPUT_LIMITS.message_text)
            || self.prompt.trim().is_empty()
            || self.agent_id.as_deref().is_some_and(|id| !is_identifier(id))
            || self.draft.as_ref().is_some_and(|d| !d.is_bounded())
        {
            return Err(ContractError::InvalidValue(
                "Provide a valid profile prompt and draft.".to_string(),
            ));
        }
        Ok(Self {
            prompt: self.prompt.trim().to_string(),
            agent_id: self.agent_id.clone(),
            draft: self.draft.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAgentProfileInput {
    pub operation_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub agent_id: Option<String>,
    pub draft: AgentProfileDraft,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub initial_message: Option<String>,
}

impl SaveAgentProfileInput {
    pub fn validate(&self) -> Result<Self, ContractError> {
        let missing_initial = self.agent_id.is_none()
            && self
                .initial_message
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty);
        if !is_uuid_v4(&self.operation_id)
            || !self.draft.is_valid()
            || self.agent_id.as_deref().is_some_and(|id| !is_identifier(id))
            || self
                .initial_message
                .as_deref()
                .is_some_and(|m| !is_bounded_string(m, INPUT_LIMITS.message_text))
            || missing_initial
        {
            return Err(ContractError::InvalidValue(
                "Provide a valid reviewed profile and initial message.".to_string(),
            ));
        }
        Ok(Self {
            operation_id: self.operation_id.clone(),
            agent_id: self.agent_id.clone(),
            draft: self.draft.normalized()?,
            initial_message: self.initial_message.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWelcomeInput {
    pub language: String,
    #[serde(default)]
    pub first_request_follows: bool,
}

/// Exact setup message builder (`packages/contracts/src/agent-welcome.ts:L6-L27`).
pub fn agent_welcome_message(input: &AgentWelcomeInput) -> String {
    let intro_1 = "OpenBot sent this setup message when the user created you. The user does not see it. Your profile is your role.";
    let intro_2 = format!(
        "Write in {}, unless your profile description is in another language; then write in that language.",
        input.language
    );
    if input.first_request_follows {
        return [
            intro_1,
            &intro_2,
            "Welcome the user in one warm, natural sentence that says who you are. Their first request comes in the next message.",
            "Do not call tools, start work, ask questions, or mention this message.",
        ]
        .join("\n");
    }
    [
        intro_1,
        &intro_2,
        "1. Ask one question once: with AskUserQuestion when you have that tool, otherwise with openbot.ask_user. Write no text before the call: the question is your welcome.",
        "   - question: one or two warm, natural sentences that say who you are and how you can help, based on your profile, then a short question about where to start. Do not list features or ask setup questions.",
        "   - header: one or two words, such as the first step.",
        "   - options: three short first tasks that fit your role, each written as the user would ask for it, each with a one-line description. Single choice. The user can also write their own answer.",
        "2. When the user answers, do what they chose or wrote. If the answer is empty, the user skipped it: write one short, friendly sentence that they can message you whenever they are ready. Do not mention the question, a skip or an expiry.",
        "Before the answer, do not call other tools, start work, or mention this message.",
    ]
    .join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentTaskState {
    Running,
    Queued,
    Idle,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentTaskHold {
    TaskLimit,
    AgentRestarting,
    UsageLimit,
    ServerBusy,
    AgentBusy,
    OneTurn,
    SignIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentTaskOutcome {
    Completed,
    Failed,
    Interrupted,
}

pub const AGENT_TASK_QUEUED_PREVIEW: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentTaskOrigin {
    User,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentTaskAwaitedState {
    Queued,
    Starting,
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskAwaitedReply {
    pub message_id: String,
    pub recipient_agent_id: String,
    pub status: AgentTaskAwaitedState,
}

impl AgentTaskAwaitedReply {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.message_id) && is_identifier(&self.recipient_agent_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskQueuedMessage {
    pub delivery_id: String,
    pub text: String,
    pub created_at: String,
}

impl AgentTaskQueuedMessage {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.delivery_id)
            && is_bounded_string(&self.text, AGENT_TASK_QUEUED_PREVIEW)
            && is_bounded_string(&self.created_at, 160)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskBranch {
    #[serde(deserialize_with = "Option::deserialize")]
    pub source_task_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub source_title: Option<String>,
    pub source_message_id: String,
    pub through_message_id: String,
}

impl AgentTaskBranch {
    pub fn is_valid(&self) -> bool {
        self.source_task_id.as_deref().is_none_or(is_identifier)
            && self
                .source_title
                .as_deref()
                .is_none_or(|t| is_bounded_string(t, INPUT_LIMITS.agent_task_title))
            && is_identifier(&self.source_message_id)
            && is_identifier(&self.through_message_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTask {
    pub id: String,
    pub agent_id: String,
    pub thread_id: String,
    pub origin: AgentTaskOrigin,
    pub title: String,
    pub state: AgentTaskState,
    #[serde(deserialize_with = "Option::deserialize")]
    pub outcome: Option<AgentTaskOutcome>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub hold: Option<AgentTaskHold>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub active_turn_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub progress: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub error: Option<String>,
    pub queued_messages: Vec<AgentTaskQueuedMessage>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub queue: Option<Vec<QueueDelivery>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub awaited_replies: Option<Vec<AgentTaskAwaitedReply>>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub closed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub branch: Option<Option<AgentTaskBranch>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub routine_id: Option<Option<String>>,
}

impl AgentTask {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.id)
            && is_identifier(&self.agent_id)
            && is_identifier(&self.thread_id)
            && is_bounded_string(&self.title, INPUT_LIMITS.agent_task_title)
            && self.active_turn_id.as_deref().is_none_or(is_identifier)
            && self
                .progress
                .as_deref()
                .is_none_or(|p| is_bounded_string(p, INPUT_LIMITS.agent_task_progress))
            && self
                .error
                .as_deref()
                .is_none_or(|e| is_bounded_string(e, INPUT_LIMITS.agent_task_error))
            && self
                .queued_messages
                .iter()
                .all(AgentTaskQueuedMessage::is_valid)
            && self
                .queue
                .as_ref()
                .is_none_or(|q| q.iter().all(QueueDelivery::is_valid))
            && self
                .awaited_replies
                .as_ref()
                .is_none_or(|r| r.iter().all(AgentTaskAwaitedReply::is_valid))
            && is_bounded_string(&self.created_at, 160)
            && is_bounded_string(&self.updated_at, 160)
            && self
                .closed_at
                .as_deref()
                .is_none_or(|c| is_bounded_string(c, 160))
            && self
                .branch
                .as_ref()
                .is_none_or(|opt| opt.as_ref().is_none_or(AgentTaskBranch::is_valid))
            && self
                .routine_id
                .as_ref()
                .is_none_or(|opt| opt.as_deref().is_none_or(is_identifier))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTasksSnapshot {
    pub agent_id: String,
    pub tasks: Vec<AgentTask>,
    pub limit: u32,
}

impl AgentTasksSnapshot {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.agent_id)
            && self.tasks.len() <= INPUT_LIMITS.agent_tasks
            && self.tasks.iter().all(AgentTask::is_valid)
            && is_host_agent_task_limit(self.limit)
    }
}
