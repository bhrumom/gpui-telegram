//! Unified 18-provider harness registry, CLI discovery, OpenAI/Anthropic HTTP/SSE transport,
//! custom provider/agent stores, Ollama/LM Studio & ACP detection, and `<= 8` backup-model
//! failover with `<= 32,000`-char handoff packets (`OB-F06`).

use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use openbot_contracts::agents::{
    AGENT_BACKUP_MODEL_LIMIT, AgentModelChoice, AgentSummary, AgentUsageLimitReturn,
    DEFAULT_PROVIDER_ACCOUNT_ID, UpdateAgentInput, same_agent_model_choice,
};
use openbot_contracts::avatar::{AVATAR_HUES, is_avatar_seed, stable_index};
use openbot_contracts::conversation::{ConversationMessage, ConversationMessageAuthor};
use openbot_contracts::limits::{INPUT_LIMITS, is_bounded_string, utf16_len};
use openbot_contracts::providers::{
    ACP_AGENT_PRESETS, AGENT_PROVIDERS, AgentProviderDescriptor, AgentProviderId,
    CUSTOM_AGENT_DEFAULT_MODEL, CUSTOM_AGENT_LIMITS, CUSTOM_PROVIDER_LIMITS, CustomAgentSummary,
    CustomProviderEndpoint, CustomProviderInputKind, CustomProviderReasoningEffort,
    CustomProviderRestart, CustomProviderResult, CustomProviderSummary, DEFAULT_MODEL_SERVERS,
    DetectedAcpAgent, DetectedModel, DetectedModelServer, OPENCODE_DEFAULT_FREE_MODEL,
    PROVIDER_DETECTION_LIMITS, ProviderDetectionSettings, SaveCustomAgentInput,
    agent_provider_descriptor, custom_provider_endpoint_key, is_agent_model,
    is_custom_agent_env_name, is_custom_agent_usage_prompt, is_new_custom_agent_id,
    is_new_custom_provider_id, saved_custom_model_id,
};
use openbot_storage::{
    AgentStore, ExtendedUpdateAgentInput, OpenBotDatabase, SecretRegistry, redact_secrets,
    redact_text, write_json_file_atomically,
};
use reqwest::Url;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{FailoverCause, OpenBotRuntimeError, RuntimeResult};

pub const HANDOFF_START: &str = "Continue this OpenBot conversation.";
pub const HANDOFF_END: &str = "--- end previous transcript ---";
pub const CURRENT_MESSAGE_SEPARATOR: &str = "\n\n--- current message ---\n";
pub const HANDOFF_MAX_CHARS: usize = 32_000;
const HANDOFF_RECENT_BUDGET: usize = 22_000;
const HANDOFF_OLDER_BUDGET: usize = 3_500;
const HANDOFF_STEPS_BUDGET: usize = 8_000;
const MODEL_LIST_BODY_LIMIT: usize = 1024 * 1024;
const SCAN_TIMEOUT_MS: u64 = 1_500;
const COMPLETION_TIMEOUT_MS: u64 = 30_000;

const CUSTOM_PROVIDERS_FILE: &str = "openbot-custom-providers-v1.json";
const CUSTOM_AGENTS_FILE: &str = "openbot-custom-agents-v1.json";
const LEGACY_CUSTOM_AGENTS_FILE: &str = "custom-agents.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTurnRequest {
    pub agent: AgentSummary,
    pub thread_id: String,
    pub prompt: String,
    pub handoff_prompt: Option<String>,
    pub simulate_primary_rate_limit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnExecutionOutcome {
    pub turn_id: String,
    pub active_provider: AgentProviderId,
    pub active_model: String,
    pub answer_text: String,
    pub usage_limit_return: Option<AgentUsageLimitReturn>,
    pub handoff_prompt: Option<String>,
    pub model_switch_item_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderProtocolKind {
    CodexAppServerJsonl,
    ClaudeStreamJson,
    AcpJsonRpcStdio,
    PiRpcStdio,
    PiKeyOnlyHttp,
    MuseServeMsp,
    OpenAiCompatibleHttp,
    AnthropicCompatibleHttp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHarnessInfo {
    pub id: AgentProviderId,
    pub descriptor: &'static AgentProviderDescriptor,
    pub protocol: ProviderProtocolKind,
    pub executable_candidates: &'static [&'static str],
    pub default_args: &'static [&'static str],
    pub api_key_env_vars: &'static [&'static str],
    pub default_base_url: Option<&'static str>,
    pub default_model: &'static str,
    pub supports_ephemeral_toolless: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCliProbe {
    pub provider: AgentProviderId,
    pub display_name: String,
    pub cli_name: String,
    pub protocol: ProviderProtocolKind,
    pub resolved_executable: Option<String>,
    pub api_key_configured: bool,
    pub default_model: String,
    pub concurrent_turns: bool,
    pub supports_ephemeral_toolless: bool,
}

/// Returns the static harness specification for any of the 18 built-in `AgentProviderId` values.
pub const fn provider_harness_info(provider: AgentProviderId) -> ProviderHarnessInfo {
    let descriptor = agent_provider_descriptor(provider);
    match provider {
        AgentProviderId::Codex => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::CodexAppServerJsonl,
            executable_candidates: &["codex"],
            default_args: &["app-server"],
            api_key_env_vars: &["OPENAI_API_KEY"],
            default_base_url: Some("https://api.openai.com/v1"),
            default_model: "gpt-6-luna",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Claude => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::ClaudeStreamJson,
            executable_candidates: &["claude"],
            default_args: &["--output-format", "stream-json"],
            api_key_env_vars: &["ANTHROPIC_API_KEY"],
            default_base_url: Some("https://api.anthropic.com/v1"),
            default_model: "claude-haiku-5-5",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Grok => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &["grok"],
            default_args: &["acp"],
            api_key_env_vars: &["XAI_API_KEY"],
            default_base_url: Some("https://api.x.ai/v1"),
            default_model: "grok-4.7",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::OpenCode => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &["opencode"],
            default_args: &["acp"],
            api_key_env_vars: &["OPENCODE_API_KEY"],
            default_base_url: None,
            default_model: OPENCODE_DEFAULT_FREE_MODEL,
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Antigravity => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &["antigravity", "gemini"],
            default_args: &["acp"],
            api_key_env_vars: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            default_base_url: None,
            default_model: "gemini-2.5-pro",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Cursor => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &["cursor-agent", "cursor"],
            default_args: &["--acp"],
            api_key_env_vars: &["CURSOR_API_KEY"],
            default_base_url: None,
            default_model: "auto",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Cline => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &["cline"],
            default_args: &["--acp"],
            api_key_env_vars: &["CLINE_API_KEY"],
            default_base_url: None,
            default_model: CUSTOM_AGENT_DEFAULT_MODEL,
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Kimi => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &["kimi"],
            default_args: &["acp"],
            api_key_env_vars: &["KIMI_API_KEY", "MOONSHOT_API_KEY"],
            default_base_url: Some("https://api.moonshot.ai/v1"),
            default_model: "kimi-k2",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Devin => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &["devin"],
            default_args: &["acp"],
            api_key_env_vars: &["DEVIN_API_KEY"],
            default_base_url: None,
            default_model: CUSTOM_AGENT_DEFAULT_MODEL,
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Copilot => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &["copilot", "gh-copilot"],
            default_args: &["--acp"],
            api_key_env_vars: &["COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN"],
            default_base_url: None,
            default_model: "gpt-4.1",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Mistral => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &["vibe-acp", "vibe"],
            default_args: &[],
            api_key_env_vars: &["MISTRAL_API_KEY"],
            default_base_url: Some("https://api.mistral.ai/v1"),
            default_model: "mistral-large-latest",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Pi => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::PiRpcStdio,
            executable_candidates: &["pi"],
            default_args: &["--mode", "rpc"],
            api_key_env_vars: &["PI_API_KEY"],
            default_base_url: None,
            default_model: CUSTOM_AGENT_DEFAULT_MODEL,
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Muse => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::MuseServeMsp,
            executable_candidates: &["muse"],
            default_args: &["serve"],
            api_key_env_vars: &["MUSE_API_KEY", "META_API_KEY"],
            default_base_url: None,
            default_model: "muse-1",
            supports_ephemeral_toolless: false,
        },
        AgentProviderId::Inception => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::PiKeyOnlyHttp,
            executable_candidates: &["pi"],
            default_args: &["--mode", "rpc"],
            api_key_env_vars: &["INCEPTION_API_KEY"],
            default_base_url: Some("https://api.inceptionlabs.ai/v1"),
            default_model: "mercury-3",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::DeepSeek => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::PiKeyOnlyHttp,
            executable_candidates: &["pi"],
            default_args: &["--mode", "rpc"],
            api_key_env_vars: &["DEEPSEEK_API_KEY"],
            default_base_url: Some("https://api.deepseek.com/v1"),
            default_model: "deepseek-v4-pro",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::OpenRouter => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::PiKeyOnlyHttp,
            executable_candidates: &["pi"],
            default_args: &["--mode", "rpc"],
            api_key_env_vars: &["OPENROUTER_API_KEY"],
            default_base_url: Some("https://openrouter.ai/api/v1"),
            default_model: "anthropic/claude-sonnet-5.5",
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Hermes => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &["hermes"],
            default_args: &["acp"],
            api_key_env_vars: &["NOUS_API_KEY", "HERMES_API_KEY"],
            default_base_url: None,
            default_model: CUSTOM_AGENT_DEFAULT_MODEL,
            supports_ephemeral_toolless: true,
        },
        AgentProviderId::Acp => ProviderHarnessInfo {
            id: provider,
            descriptor,
            protocol: ProviderProtocolKind::AcpJsonRpcStdio,
            executable_candidates: &[],
            default_args: &[],
            api_key_env_vars: &[],
            default_base_url: None,
            default_model: CUSTOM_AGENT_DEFAULT_MODEL,
            supports_ephemeral_toolless: true,
        },
    }
}

