//! Complete Agent Catalog (`F12`):
//! - 22 live Bot / Native-Chat CLI agents (`BotAgentId::ALL`) + retired `gemini -> antigravity`
//! - 44 TUI agent catalog entries with binary overrides, yolo mode, resume flags & auto-update commands
//! - 16 ACP dialects + native `ClaudeSdk` and `CodexAppServer` JSONL-RPC drivers
//! - Dynamic model/effort catalog & launch plan builder (Claude, Codex `/model <id>`, Grok 4.6 Extra high vs 4.5 High)
//! - Shared `/` slash-command & skill catalog registry across desktop & mobile surfaces.

use std::collections::BTreeMap;

use orca_core::bot_types::{
    BotAgentId, BotLaunchConfig, BotPermissionMode, migrate_retired_agent_id,
};
use serde::{Deserialize, Serialize};

use crate::{OrcaIdeRuntimeError, Result};

/// Exact count of live Bot / Native-Chat CLI agents.
pub const BOT_AGENT_CATALOG_COUNT: usize = 22;
/// Exact count of total TUI agent catalog entries.
pub const TUI_AGENT_CATALOG_COUNT: usize = 44;
/// Exact count of Agent Client Protocol (ACP) dialects.
pub const ACP_DIALECT_COUNT: usize = 16;

/// All 16 supported Agent Client Protocol (ACP) dialects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcpDialect {
    Antigravity,
    Auggie,
    Cline,
    Codebuddy,
    Copilot,
    Cursor,
    Devin,
    Droid,
    Goose,
    Grok,
    Hermes,
    Kimi,
    Kiro,
    Omp,
    Opencode,
    QwenCode,
}

impl AcpDialect {
    pub const ALL: [Self; ACP_DIALECT_COUNT] = [
        Self::Antigravity,
        Self::Auggie,
        Self::Cline,
        Self::Codebuddy,
        Self::Copilot,
        Self::Cursor,
        Self::Devin,
        Self::Droid,
        Self::Goose,
        Self::Grok,
        Self::Hermes,
        Self::Kimi,
        Self::Kiro,
        Self::Omp,
        Self::Opencode,
        Self::QwenCode,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::Auggie => "auggie",
            Self::Cline => "cline",
            Self::Codebuddy => "codebuddy",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
            Self::Devin => "devin",
            Self::Droid => "droid",
            Self::Goose => "goose",
            Self::Grok => "grok",
            Self::Hermes => "hermes",
            Self::Kimi => "kimi",
            Self::Kiro => "kiro",
            Self::Omp => "omp",
            Self::Opencode => "opencode",
            Self::QwenCode => "qwen-code",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let norm = raw.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|d| d.as_str() == norm)
    }

    /// Returns protocol capabilities for this ACP dialect.
    pub const fn capabilities(self) -> AcpDialectSpec {
        match self {
            Self::Antigravity | Self::Cursor | Self::Grok | Self::Opencode => AcpDialectSpec {
                dialect: self,
                jsonl_stdio: true,
                supports_tool_approvals: true,
                supports_session_resume: true,
                supports_dynamic_model_probe: true,
            },
            Self::Copilot | Self::Hermes | Self::Kimi | Self::Kiro | Self::Omp => AcpDialectSpec {
                dialect: self,
                jsonl_stdio: true,
                supports_tool_approvals: true,
                supports_session_resume: true,
                supports_dynamic_model_probe: false,
            },
            Self::Auggie
            | Self::Cline
            | Self::Codebuddy
            | Self::Devin
            | Self::Droid
            | Self::Goose
            | Self::QwenCode => AcpDialectSpec {
                dialect: self,
                jsonl_stdio: true,
                supports_tool_approvals: true,
                supports_session_resume: false,
                supports_dynamic_model_probe: false,
            },
        }
    }
}

/// Capabilities metadata for an `AcpDialect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpDialectSpec {
    pub dialect: AcpDialect,
    pub jsonl_stdio: bool,
    pub supports_tool_approvals: bool,
    pub supports_session_resume: bool,
    pub supports_dynamic_model_probe: bool,
}

/// Native driver architecture used to run an agent in Chat or TUI mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeDriverKind {
    ClaudeSdk,
    CodexAppServer,
    Acp,
    PtyTui,
}

/// Model & effort preset exposed for an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModelOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub efforts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slash_model_command: Option<String>,
}

