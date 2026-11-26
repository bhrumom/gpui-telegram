//! All 18 `AgentProviderId`s, `ForeignProviderId`, `AgentProviderDescriptor` table,
//! custom provider endpoints & model ID codecs, custom ACP agents, and local server detection.
//! Mirrors `packages/contracts/src/{agent-providers,ipc-custom-providers,ipc-provider-detection,ipc-custom-agents,acp-agent-presets}.ts`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::limits::{INPUT_LIMITS, double_option, forbid_field, is_bounded_string, non_null_option};

pub use crate::agents::{
    AgentBackupModels, AgentModelChoice, AgentReasoningEffort, AgentSummary, BackupModelSpec,
    ClientAgentSummary, LegacyAgentReasoningEffort, legacy_reasoning_effort,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentProviderId {
    Codex,
    Claude,
    Grok,
    OpenCode,
    Antigravity,
    Cursor,
    Cline,
    Pi,
    Muse,
    Inception,
    DeepSeek,
    OpenRouter,
    Hermes,
    Acp,
    Kimi,
    Devin,
    Copilot,
    Mistral,
}

impl AgentProviderId {
    pub const ALL: [Self; 18] = [
        Self::Codex,
        Self::Claude,
        Self::Grok,
        Self::OpenCode,
        Self::Antigravity,
        Self::Cursor,
        Self::Cline,
        Self::Pi,
        Self::Muse,
        Self::Inception,
        Self::DeepSeek,
        Self::OpenRouter,
        Self::Hermes,
        Self::Acp,
        Self::Kimi,
        Self::Devin,
        Self::Copilot,
        Self::Mistral,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::Antigravity => "antigravity",
            Self::Cursor => "cursor",
            Self::Cline => "cline",
            Self::Pi => "pi",
            Self::Muse => "muse",
            Self::Inception => "inception",
            Self::DeepSeek => "deepseek",
            Self::OpenRouter => "openrouter",
            Self::Hermes => "hermes",
            Self::Acp => "acp",
            Self::Kimi => "kimi",
            Self::Devin => "devin",
            Self::Copilot => "copilot",
            Self::Mistral => "mistral",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.as_str() == raw)
    }

    pub const fn descriptor(self) -> &'static AgentProviderDescriptor {
        agent_provider_descriptor(self)
    }
}

pub const AGENT_PROVIDERS: [AgentProviderId; 18] = AgentProviderId::ALL;