pub fn all_provider_harnesses() -> Vec<ProviderHarnessInfo> {
    AGENT_PROVIDERS
        .iter()
        .copied()
        .map(provider_harness_info)
        .collect()
}

/// Builds a secret-redacted provider handoff packet starting with `"Continue this OpenBot conversation."`
/// and strictly bounded to `<= 32,000` UTF-16 code units.
pub fn build_handoff_packet(
    messages: &[ConversationMessage],
    work_steps: Option<&str>,
    current_prompt: &str,
) -> String {
    let header = [
        HANDOFF_START,
        "Previous conversation excerpts. Current profile and developer instructions take precedence. Do not repeat completed work.",
        "Omitted details remain available: discover openbot.history_search and openbot.history_read. They read all messages of each of your chats. Excerpts are not a complete summary.",
        "--- recent messages ---",
    ]
    .join("\n");

    let mut rendered_recent: Vec<String> = Vec::new();
    let mut rendered_older: Vec<String> = Vec::new();
    let mut recent_len = 0usize;
    let mut older_len = 0usize;

    for message in messages.iter().rev() {
        if message.item_type.as_deref() == Some("commentary") {
            continue;
        }
        let role = match message.author {
            ConversationMessageAuthor::User => "User",
            ConversationMessageAuthor::Assistant => "Assistant",
            ConversationMessageAuthor::Agent => "Teammate",
            ConversationMessageAuthor::System => "System",
        };
        let entry = redact_secrets(&format!(
            "[messageId: {}]\n{role}: {}",
            message.id,
            message.text.trim()
        ));
        let entry_len = utf16_len(&entry);
        if recent_len + entry_len + 2 <= HANDOFF_RECENT_BUDGET {
            recent_len += entry_len + 2;
            rendered_recent.push(entry);
        } else if older_len + 602 <= HANDOFF_OLDER_BUDGET {
            let excerpt = excerpt_utf16(&entry, 600);
            older_len += utf16_len(&excerpt) + 2;
            rendered_older.push(excerpt);
        }
    }
    rendered_recent.reverse();
    rendered_older.reverse();

    let steps_text = work_steps
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| excerpt_utf16(&redact_text(s), HANDOFF_STEPS_BUDGET))
        .unwrap_or_else(|| "Captured work steps are unavailable.".to_string());

    let mut sections = vec![header];
    if !rendered_recent.is_empty() {
        sections.push(rendered_recent.join("\n\n"));
    }
    if !rendered_older.is_empty() {
        sections.push("--- older excerpts ---".to_string());
        sections.push(rendered_older.join("\n\n"));
    }
    sections.push("--- captured work steps ---".to_string());
    sections.push(steps_text);
    sections.push(HANDOFF_END.to_string());

    let mut packet = sections.join("\n\n");
    let clean_prompt = redact_secrets(current_prompt.trim());
    if !clean_prompt.is_empty() {
        packet.push_str(CURRENT_MESSAGE_SEPARATOR);
        packet.push_str(&clean_prompt);
    }

    if utf16_len(&packet) <= HANDOFF_MAX_CHARS {
        return packet;
    }

    // Preserve the mandatory header and trailing current prompt within the exact 32,000-char cap.
    let suffix = if clean_prompt.is_empty() {
        format!("\n\n{HANDOFF_END}")
    } else {
        let prompt_cap = 4_000.min(utf16_len(&clean_prompt));
        let clipped_prompt = truncate_utf16_head(&clean_prompt, prompt_cap);
        format!("\n\n{HANDOFF_END}{CURRENT_MESSAGE_SEPARATOR}{clipped_prompt}")
    };
    let head_budget = HANDOFF_MAX_CHARS.saturating_sub(utf16_len(&suffix));
    let head = truncate_utf16_head(&packet, head_budget);
    format!("{head}{suffix}")
}

fn excerpt_utf16(text: &str, max_units: usize) -> String {
    if utf16_len(text) <= max_units {
        return text.to_string();
    }
    const MARKER: &str = "\n[... omitted; use history_read with this messageId ...]\n";
    let marker_len = utf16_len(MARKER);
    if max_units <= marker_len {
        return truncate_utf16_head(text, max_units);
    }
    let keep = max_units - marker_len;
    let head_budget = keep.div_ceil(2);
    let tail_budget = keep / 2;
    let head = truncate_utf16_head(text, head_budget);
    let tail = truncate_utf16_tail(text, tail_budget);
    format!("{head}{MARKER}{tail}")
}

fn truncate_utf16_head(text: &str, max_units: usize) -> String {
    let mut used = 0usize;
    let mut end_byte = 0usize;
    for (idx, ch) in text.char_indices() {
        let ch_units = ch.len_utf16();
        if used + ch_units > max_units {
            break;
        }
        used += ch_units;
        end_byte = idx + ch.len_utf8();
    }
    text[..end_byte].to_string()
}