/// Complete catalog entry for one of the 44 supported agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCatalogEntry {
    pub id: String,
    pub display_name: String,
    pub binary: String,
    pub binary_override_env: String,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub default_env: BTreeMap<String, String>,
    #[serde(default)]
    pub yolo_args: Vec<String>,
    pub supports_resume: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_dialect: Option<AcpDialect>,
    pub native_driver_kind: NativeDriverKind,
    pub supports_hooks: bool,
    pub is_bot_agent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_update_cmd: Option<Vec<String>>,
    #[serde(default)]
    pub models: Vec<AgentModelOption>,
    #[serde(default)]
    pub efforts: Vec<String>,
}

/// Concrete CLI launch plan synthesized by `AgentCatalog::build_launch_plan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLaunchPlan {
    pub resolved_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_from: Option<String>,
    pub binary: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub native_driver_kind: NativeDriverKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_dialect: Option<AcpDialect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slash_model_command: Option<String>,
    pub yolo_enabled: bool,
}

/// Surface availability for shared `/` slash commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashCommandSurface {
    Desktop,
    Mobile,
    Both,
}

/// Functional grouping for `/` slash commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashCommandCategory {
    Session,
    Model,
    Agent,
    Orchestration,
    Workspace,
    Skill,
    Bot,
}

/// Shared `/` slash-command or skill command descriptor across desktop & mobile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommandEntry {
    pub command: String,
    pub description: String,
    pub usage: String,
    pub category: SlashCommandCategory,
    pub surface: SlashCommandSurface,
    /// Empty means available across all agents; non-empty restricts to listed agent IDs.
    #[serde(default)]
    pub supported_agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
}

struct EntrySeed<'a> {
    id: &'a str,
    display_name: &'a str,
    binary: &'a str,
    default_args: &'a [&'a str],
    yolo_args: &'a [&'a str],
    resume_flag: Option<&'a str>,
    acp_dialect: Option<AcpDialect>,
    native_driver_kind: NativeDriverKind,
    supports_hooks: bool,
    is_bot_agent: bool,
    auto_update_cmd: Option<&'a [&'a str]>,
}

fn override_env_for(agent_id: &str) -> String {
    let upper: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("ORCA_{upper}_BIN")
}

fn build_models_for(agent_id: &str) -> (Vec<AgentModelOption>, Vec<String>) {
    match agent_id {
        "claude" | "claude-agent-teams" => {
            let efforts = vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "max".into(),
            ];
            let models = vec![
                AgentModelOption {
                    id: "claude-opus-4-6".into(),
                    label: "Claude Opus 4.6".into(),
                    efforts: efforts.clone(),
                    default_effort: Some("high".into()),
                    slash_model_command: Some("/model claude-opus-4-6".into()),
                },
                AgentModelOption {
                    id: "claude-sonnet-4-6".into(),
                    label: "Claude Sonnet 4.6".into(),
                    efforts: efforts.clone(),
                    default_effort: Some("medium".into()),
                    slash_model_command: Some("/model claude-sonnet-4-6".into()),
                },
                AgentModelOption {
                    id: "claude-haiku-4-5".into(),
                    label: "Claude Haiku 4.5".into(),
                    efforts: vec!["low".into(), "medium".into()],
                    default_effort: Some("low".into()),
                    slash_model_command: Some("/model claude-haiku-4-5".into()),
                },
            ];
            (models, efforts)
        }
        "codex" => {
            let efforts = vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
            ];
            let models = vec![
                AgentModelOption {
                    id: "gpt-5.4".into(),
                    label: "GPT-5.4".into(),
                    efforts: efforts.clone(),
                    default_effort: Some("high".into()),
                    slash_model_command: Some("/model gpt-5.4".into()),
                },
                AgentModelOption {
                    id: "gpt-5.3-codex".into(),
                    label: "GPT-5.3 Codex".into(),
                    efforts: efforts.clone(),
                    default_effort: Some("high".into()),
                    slash_model_command: Some("/model gpt-5.3-codex".into()),
                },
                AgentModelOption {
                    id: "o3".into(),
                    label: "o3".into(),
                    efforts: vec!["medium".into(), "high".into()],
                    default_effort: Some("high".into()),
                    slash_model_command: Some("/model o3".into()),
                },
            ];
            (models, efforts)
        }
        "grok" => {
            let efforts = vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "extra_high".into(),
            ];
            let models = vec![
                AgentModelOption {
                    id: "grok-4.6".into(),
                    label: "Grok 4.6".into(),
                    efforts: efforts.clone(),
                    default_effort: Some("extra_high".into()),
                    slash_model_command: Some("/model grok-4.6".into()),
                },
                AgentModelOption {
                    id: "grok-4.5".into(),
                    label: "Grok 4.5".into(),
                    efforts: vec!["low".into(), "medium".into(), "high".into()],
                    default_effort: Some("high".into()),
                    slash_model_command: Some("/model grok-4.5".into()),
                },
            ];
            (models, efforts)
        }
        "antigravity" | "gemini-cli" => {
            let efforts = vec!["low".into(), "medium".into(), "high".into()];
            let models = vec![
                AgentModelOption {
                    id: "gemini-2.5-pro".into(),
                    label: "Gemini 2.5 Pro".into(),
                    efforts: efforts.clone(),
                    default_effort: Some("high".into()),
                    slash_model_command: Some("/model gemini-2.5-pro".into()),
                },
                AgentModelOption {
                    id: "gemini-2.5-flash".into(),
                    label: "Gemini 2.5 Flash".into(),
                    efforts: vec!["low".into(), "medium".into()],
                    default_effort: Some("medium".into()),
                    slash_model_command: Some("/model gemini-2.5-flash".into()),
                },
            ];
            (models, efforts)
        }
        "cursor" => {
            let efforts = vec!["standard".into(), "max".into()];
            let models = vec![AgentModelOption {
                id: "cursor-auto".into(),
                label: "Cursor Auto".into(),
                efforts: efforts.clone(),
                default_effort: Some("standard".into()),
                slash_model_command: Some("/model cursor-auto".into()),
            }];
            (models, efforts)
        }
        "kimi" => {
            let efforts = vec!["standard".into(), "thinking".into()];
            let models = vec![AgentModelOption {
                id: "kimi-k2".into(),
                label: "Kimi K2".into(),
                efforts: efforts.clone(),
                default_effort: Some("thinking".into()),
                slash_model_command: Some("/model kimi-k2".into()),
            }];
            (models, efforts)
        }
        _ => (Vec::new(), vec!["standard".into(), "high".into()]),
    }
}