pub fn is_provider_id_pattern(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Validated `^[a-z][a-z0-9-]{0,63}$` that is NOT one of the 18 built-in `AgentProviderId`s.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ForeignProviderId(String);

impl ForeignProviderId {
    pub fn parse(value: &str) -> Option<Self> {
        if is_provider_id_pattern(value) && AgentProviderId::parse(value).is_none() {
            Some(Self(value.to_string()))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for ForeignProviderId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ForeignProviderId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid ForeignProviderId: {raw}")))
    }
}

/// Either a known `AgentProviderId` or an open-protocol `ForeignProviderId`, serialized as plain JSON string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AgentProviderRef {
    Known(AgentProviderId),
    Foreign(ForeignProviderId),
}

impl AgentProviderRef {
    pub fn parse(value: &str) -> Option<Self> {
        if let Some(known) = AgentProviderId::parse(value) {
            Some(Self::Known(known))
        } else {
            ForeignProviderId::parse(value).map(Self::Foreign)
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Known(k) => k.as_str(),
            Self::Foreign(f) => f.as_str(),
        }
    }

    pub const fn as_known(&self) -> Option<AgentProviderId> {
        match self {
            Self::Known(k) => Some(*k),
            Self::Foreign(_) => None,
        }
    }

    pub const fn as_foreign(&self) -> Option<&ForeignProviderId> {
        match self {
            Self::Known(_) => None,
            Self::Foreign(f) => Some(f),
        }
    }
}

impl From<AgentProviderId> for AgentProviderRef {
    fn from(value: AgentProviderId) -> Self {
        Self::Known(value)
    }
}

impl Serialize for AgentProviderRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AgentProviderRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid AgentProviderRef: {raw}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderCodeSignInKind {
    Device,
    Paste,
    Link,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderSignInPageEnd {
    Callback,
    Page,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderHomeSkillsMode {
    Native,
    Plugin,
    Instructions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceEnforcementKind {
    ToolSandbox,
    CommandSandbox,
    ConfinedProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderSteerMode {
    Native,
    Deferred,
    Reported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProviderDescriptor {
    pub id: AgentProviderId,
    pub display_name: &'static str,
    pub cli_name: &'static str,
    pub onboarding_description: &'static str,
    pub sign_in_message: &'static str,
    pub install_guide_link: Option<&'static str>,
    pub default_model: &'static str,
    pub legacy_model_prefix: Option<&'static str>,
    pub auth_kind: &'static str,
    pub picker_order: u8,
    pub local_code_sign_in: Option<ProviderCodeSignInKind>,
    pub windows_code_sign_in: Option<ProviderCodeSignInKind>,
    pub account_sign_in: Option<&'static str>,
    pub account_code_sign_in: Option<ProviderCodeSignInKind>,
    pub sign_in_page_end: ProviderSignInPageEnd,
    pub mainland_china_sign_in_site: Option<&'static str>,
    pub skill_folders: &'static [&'static str],
    pub home_skills: ProviderHomeSkillsMode,
    pub workspace_instruction_files: &'static [&'static str],
    pub workspace_enforcement: WorkspaceEnforcementKind,
    pub steer: ProviderSteerMode,
    pub concurrent_turns: bool,
    pub reports_usage: bool,
    pub approvals_in_full_access: bool,
    pub folders_in_full_access: bool,
}

pub const AGENT_PROVIDER_DESCRIPTORS: [AgentProviderDescriptor; 18] = [
    AgentProviderDescriptor {
        id: AgentProviderId::Codex,
        display_name: "ChatGPT",
        cli_name: "Codex CLI",
        onboarding_description: "Included with OpenBot",
        sign_in_message: "Connect ChatGPT to continue.",
        install_guide_link: None,
        default_model: "gpt-6-luna",
        legacy_model_prefix: None,
        auth_kind: "chatgpt",
        picker_order: 1,
        local_code_sign_in: Some(ProviderCodeSignInKind::Device),
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: Some(ProviderCodeSignInKind::Device),
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md"],
        workspace_enforcement: WorkspaceEnforcementKind::CommandSandbox,
        steer: ProviderSteerMode::Native,
        concurrent_turns: true,
        reports_usage: true,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Claude,
        display_name: "Claude",
        cli_name: "Claude Code",
        onboarding_description: "Included with OpenBot",
        sign_in_message: "Connect Claude to continue.",
        install_guide_link: Some("claude-install"),
        default_model: "claude-haiku-5-5",
        legacy_model_prefix: Some("claude-"),
        auth_kind: "claude",
        picker_order: 0,
        local_code_sign_in: Some(ProviderCodeSignInKind::Paste),
        windows_code_sign_in: Some(ProviderCodeSignInKind::Link),
        account_sign_in: None,
        account_code_sign_in: Some(ProviderCodeSignInKind::Paste),
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".claude/skills"],
        home_skills: ProviderHomeSkillsMode::Plugin,
        workspace_instruction_files: &["CLAUDE.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ToolSandbox,
        steer: ProviderSteerMode::Native,
        concurrent_turns: true,
        reports_usage: true,
        approvals_in_full_access: false,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Grok,
        display_name: "Grok",
        cli_name: "Grok CLI",
        onboarding_description: "Included with OpenBot",
        sign_in_message: "Run `grok login` or set XAI_API_KEY to use Grok.",
        install_guide_link: None,
        default_model: "grok-4.7",
        legacy_model_prefix: Some("grok-"),
        auth_kind: "grok",
        picker_order: 2,
        local_code_sign_in: Some(ProviderCodeSignInKind::Device),
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: Some(ProviderCodeSignInKind::Device),
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &[],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Native,
        concurrent_turns: true,
        reports_usage: true,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::OpenCode,
        display_name: "OpenCode",
        cli_name: "OpenCode CLI",
        onboarding_description: "Free models, no account needed",
        sign_in_message: "OpenCode listed no model. Add an OpenCode Go key to continue.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "opencode",
        picker_order: 3,
        local_code_sign_in: None,
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".opencode/skills", ".agents/skills", ".claude/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Reported,
        concurrent_turns: true,
        reports_usage: true,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Antigravity,
        display_name: "Gemini",
        cli_name: "Antigravity CLI",
        onboarding_description: "Google AI Pro or Ultra plan",
        sign_in_message: "Sign in with Google to use Gemini.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "antigravity",
        picker_order: 4,
        local_code_sign_in: None,
        windows_code_sign_in: Some(ProviderCodeSignInKind::Paste),
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".gemini/skills", ".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Instructions,
        workspace_instruction_files: &[],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Native,
        concurrent_turns: true,
        reports_usage: true,
        approvals_in_full_access: true,
        folders_in_full_access: true,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Cursor,
        display_name: "Cursor",
        cli_name: "Cursor CLI",
        onboarding_description: "Cursor plan or API key",
        sign_in_message: "Sign in with Cursor or set CURSOR_API_KEY to use Cursor.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "cursor",
        picker_order: 5,
        local_code_sign_in: Some(ProviderCodeSignInKind::Device),
        windows_code_sign_in: None,
        account_sign_in: Some("key"),
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".cursor/skills", ".claude/skills", ".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md", "CLAUDE.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Deferred,
        concurrent_turns: true,
        reports_usage: true,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Cline,
        display_name: "Cline",
        cli_name: "Cline CLI",
        onboarding_description: "Free models with a Cline account",
        sign_in_message: "Sign in with Cline or set CLINE_API_KEY to use Cline.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "cline",
        picker_order: 6,
        local_code_sign_in: Some(ProviderCodeSignInKind::Device),
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".cline/skills", ".clinerules/skills", ".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Deferred,
        concurrent_turns: true,
        reports_usage: false,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Pi,
        display_name: "Pi",
        cli_name: "Pi CLI",
        onboarding_description: "Included with OpenBot",
        sign_in_message: "Use /login in Pi on the host computer. Close Pi when sign-in is complete.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "pi",
        picker_order: 11,
        local_code_sign_in: None,
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md", "CLAUDE.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Native,
        concurrent_turns: true,
        reports_usage: false,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Muse,
        display_name: "Muse",
        cli_name: "Muse CLI",
        onboarding_description: "Included with OpenBot",
        sign_in_message: "Sign in to Muse on the host computer to continue.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "muse",
        picker_order: 12,
        local_code_sign_in: Some(ProviderCodeSignInKind::Device),
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Page,
        mainland_china_sign_in_site: None,
        skill_folders: &[".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Instructions,
        workspace_instruction_files: &["AGENTS.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Native,
        concurrent_turns: true,
        reports_usage: true,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Inception,
        display_name: "Inception",
        cli_name: "Pi CLI",
        onboarding_description: "Mercury models with an Inception API key",
        sign_in_message: "Add an Inception API key to continue.",
        install_guide_link: None,
        default_model: "mercury-3",
        legacy_model_prefix: None,
        auth_kind: "inception",
        picker_order: 13,
        local_code_sign_in: None,
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md", "CLAUDE.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Native,
        concurrent_turns: true,
        reports_usage: false,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::DeepSeek,
        display_name: "DeepSeek",
        cli_name: "Pi CLI",
        onboarding_description: "DeepSeek models with a DeepSeek API key",
        sign_in_message: "Add a DeepSeek API key to continue.",
        install_guide_link: None,
        default_model: "deepseek-v4-pro",
        legacy_model_prefix: None,
        auth_kind: "deepseek",
        picker_order: 14,
        local_code_sign_in: None,
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md", "CLAUDE.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Native,
        concurrent_turns: true,
        reports_usage: false,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::OpenRouter,
        display_name: "OpenRouter",
        cli_name: "Pi CLI",
        onboarding_description: "Many models with an OpenRouter API key",
        sign_in_message: "Add an OpenRouter API key to continue.",
        install_guide_link: None,
        default_model: "anthropic/claude-sonnet-5.5",
        legacy_model_prefix: None,
        auth_kind: "openrouter",
        picker_order: 15,
        local_code_sign_in: None,
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md", "CLAUDE.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Native,
        concurrent_turns: true,
        reports_usage: false,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Hermes,
        display_name: "Hermes",
        cli_name: "Hermes Agent",
        onboarding_description: "Nous Portal plan or your Hermes setup",
        sign_in_message: "Sign in with Nous Portal or run `hermes setup` to use Hermes.",
        install_guide_link: Some("hermes-install"),
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "hermes",
        picker_order: 16,
        local_code_sign_in: Some(ProviderCodeSignInKind::Device),
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Page,
        mainland_china_sign_in_site: None,
        skill_folders: &[".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Instructions,
        workspace_instruction_files: &["AGENTS.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Deferred,
        concurrent_turns: true,
        reports_usage: false,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Acp,
        display_name: "Custom agent",
        cli_name: "ACP agent",
        onboarding_description: "An agent you run by command",
        sign_in_message: "Sign in with the agent's own command, then try again.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "acp",
        picker_order: 7,
        local_code_sign_in: None,
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Instructions,
        workspace_instruction_files: &[],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Reported,
        concurrent_turns: false,
        reports_usage: false,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Kimi,
        display_name: "Kimi",
        cli_name: "Kimi Code CLI",
        onboarding_description: "Kimi models with a Kimi Code account",
        sign_in_message: "Sign in with Kimi to use Kimi.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "kimi",
        picker_order: 7,
        local_code_sign_in: Some(ProviderCodeSignInKind::Device),
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Page,
        mainland_china_sign_in_site: Some("kimi.com"),
        skill_folders: &[".kimi/skills", ".claude/skills", ".codex/skills", ".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Deferred,
        concurrent_turns: true,
        reports_usage: true,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Devin,
        display_name: "Devin",
        cli_name: "Devin CLI",
        onboarding_description: "Many models with a Devin account",
        sign_in_message: "Sign in with Devin to use Devin.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "devin",
        picker_order: 8,
        local_code_sign_in: None,
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".devin/skills", ".claude/skills", ".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md", "CLAUDE.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Native,
        concurrent_turns: true,
        reports_usage: false,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Copilot,
        display_name: "GitHub Copilot",
        cli_name: "GitHub Copilot CLI",
        onboarding_description: "Many models with a Copilot plan",
        sign_in_message: "Sign in with GitHub or set COPILOT_GITHUB_TOKEN to use GitHub Copilot.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "copilot",
        picker_order: 9,
        local_code_sign_in: Some(ProviderCodeSignInKind::Device),
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Callback,
        mainland_china_sign_in_site: None,
        skill_folders: &[".github/skills", ".agents/skills", ".claude/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md", "CLAUDE.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Deferred,
        concurrent_turns: true,
        reports_usage: false,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
    AgentProviderDescriptor {
        id: AgentProviderId::Mistral,
        display_name: "Mistral",
        cli_name: "Mistral Vibe",
        onboarding_description: "Mistral models with a Mistral account",
        sign_in_message: "Sign in with Mistral or set MISTRAL_API_KEY to use Mistral.",
        install_guide_link: None,
        default_model: "",
        legacy_model_prefix: None,
        auth_kind: "mistral",
        picker_order: 10,
        local_code_sign_in: None,
        windows_code_sign_in: None,
        account_sign_in: None,
        account_code_sign_in: None,
        sign_in_page_end: ProviderSignInPageEnd::Page,
        mainland_china_sign_in_site: None,
        skill_folders: &[".vibe/skills", ".agents/skills"],
        home_skills: ProviderHomeSkillsMode::Native,
        workspace_instruction_files: &["AGENTS.md"],
        workspace_enforcement: WorkspaceEnforcementKind::ConfinedProcess,
        steer: ProviderSteerMode::Deferred,
        concurrent_turns: true,
        reports_usage: false,
        approvals_in_full_access: true,
        folders_in_full_access: false,
    },
];

pub const fn agent_provider_descriptor(provider: AgentProviderId) -> &'static AgentProviderDescriptor {
    let idx = match provider {
        AgentProviderId::Codex => 0,
        AgentProviderId::Claude => 1,
        AgentProviderId::Grok => 2,
        AgentProviderId::OpenCode => 3,
        AgentProviderId::Antigravity => 4,
        AgentProviderId::Cursor => 5,
        AgentProviderId::Cline => 6,
        AgentProviderId::Pi => 7,
        AgentProviderId::Muse => 8,
        AgentProviderId::Inception => 9,
        AgentProviderId::DeepSeek => 10,
        AgentProviderId::OpenRouter => 11,
        AgentProviderId::Hermes => 12,
        AgentProviderId::Acp => 13,
        AgentProviderId::Kimi => 14,
        AgentProviderId::Devin => 15,
        AgentProviderId::Copilot => 16,
        AgentProviderId::Mistral => 17,
    };
    &AGENT_PROVIDER_DESCRIPTORS[idx]
}

pub const PICKER_PROVIDERS: [AgentProviderId; 17] = [
    AgentProviderId::Claude,
    AgentProviderId::Codex,
    AgentProviderId::Grok,
    AgentProviderId::OpenCode,
    AgentProviderId::Antigravity,
    AgentProviderId::Cursor,
    AgentProviderId::Cline,
    AgentProviderId::Kimi,
    AgentProviderId::Devin,
    AgentProviderId::Copilot,
    AgentProviderId::Mistral,
    AgentProviderId::Pi,
    AgentProviderId::Muse,
    AgentProviderId::Inception,
    AgentProviderId::DeepSeek,
    AgentProviderId::OpenRouter,
    AgentProviderId::Hermes,
];

pub const MANAGED_RUNTIME_PROVIDERS: [AgentProviderId; 13] = [
    AgentProviderId::Codex,
    AgentProviderId::Claude,
    AgentProviderId::Grok,
    AgentProviderId::OpenCode,
    AgentProviderId::Antigravity,
    AgentProviderId::Cursor,
    AgentProviderId::Cline,
    AgentProviderId::Kimi,
    AgentProviderId::Devin,
    AgentProviderId::Copilot,
    AgentProviderId::Mistral,
    AgentProviderId::Pi,
    AgentProviderId::Muse,
];

pub const LOCAL_ONLY_PROVIDERS: [AgentProviderId; 14] = [
    AgentProviderId::Antigravity,
    AgentProviderId::Cursor,
    AgentProviderId::Cline,
    AgentProviderId::Kimi,
    AgentProviderId::Devin,
    AgentProviderId::Copilot,
    AgentProviderId::Mistral,
    AgentProviderId::Pi,
    AgentProviderId::Muse,
    AgentProviderId::Inception,
    AgentProviderId::DeepSeek,
    AgentProviderId::OpenRouter,
    AgentProviderId::Hermes,
    AgentProviderId::Acp,
];

pub const KEY_ONLY_PROVIDERS: [AgentProviderId; 3] = [
    AgentProviderId::Inception,
    AgentProviderId::DeepSeek,
    AgentProviderId::OpenRouter,
];

pub const INSTALLER_PROVIDERS: [AgentProviderId; 1] = [AgentProviderId::Hermes];
pub const MANAGED_TOOL_RUNTIMES: [&str; 1] = ["bun"];
pub const MANAGED_SKILL_FOLDERS: [&str; 2] = [".agents/skills", ".claude/skills"];
pub const OPENCODE_DEFAULT_FREE_MODEL: &str = "opencode/beam-free";
pub const OPENCODE_RECOMMENDED_FREE_MODELS: [&str; 3] = [
    "opencode/beam-free",
    "opencode/big-pickle",
    "opencode/glm-5.3-flash-free",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcpAgentPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
}

pub const ACP_AGENT_PRESETS: [AcpAgentPreset; 3] = [
    AcpAgentPreset { id: "goose", name: "Goose", command: "goose", args: &["acp"] },
    AcpAgentPreset { id: "qwen", name: "Qwen Code", command: "qwen", args: &["--acp"] },
    AcpAgentPreset { id: "omp", name: "Oh My Pi", command: "omp", args: &["acp"] },
];

pub fn is_managed_runtime_provider(provider: AgentProviderId) -> bool {
    MANAGED_RUNTIME_PROVIDERS.contains(&provider)
}

pub fn is_local_only_provider(provider: AgentProviderId) -> bool {
    LOCAL_ONLY_PROVIDERS.contains(&provider)
}

pub fn is_key_only_provider(provider: &AgentProviderRef) -> bool {
    provider
        .as_known()
        .is_some_and(|p| KEY_ONLY_PROVIDERS.contains(&p))
}

pub fn is_installer_provider(provider: AgentProviderId) -> bool {
    INSTALLER_PROVIDERS.contains(&provider)
}

pub fn is_downloadable_provider(provider: AgentProviderId) -> bool {
    is_managed_runtime_provider(provider) || is_installer_provider(provider)
}

pub fn provider_display_name(provider: &AgentProviderRef, host_label: Option<&str>) -> String {
    match provider {
        AgentProviderRef::Known(k) => k.descriptor().display_name.to_string(),
        AgentProviderRef::Foreign(f) => host_label
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| f.as_str())
            .to_string(),
    }
}

pub fn provider_steers_turns(provider: &AgentProviderRef) -> bool {
    match provider {
        AgentProviderRef::Foreign(_) => true,
        AgentProviderRef::Known(k) => k.descriptor().steer == ProviderSteerMode::Native,
    }
}

pub fn is_agent_model(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 160
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'.' | b'_' | b':' | b'/' | b'[' | b']' | b',' | b'=' | b'-')
        })
}

pub const fn default_provider_model(provider: AgentProviderId) -> &'static str {
    agent_provider_descriptor(provider).default_model
}

pub fn provider_for_legacy_model(model: &str) -> AgentProviderId {
    for d in &AGENT_PROVIDER_DESCRIPTORS {
        if let Some(prefix) = d.legacy_model_prefix
            && model.starts_with(prefix)
        {
            return d.id;
        }
    }
    AgentProviderId::Codex
}

pub fn is_free_opencode_model(id: &str, name: &str) -> bool {
    if id.trim().eq_ignore_ascii_case("opencode/big-pickle") {
        return true;
    }
    let trimmed = name.trim().to_ascii_lowercase();
    trimmed == "free"
        || trimmed
            .strip_suffix("free")
            .is_some_and(|prefix| prefix.ends_with(|c: char| !c.is_ascii_alphanumeric() && c != '_'))
}

pub fn account_usage_covers_model(provider: AgentProviderId, model: Option<&str>) -> bool {
    match provider {
        AgentProviderId::OpenCode => model.is_some_and(|m| m.to_ascii_lowercase().starts_with("opencode-go/")),
        AgentProviderId::Antigravity => {
            let lower = model.unwrap_or("").to_ascii_lowercase();
            !(lower.starts_with("claude") || lower.starts_with("gpt"))
        }
        _ => true,
    }
}

const AGENT_IDS_RESERVED_AT_RELEASE: [&str; 7] =
    ["codex", "claude", "grok", "opencode", "antigravity", "acp", "custom"];
pub const CUSTOM_AGENT_DEFAULT_MODEL: &str = "default";

pub fn is_custom_agent_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 40
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && !AGENT_IDS_RESERVED_AT_RELEASE.contains(&value)
}

pub fn is_new_custom_agent_id(value: &str) -> bool {
    is_custom_agent_id(value) && AgentProviderId::parse(value).is_none()
}

pub fn custom_agent_id_of_model(model: &str) -> Option<&str> {
    let slash = model.find('/')?;
    if slash == 0 {
        return None;
    }
    let id = &model[..slash];
    is_custom_agent_id(id).then_some(id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomAgentLimits {
    pub agents: usize,
    pub command: usize,
    pub args: usize,
    pub arg: usize,
    pub env: usize,
    pub env_value: usize,
    pub capabilities: usize,
    pub check_models: usize,
    pub usage_prompt: usize,
    pub usage_answer: usize,
}

pub const CUSTOM_AGENT_LIMITS: CustomAgentLimits = CustomAgentLimits {
    agents: 16,
    command: 1_024,
    args: 32,
    arg: 1_024,
    env: 16,
    env_value: 4_096,
    capabilities: 32,
    check_models: 256,
    usage_prompt: 256,
    usage_answer: 4_000,
};

pub fn is_custom_agent_env_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && is_bounded_string(value, INPUT_LIMITS.identifier)
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes[1..].iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

pub fn is_custom_agent_usage_prompt(value: &str) -> bool {
    is_bounded_string(value, CUSTOM_AGENT_LIMITS.usage_prompt)
        && !value.trim().is_empty()
        && !value.chars().any(|c| matches!(c, '\0' | '\r' | '\n'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAgentEnvInput {
    pub name: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveCustomAgentInput {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<CustomAgentEnvInput>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub usage_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAgentSummary {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env_names: Vec<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "double_option::serialize"
    )]
    pub usage_prompt: Option<Option<String>>,
    pub resolved_command: Option<String>,
}

impl<'de> Deserialize<'de> for CustomAgentSummary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawCustomAgentSummary {
            id: String,
            name: String,
            command: String,
            args: Vec<String>,
            env_names: Vec<String>,
            #[serde(default, with = "double_option")]
            usage_prompt: Option<Option<String>>,
            #[serde(deserialize_with = "Option::deserialize")]
            resolved_command: Option<String>,
            #[serde(default, rename = "env", deserialize_with = "forbid_field")]
            _forbid_env: (),
            #[serde(default, rename = "secret", deserialize_with = "forbid_field")]
            _forbid_secret: (),
            #[serde(default, rename = "value", deserialize_with = "forbid_field")]
            _forbid_value: (),
        }

        let raw = RawCustomAgentSummary::deserialize(deserializer)?;
        Ok(Self {
            id: raw.id,
            name: raw.name,
            command: raw.command,
            args: raw.args,
            env_names: raw.env_names,
            usage_prompt: raw.usage_prompt,
            resolved_command: raw.resolved_command,
        })
    }
}

impl CustomAgentSummary {
    pub fn is_valid(&self) -> bool {
        is_custom_agent_id(&self.id)
            && is_bounded_string(&self.name, INPUT_LIMITS.agent_name)
            && is_bounded_string(&self.command, CUSTOM_AGENT_LIMITS.command)
            && self.args.len() <= CUSTOM_AGENT_LIMITS.args
            && self
                .args
                .iter()
                .all(|arg| is_bounded_string(arg, CUSTOM_AGENT_LIMITS.arg))
            && self.env_names.len() <= CUSTOM_AGENT_LIMITS.env
            && self
                .env_names
                .iter()
                .all(|name| is_custom_agent_env_name(name))
            && self
                .usage_prompt
                .as_ref()
                .is_none_or(|opt| opt.as_deref().is_none_or(is_custom_agent_usage_prompt))
            && self
                .resolved_command
                .as_deref()
                .is_none_or(|cmd| is_bounded_string(cmd, CUSTOM_AGENT_LIMITS.command))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAgentCheckResult {
    pub agent_name: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub version: Option<String>,
    pub protocol_version: i64,
    pub capabilities: Vec<String>,
    pub models: Vec<String>,
}

impl CustomAgentCheckResult {
    pub fn is_valid(&self) -> bool {
        is_bounded_string(&self.agent_name, INPUT_LIMITS.agent_name)
            && self
                .version
                .as_deref()
                .is_none_or(|v| is_bounded_string(v, 160))
            && self.capabilities.len() <= CUSTOM_AGENT_LIMITS.capabilities
            && self
                .capabilities
                .iter()
                .all(|name| is_bounded_string(name, INPUT_LIMITS.identifier))
            && self.models.len() <= CUSTOM_AGENT_LIMITS.check_models
            && self
                .models
                .iter()
                .all(|name| is_bounded_string(name, INPUT_LIMITS.model_name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedAcpAgent {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

impl<'de> Deserialize<'de> for DetectedAcpAgent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawDetectedAcpAgent {
            id: String,
            name: String,
            command: String,
            args: Vec<String>,
            #[serde(default, rename = "env", deserialize_with = "forbid_field")]
            _forbid_env: (),
        }

        let raw = RawDetectedAcpAgent::deserialize(deserializer)?;
        Ok(Self {
            id: raw.id,
            name: raw.name,
            command: raw.command,
            args: raw.args,
        })
    }
}

impl DetectedAcpAgent {
    pub fn is_valid(&self) -> bool {
        is_new_custom_agent_id(&self.id)
            && is_bounded_string(&self.name, INPUT_LIMITS.agent_name)
            && is_bounded_string(&self.command, CUSTOM_AGENT_LIMITS.command)
            && self.args.len() <= CUSTOM_AGENT_LIMITS.args
            && self
                .args
                .iter()
                .all(|arg| is_bounded_string(arg, CUSTOM_AGENT_LIMITS.arg))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomProviderLimits {
    pub api_key: usize,
    pub base_url: usize,
    pub models: usize,
    pub headers: usize,
    pub context_window_min: u64,
    pub context_window_max: u64,
    pub max_output_min: u64,
    pub max_output_max: u64,
}

pub const CUSTOM_PROVIDER_LIMITS: CustomProviderLimits = CustomProviderLimits {
    api_key: 4_096,
    base_url: 2_048,
    models: 64,
    headers: 32,
    context_window_min: 1_024,
    context_window_max: 100_000_000,
    max_output_min: 256,
    max_output_max: 1_000_000,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomProviderHarness {
    OpenCode,
    Pi,
}

pub const DEFAULT_CUSTOM_PROVIDER_HARNESS: CustomProviderHarness = CustomProviderHarness::OpenCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomProviderInputKind {
    Image,
    Pdf,
}

impl CustomProviderInputKind {
    pub const ALL: [Self; 2] = [Self::Image, Self::Pdf];
}

/// Custom provider reasoning efforts (`"ultra"` is intentionally absent per `ipc-custom-providers.ts:L54-L63`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomProviderReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl CustomProviderReasoningEffort {
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];
}

fn is_ordered_subset<T: PartialEq>(known: &[T], values: &[T]) -> bool {
    if values.is_empty() {
        return false;
    }
    let mut previous: isize = -1;
    for item in values {
        let Some(index) = known.iter().position(|k| k == item) else {
            return false;
        };
        let idx = index as isize;
        if idx <= previous {
            return false;
        }
        previous = idx;
    }
    true
}

pub fn is_custom_provider_inputs(values: &[CustomProviderInputKind]) -> bool {
    values.is_empty() || is_ordered_subset(&CustomProviderInputKind::ALL, values)
}

pub fn is_custom_provider_reasoning_efforts(values: &[CustomProviderReasoningEffort]) -> bool {
    is_ordered_subset(&CustomProviderReasoningEffort::ALL, values)
}

pub fn is_custom_provider_model_efforts(values: &[CustomProviderReasoningEffort]) -> bool {
    values.is_empty() || is_custom_provider_reasoning_efforts(values)
}

const CUSTOM_PROVIDER_RESERVED_AT_RELEASE: [&str; 4] = ["codex", "claude", "grok", "opencode"];

pub fn is_custom_provider_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && is_bounded_string(value, INPUT_LIMITS.identifier)
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
        && !CUSTOM_PROVIDER_RESERVED_AT_RELEASE.contains(&value)
}

pub fn is_new_custom_provider_id(value: &str) -> bool {
    is_custom_provider_id(value) && AgentProviderId::parse(value).is_none()
}

/// RFC 7230 HTTP token header name validator (`^[A-Za-z0-9!#$%&'*+.^_`|~-]+$`).
pub fn is_custom_provider_header_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+'
                        | b'.' | b'^' | b'_' | b'`' | b'|' | b'~' | b'-'
                )
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderModel {
    pub id: String,
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub context_window: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub max_output: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub reasoning_efforts: Option<Vec<CustomProviderReasoningEffort>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub inputs: Option<Vec<CustomProviderInputKind>>,
}

impl CustomProviderModel {
    pub fn is_valid(&self) -> bool {
        is_bounded_string(&self.id, INPUT_LIMITS.identifier)
            && is_bounded_string(&self.name, INPUT_LIMITS.model_name)
            && self.context_window.is_none_or(|cw| {
                (CUSTOM_PROVIDER_LIMITS.context_window_min
                    ..=CUSTOM_PROVIDER_LIMITS.context_window_max)
                    .contains(&cw)
            })
            && self.max_output.is_none_or(|mo| {
                (CUSTOM_PROVIDER_LIMITS.max_output_min..=CUSTOM_PROVIDER_LIMITS.max_output_max)
                    .contains(&mo)
            })
            && self
                .reasoning_efforts
                .as_ref()
                .is_none_or(|efforts| is_custom_provider_model_efforts(efforts))
            && self
                .inputs
                .as_ref()
                .is_none_or(|inputs| is_custom_provider_inputs(inputs))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomProviderHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomProviderHeaderInput {
    pub name: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderEndpoint {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub api_key: Option<String>,
    pub models: Vec<CustomProviderModel>,
    #[serde(default)]
    pub headers: Vec<CustomProviderHeader>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub harness: Option<CustomProviderHarness>,
}

pub type SaveCustomProviderInput = CustomProviderEndpoint;

pub fn is_valid_custom_provider_base_url(value: &str) -> bool {
    if value.trim().is_empty() || !is_bounded_string(value, CUSTOM_PROVIDER_LIMITS.base_url) {
        return false;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
}

impl CustomProviderEndpoint {
    pub fn is_valid(&self) -> bool {
        if !is_custom_provider_id(&self.id)
            || self.name.trim().is_empty()
            || !is_bounded_string(&self.name, INPUT_LIMITS.agent_name)
            || !is_valid_custom_provider_base_url(&self.base_url)
            || self
                .api_key
                .as_ref()
                .is_some_and(|k| !is_bounded_string(k, CUSTOM_PROVIDER_LIMITS.api_key))
            || self.models.is_empty()
            || self.models.len() > CUSTOM_PROVIDER_LIMITS.models
            || self.headers.len() > CUSTOM_PROVIDER_LIMITS.headers
        {
            return false;
        }
        let mut seen_models = std::collections::HashSet::with_capacity(self.models.len());
        for m in &self.models {
            if m.id.trim().is_empty() || m.name.trim().is_empty() || !m.is_valid() {
                return false;
            }
            let listed = composed_custom_model_id(&self.id, &m.id);
            if !is_agent_model(&listed) || !seen_models.insert(listed) {
                return false;
            }
        }
        let mut seen_headers = std::collections::HashSet::with_capacity(self.headers.len());
        for h in &self.headers {
            if !is_custom_provider_header_name(&h.name)
                || !is_bounded_string(&h.name, INPUT_LIMITS.identifier)
                || !is_bounded_string(&h.value, CUSTOM_PROVIDER_LIMITS.api_key)
                || !seen_headers.insert(h.name.to_ascii_lowercase())
            {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCustomProviderInput {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<CustomProviderModel>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub api_key: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub headers: Option<Vec<CustomProviderHeaderInput>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub harness: Option<CustomProviderHarness>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderSummary {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub has_api_key: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_names: Option<Vec<String>>,
    pub models: Vec<CustomProviderModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<CustomProviderHarness>,
}

impl<'de> Deserialize<'de> for CustomProviderSummary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawCustomProviderSummary {
            id: String,
            name: String,
            base_url: String,
            has_api_key: bool,
            #[serde(default, deserialize_with = "non_null_option")]
            header_names: Option<Vec<String>>,
            models: Vec<CustomProviderModel>,
            #[serde(default, deserialize_with = "non_null_option")]
            harness: Option<CustomProviderHarness>,
            #[serde(default, rename = "apiKey", deserialize_with = "forbid_field")]
            _forbid_api_key: (),
            #[serde(default, rename = "headers", deserialize_with = "forbid_field")]
            _forbid_headers: (),
        }

        let raw = RawCustomProviderSummary::deserialize(deserializer)?;
        Ok(Self {
            id: raw.id,
            name: raw.name,
            base_url: raw.base_url,
            has_api_key: raw.has_api_key,
            header_names: raw.header_names,
            models: raw.models,
            harness: raw.harness,
        })
    }
}

impl CustomProviderSummary {
    pub fn is_valid(&self) -> bool {
        is_custom_provider_id(&self.id)
            && is_bounded_string(&self.name, INPUT_LIMITS.agent_name)
            && is_bounded_string(&self.base_url, CUSTOM_PROVIDER_LIMITS.base_url)
            && self.header_names.as_ref().is_none_or(|names| {
                names.len() <= CUSTOM_PROVIDER_LIMITS.headers
                    && names.iter().all(|name| {
                        is_custom_provider_header_name(name)
                            && is_bounded_string(name, INPUT_LIMITS.identifier)
                    })
            })
            && self.models.len() <= CUSTOM_PROVIDER_LIMITS.models
            && self.models.iter().all(CustomProviderModel::is_valid)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CustomProviderRestart {
    Restarted,
    SkippedBusy,
    NotRunning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomProviderResult {
    pub providers: Vec<CustomProviderSummary>,
    pub restart: CustomProviderRestart,
}

impl CustomProviderResult {
    pub fn is_valid(&self) -> bool {
        self.providers.iter().all(CustomProviderSummary::is_valid)
    }
}

fn is_escaped_model_char(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'(' | b')' | b'*' | b'+'
            | b';' | b'<' | b'>' | b'?' | b'@' | b'^' | b'{' | b'|' | b'}' | b'~' | b'='
    )
}

fn is_escapable_model_id(value: &str) -> bool {
    value.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || matches!(b, b'.' | b'_' | b':' | b'/' | b'[' | b']' | b',' | b'-')
            || is_escaped_model_char(b)
    })
}

/// OpenCode custom model ID composer (`ipc-custom-providers.ts:composedCustomModelId`).
pub fn composed_custom_model_id(provider_id: &str, model_id: &str) -> String {
    let plain = format!("{provider_id}/{model_id}");
    if is_agent_model(&plain) || !is_escapable_model_id(model_id) {
        return plain;
    }
    let mut escaped = String::with_capacity(provider_id.len() + 1 + model_id.len() * 3);
    escaped.push_str(provider_id);
    escaped.push('/');
    for b in model_id.bytes() {
        if is_escaped_model_char(b) {
            escaped.push_str(&format!("={b:02X}"));
        } else {
            escaped.push(char::from(b));
        }
    }
    escaped
}

pub fn custom_model_key(provider_id: &str, model_id: &str) -> String {
    composed_custom_model_id(provider_id, model_id)[provider_id.len() + 1..].to_string()
}

/// Pi harness model ID encoder (`ipc-custom-providers.ts:encodedModelId`).
pub fn encoded_model_id(prefix: &str, value: &str) -> String {
    let plain = format!("{prefix}{value}");
    if is_agent_model(&plain) && !value.starts_with("b64:") {
        plain
    } else {
        format!("{prefix}b64:{}", URL_SAFE_NO_PAD.encode(value.as_bytes()))
    }
}

pub fn saved_custom_model_id(
    endpoint_id: &str,
    harness: Option<CustomProviderHarness>,
    model_id: &str,
) -> String {
    match harness.unwrap_or(DEFAULT_CUSTOM_PROVIDER_HARNESS) {
        CustomProviderHarness::Pi => encoded_model_id(&format!("{endpoint_id}/"), model_id),
        CustomProviderHarness::OpenCode => composed_custom_model_id(endpoint_id, model_id),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderDetectionLimits {
    pub entries: usize,
    pub entry_length: usize,
    pub hidden: usize,
    pub models: usize,
}

pub const PROVIDER_DETECTION_LIMITS: ProviderDetectionLimits = ProviderDetectionLimits {
    entries: 16,
    entry_length: 2_048,
    hidden: 64,
    models: 256,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultModelServer {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
}

pub const DEFAULT_MODEL_SERVERS: [DefaultModelServer; 2] = [
    DefaultModelServer {
        id: "ollama",
        name: "Ollama",
        base_url: "http://127.0.0.1:11434/v1",
    },
    DefaultModelServer {
        id: "lmstudio",
        name: "LM Studio",
        base_url: "http://127.0.0.1:1234/v1",
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDetectionSettings {
    pub enabled: bool,
    pub addresses: Vec<String>,
    pub folders: Vec<String>,
    pub hidden_ids: Vec<String>,
}

impl Default for ProviderDetectionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            addresses: Vec::new(),
            folders: Vec::new(),
            hidden_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedModel {
    pub id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub inputs: Option<Vec<CustomProviderInputKind>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub context_window: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub max_output: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub reasoning_efforts: Option<Vec<CustomProviderReasoningEffort>>,
}

impl DetectedModel {
    pub fn is_valid(&self) -> bool {
        !self.id.is_empty()
            && is_bounded_string(&self.id, INPUT_LIMITS.identifier)
            && self
                .inputs
                .as_ref()
                .is_none_or(|inputs| is_custom_provider_inputs(inputs))
            && self.context_window.is_none_or(|cw| {
                (CUSTOM_PROVIDER_LIMITS.context_window_min
                    ..=CUSTOM_PROVIDER_LIMITS.context_window_max)
                    .contains(&cw)
            })
            && self.max_output.is_none_or(|mo| {
                self.context_window.is_some()
                    && (CUSTOM_PROVIDER_LIMITS.max_output_min
                        ..=CUSTOM_PROVIDER_LIMITS.max_output_max)
                        .contains(&mo)
            })
            && self
                .reasoning_efforts
                .as_ref()
                .is_none_or(|efforts| is_custom_provider_reasoning_efforts(efforts))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedModelServer {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<DetectedModel>,
}

impl<'de> Deserialize<'de> for DetectedModelServer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawDetectedModelServer {
            id: String,
            name: String,
            base_url: String,
            models: Vec<DetectedModel>,
            #[serde(default, rename = "apiKey", deserialize_with = "forbid_field")]
            _forbid_api_key: (),
            #[serde(default, rename = "headers", deserialize_with = "forbid_field")]
            _forbid_headers: (),
        }

        let raw = RawDetectedModelServer::deserialize(deserializer)?;
        Ok(Self {
            id: raw.id,
            name: raw.name,
            base_url: raw.base_url,
            models: raw.models,
        })
    }
}

impl DetectedModelServer {
    pub fn is_valid(&self) -> bool {
        is_new_custom_provider_id(&self.id)
            && is_bounded_string(&self.name, INPUT_LIMITS.agent_name)
            && is_bounded_string(&self.base_url, CUSTOM_PROVIDER_LIMITS.base_url)
            && self.models.len() <= PROVIDER_DETECTION_LIMITS.models
            && self.models.iter().all(DetectedModel::is_valid)
    }
}

pub fn is_detection_folder(value: &str) -> bool {
    if value.contains('\0') {
        return false;
    }
    if value.starts_with('/') || value.starts_with("~/") || value == "~" {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

fn parse_http_url(value: &str) -> Option<Url> {
    let url = Url::parse(value.trim()).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

pub fn custom_provider_endpoint_key(base_url: &str) -> Option<String> {
    let url = parse_http_url(base_url)?;
    let raw_host = url.host_str()?.to_ascii_lowercase();
    let host = match raw_host.as_str() {
        "localhost" | "127.0.0.1" | "[::1]" => "127.0.0.1",
        other => other,
    };
    let port_suffix = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let path = url.path().trim_end_matches('/');
    Some(format!("{}://{host}{port_suffix}{path}", url.scheme()))
}

pub fn detected_model_server_key(base_url: &str) -> String {
    format!(
        "models:{}",
        custom_provider_endpoint_key(base_url).unwrap_or_else(|| base_url.to_string())
    )
}

pub fn same_custom_provider_origin(a: &str, b: &str) -> bool {
    match (parse_http_url(a), parse_http_url(b)) {
        (Some(first), Some(second)) => first.origin() == second.origin(),
        _ => false,
    }
}