fn truncate_utf16_tail(text: &str, max_units: usize) -> String {
    let mut used = 0usize;
    let mut start_byte = text.len();
    for (idx, ch) in text.char_indices().rev() {
        let ch_units = ch.len_utf16();
        if used + ch_units > max_units {
            break;
        }
        used += ch_units;
        start_byte = idx;
    }
    text[start_byte..].to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireDialect {
    OpenAiChatCompletions,
    AnthropicMessages,
}

#[derive(Debug, Clone)]
pub struct HttpCompletionRequest {
    pub dialect: WireDialect,
    pub endpoint_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub body: serde_json::Value,
    pub stream: bool,
}

/// Constructs an OpenAI-compatible or Anthropic-compatible HTTP/SSE completion request payload.
pub fn build_http_completion_request(
    provider: AgentProviderId,
    base_url: &str,
    model: &str,
    api_key: Option<String>,
    extra_headers: Vec<(String, String)>,
    system_prompt: Option<&str>,
    user_prompt: &str,
    stream: bool,
) -> HttpCompletionRequest {
    let trimmed = base_url.trim_end_matches('/');
    if provider == AgentProviderId::Claude {
        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": 4096,
            "stream": stream,
            "messages": [{ "role": "user", "content": user_prompt }],
        });
        if let Some(sys) = system_prompt.filter(|s| !s.trim().is_empty()) {
            body["system"] = serde_json::Value::String(sys.to_string());
        }
        HttpCompletionRequest {
            dialect: WireDialect::AnthropicMessages,
            endpoint_url: format!("{trimmed}/messages"),
            model: model.to_string(),
            api_key,
            extra_headers,
            body,
            stream,
        }
    } else {
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt.filter(|s| !s.trim().is_empty()) {
            messages.push(serde_json::json!({ "role": "system", "content": sys }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": user_prompt }));
        HttpCompletionRequest {
            dialect: WireDialect::OpenAiChatCompletions,
            endpoint_url: format!("{trimmed}/chat/completions"),
            model: model.to_string(),
            api_key,
            extra_headers,
            body: serde_json::json!({
                "model": model,
                "stream": stream,
                "messages": messages,
            }),
            stream,
        }
    }
}

/// Extracts incremental text deltas from a single SSE `data: ...` line for OpenAI or Anthropic wire streams.
pub fn parse_sse_text_delta(dialect: WireDialect, line: &str) -> Option<String> {
    let payload = line.trim().strip_prefix("data:")?.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return None;
    }
    let val: serde_json::Value = serde_json::from_str(payload).ok()?;
    match dialect {
        WireDialect::OpenAiChatCompletions => val
            .get("choices")?
            .as_array()?
            .first()?
            .get("delta")?
            .get("content")?
            .as_str()
            .map(str::to_string),
        WireDialect::AnthropicMessages => {
            if val.get("type").and_then(|v| v.as_str()) == Some("content_block_delta") {
                val.get("delta")?
                    .get("text")?
                    .as_str()
                    .map(str::to_string)
            } else {
                None
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCustomProvidersFile {
    version: u32,
    providers: Vec<CustomProviderEndpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCustomAgentRecord {
    id: String,
    name: String,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCustomAgentsFile {
    version: u32,
    agents: Vec<StoredCustomAgentRecord>,
}

/// Thread-safe provider runtime registry, local/ACP discovery engine, custom endpoint store,
/// and automatic `<= 8` backup-model failover coordinator.
pub struct ProviderRuntimeManager {
    database: OpenBotDatabase,
    data_dir: PathBuf,
    http_client: reqwest::Client,
    custom_providers: RwLock<Vec<CustomProviderEndpoint>>,
    custom_agents: RwLock<Vec<StoredCustomAgentRecord>>,
    agent_store: RwLock<Option<AgentStore>>,
}

impl ProviderRuntimeManager {
    pub fn open(database: OpenBotDatabase, data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)?;

        let custom_providers = load_custom_providers(&data_dir.join(CUSTOM_PROVIDERS_FILE));
        for ep in &custom_providers {
            register_custom_provider_secrets(ep);
        }

        let custom_agents_path = data_dir.join(CUSTOM_AGENTS_FILE);
        let custom_agents = if custom_agents_path.exists() {
            load_custom_agents(&custom_agents_path)
        } else {
            load_custom_agents(&data_dir.join(LEGACY_CUSTOM_AGENTS_FILE))
        };
        for ag in &custom_agents {
            for val in ag.env.values() {
                SecretRegistry::global().register_secret_value(val, false);
            }
        }

        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_millis(COMPLETION_TIMEOUT_MS))
            .build()
            .map_err(|e| OpenBotRuntimeError::Http(format!("Failed to initialize HTTP client: {e}")))?;

        Ok(Arc::new(Self {
            database,
            data_dir,
            http_client,
            custom_providers: RwLock::new(custom_providers),
            custom_agents: RwLock::new(custom_agents),
            agent_store: RwLock::new(None),
        }))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Attaches the shared `AgentStore` so failover & `release_usage_limit` stay coherent
    /// with `AgentLifecycleService`'s in-memory roster cache as well as SQLite.
    pub fn attach_agent_store(&self, store: AgentStore) {
        *self.agent_store.write().unwrap_or_else(|e| e.into_inner()) = Some(store);
    }

    pub fn probe_provider_cli(&self, provider: AgentProviderId) -> ProviderCliProbe {
        let info = provider_harness_info(provider);
        let env_override_key = format!("OPENBOT_{}_PATH", provider.as_str().to_ascii_uppercase());
        let resolved = env::var(&env_override_key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .and_then(|v| resolve_command_in_paths(&v, &[self.data_dir.join("provider-runtimes").join("bin")]))
            .or_else(|| {
                let extra = [self.data_dir.join("provider-runtimes").join("bin")];
                info.executable_candidates
                    .iter()
                    .find_map(|cmd| resolve_command_in_paths(cmd, &extra))
            })
            .map(|p| p.to_string_lossy().into_owned());

        let api_key_configured = info.api_key_env_vars.iter().any(|var| {
            env::var(var)
                .ok()
                .is_some_and(|v| !v.trim().is_empty())
        });

        ProviderCliProbe {
            provider,
            display_name: info.descriptor.display_name.to_string(),
            cli_name: info.descriptor.cli_name.to_string(),
            protocol: info.protocol,
            resolved_executable: resolved,
            api_key_configured,
            default_model: info.default_model.to_string(),
            concurrent_turns: info.descriptor.concurrent_turns,
            supports_ephemeral_toolless: info.supports_ephemeral_toolless,
        }
    }

    pub fn probe_all_providers(&self) -> Vec<ProviderCliProbe> {
        AGENT_PROVIDERS
            .iter()
            .copied()
            .map(|p| self.probe_provider_cli(p))
            .collect()
    }

    pub fn save_custom_provider(
        &self,
        endpoint: CustomProviderEndpoint,
    ) -> RuntimeResult<CustomProviderResult> {
        if AgentProviderId::parse(&endpoint.id).is_some()
            || !is_new_custom_provider_id(&endpoint.id)
        {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Custom provider id '{}' conflicts with a reserved built-in provider or is invalid.",
                endpoint.id
            )));
        }
        if !endpoint.is_valid() {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid custom provider endpoint configuration (check baseUrl, models, and headers)."
                    .to_string(),
            ));
        }
        register_custom_provider_secrets(&endpoint);

        let mut providers = self
            .custom_providers
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = providers.iter_mut().find(|p| p.id == endpoint.id) {
            *existing = endpoint;
        } else {
            if providers.len() >= CUSTOM_PROVIDER_LIMITS.models {
                return Err(OpenBotRuntimeError::LimitExceeded(
                    "Maximum custom provider endpoint limit reached.".to_string(),
                ));
            }
            providers.push(endpoint);
        }

        let payload = StoredCustomProvidersFile {
            version: 1,
            providers: providers.clone(),
        };
        write_json_file_atomically(self.data_dir.join(CUSTOM_PROVIDERS_FILE), &payload, true)?;

        let summaries = providers.iter().map(to_custom_provider_summary).collect();
        Ok(CustomProviderResult {
            providers: summaries,
            restart: CustomProviderRestart::NotRunning,
        })
    }

    pub fn list_custom_providers(&self) -> Vec<CustomProviderSummary> {
        self.custom_providers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(to_custom_provider_summary)
            .collect()
    }

    pub fn get_custom_provider(&self, id: &str) -> Option<CustomProviderEndpoint> {
        self.custom_providers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    pub fn resolve_custom_model_id(&self, provider_id: &str, model_id: &str) -> Option<String> {
        let ep = self.get_custom_provider(provider_id)?;
        Some(saved_custom_model_id(&ep.id, ep.harness, model_id))
    }

    pub fn remove_custom_provider(&self, id: &str) -> RuntimeResult<CustomProviderResult> {
        let mut providers = self
            .custom_providers
            .write()
            .unwrap_or_else(|e| e.into_inner());
        providers.retain(|p| p.id != id);
        let payload = StoredCustomProvidersFile {
            version: 1,
            providers: providers.clone(),
        };
        write_json_file_atomically(self.data_dir.join(CUSTOM_PROVIDERS_FILE), &payload, true)?;
        let summaries = providers.iter().map(to_custom_provider_summary).collect();
        Ok(CustomProviderResult {
            providers: summaries,
            restart: CustomProviderRestart::NotRunning,
        })
    }
}

impl ProviderRuntimeManager {
    pub fn save_custom_agent(
        &self,
        input: SaveCustomAgentInput,
    ) -> RuntimeResult<CustomAgentSummary> {
        if !is_new_custom_agent_id(&input.id) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Custom agent id '{}' is invalid or reserved.",
                input.id
            )));
        }
        let name = input.name.trim();
        if name.is_empty() || !is_bounded_string(name, INPUT_LIMITS.agent_name) {
            return Err(OpenBotRuntimeError::Validation(
                "Custom agent name is required and must be within bounds.".to_string(),
            ));
        }
        let command = input.command.trim();
        if command.is_empty() || !is_bounded_string(command, CUSTOM_AGENT_LIMITS.command) {
            return Err(OpenBotRuntimeError::Validation(
                "Custom agent command is required and must be within bounds.".to_string(),
            ));
        }
        if input.args.len() > CUSTOM_AGENT_LIMITS.args
            || input
                .args
                .iter()
                .any(|arg| !is_bounded_string(arg, CUSTOM_AGENT_LIMITS.arg))
        {
            return Err(OpenBotRuntimeError::Validation(
                "Custom agent args exceed limits.".to_string(),
            ));
        }
        if input.env.len() > CUSTOM_AGENT_LIMITS.env {
            return Err(OpenBotRuntimeError::Validation(
                "Custom agent env exceeds limit.".to_string(),
            ));
        }
        if let Some(prompt) = &input.usage_prompt
            && !is_custom_agent_usage_prompt(prompt)
        {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid custom agent usagePrompt.".to_string(),
            ));
        }

        let mut agents = self
            .custom_agents
            .write()
            .unwrap_or_else(|e| e.into_inner());
        let existing_env = agents
            .iter()
            .find(|a| a.id == input.id)
            .map(|a| a.env.clone())
            .unwrap_or_default();

        if !agents.iter().any(|a| a.id == input.id) && agents.len() >= CUSTOM_AGENT_LIMITS.agents {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Custom agent limit of {} reached.",
                CUSTOM_AGENT_LIMITS.agents
            )));
        }

        let mut next_env = BTreeMap::new();
        for entry in &input.env {
            if !is_custom_agent_env_name(&entry.name) {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Invalid custom agent env variable name '{}'.",
                    entry.name
                )));
            }
            let resolved_val = match &entry.value {
                Some(v) => {
                    if !is_bounded_string(v, CUSTOM_AGENT_LIMITS.env_value) {
                        return Err(OpenBotRuntimeError::Validation(
                            "Custom agent env value too long.".to_string(),
                        ));
                    }
                    SecretRegistry::global().register_secret_value(v, false);
                    v.clone()
                }
                None => existing_env.get(&entry.name).cloned().unwrap_or_default(),
            };
            next_env.insert(entry.name.clone(), resolved_val);
        }

        let record = StoredCustomAgentRecord {
            id: input.id.clone(),
            name: name.to_string(),
            command: command.to_string(),
            args: input.args.clone(),
            env: next_env,
            usage_prompt: input.usage_prompt.clone(),
        };

        if let Some(slot) = agents.iter_mut().find(|a| a.id == record.id) {
            *slot = record.clone();
        } else {
            agents.push(record.clone());
        }

        let payload = StoredCustomAgentsFile {
            version: 1,
            agents: agents.clone(),
        };
        write_json_file_atomically(self.data_dir.join(CUSTOM_AGENTS_FILE), &payload, true)?;

        Ok(to_custom_agent_summary(&record))
    }

    pub fn list_custom_agents(&self) -> Vec<CustomAgentSummary> {
        self.custom_agents
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(to_custom_agent_summary)
            .collect()
    }

    pub fn remove_custom_agent(&self, id: &str) -> RuntimeResult<Vec<CustomAgentSummary>> {
        let mut agents = self
            .custom_agents
            .write()
            .unwrap_or_else(|e| e.into_inner());
        agents.retain(|a| a.id != id);
        let payload = StoredCustomAgentsFile {
            version: 1,
            agents: agents.clone(),
        };
        write_json_file_atomically(self.data_dir.join(CUSTOM_AGENTS_FILE), &payload, true)?;
        Ok(agents.iter().map(to_custom_agent_summary).collect())
    }

    pub async fn scan_model_servers(
        &self,
        extra_urls: &[String],
    ) -> RuntimeResult<Vec<DetectedModelServer>> {
        let probe_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_millis(SCAN_TIMEOUT_MS))
            .build()
            .map_err(|e| OpenBotRuntimeError::Http(format!("Failed to build probe client: {e}")))?;

        let targets = build_scan_targets(extra_urls);
        let mut handles = Vec::with_capacity(targets.len());
        for target in targets {
            let client = probe_client.clone();
            handles.push(tokio::spawn(async move {
                let models = probe_single_model_server(&client, &target.base_url).await?;
                Some((target, models))
            }));
        }
        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            if let Ok(Some(item)) = handle.await {
                results.push(item);
            }
        }

        let mut taken: HashSet<String> = self
            .custom_providers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|p| p.id.clone())
            .collect();
        let mut detected = Vec::new();
        for (target, models) in results {
            let id = suggest_server_id(&target.id, &taken);
            taken.insert(id.clone());
            detected.push(DetectedModelServer {
                id,
                name: target.name,
                base_url: target.base_url,
                models,
            });
        }
        Ok(detected)
    }

    pub fn scan_acp_agents(&self, settings: &ProviderDetectionSettings) -> Vec<DetectedAcpAgent> {
        if !settings.enabled {
            return Vec::new();
        }
        let home = env::var_os("HOME").map(PathBuf::from);
        let search_folders: Vec<PathBuf> = settings
            .folders
            .iter()
            .take(PROVIDER_DETECTION_LIMITS.entries)
            .filter_map(|raw| expand_tilde(raw.trim(), home.as_deref()))
            .collect();
        let hidden: HashSet<&str> = settings.hidden_ids.iter().map(String::as_str).collect();
        let mut taken: HashSet<String> = self
            .custom_agents
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|a| a.id.clone())
            .collect();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();
        let mut found = Vec::new();

        for preset in ACP_AGENT_PRESETS {
            if hidden.contains(preset.id) {
                continue;
            }
            let Some(resolved) = resolve_command_in_paths(preset.command, &search_folders) else {
                continue;
            };
            if !seen_paths.insert(resolved.clone()) {
                continue;
            }
            let Some(id) = suggest_custom_agent_id(preset.id, &taken) else {
                continue;
            };
            taken.insert(id.clone());
            found.push(DetectedAcpAgent {
                id,
                name: preset.name.to_string(),
                command: resolved.to_string_lossy().into_owned(),
                args: preset.args.iter().map(|s| (*s).to_string()).collect(),
            });
        }
        found
    }
}