fn seed_to_entry(seed: EntrySeed<'_>) -> AgentCatalogEntry {
    let (models, efforts) = build_models_for(seed.id);
    AgentCatalogEntry {
        id: seed.id.to_string(),
        display_name: seed.display_name.to_string(),
        binary: seed.binary.to_string(),
        binary_override_env: override_env_for(seed.id),
        default_args: seed.default_args.iter().map(|s| (*s).to_string()).collect(),
        default_env: BTreeMap::new(),
        yolo_args: seed.yolo_args.iter().map(|s| (*s).to_string()).collect(),
        supports_resume: seed.resume_flag.is_some(),
        resume_flag: seed.resume_flag.map(str::to_string),
        acp_dialect: seed.acp_dialect,
        native_driver_kind: seed.native_driver_kind,
        supports_hooks: seed.supports_hooks,
        is_bot_agent: seed.is_bot_agent,
        auto_update_cmd: seed
            .auto_update_cmd
            .map(|parts| parts.iter().map(|s| (*s).to_string()).collect()),
        models,
        efforts,
    }
}

const CATALOG_SEEDS: [EntrySeed<'static>; TUI_AGENT_CATALOG_COUNT] = [
    // --- 22 Live Bot / Native-Chat CLI Agents (exact BotAgentId::ALL) ---
    EntrySeed { id: "claude", display_name: "Claude Code", binary: "claude", default_args: &[], yolo_args: &["--dangerously-skip-permissions"], resume_flag: Some("--resume"), acp_dialect: None, native_driver_kind: NativeDriverKind::ClaudeSdk, supports_hooks: true, is_bot_agent: true, auto_update_cmd: Some(&["claude", "update"]) },
    EntrySeed { id: "codex", display_name: "Codex CLI", binary: "codex", default_args: &[], yolo_args: &["--dangerously-bypass-approvals-and-sandbox"], resume_flag: Some("resume"), acp_dialect: None, native_driver_kind: NativeDriverKind::CodexAppServer, supports_hooks: true, is_bot_agent: true, auto_update_cmd: Some(&["npm", "i", "-g", "@openai/codex@latest"]) },
    EntrySeed { id: "grok", display_name: "Grok CLI", binary: "grok", default_args: &[], yolo_args: &["--yolo"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Grok), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: Some(&["grok", "update"]) },
    EntrySeed { id: "opencode", display_name: "OpenCode", binary: "opencode", default_args: &[], yolo_args: &["--auto-approve"], resume_flag: Some("--session"), acp_dialect: Some(AcpDialect::Opencode), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: Some(&["opencode", "upgrade"]) },
    EntrySeed { id: "omp", display_name: "Oh My Pi", binary: "omp", default_args: &[], yolo_args: &["--yolo"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Omp), native_driver_kind: NativeDriverKind::Acp, supports_hooks: false, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "pi", display_name: "Pi Coding Agent", binary: "pi", default_args: &[], yolo_args: &["--yes"], resume_flag: Some("--resume"), acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: true, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "antigravity", display_name: "Antigravity CLI", binary: "antigravity", default_args: &[], yolo_args: &["--yolo"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Antigravity), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: Some(&["antigravity", "update"]) },
    EntrySeed { id: "cursor", display_name: "Cursor Agent", binary: "cursor-agent", default_args: &[], yolo_args: &["--force"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Cursor), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "hermes", display_name: "Hermes Agent", binary: "hermes", default_args: &[], yolo_args: &["--yolo"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Hermes), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "copilot", display_name: "GitHub Copilot CLI", binary: "copilot", default_args: &[], yolo_args: &["--allow-all-tools"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Copilot), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: Some(&["gh", "extension", "upgrade", "gh-copilot"]) },
    EntrySeed { id: "opencode2", display_name: "OpenCode v2", binary: "opencode2", default_args: &[], yolo_args: &["--auto-approve"], resume_flag: Some("--session"), acp_dialect: Some(AcpDialect::Opencode), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: Some(&["opencode2", "upgrade"]) },
    EntrySeed { id: "kimi", display_name: "Kimi CLI", binary: "kimi", default_args: &[], yolo_args: &["--yolo"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Kimi), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "kiro", display_name: "Kiro CLI", binary: "kiro-cli", default_args: &[], yolo_args: &["--trust-all-tools"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Kiro), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "aug", display_name: "Augment CLI", binary: "auggie", default_args: &[], yolo_args: &["--yes"], resume_flag: None, acp_dialect: Some(AcpDialect::Auggie), native_driver_kind: NativeDriverKind::Acp, supports_hooks: false, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "goose", display_name: "Goose", binary: "goose", default_args: &["session"], yolo_args: &["--no-confirm"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Goose), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: Some(&["goose", "update"]) },
    EntrySeed { id: "kilo", display_name: "Kilo Code", binary: "kilo", default_args: &[], yolo_args: &["--auto"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "mimo-code", display_name: "MiMo Code", binary: "mimo-code", default_args: &[], yolo_args: &["--auto-approve"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "droid", display_name: "Factory Droid", binary: "droid", default_args: &[], yolo_args: &["--auto", "high"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Droid), native_driver_kind: NativeDriverKind::Acp, supports_hooks: false, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "qwen-code", display_name: "Qwen Code", binary: "qwen", default_args: &[], yolo_args: &["--yolo"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::QwenCode), native_driver_kind: NativeDriverKind::Acp, supports_hooks: false, is_bot_agent: true, auto_update_cmd: Some(&["npm", "i", "-g", "@qwen-code/qwen-code@latest"]) },
    EntrySeed { id: "cline", display_name: "Cline CLI", binary: "cline", default_args: &[], yolo_args: &["--auto-approve"], resume_flag: Some("--task"), acp_dialect: Some(AcpDialect::Cline), native_driver_kind: NativeDriverKind::Acp, supports_hooks: true, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "devin", display_name: "Devin CLI", binary: "devin", default_args: &[], yolo_args: &["--autonomous"], resume_flag: Some("--session"), acp_dialect: Some(AcpDialect::Devin), native_driver_kind: NativeDriverKind::Acp, supports_hooks: false, is_bot_agent: true, auto_update_cmd: None },
    EntrySeed { id: "codebuddy", display_name: "CodeBuddy", binary: "codebuddy", default_args: &[], yolo_args: &["--yes"], resume_flag: Some("--resume"), acp_dialect: Some(AcpDialect::Codebuddy), native_driver_kind: NativeDriverKind::Acp, supports_hooks: false, is_bot_agent: true, auto_update_cmd: None },
    // --- 22 Extended TUI Agent Catalog Entries (44 total) ---
    EntrySeed { id: "claude-agent-teams", display_name: "Claude Agent Teams", binary: "claude", default_args: &["--agent-teams"], yolo_args: &["--dangerously-skip-permissions"], resume_flag: Some("--resume"), acp_dialect: None, native_driver_kind: NativeDriverKind::ClaudeSdk, supports_hooks: true, is_bot_agent: false, auto_update_cmd: Some(&["claude", "update"]) },
    EntrySeed { id: "aider", display_name: "Aider", binary: "aider", default_args: &[], yolo_args: &["--yes-always"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: true, is_bot_agent: false, auto_update_cmd: Some(&["pip", "install", "-U", "aider-chat"]) },
    EntrySeed { id: "amp", display_name: "Sourcegraph Amp", binary: "amp", default_args: &[], yolo_args: &["--dangerously-allow-all"], resume_flag: Some("threads continue"), acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "crush", display_name: "Charm Crush", binary: "crush", default_args: &[], yolo_args: &["--yolo"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "auggie", display_name: "Auggie TUI", binary: "auggie", default_args: &[], yolo_args: &["--yes"], resume_flag: None, acp_dialect: Some(AcpDialect::Auggie), native_driver_kind: NativeDriverKind::Acp, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "muse", display_name: "Muse CLI", binary: "muse", default_args: &[], yolo_args: &["--auto"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "zcode", display_name: "ZCode CLI", binary: "zcode", default_args: &[], yolo_args: &["--auto"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "dsh", display_name: "DSH Agent Shell", binary: "dsh", default_args: &[], yolo_args: &["--yes"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "mistral", display_name: "Mistral Vibe", binary: "vibe", default_args: &[], yolo_args: &["--auto-approve"], resume_flag: Some("--resume"), acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "gemini-cli", display_name: "Gemini CLI", binary: "gemini", default_args: &[], yolo_args: &["--yolo"], resume_flag: Some("--resume"), acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: Some(&["npm", "i", "-g", "@google/gemini-cli@latest"]) },
    EntrySeed { id: "amazon-q", display_name: "Amazon Q Developer", binary: "q", default_args: &["chat"], yolo_args: &["--trust-all-tools"], resume_flag: Some("--resume"), acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "continue", display_name: "Continue CLI", binary: "cn", default_args: &[], yolo_args: &["--auto"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "roo-code", display_name: "Roo Code CLI", binary: "roo", default_args: &[], yolo_args: &["--auto-approve"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "windsurf", display_name: "Windsurf Cascade CLI", binary: "windsurf-cli", default_args: &[], yolo_args: &["--auto"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "bolt", display_name: "Bolt CLI", binary: "bolt", default_args: &[], yolo_args: &["--yes"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "cody", display_name: "Sourcegraph Cody", binary: "cody", default_args: &["chat"], yolo_args: &["--yes"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "sweep", display_name: "Sweep CLI", binary: "sweep", default_args: &[], yolo_args: &["--auto"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "mentat", display_name: "Mentat", binary: "mentat", default_args: &[], yolo_args: &["--auto-approve"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "plandex", display_name: "Plandex", binary: "plandex", default_args: &[], yolo_args: &["--auto-apply"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "forge", display_name: "Forge Code", binary: "forge", default_args: &[], yolo_args: &["--yes"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "tabby", display_name: "Tabby Agent", binary: "tabby-agent", default_args: &[], yolo_args: &["--yes"], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
    EntrySeed { id: "custom-cli", display_name: "Custom CLI Agent", binary: "sh", default_args: &[], yolo_args: &[], resume_flag: None, acp_dialect: None, native_driver_kind: NativeDriverKind::PtyTui, supports_hooks: false, is_bot_agent: false, auto_update_cmd: None },
];

/// Runtime agent catalog supporting 22 live Bot agents, 44 TUI entries, 16 ACP dialects,
/// retired agent migration (`gemini -> antigravity`), dynamic model probing, launch plan
/// synthesis, and shared desktop/mobile `/` slash commands.
#[derive(Debug, Clone)]
pub struct AgentCatalog {
    entries: BTreeMap<String, AgentCatalogEntry>,
    binary_overrides: BTreeMap<String, String>,
    slash_commands: Vec<SlashCommandEntry>,
}

impl Default for AgentCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentCatalog {
    /// Constructs the built-in catalog containing all 44 TUI entries (22 live Bot agents +
    /// 22 extended TUI agents) and the default shared `/` slash-command registry.
    pub fn new() -> Self {
        let mut entries = BTreeMap::new();
        for seed in CATALOG_SEEDS {
            let entry = seed_to_entry(seed);
            entries.insert(entry.id.clone(), entry);
        }
        Self {
            entries,
            binary_overrides: BTreeMap::new(),
            slash_commands: default_slash_commands(),
        }
    }

    /// Returns all 44 TUI catalog entries in deterministic ID order.
    pub fn entries(&self) -> Vec<&AgentCatalogEntry> {
        self.entries.values().collect()
    }

    /// Returns the exact 22 live Bot / Native-Chat CLI agents matching `BotAgentId::ALL`.
    pub fn bot_agents(&self) -> Vec<&AgentCatalogEntry> {
        BotAgentId::ALL
            .iter()
            .filter_map(|id| self.entries.get(id.as_str()))
            .collect()
    }

    /// Resolves an agent ID, transparently migrating retired IDs (e.g., `"gemini"` -> `"antigravity"`)
    /// via `orca_core::bot_types::migrate_retired_agent_id`.
    pub fn resolve_agent(&self, raw_id: &str) -> Result<(&AgentCatalogEntry, Option<String>)> {
        let trimmed = raw_id.trim();
        if trimmed.is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "agent_id cannot be empty",
            ));
        }
        if let Some((bot_id, migrated)) = migrate_retired_agent_id(trimmed)
            && let Some(entry) = self.entries.get(bot_id.as_str())
        {
            let migrated_from = migrated.then(|| trimmed.to_string());
            return Ok((entry, migrated_from));
        }
        let lower = trimmed.to_ascii_lowercase();
        let entry = self
            .entries
            .get(&lower)
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("unknown agent '{raw_id}'")))?;
        Ok((entry, None))
    }

    /// Registers an explicit binary override for `agent_id`.
    pub fn set_binary_override(&mut self, agent_id: &str, binary_path: impl Into<String>) -> Result<()> {
        let resolved_id = self.resolve_agent(agent_id)?.0.id.clone();
        self.binary_overrides.insert(resolved_id, binary_path.into());
        Ok(())
    }

    /// Updates dynamic models & effort levels discovered from a live agent probe.
    pub fn register_dynamic_models(
        &mut self,
        agent_id: &str,
        models: Vec<AgentModelOption>,
    ) -> Result<()> {
        let resolved_id = self.resolve_agent(agent_id)?.0.id.clone();
        if let Some(entry) = self.entries.get_mut(&resolved_id) {
            let mut effort_set = std::collections::BTreeSet::new();
            for m in &models {
                for e in &m.efforts {
                    effort_set.insert(e.clone());
                }
            }
            if !effort_set.is_empty() {
                entry.efforts = effort_set.into_iter().collect();
            }
            entry.models = models;
        }
        Ok(())
    }

    /// Builds a complete CLI launch plan (binary override resolution, model/effort args,
    /// yolo args on `FullAccess`, resume flag, and merged env vars).
    pub fn build_launch_plan(
        &self,
        agent_id: &str,
        launch: &BotLaunchConfig,
        permission_mode: Option<BotPermissionMode>,
        resume_handle: Option<&str>,
        extra_env: &BTreeMap<String, String>,
    ) -> Result<AgentLaunchPlan> {
        let (entry, migrated_from) = self.resolve_agent(agent_id)?;

        let binary = self
            .binary_overrides
            .get(&entry.id)
            .cloned()
            .or_else(|| extra_env.get(&entry.binary_override_env).cloned())
            .or_else(|| std::env::var(&entry.binary_override_env).ok().filter(|v| !v.trim().is_empty()))
            .unwrap_or_else(|| entry.binary.clone());

        let mut args = entry.default_args.clone();
        let selected_model = launch
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string)
            .or_else(|| entry.models.first().map(|m| m.id.clone()));

        let model_opt = selected_model
            .as_deref()
            .and_then(|mid| entry.models.iter().find(|m| m.id == mid));

        let selected_effort = launch
            .effort
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .or_else(|| model_opt.and_then(|m| m.default_effort.clone()));

        if let Some(ref model) = selected_model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        if let Some(ref effort) = selected_effort {
            if entry.id == "codex" {
                args.push("-c".to_string());
                args.push(format!("model_reasoning_effort={effort}"));
            } else {
                args.push("--effort".to_string());
                args.push(effort.clone());
            }
        }

        let yolo_enabled = matches!(permission_mode, Some(BotPermissionMode::FullAccess));
        if yolo_enabled {
            args.extend(entry.yolo_args.iter().cloned());
        }

        if let Some(handle) = resume_handle.map(str::trim).filter(|h| !h.is_empty()) {
            let flag = entry.resume_flag.as_deref().ok_or_else(|| {
                OrcaIdeRuntimeError::invalid_argument(format!(
                    "agent '{}' does not support session resume",
                    entry.id
                ))
            })?;
            for tok in flag.split_whitespace() {
                args.push(tok.to_string());
            }
            args.push(handle.to_string());
        }

        args.extend(launch.extra_args.iter().cloned());

        let mut env = entry.default_env.clone();
        for (k, v) in extra_env {
            env.insert(k.clone(), v.clone());
        }

        let slash_model_command = selected_model.as_ref().map(|m| format!("/model {m}"));

        Ok(AgentLaunchPlan {
            resolved_agent_id: entry.id.clone(),
            migrated_from,
            binary,
            args,
            env,
            native_driver_kind: entry.native_driver_kind,
            acp_dialect: entry.acp_dialect,
            selected_model,
            selected_effort,
            slash_model_command,
            yolo_enabled,
        })
    }

    /// Registers a skill command into the shared `/` slash-command registry.
    pub fn register_skill_command(
        &mut self,
        skill_id: impl Into<String>,
        description: impl Into<String>,
        surface: SlashCommandSurface,
    ) {
        let skill_id = skill_id.into();
        let command = format!("/skill:{skill_id}");
        self.slash_commands.retain(|c| c.command != command);
        self.slash_commands.push(SlashCommandEntry {
            usage: format!("{command} [args]"),
            command,
            description: description.into(),
            category: SlashCommandCategory::Skill,
            surface,
            supported_agents: Vec::new(),
            skill_id: Some(skill_id),
        });
    }

    /// Resolves shared `/` slash commands filtered by agent, surface (`Desktop`/`Mobile`/`Both`),
    /// and optional prefix/substring query.
    pub fn resolve_slash_commands(
        &self,
        agent_id: Option<&str>,
        surface: SlashCommandSurface,
        query: Option<&str>,
    ) -> Vec<SlashCommandEntry> {
        let resolved_agent = agent_id.and_then(|raw| self.resolve_agent(raw).ok().map(|(e, _)| e.id.as_str()));
        let q = query.map(|s| s.trim().to_ascii_lowercase());

        self.slash_commands
            .iter()
            .filter(|entry| {
                let surface_ok = match (surface, entry.surface) {
                    (SlashCommandSurface::Both, _) | (_, SlashCommandSurface::Both) => true,
                    (a, b) => a == b,
                };
                if !surface_ok {
                    return false;
                }
                if let Some(aid) = resolved_agent
                    && !entry.supported_agents.is_empty()
                    && !entry.supported_agents.iter().any(|a| a == aid)
                {
                    return false;
                }
                if let Some(ref needle) = q
                    && !needle.is_empty()
                {
                    let cmd_match = entry.command.to_ascii_lowercase().contains(needle);
                    let desc_match = entry.description.to_ascii_lowercase().contains(needle);
                    if !cmd_match && !desc_match {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect()
    }
}

fn default_slash_commands() -> Vec<SlashCommandEntry> {
    let items: [(&str, &str, &str, SlashCommandCategory, SlashCommandSurface); 12] = [
        ("/clear", "Clear context window and start a new journal epoch", "/clear", SlashCommandCategory::Session, SlashCommandSurface::Both),
        ("/model", "Switch active model or reasoning effort", "/model <model-id>", SlashCommandCategory::Model, SlashCommandSurface::Both),
        ("/resume", "Resume session in a new Chat or Terminal view", "/resume [chat|terminal]", SlashCommandCategory::Session, SlashCommandSurface::Both),
        ("/fork", "Fork current conversation into a new branch session", "/fork", SlashCommandCategory::Session, SlashCommandSurface::Both),
        ("/compact", "Compact conversation context while keeping summary", "/compact", SlashCommandCategory::Session, SlashCommandSurface::Both),
        ("/diff", "Open or summarize current workspace diff review", "/diff", SlashCommandCategory::Workspace, SlashCommandSurface::Desktop),
        ("/plan", "Enter structured implementation planning mode", "/plan [goal]", SlashCommandCategory::Agent, SlashCommandSurface::Both),
        ("/review", "Run code review over staged or branch changes", "/review", SlashCommandCategory::Workspace, SlashCommandSurface::Both),
        ("/status", "Inspect agent status, subagents, and prompt-cache TTL", "/status", SlashCommandCategory::Agent, SlashCommandSurface::Both),
        ("/agents", "List available CLI/TUI/ACP agents and models", "/agents", SlashCommandCategory::Agent, SlashCommandSurface::Both),
        ("/orchestrate", "Create or inspect multi-agent orchestration runs", "/orchestrate [status]", SlashCommandCategory::Orchestration, SlashCommandSurface::Both),
        ("/bot", "Interact with configured Orca Bots and items", "/bot <action>", SlashCommandCategory::Bot, SlashCommandSurface::Both),
    ];
    items
        .into_iter()
        .map(|(command, description, usage, category, surface)| SlashCommandEntry {
            command: command.to_string(),
            description: description.to_string(),
            usage: usage.to_string(),
            category,
            surface,
            supported_agents: Vec::new(),
            skill_id: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn exact_catalog_counts_and_acp_dialects() {
        let catalog = AgentCatalog::new();
        assert_eq!(catalog.entries().len(), 44);
        assert_eq!(catalog.bot_agents().len(), 22);
        assert_eq!(AcpDialect::ALL.len(), 16);

        // Verify all 16 ACP dialects are wired into catalog entries.
        let wired_dialects: BTreeSet<AcpDialect> = catalog
            .entries()
            .into_iter()
            .filter_map(|e| e.acp_dialect)
            .collect();
        assert_eq!(wired_dialects.len(), 16);

        // Verify native drivers for claude and codex.
        let (claude, _) = catalog.resolve_agent("claude").expect("claude");
        assert_eq!(claude.native_driver_kind, NativeDriverKind::ClaudeSdk);
        let (codex, _) = catalog.resolve_agent("codex").expect("codex");
        assert_eq!(codex.native_driver_kind, NativeDriverKind::CodexAppServer);
    }

    #[test]
    fn retired_gemini_migrates_to_antigravity_and_launch_plans_work() {
        let mut catalog = AgentCatalog::new();
        let (resolved, migrated_from) = catalog.resolve_agent("gemini").expect("migrate gemini");
        assert_eq!(resolved.id, "antigravity");
        assert_eq!(migrated_from.as_deref(), Some("gemini"));

        // Grok 4.6 Extra high vs Grok 4.5 High defaults.
        let grok46 = catalog
            .build_launch_plan(
                "grok",
                &BotLaunchConfig {
                    model: Some("grok-4.6".into()),
                    effort: None,
                    extra_args: vec![],
                },
                Some(BotPermissionMode::FullAccess),
                Some("sess-g1"),
                &BTreeMap::new(),
            )
            .expect("grok 4.6");
        assert_eq!(grok46.selected_effort.as_deref(), Some("extra_high"));
        assert!(grok46.yolo_enabled);
        assert!(grok46.args.contains(&"--yolo".to_string()));
        assert!(grok46.args.contains(&"--resume".to_string()));
        assert!(grok46.args.contains(&"sess-g1".to_string()));

        let grok45 = catalog
            .build_launch_plan(
                "grok",
                &BotLaunchConfig {
                    model: Some("grok-4.5".into()),
                    effort: None,
                    extra_args: vec![],
                },
                None,
                None,
                &BTreeMap::new(),
            )
            .expect("grok 4.5");
        assert_eq!(grok45.selected_effort.as_deref(), Some("high"));

        // Codex `/model <id>` + binary override + reasoning effort flag.
        catalog
            .set_binary_override("codex", "/opt/bin/codex-custom")
            .expect("override");
        let codex_plan = catalog
            .build_launch_plan(
                "codex",
                &BotLaunchConfig {
                    model: Some("gpt-5.4".into()),
                    effort: Some("xhigh".into()),
                    extra_args: vec!["--search".into()],
                },
                Some(BotPermissionMode::Ask),
                Some("thread-9"),
                &BTreeMap::new(),
            )
            .expect("codex plan");
        assert_eq!(codex_plan.binary, "/opt/bin/codex-custom");
        assert_eq!(
            codex_plan.slash_model_command.as_deref(),
            Some("/model gpt-5.4")
        );
        assert!(codex_plan.args.contains(&"model_reasoning_effort=xhigh".to_string()));
        assert!(codex_plan.args.contains(&"resume".to_string()));
        assert!(codex_plan.args.contains(&"--search".to_string()));
    }

    #[test]
    fn shared_slash_commands_and_skills_filter_across_surfaces() {
        let mut catalog = AgentCatalog::new();
        catalog.register_skill_command(
            "triage-pr",
            "Triage incoming pull requests",
            SlashCommandSurface::Both,
        );

        let mobile_cmds = catalog.resolve_slash_commands(
            Some("claude"),
            SlashCommandSurface::Mobile,
            None,
        );
        assert!(mobile_cmds.iter().any(|c| c.command == "/clear"));
        assert!(mobile_cmds.iter().any(|c| c.command == "/skill:triage-pr"));
        // `/diff` is Desktop-only.
        assert!(!mobile_cmds.iter().any(|c| c.command == "/diff"));

        let desktop_diff = catalog.resolve_slash_commands(
            Some("claude"),
            SlashCommandSurface::Desktop,
            Some("diff"),
        );
        assert_eq!(desktop_diff.len(), 1);
        assert_eq!(desktop_diff[0].command, "/diff");
    }
}