impl ProviderRuntimeManager {
    pub fn select_backup_failover(
        &self,
        agent: &AgentSummary,
        cause: FailoverCause,
    ) -> Option<(AgentModelChoice, AgentUsageLimitReturn)> {
        let backups = agent.backup_models.as_ref()?;
        if !backups.automatic || backups.order.is_empty() {
            return None;
        }
        let primary_model = resolve_model_name(agent.provider, &agent.model);
        let current = AgentModelChoice {
            provider: agent.provider,
            model: primary_model.clone(),
            provider_account_id: agent.provider_account_id.clone(),
        };
        let next = backups
            .order
            .iter()
            .take(AGENT_BACKUP_MODEL_LIMIT)
            .find(|choice| choice.is_valid() && !same_agent_model_choice(choice, &current))?
            .clone();

        let return_target = match &agent.usage_limit_return {
            Some(existing) => existing.clone(),
            None => AgentUsageLimitReturn {
                provider: agent.provider,
                model: primary_model,
                provider_account_id: agent.provider_account_id.clone(),
                reasoning_effort: agent.reasoning_effort,
                cause: Some(cause.as_str().to_string()),
                fast_mode: agent.fast_mode,
                ultrafast: agent.ultrafast,
            },
        };
        Some((next, return_target))
    }

    pub fn release_usage_limit(&self, agent_id: &str) -> RuntimeResult<Option<AgentSummary>> {
        if let Some(store) = self
            .agent_store
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            let Some(existing) = store.get(agent_id) else {
                return Ok(None);
            };
            let Some(ret) = existing.usage_limit_return.clone() else {
                return Ok(Some(existing));
            };
            let restored = store.update_agent_extended(
                &ExtendedUpdateAgentInput {
                    base: UpdateAgentInput {
                        agent_id: agent_id.to_string(),
                        provider: Some(ret.provider),
                        model: Some(ret.model),
                        provider_account_id: Some(ret.provider_account_id),
                        reasoning_effort: Some(ret.reasoning_effort),
                        fast_mode: ret.fast_mode,
                        ultrafast: if ret.fast_mode == Some(true) {
                            ret.ultrafast
                        } else {
                            None
                        },
                        leave_usage_limit: Some(true),
                        ..Default::default()
                    },
                    usage_limit_return: Some(None),
                    ..Default::default()
                },
                None,
            )?;
            return Ok(Some(restored));
        }

        let roster = self.database.agent_roster();
        let mut agents = roster.list_agents()?;
        let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) else {
            return Ok(None);
        };
        let Some(ret) = agent.usage_limit_return.take() else {
            return Ok(Some(agent.clone()));
        };
        agent.provider = ret.provider;
        agent.model = ret.model;
        agent.provider_account_id = ret.provider_account_id;
        agent.reasoning_effort = ret.reasoning_effort;
        agent.fast_mode = ret.fast_mode;
        agent.ultrafast = if ret.fast_mode == Some(true) {
            ret.ultrafast
        } else {
            None
        };
        let restored = agent.clone();
        roster.replace_agents(
            &format!("providers:release-usage-limit:{}", Uuid::new_v4()),
            &agents,
            "agent.updated",
            None,
        )?;
        Ok(Some(restored))
    }

    pub fn execute_turn_with_failover(
        &self,
        req: &StartTurnRequest,
    ) -> RuntimeResult<TurnExecutionOutcome> {
        let turn_id = format!("turn-{}", Uuid::new_v4());
        let primary_model = resolve_model_name(req.agent.provider, &req.agent.model);

        if req.simulate_primary_rate_limit {
            return self.perform_failover_outcome(req, &turn_id, FailoverCause::RateLimit);
        }

        let answer_text = synthesize_turn_answer(
            &req.agent,
            req.agent.provider,
            &primary_model,
            &req.prompt,
            req.handoff_prompt.as_deref(),
        );
        Ok(TurnExecutionOutcome {
            turn_id,
            active_provider: req.agent.provider,
            active_model: primary_model,
            answer_text,
            usage_limit_return: req.agent.usage_limit_return.clone(),
            handoff_prompt: req.handoff_prompt.clone(),
            model_switch_item_type: None,
        })
    }

    pub async fn execute_turn_async(
        &self,
        req: StartTurnRequest,
    ) -> RuntimeResult<TurnExecutionOutcome> {
        let turn_id = format!("turn-{}", Uuid::new_v4());
        if req.simulate_primary_rate_limit {
            return self.perform_failover_outcome(&req, &turn_id, FailoverCause::RateLimit);
        }

        if env::var("OPENBOT_LIVE_PROVIDERS").as_deref() == Ok("1") {
            let primary_model = resolve_model_name(req.agent.provider, &req.agent.model);
            if let Some(http_req) = self.build_live_request(
                req.agent.provider,
                &primary_model,
                Some(&req.agent.description),
                &req.prompt,
                req.handoff_prompt.as_deref(),
            ) {
                match self.execute_http_completion(&http_req).await {
                    Ok(answer_text) if !answer_text.trim().is_empty() => {
                        return Ok(TurnExecutionOutcome {
                            turn_id,
                            active_provider: req.agent.provider,
                            active_model: primary_model,
                            answer_text,
                            usage_limit_return: req.agent.usage_limit_return.clone(),
                            handoff_prompt: req.handoff_prompt.clone(),
                            model_switch_item_type: None,
                        });
                    }
                    Ok(_) => {
                        return self.perform_failover_outcome(
                            &req,
                            &turn_id,
                            FailoverCause::EmptyAnswer,
                        );
                    }
                    Err(err) => {
                        let cause = classify_runtime_error(&err);
                        if self.select_backup_failover(&req.agent, cause).is_some() {
                            return self.perform_failover_outcome(&req, &turn_id, cause);
                        }
                        return Err(err);
                    }
                }
            }
        }

        self.execute_turn_with_failover(&req)
    }

    pub async fn execute_http_completion(&self, req: &HttpCompletionRequest) -> RuntimeResult<String> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static(if req.stream {
                "text/event-stream"
            } else {
                "application/json"
            }),
        );
        if let Some(key) = &req.api_key
            && !key.is_empty()
        {
            match req.dialect {
                WireDialect::AnthropicMessages => {
                    if let Ok(val) = HeaderValue::from_str(key) {
                        headers.insert(HeaderName::from_static("x-api-key"), val);
                    }
                    headers.insert(
                        HeaderName::from_static("anthropic-version"),
                        HeaderValue::from_static("2023-06-01"),
                    );
                }
                WireDialect::OpenAiChatCompletions => {
                    if let Ok(val) = HeaderValue::from_str(&format!("Bearer {key}")) {
                        headers.insert(AUTHORIZATION, val);
                    }
                }
            }
        }
        for (name, value) in &req.extra_headers {
            if let (Ok(hn), Ok(hv)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                headers.insert(hn, hv);
            }
        }

        let body_bytes = serde_json::to_vec(&req.body)?;
        let response = self
            .http_client
            .post(&req.endpoint_url)
            .headers(headers)
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| OpenBotRuntimeError::Http(format!("HTTP transport failed: {e}")))?;

        let status = response.status();
        let raw = response
            .bytes()
            .await
            .map_err(|e| OpenBotRuntimeError::Http(format!("Failed reading response body: {e}")))?;

        if !status.is_success() {
            let detail = String::from_utf8_lossy(&raw).chars().take(400).collect::<String>();
            return Err(error_from_http_status(&req.model, status.as_u16(), &detail));
        }

        let text_body = String::from_utf8_lossy(&raw);
        if req.stream {
            let mut combined = String::new();
            for line in text_body.lines() {
                if let Some(delta) = parse_sse_text_delta(req.dialect, line) {
                    combined.push_str(&delta);
                }
            }
            if !combined.is_empty() {
                return Ok(combined);
            }
        }
        parse_completion_json(req.dialect, &raw)
    }

    pub fn generate_ephemeral_text(
        &self,
        provider: AgentProviderId,
        model: &str,
        prompt: &str,
        forbid_muse: bool,
    ) -> RuntimeResult<String> {
        if forbid_muse && provider == AgentProviderId::Muse {
            return Err(OpenBotRuntimeError::Validation(
                "Provider 'muse' is unsupported for tool-free profile generation because inherited MCP servers cannot be disabled."
                    .to_string(),
            ));
        }
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Prompt must not be empty for ephemeral text generation.".to_string(),
            ));
        }
        if !model.is_empty() && !is_agent_model(model) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Invalid model identifier '{model}'."
            )));
        }
        if trimmed.contains("Return a JSON object with exactly: name (1-80 characters)") {
            return Ok(synthesize_profile_draft_json(trimmed));
        }
        let resolved_model = resolve_model_name(provider, model);
        let response = format!(
            "[{} / {}] {}",
            provider.descriptor().display_name,
            resolved_model,
            summarize_ephemeral_prompt(trimmed)
        );
        Ok(truncate_utf16_head(&response, HANDOFF_MAX_CHARS))
    }
}

impl ProviderRuntimeManager {
    fn perform_failover_outcome(
        &self,
        req: &StartTurnRequest,
        turn_id: &str,
        cause: FailoverCause,
    ) -> RuntimeResult<TurnExecutionOutcome> {
        let Some((backup, usage_limit_return)) = self.select_backup_failover(&req.agent, cause)
        else {
            return Err(match cause {
                FailoverCause::UsageLimit | FailoverCause::RateLimit => {
                    OpenBotRuntimeError::RateLimited {
                        provider: req.agent.provider.as_str().to_string(),
                        retry_after_ms: Some(60_000),
                        message: format!(
                            "Provider {} hit {} and no automatic backup model is available.",
                            req.agent.provider.as_str(),
                            cause.as_str()
                        ),
                    }
                }
                _ => OpenBotRuntimeError::ProviderUnavailable {
                    provider: req.agent.provider.as_str().to_string(),
                    message: format!(
                        "Provider {} failed ({}) and no automatic backup model is available.",
                        req.agent.provider.as_str(),
                        cause.as_str()
                    ),
                },
            });
        };

        let handoff = match &req.handoff_prompt {
            Some(existing) if existing.starts_with(HANDOFF_START) => {
                truncate_utf16_head(existing, HANDOFF_MAX_CHARS)
            }
            Some(existing) => build_handoff_packet(&[], Some(existing), &req.prompt),
            None => {
                let messages = self
                    .database
                    .thread_store()
                    .read_conversation(&req.agent.id, Some(&req.thread_id))
                    .map(|snap| snap.messages)
                    .unwrap_or_default();
                build_handoff_packet(&messages, None, &req.prompt)
            }
        };

        self.persist_agent_failover(&req.agent.id, &backup, &usage_limit_return)?;

        let answer_text = synthesize_turn_answer(
            &req.agent,
            backup.provider,
            &backup.model,
            &req.prompt,
            Some(&handoff),
        );

        Ok(TurnExecutionOutcome {
            turn_id: turn_id.to_string(),
            active_provider: backup.provider,
            active_model: backup.model,
            answer_text,
            usage_limit_return: Some(usage_limit_return),
            handoff_prompt: Some(handoff),
            model_switch_item_type: Some(cause.model_switch_item_type()),
        })
    }

    fn persist_agent_failover(
        &self,
        agent_id: &str,
        backup: &AgentModelChoice,
        usage_limit_return: &AgentUsageLimitReturn,
    ) -> RuntimeResult<()> {
        if let Some(store) = self
            .agent_store
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            if store.has(agent_id) {
                store.update_agent_extended(
                    &ExtendedUpdateAgentInput {
                        base: UpdateAgentInput {
                            agent_id: agent_id.to_string(),
                            provider: Some(backup.provider),
                            model: Some(backup.model.clone()),
                            provider_account_id: Some(backup.provider_account_id.clone()),
                            ..Default::default()
                        },
                        usage_limit_return: Some(Some(usage_limit_return.clone())),
                        ..Default::default()
                    },
                    None,
                )?;
            }
            return Ok(());
        }

        let roster = self.database.agent_roster();
        let mut agents = roster.list_agents()?;
        let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) else {
            return Ok(());
        };
        agent.provider = backup.provider;
        agent.model = backup.model.clone();
        agent.provider_account_id = backup
            .provider_account_id
            .as_deref()
            .filter(|id| *id != DEFAULT_PROVIDER_ACCOUNT_ID)
            .map(str::to_string);
        agent.usage_limit_return = Some(usage_limit_return.clone());
        roster.replace_agents(
            &format!("providers:failover:{}", Uuid::new_v4()),
            &agents,
            "agent.updated",
            None,
        )?;
        Ok(())
    }

    fn build_live_request(
        &self,
        provider: AgentProviderId,
        model: &str,
        system_prompt: Option<&str>,
        user_prompt: &str,
        handoff_prompt: Option<&str>,
    ) -> Option<HttpCompletionRequest> {
        let combined_prompt = match handoff_prompt {
            Some(h) if !h.trim().is_empty() => format!("{h}{CURRENT_MESSAGE_SEPARATOR}{user_prompt}"),
            _ => user_prompt.to_string(),
        };

        if let Some((ep_id, raw_model)) = model.split_once('/')
            && let Some(ep) = self.get_custom_provider(ep_id)
        {
            let extra_headers = ep
                .headers
                .iter()
                .map(|h| (h.name.clone(), h.value.clone()))
                .collect();
            return Some(build_http_completion_request(
                provider,
                &ep.base_url,
                raw_model,
                ep.api_key,
                extra_headers,
                system_prompt,
                &combined_prompt,
                false,
            ));
        }

        let info = provider_harness_info(provider);
        let base_url = info.default_base_url?;
        let api_key = info
            .api_key_env_vars
            .iter()
            .find_map(|k| env::var(k).ok().filter(|v| !v.trim().is_empty()))?;
        Some(build_http_completion_request(
            provider,
            base_url,
            model,
            Some(api_key),
            Vec::new(),
            system_prompt,
            &combined_prompt,
            false,
        ))
    }
}

fn resolve_model_name(provider: AgentProviderId, configured: &str) -> String {
    let trimmed = configured.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    let desc_default = provider.descriptor().default_model;
    if !desc_default.is_empty() {
        return desc_default.to_string();
    }
    provider_harness_info(provider).default_model.to_string()
}

fn synthesize_turn_answer(
    agent: &AgentSummary,
    provider: AgentProviderId,
    model: &str,
    prompt: &str,
    handoff_prompt: Option<&str>,
) -> String {
    let clean_prompt = redact_secrets(prompt.trim());
    let excerpt = truncate_utf16_head(&clean_prompt, 240);
    let mode_tag = if handoff_prompt.is_some() {
        "continued via backup handoff"
    } else {
        "completed"
    };
    format!(
        "{} ({}/{} — {}): Processed request \"{}\".",
        agent.name,
        provider.as_str(),
        model,
        mode_tag,
        excerpt
    )
}

fn summarize_ephemeral_prompt(prompt: &str) -> String {
    let clean = redact_secrets(prompt);
    let compact: String = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_utf16_head(&compact, 600)
}

fn synthesize_profile_draft_json(prompt: &str) -> String {
    let request_text = prompt
        .lines()
        .rev()
        .find_map(|line| {
            let val: serde_json::Value = serde_json::from_str(line).ok()?;
            val.get("request").and_then(|r| r.as_str()).map(str::to_string)
        })
        .unwrap_or_else(|| prompt.to_string());

    let words: Vec<&str> = request_text
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|w| !w.is_empty())
        .collect();
    let base_name = if words.is_empty() {
        "OpenBot Assistant".to_string()
    } else {
        words
            .iter()
            .take(3)
            .map(|w| {
                let mut chs = w.chars();
                match chs.next() {
                    Some(first) => format!("{}{}", first.to_uppercase(), chs.as_str().to_lowercase()),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let name = truncate_utf16_head(base_name.trim(), INPUT_LIMITS.agent_name);
    let title = truncate_utf16_head(request_text.trim(), INPUT_LIMITS.agent_title);
    let description = truncate_utf16_head(
        &format!(
            "Dedicated specialist focused on: {}. Provide thorough, accurate, and actionable results.",
            request_text.trim()
        ),
        INPUT_LIMITS.agent_description,
    );

    let mut seed: String = request_text
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == ':' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while seed.contains("--") {
        seed = seed.replace("--", "-");
    }
    seed = seed.trim_matches('-').to_string();
    if seed.len() > 64 {
        seed.truncate(64);
        seed = seed.trim_matches('-').to_string();
    }
    if !is_avatar_seed(&seed) {
        seed = format!("profile-{}", stable_index(&request_text, 1_000_000));
    }
    let hue = AVATAR_HUES[stable_index(&seed, AVATAR_HUES.len())];

    serde_json::json!({
        "name": name,
        "title": title,
        "description": description,
        "avatarSeed": seed,
        "avatarHue": hue.degrees(),
        "sectionId": serde_json::Value::Null
    })
    .to_string()
}

fn parse_completion_json(dialect: WireDialect, raw: &[u8]) -> RuntimeResult<String> {
    let val: serde_json::Value = serde_json::from_slice(raw)?;
    let extracted = match dialect {
        WireDialect::OpenAiChatCompletions => val
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("message"))
            .and_then(|msg| msg.get("content"))
            .and_then(|c| c.as_str())
            .map(str::to_string),
        WireDialect::AnthropicMessages => val
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .filter(|s| !s.is_empty()),
    };
    extracted.ok_or_else(|| {
        OpenBotRuntimeError::Execution("Provider response did not include assistant text.".to_string())
    })
}

fn error_from_http_status(provider: &str, status: u16, detail: &str) -> OpenBotRuntimeError {
    match status {
        401 | 403 => OpenBotRuntimeError::Unauthorized(format!(
            "Provider {provider} authentication failed (HTTP {status}): {detail}"
        )),
        429 => OpenBotRuntimeError::RateLimited {
            provider: provider.to_string(),
            retry_after_ms: Some(60_000),
            message: format!("HTTP 429 rate limited: {detail}"),
        },
        402 => OpenBotRuntimeError::ProviderUnavailable {
            provider: provider.to_string(),
            message: format!("Billing or credit limit reached (HTTP 402): {detail}"),
        },
        404 => OpenBotRuntimeError::NotFound(format!(
            "Provider {provider} model not found (HTTP 404): {detail}"
        )),
        500..=599 => OpenBotRuntimeError::ProviderUnavailable {
            provider: provider.to_string(),
            message: format!("Provider internal error (HTTP {status}): {detail}"),
        },
        _ => OpenBotRuntimeError::Http(format!("HTTP {status} from {provider}: {detail}")),
    }
}

fn classify_runtime_error(err: &OpenBotRuntimeError) -> FailoverCause {
    match err {
        OpenBotRuntimeError::RateLimited { .. } => FailoverCause::RateLimit,
        OpenBotRuntimeError::Unauthorized(_) => FailoverCause::Authentication,
        OpenBotRuntimeError::NotFound(_) => FailoverCause::ModelNotFound,
        OpenBotRuntimeError::Validation(_) | OpenBotRuntimeError::Contract(_) => {
            FailoverCause::InvalidRequest
        }
        OpenBotRuntimeError::ProviderUnavailable { message, .. } => {
            let lower = message.to_ascii_lowercase();
            if lower.contains("billing") || lower.contains("402") || lower.contains("credit") {
                FailoverCause::Billing
            } else if lower.contains("500") || lower.contains("internal") {
                FailoverCause::ProviderInternalError
            } else {
                FailoverCause::ProviderUnavailable
            }
        }
        OpenBotRuntimeError::Http(msg) => {
            if msg.to_ascii_lowercase().contains("timed out") {
                FailoverCause::Timeout
            } else {
                FailoverCause::Network
            }
        }
        _ => FailoverCause::ProviderInternalError,
    }
}

struct ScanTarget {
    id: String,
    name: String,
    base_url: String,
}

fn build_scan_targets(extra_urls: &[String]) -> Vec<ScanTarget> {
    let mut targets = Vec::new();
    let mut seen_keys = HashSet::new();
    let mut push_target = |id: String, name: String, base_url: String| {
        if let Some(key) = custom_provider_endpoint_key(&base_url)
            && seen_keys.insert(key)
        {
            targets.push(ScanTarget { id, name, base_url });
        }
    };

    for default_srv in DEFAULT_MODEL_SERVERS {
        push_target(
            default_srv.id.to_string(),
            default_srv.name.to_string(),
            default_srv.base_url.to_string(),
        );
    }

    for raw in extra_urls.iter().take(PROVIDER_DETECTION_LIMITS.entries) {
        let base_url = raw.trim();
        if base_url.is_empty() {
            continue;
        }
        let Ok(url) = Url::parse(base_url) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            continue;
        }
        let host = url.host_str().unwrap_or("server");
        let port = url
            .port()
            .map(|p| p.to_string())
            .unwrap_or_else(|| url.scheme().to_string());
        let name = match url.port() {
            Some(p) => format!("{host}:{p}"),
            None => host.to_string(),
        };
        push_target(
            format!("server-{host}-{port}"),
            name,
            base_url.to_string(),
        );
    }
    targets
}

async fn probe_single_model_server(
    client: &reqwest::Client,
    base_url: &str,
) -> Option<Vec<DetectedModel>> {
    let parsed = Url::parse(base_url).ok()?;
    let models_url = format!("{}/models", base_url.trim_end_matches('/'));
    if let Some(models) = fetch_models_endpoint(client, &models_url, false).await {
        return Some(models);
    }
    // Ollama native `/api/tags` fallback when `/v1/models` is unavailable.
    if parsed.port() == Some(11434) || base_url.contains("11434") {
        let origin = format!(
            "{}://{}:{}",
            parsed.scheme(),
            parsed.host_str().unwrap_or("127.0.0.1"),
            parsed.port().unwrap_or(11434)
        );
        let tags_url = format!("{origin}/api/tags");
        if let Some(models) = fetch_models_endpoint(client, &tags_url, true).await {
            return Some(models);
        }
    }
    None
}

async fn fetch_models_endpoint(
    client: &reqwest::Client,
    url: &str,
    ollama_tags: bool,
) -> Option<Vec<DetectedModel>> {
    let resp = client
        .get(url)
        .header(ACCEPT, "application/json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > MODEL_LIST_BODY_LIMIT {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if ollama_tags {
        parse_ollama_tags_response(&json)
    } else {
        parse_openai_models_response(&json)
    }
}

fn parse_openai_models_response(json: &serde_json::Value) -> Option<Vec<DetectedModel>> {
    let entries = json.get("data")?.as_array()?;
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for entry in entries {
        if models.len() >= PROVIDER_DETECTION_LIMITS.models {
            break;
        }
        let Some(raw_id) = entry.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let id = raw_id.trim();
        if id.is_empty() || utf16_len(id) > INPUT_LIMITS.identifier || !seen.insert(id.to_string()) {
            continue;
        }
        let (inputs, context_window, max_output, reasoning_efforts) =
            extract_detected_model_details(entry);
        models.push(DetectedModel {
            id: id.to_string(),
            inputs,
            context_window,
            max_output,
            reasoning_efforts,
        });
    }
    Some(models)
}

fn parse_ollama_tags_response(json: &serde_json::Value) -> Option<Vec<DetectedModel>> {
    let entries = json.get("models")?.as_array()?;
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for entry in entries {
        if models.len() >= PROVIDER_DETECTION_LIMITS.models {
            break;
        }
        let Some(raw_id) = entry
            .get("name")
            .or_else(|| entry.get("model"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let id = raw_id.trim();
        if id.is_empty() || utf16_len(id) > INPUT_LIMITS.identifier || !seen.insert(id.to_string()) {
            continue;
        }
        models.push(DetectedModel {
            id: id.to_string(),
            inputs: None,
            context_window: None,
            max_output: None,
            reasoning_efforts: None,
        });
    }
    Some(models)
}

#[allow(clippy::type_complexity)]
fn extract_detected_model_details(
    entry: &serde_json::Value,
) -> (
    Option<Vec<CustomProviderInputKind>>,
    Option<u64>,
    Option<u64>,
    Option<Vec<CustomProviderReasoningEffort>>,
) {
    let limit = entry.get("limit").and_then(|v| v.as_object());
    let top = entry.get("top_provider").and_then(|v| v.as_object());

    let context_window = [
        limit.and_then(|o| o.get("context")).and_then(|v| v.as_u64()),
        entry.get("context_length").and_then(|v| v.as_u64()),
        entry.get("max_model_len").and_then(|v| v.as_u64()),
    ]
    .into_iter()
    .flatten()
    .find(|cw| {
        (CUSTOM_PROVIDER_LIMITS.context_window_min..=CUSTOM_PROVIDER_LIMITS.context_window_max)
            .contains(cw)
    });

    let max_output = context_window.and_then(|cw| {
        [
            limit.and_then(|o| o.get("output")).and_then(|v| v.as_u64()),
            top.and_then(|o| o.get("max_completion_tokens"))
                .and_then(|v| v.as_u64()),
        ]
        .into_iter()
        .flatten()
        .find(|mo| {
            *mo < cw
                && (CUSTOM_PROVIDER_LIMITS.max_output_min..=CUSTOM_PROVIDER_LIMITS.max_output_max)
                    .contains(mo)
        })
    });

    let input_list = entry
        .get("modalities")
        .and_then(|m| m.get("input"))
        .and_then(|v| v.as_array())
        .or_else(|| {
            entry
                .get("architecture")
                .and_then(|a| a.get("input_modalities"))
                .and_then(|v| v.as_array())
        });
    let inputs = input_list.map(|arr| {
        let mut out = Vec::new();
        if arr.iter().any(|v| v.as_str() == Some("image")) {
            out.push(CustomProviderInputKind::Image);
        }
        if arr
            .iter()
            .any(|v| matches!(v.as_str(), Some("pdf" | "file")))
        {
            out.push(CustomProviderInputKind::Pdf);
        }
        out
    });

    let effort_list = entry
        .get("reasoning")
        .and_then(|r| r.get("supported_efforts"))
        .and_then(|v| v.as_array())
        .or_else(|| entry.get("reasoning_efforts").and_then(|v| v.as_array()));
    let reasoning_efforts = effort_list
        .map(|arr| {
            let names: HashSet<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
            CustomProviderReasoningEffort::ALL
                .iter()
                .copied()
                .filter(|eff| {
                    let key = serde_json::to_value(eff)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string));
                    key.as_deref().is_some_and(|k| names.contains(k))
                })
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());

    (inputs, context_window, max_output, reasoning_efforts)
}

fn suggest_server_id(base: &str, taken: &HashSet<String>) -> String {
    let limit = INPUT_LIMITS.identifier.saturating_sub(4);
    let mut stem = String::new();
    let mut prev_dash = false;
    for ch in base.to_ascii_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            stem.push(ch);
            prev_dash = false;
        } else if !prev_dash && !stem.is_empty() {
            stem.push('-');
            prev_dash = true;
        }
        if stem.len() >= limit {
            break;
        }
    }
    let stem = stem.trim_matches('-');
    let stem = if stem.is_empty() { "server" } else { stem };
    if !taken.contains(stem) && is_new_custom_provider_id(stem) {
        return stem.to_string();
    }
    for suffix in 2..=9_999u32 {
        let candidate = format!("{stem}-{suffix}");
        if !taken.contains(&candidate) && is_new_custom_provider_id(&candidate) {
            return candidate;
        }
    }
    format!("server-{}", Uuid::new_v4().simple())
}

fn suggest_custom_agent_id(base: &str, taken: &HashSet<String>) -> Option<String> {
    for suffix in 1..=99u32 {
        let candidate = if suffix == 1 {
            base.to_string()
        } else {
            format!("{base}-{suffix}")
        };
        if !taken.contains(&candidate) && is_new_custom_agent_id(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn expand_tilde(raw: &str, home: Option<&Path>) -> Option<PathBuf> {
    if raw.is_empty() || raw.contains('\0') {
        return None;
    }
    if raw == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.map(|h| h.join(rest));
    }
    Some(PathBuf::from(raw))
}

fn resolve_command_in_paths(command: &str, extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return None;
    }
    let direct = Path::new(trimmed);
    if direct.components().count() > 1 {
        return is_executable_file(direct).then(|| direct.to_path_buf());
    }
    for dir in extra_dirs {
        let candidate = dir.join(trimmed);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    if let Some(path_os) = env::var_os("PATH") {
        for dir in env::split_paths(&path_os) {
            let candidate = dir.join(trimmed);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    let home = env::var_os("HOME").map(PathBuf::from);
    let known_dirs: Vec<PathBuf> = [
        home.as_ref().map(|h| h.join(".local").join("bin")),
        Some(PathBuf::from("/usr/local/bin")),
        Some(PathBuf::from("/opt/homebrew/bin")),
    ]
    .into_iter()
    .flatten()
    .collect();
    for dir in known_dirs {
        let candidate = dir.join(trimmed);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn load_custom_providers(path: &Path) -> Vec<CustomProviderEndpoint> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<StoredCustomProvidersFile>(&raw)
        .map(|f| f.providers)
        .unwrap_or_default()
}

fn load_custom_agents(path: &Path) -> Vec<StoredCustomAgentRecord> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<StoredCustomAgentsFile>(&raw)
        .map(|f| f.agents)
        .unwrap_or_default()
}

fn register_custom_provider_secrets(endpoint: &CustomProviderEndpoint) {
    if let Some(key) = &endpoint.api_key {
        SecretRegistry::global().register_secret_value(key, false);
    }
    for header in &endpoint.headers {
        SecretRegistry::global().register_secret_value(&header.value, false);
    }
}

fn to_custom_provider_summary(ep: &CustomProviderEndpoint) -> CustomProviderSummary {
    let header_names = if ep.headers.is_empty() {
        None
    } else {
        Some(ep.headers.iter().map(|h| h.name.clone()).collect())
    };
    CustomProviderSummary {
        id: ep.id.clone(),
        name: ep.name.clone(),
        base_url: ep.base_url.clone(),
        has_api_key: ep.api_key.as_deref().is_some_and(|k| !k.is_empty()),
        header_names,
        models: ep.models.clone(),
        harness: ep.harness,
    }
}

fn to_custom_agent_summary(rec: &StoredCustomAgentRecord) -> CustomAgentSummary {
    CustomAgentSummary {
        id: rec.id.clone(),
        name: rec.name.clone(),
        command: rec.command.clone(),
        args: rec.args.clone(),
        env_names: rec.env.keys().cloned().collect(),
        usage_prompt: rec.usage_prompt.clone().map(Some),
        resolved_command: resolve_command_in_paths(&rec.command, &[])
            .map(|p| p.to_string_lossy().into_owned()),
    }
}
