//! 4-State Agent Status Engine, Loopback HTTP/1.1 Hook Server, 13 Linear-Time `regex`
//! JSON Screen-Anchor Rule Packs, and OSC 7501 `programStatus` Parser (`F13`).
//!
//! Enforces strict 3-layer signal precedence:
//!   1. Native CLI Hook HTTP POST (`StatusSignalLayer::Hook`)
//!   2. ReDoS-safe linear-time `regex` JSON screen-anchor rules (`StatusSignalLayer::ScreenAnchor`)
//!   3. OSC 7501 `programStatus` terminal facets (`StatusSignalLayer::Osc7501`)

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use http::header::{HeaderName, HeaderValue};
use http::{HeaderMap, Method, StatusCode};
use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::{OrcaIdeRuntimeError, Result, now_epoch_ms};

/// Exact count of built-in linear-time `regex` JSON screen-anchor rule packs.
pub const BUILTIN_SCREEN_ANCHOR_PACK_COUNT: usize = 13;

/// Default window (ms) during which a higher-precedence signal layer locks out lower layers.
pub const DEFAULT_PRECEDENCE_WINDOW_MS: i64 = 30_000;

const MAX_HOOK_HEADER_BYTES: usize = 32 * 1024;
const MAX_HOOK_HEADERS: usize = 64;
const MAX_HOOK_BODY_BYTES: usize = 512 * 1024;
const HOOK_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// 4 core agent lifecycle states plus `Idle` display state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunState {
    Working,
    Blocked,
    Waiting,
    Done,
    #[default]
    Idle,
}

impl AgentRunState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Idle => "idle",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "working" | "running" | "busy" | "thinking" => Some(Self::Working),
            "blocked" | "permission" | "approval" | "needs_approval" => Some(Self::Blocked),
            "waiting" | "input" | "needs_input" | "question" => Some(Self::Waiting),
            "done" | "completed" | "finished" | "stopped" => Some(Self::Done),
            "idle" | "ready" => Some(Self::Idle),
            _ => None,
        }
    }
}

/// 3-layer signal precedence: `Hook (3) > ScreenAnchor (2) > Osc7501 (1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusSignalLayer {
    Osc7501 = 1,
    ScreenAnchor = 2,
    Hook = 3,
}

impl StatusSignalLayer {
    pub const fn rank(self) -> u8 {
        self as u8
    }
}

/// Live subagent status tracked inside `AgentSessionStatus::subagents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentStatus {
    pub subagent_id: String,
    pub state: AgentRunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub updated_at_ms: i64,
}

/// Selectable option inside an `AskUserQuestionPrompt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Structured interactive prompt surfaced by hooks or OSC 7501 `interactivePrompt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionPrompt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub question: String,
    #[serde(default)]
    pub options: Vec<AskUserQuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
}

/// Rich live status snapshot for an agent session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionStatus {
    pub session_id: String,
    pub agent_id: String,
    pub state: AgentRunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authoritative_layer: Option<StatusSignalLayer>,
    pub monitoring_mode: bool,
    pub session_boundary: bool,
    pub completion_alerts_suppressed: bool,
    #[serde(default)]
    pub subagents: BTreeMap<String, SubagentStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive_prompt: Option<AskUserQuestionPrompt>,
    pub restored_unconfirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_expires_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_hook_event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_matched_rule: Option<String>,
    pub updated_at_ms: i64,
}

impl AgentSessionStatus {
    /// Returns `true` only when the session is `Done` AND neither a `session_boundary`
    /// nor a `restored_unconfirmed` hydration flag suppresses completion alerts.
    pub fn should_emit_completion_alert(&self) -> bool {
        self.state == AgentRunState::Done
            && !self.session_boundary
            && !self.restored_unconfirmed
            && !self.completion_alerts_suppressed
    }

    /// Returns remaining milliseconds until prompt cache expiry, or `None` if untracked.
    pub fn prompt_cache_remaining_ms(&self, now_ms: i64) -> Option<i64> {
        self.prompt_cache_expires_at_ms
            .map(|exp| exp.saturating_sub(now_ms).max(0))
    }

    /// Returns `true` if any tracked subagent is currently `Working`, `Blocked`, or `Waiting`.
    pub fn has_active_subagents(&self) -> bool {
        self.subagents.values().any(|s| {
            matches!(
                s.state,
                AgentRunState::Working | AgentRunState::Blocked | AgentRunState::Waiting
            )
        })
    }
}

/// Broadcast transition emitted when `AgentStatusEngine` processes a signal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatusTransition {
    pub session_id: String,
    pub previous_state: AgentRunState,
    pub current_state: AgentRunState,
    pub signal_layer: StatusSignalLayer,
    pub state_applied: bool,
    pub completion_alert_emitted: bool,
    pub status: AgentSessionStatus,
}

/// Payload POSTed to `AgentHookServer` (`/hook`) by native agent CLI hooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEventPayload {
    pub session_id: String,
    pub agent_id: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<AgentRunState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_boundary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitoring_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_state: Option<AgentRunState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive_prompt: Option<AskUserQuestionPrompt>,
}

/// Declarative JSON screen-anchor rule pack compiled strictly via linear-time `regex::Regex`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenAnchorRulePack {
    pub agent_id: String,
    #[serde(default)]
    pub title_working: Vec<String>,
    #[serde(default)]
    pub title_waiting: Vec<String>,
    #[serde(default)]
    pub working_anchors: Vec<String>,
    #[serde(default)]
    pub blocked_anchors: Vec<String>,
    #[serde(default)]
    pub waiting_anchors: Vec<String>,
    #[serde(default)]
    pub done_anchors: Vec<String>,
}

/// Compiled ReDoS-safe linear-time `regex::Regex` screen-anchor matcher for a single agent.
#[derive(Debug, Clone)]
pub struct CompiledScreenAnchorPack {
    pub agent_id: String,
    title_working: Vec<Regex>,
    title_waiting: Vec<Regex>,
    working_anchors: Vec<Regex>,
    blocked_anchors: Vec<Regex>,
    waiting_anchors: Vec<Regex>,
    done_anchors: Vec<Regex>,
}

impl CompiledScreenAnchorPack {
    pub fn compile(pack: &ScreenAnchorRulePack) -> Result<Self> {
        let compile_all = |patterns: &[String]| -> Result<Vec<Regex>> {
            patterns
                .iter()
                .map(|p| Regex::new(p).map_err(OrcaIdeRuntimeError::from))
                .collect()
        };
        Ok(Self {
            agent_id: pack.agent_id.clone(),
            title_working: compile_all(&pack.title_working)?,
            title_waiting: compile_all(&pack.title_waiting)?,
            working_anchors: compile_all(&pack.working_anchors)?,
            blocked_anchors: compile_all(&pack.blocked_anchors)?,
            waiting_anchors: compile_all(&pack.waiting_anchors)?,
            done_anchors: compile_all(&pack.done_anchors)?,
        })
    }

    /// Evaluates window `title` and terminal `screen_text`, returning the matched state
    /// and matching regex pattern with priority: `Blocked > Working > Waiting > Done`.
    pub fn evaluate(
        &self,
        title: Option<&str>,
        screen_text: &str,
    ) -> Option<(AgentRunState, String)> {
        for rx in &self.blocked_anchors {
            if rx.is_match(screen_text) {
                return Some((AgentRunState::Blocked, rx.as_str().to_string()));
            }
        }
        if let Some(t) = title {
            for rx in &self.title_working {
                if rx.is_match(t) {
                    return Some((AgentRunState::Working, rx.as_str().to_string()));
                }
            }
        }
        for rx in &self.working_anchors {
            if rx.is_match(screen_text) {
                return Some((AgentRunState::Working, rx.as_str().to_string()));
            }
        }
        if let Some(t) = title {
            for rx in &self.title_waiting {
                if rx.is_match(t) {
                    return Some((AgentRunState::Waiting, rx.as_str().to_string()));
                }
            }
        }
        for rx in &self.waiting_anchors {
            if rx.is_match(screen_text) {
                return Some((AgentRunState::Waiting, rx.as_str().to_string()));
            }
        }
        for rx in &self.done_anchors {
            if rx.is_match(screen_text) {
                return Some((AgentRunState::Done, rx.as_str().to_string()));
            }
        }
        None
    }
}

/// Deserializes and compiles a JSON `ScreenAnchorRulePack` with linear-time `regex::Regex`.
pub fn compile_rule_pack_json(json: &str) -> Result<CompiledScreenAnchorPack> {
    let pack: ScreenAnchorRulePack = serde_json::from_str(json)?;
    CompiledScreenAnchorPack::compile(&pack)
}

/// All 13 built-in JSON screen-anchor rule packs (`claude`, `codex`, `grok`, `opencode`,
/// `cursor`, `copilot`, `hermes`, `kimi`, `kiro`, `goose`, `cline`, `aider`, `pi`).
pub const BUILTIN_SCREEN_ANCHOR_PACKS_JSON: [&str; BUILTIN_SCREEN_ANCHOR_PACK_COUNT] = [
    r#"{"agent_id":"claude","title_working":["[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏].*Claude","Claude.*Working"],"title_waiting":["Claude.*Waiting"],"working_anchors":["[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]\\s+(Thinking|Working|Running|Reading|Editing)","esc to interrupt"],"blocked_anchors":["Do you want to (proceed|allow|run)","Allow once.*Allow always","Permission required"],"waiting_anchors":["(?m)^❯\\s*$","(?m)^>\\s*Try "],"done_anchors":["Total cost:|Cost: \\$[0-9]+\\.[0-9]+","Turn complete"]}"#,
    r#"{"agent_id":"codex","title_working":["Codex.*(Working|Running)"],"title_waiting":["Codex.*Idle"],"working_anchors":["[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]\\s*(Working|Thinking|Executing)","Esc to interrupt"],"blocked_anchors":["Approve command\\?","Run command\\?.*\\[y/n\\]","Would you like to run"],"waiting_anchors":["(?m)^›\\s*$","Ask Codex anything"],"done_anchors":["Token usage:","codex finished"]}"#,
    r#"{"agent_id":"grok","title_working":["Grok.*Thinking"],"title_waiting":["Grok.*Ready"],"working_anchors":["Grok is thinking","[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]\\s+Grok"],"blocked_anchors":["Approve tool call\\?","Confirm execution \\[y/N\\]"],"waiting_anchors":["(?m)^grok>\\s*$","Ask Grok"],"done_anchors":["Completed in [0-9]+\\.?[0-9]*s"]}"#,
    r#"{"agent_id":"opencode","title_working":["OpenCode.*Working"],"title_waiting":["OpenCode.*Ready"],"working_anchors":["Generating\\.\\.\\.","Running tool:"],"blocked_anchors":["Allow tool execution\\?","Permission: .*\\[a\\]llow"],"waiting_anchors":["(?m)^opencode>\\s*$","Type a message"],"done_anchors":["Session idle","Done \\([0-9]+ tokens\\)"]}"#,
    r#"{"agent_id":"cursor","title_working":["Cursor.*Working"],"title_waiting":["Cursor.*Waiting"],"working_anchors":["Applying edits\\.\\.\\.","Searching codebase\\.\\.\\."],"blocked_anchors":["Approve shell command\\?","Allow edit\\?"],"waiting_anchors":["(?m)^cursor>\\s*$","Add a follow-up"],"done_anchors":["Edits applied","Task complete"]}"#,
    r#"{"agent_id":"copilot","title_working":["Copilot.*Working"],"title_waiting":["Copilot.*Waiting"],"working_anchors":["Thinking\\.\\.\\.","Running tool\\.\\.\\."],"blocked_anchors":["Allow this tool\\?","Confirm command \\[y/n\\]"],"waiting_anchors":["(?m)^copilot>\\s*$","Ask Copilot"],"done_anchors":["Copilot finished"]}"#,
    r#"{"agent_id":"hermes","title_working":["Hermes.*Working"],"title_waiting":["Hermes.*Ready"],"working_anchors":["Hermes running","[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]\\s+Hermes"],"blocked_anchors":["Approve action\\?","Confirm tool \\[y/n\\]"],"waiting_anchors":["(?m)^hermes>\\s*$"],"done_anchors":["Hermes task complete"]}"#,
    r#"{"agent_id":"kimi","title_working":["Kimi.*Thinking"],"title_waiting":["Kimi.*Ready"],"working_anchors":["Kimi thinking\\.\\.\\.","Executing tool"],"blocked_anchors":["Approve tool\\?","Confirm \\[y/N\\]"],"waiting_anchors":["(?m)^kimi>\\s*$"],"done_anchors":["Kimi response complete"]}"#,
    r#"{"agent_id":"kiro","title_working":["Kiro.*Working"],"title_waiting":["Kiro.*Ready"],"working_anchors":["Kiro executing","Working on task\\.\\.\\."],"blocked_anchors":["Trust this tool\\?","Approve execution \\[y/n\\]"],"waiting_anchors":["(?m)^kiro>\\s*$"],"done_anchors":["Kiro done"]}"#,
    r#"{"agent_id":"goose","title_working":["Goose.*Working"],"title_waiting":["Goose.*Ready"],"working_anchors":["goose is working","Running extension tool"],"blocked_anchors":["Allow tool call\\? \\(y/n\\)","Confirm tool"],"waiting_anchors":["(?m)^\\( O\\)>\\s*$","(?m)^goose>\\s*$"],"done_anchors":["Goose turn finished"]}"#,
    r#"{"agent_id":"cline","title_working":["Cline.*Working"],"title_waiting":["Cline.*Waiting"],"working_anchors":["Cline is thinking","Executing command\\.\\.\\."],"blocked_anchors":["Approve action\\?","Cline wants to (execute|edit)"],"waiting_anchors":["(?m)^cline>\\s*$","Type your task"],"done_anchors":["Task completed"]}"#,
    r#"{"agent_id":"aider","title_working":["aider.*Working"],"title_waiting":["aider.*Ready"],"working_anchors":["Waiting for LLM","Applying search/replace"],"blocked_anchors":["Run shell command\\? \\(Y\\)es/\\(N\\)o","Add .* to the chat\\?"],"waiting_anchors":["(?m)^>\\s*$","(?m)^aider>\\s*$"],"done_anchors":["Tokens: [0-9.,]+k sent"]}"#,
    r#"{"agent_id":"pi","title_working":["Pi.*Working"],"title_waiting":["Pi.*Ready"],"working_anchors":["[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]\\s+Pi","Streaming response"],"blocked_anchors":["Approve tool\\? \\[y/n\\]"],"waiting_anchors":["(?m)^pi>\\s*$"],"done_anchors":["Pi turn done"]}"#,
];

/// Loads and compiles all 13 built-in ReDoS-safe JSON screen-anchor rule packs.
pub fn load_builtin_rule_packs() -> Result<BTreeMap<String, CompiledScreenAnchorPack>> {
    let mut packs = BTreeMap::new();
    for raw_json in BUILTIN_SCREEN_ANCHOR_PACKS_JSON {
        let compiled = compile_rule_pack_json(raw_json)?;
        packs.insert(compiled.agent_id.clone(), compiled);
    }
    Ok(packs)
}

/// Parsed OSC 7501 (`\x1b]7501;...(\x07|\x1b\\)`) `programStatus` facet payload.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Osc7501ProgramStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<AgentRunState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_boundary: Option<bool>,
    #[serde(default)]
    pub subagents: Vec<(String, AgentRunState)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive_prompt: Option<AskUserQuestionPrompt>,
    #[serde(default)]
    pub raw_facets: BTreeMap<String, String>,
}

/// Parses all OSC 7501 sequences (`\x1b]7501;...(\x07|\x1b\\)`) embedded in terminal output.
pub fn parse_osc7501_sequences(input: &str) -> Vec<Osc7501ProgramStatus> {
    const PREFIX: &str = "\x1b]7501;";
    let mut results = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel_start) = input[cursor..].find(PREFIX) {
        let body_start = cursor + rel_start + PREFIX.len();
        let rest = &input[body_start..];
        let mut end_rel = None;
        let mut skip_term = 0usize;
        let bytes = rest.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] == 0x07 {
                end_rel = Some(i);
                skip_term = 1;
                break;
            }
            if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                end_rel = Some(i);
                skip_term = 2;
                break;
            }
            i += 1;
        }
        let Some(body_len) = end_rel else {
            break;
        };
        let body = &rest[..body_len];
        results.push(parse_osc7501_body(body));
        cursor = body_start + body_len + skip_term;
    }

    results
}

fn split_osc7501_tokens(body: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let bytes = body.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut brace_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' | b'[' => brace_depth += 1,
                b'}' | b']' => brace_depth = brace_depth.saturating_sub(1),
                b';' if brace_depth == 0 => {
                    tokens.push(&body[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if start <= body.len() {
        tokens.push(&body[start..]);
    }
    tokens
}

fn parse_osc7501_body(body: &str) -> Osc7501ProgramStatus {
    let mut status = Osc7501ProgramStatus::default();
    for raw_tok in split_osc7501_tokens(body) {
        let tok = raw_tok.trim();
        if tok.is_empty() || tok.eq_ignore_ascii_case("programStatus") {
            continue;
        }
        let Some((key, val)) = tok.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim();
        status.raw_facets.insert(key.to_string(), val.to_string());

        match key {
            "state" => {
                if let Some(parsed) = AgentRunState::parse(val) {
                    status.state = Some(parsed);
                }
            }
            "mode" => {
                status.mode = Some(val.to_string());
            }
            "sessionBoundary" | "session_boundary" => {
                status.session_boundary = Some(val.eq_ignore_ascii_case("true") || val == "1");
            }
            "subagent" => {
                if let Some((sub_id, sub_state_raw)) = val.split_once(':')
                    && let Some(sub_state) = AgentRunState::parse(sub_state_raw)
                {
                    status.subagents.push((sub_id.trim().to_string(), sub_state));
                }
            }
            "promptCacheTtlMs" | "prompt_cache_ttl_ms" => {
                if let Ok(ttl) = val.parse::<u64>() {
                    status.prompt_cache_ttl_ms = Some(ttl);
                }
            }
            "interactivePrompt" | "interactive_prompt" => {
                if let Ok(prompt) = serde_json::from_str::<AskUserQuestionPrompt>(val) {
                    status.interactive_prompt = Some(prompt);
                }
            }
            _ => {}
        }
    }
    status
}

#[derive(Debug, Clone)]
struct SessionTracker {
    status: AgentSessionStatus,
    layer_timestamps: [Option<i64>; 4],
}

/// Thread-safe (`Send + Sync`) 4-state agent status engine enforcing strict 3-layer precedence:
///   1. Native CLI Hook HTTP POST (`Hook`)
///   2. Linear-time `regex` JSON screen-anchor rule packs (`ScreenAnchor`)
///   3. OSC 7501 `programStatus` terminal facets (`Osc7501`)
pub struct AgentStatusEngine {
    sessions: RwLock<BTreeMap<String, SessionTracker>>,
    rule_packs: RwLock<BTreeMap<String, CompiledScreenAnchorPack>>,
    precedence_window_ms: i64,
    tx: broadcast::Sender<AgentStatusTransition>,
}

impl std::fmt::Debug for AgentStatusEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentStatusEngine")
            .field("tracked_sessions", &self.sessions.read().len())
            .field("rule_packs", &self.rule_packs.read().len())
            .finish()
    }
}

impl AgentStatusEngine {
    /// Creates a new `AgentStatusEngine` preloaded with all 13 built-in JSON screen-anchor packs.
    pub fn new() -> Result<Self> {
        let packs = load_builtin_rule_packs()?;
        let (tx, _) = broadcast::channel(256);
        Ok(Self {
            sessions: RwLock::new(BTreeMap::new()),
            rule_packs: RwLock::new(packs),
            precedence_window_ms: DEFAULT_PRECEDENCE_WINDOW_MS,
            tx,
        })
    }

    /// Configures how long a higher-precedence layer locks out lower layers (ms).
    pub fn with_precedence_window_ms(mut self, window_ms: i64) -> Self {
        self.precedence_window_ms = window_ms.max(0);
        self
    }

    /// Returns the number of loaded screen-anchor rule packs (13 built-in by default).
    pub fn rule_pack_count(&self) -> usize {
        self.rule_packs.read().len()
    }

    /// Registers or replaces a compiled JSON screen-anchor rule pack at runtime.
    pub fn register_rule_pack(&self, pack: CompiledScreenAnchorPack) {
        self.rule_packs.write().insert(pack.agent_id.clone(), pack);
    }

    /// Subscribes to live status transitions emitted by this engine.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentStatusTransition> {
        self.tx.subscribe()
    }

    /// Hydrates a restored session after app restart with `restored_unconfirmed = true`
    /// so stale persisted `Done` states NEVER trigger spurious completion alerts.
    pub fn hydrate_restored_session(
        &self,
        session_id: impl Into<String>,
        agent_id: impl Into<String>,
        restored_state: AgentRunState,
        now_ms: Option<i64>,
    ) -> AgentSessionStatus {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let session_id = session_id.into();
        let status = AgentSessionStatus {
            session_id: session_id.clone(),
            agent_id: agent_id.into(),
            state: restored_state,
            authoritative_layer: None,
            monitoring_mode: false,
            session_boundary: false,
            completion_alerts_suppressed: true,
            subagents: BTreeMap::new(),
            interactive_prompt: None,
            restored_unconfirmed: true,
            prompt_cache_expires_at_ms: None,
            last_hook_event: None,
            last_matched_rule: None,
            updated_at_ms: now_ms,
        };
        let mut guard = self.sessions.write();
        guard.insert(
            session_id,
            SessionTracker {
                status: status.clone(),
                layer_timestamps: [None; 4],
            },
        );
        status
    }

    /// Fetches the current status snapshot for `session_id`.
    pub fn get_status(&self, session_id: &str) -> Option<AgentSessionStatus> {
        self.sessions.read().get(session_id).map(|t| t.status.clone())
    }

    /// Lists all tracked agent session statuses.
    pub fn list_statuses(&self) -> Vec<AgentSessionStatus> {
        self.sessions.read().values().map(|t| t.status.clone()).collect()
    }

    fn can_layer_update_state(&self, tracker: &SessionTracker, layer: StatusSignalLayer, now_ms: i64) -> bool {
        let incoming_rank = usize::from(layer.rank());
        for higher_ts in tracker
            .layer_timestamps
            .iter()
            .skip(incoming_rank + 1)
            .flatten()
        {
            if now_ms.saturating_sub(*higher_ts) <= self.precedence_window_ms {
                return false;
            }
        }
        true
    }

    fn map_hook_event_to_state(event: &str) -> Option<AgentRunState> {
        match event.trim().to_ascii_lowercase().as_str() {
            "pre_tool_use" | "user_prompt_submit" | "turn_start" | "working" | "subagent_start" => {
                Some(AgentRunState::Working)
            }
            "permission_request" | "approval_required" | "blocked" => Some(AgentRunState::Blocked),
            "notification" | "ask_user_question" | "waiting_input" | "waiting" => {
                Some(AgentRunState::Waiting)
            }
            "stop" | "turn_end" | "session_end" | "done" | "completed" => Some(AgentRunState::Done),
            "idle" => Some(AgentRunState::Idle),
            _ => None,
        }
    }

    /// Applies a Layer 1 Native CLI Hook payload (highest precedence).
    pub fn apply_hook_event(
        &self,
        payload: HookEventPayload,
        now_ms: Option<i64>,
    ) -> AgentStatusTransition {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let inferred_state = payload
            .state
            .or_else(|| Self::map_hook_event_to_state(&payload.event));

        let mut guard = self.sessions.write();
        let tracker = guard
            .entry(payload.session_id.clone())
            .or_insert_with(|| SessionTracker {
                status: empty_status(&payload.session_id, &payload.agent_id, now_ms),
                layer_timestamps: [None; 4],
            });
        let prev_state = tracker.status.state;
        tracker.status.agent_id = payload.agent_id;
        tracker.status.restored_unconfirmed = false;
        tracker.status.last_hook_event = Some(payload.event.clone());
        tracker.status.updated_at_ms = now_ms;
        tracker.layer_timestamps[usize::from(StatusSignalLayer::Hook.rank())] = Some(now_ms);

        if let Some(boundary) = payload.session_boundary {
            tracker.status.session_boundary = boundary;
        }
        if let Some(mon) = payload.monitoring_mode {
            tracker.status.monitoring_mode = mon;
        }
        if let (Some(sub_id), Some(sub_state)) = (payload.subagent_id, payload.subagent_state) {
            tracker.status.subagents.insert(
                sub_id.clone(),
                SubagentStatus {
                    subagent_id: sub_id,
                    state: sub_state,
                    label: None,
                    updated_at_ms: now_ms,
                },
            );
        }
        if let Some(ttl) = payload.prompt_cache_ttl_ms {
            let ttl_i64 = i64::try_from(ttl).unwrap_or(i64::MAX);
            tracker.status.prompt_cache_expires_at_ms = Some(now_ms.saturating_add(ttl_i64));
        }
        if let Some(prompt) = payload.interactive_prompt {
            tracker.status.interactive_prompt = Some(prompt);
        }

        let mut state_applied = false;
        if let Some(next_state) = inferred_state {
            tracker.status.state = next_state;
            tracker.status.authoritative_layer = Some(StatusSignalLayer::Hook);
            if next_state == AgentRunState::Working {
                tracker.status.interactive_prompt = None;
            }
            state_applied = true;
        }
        tracker.status.completion_alerts_suppressed =
            tracker.status.session_boundary || tracker.status.restored_unconfirmed;

        let completion_alert_emitted = state_applied
            && prev_state != AgentRunState::Done
            && tracker.status.should_emit_completion_alert();

        let transition = AgentStatusTransition {
            session_id: payload.session_id,
            previous_state: prev_state,
            current_state: tracker.status.state,
            signal_layer: StatusSignalLayer::Hook,
            state_applied,
            completion_alert_emitted,
            status: tracker.status.clone(),
        };
        drop(guard);
        let _ = self.tx.send(transition.clone());
        transition
    }

    /// Evaluates Layer 2 ReDoS-safe linear-time `regex` screen-anchor rules against terminal
    /// title & screen buffer text. Does NOT override an active Layer 1 Hook within precedence window.
    pub fn apply_screen_snapshot(
        &self,
        session_id: &str,
        agent_id: &str,
        title: Option<&str>,
        screen_text: &str,
        now_ms: Option<i64>,
    ) -> Option<AgentStatusTransition> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let matched = {
            let packs = self.rule_packs.read();
            packs
                .get(agent_id)
                .and_then(|pack| pack.evaluate(title, screen_text))
        };
        let (matched_state, matched_rule) = matched?;

        let mut guard = self.sessions.write();
        let tracker = guard
            .entry(session_id.to_string())
            .or_insert_with(|| SessionTracker {
                status: empty_status(session_id, agent_id, now_ms),
                layer_timestamps: [None; 4],
            });
        let prev_state = tracker.status.state;
        tracker.status.restored_unconfirmed = false;
        tracker.status.last_matched_rule = Some(matched_rule);
        tracker.status.updated_at_ms = now_ms;
        tracker.layer_timestamps[usize::from(StatusSignalLayer::ScreenAnchor.rank())] = Some(now_ms);

        let state_applied = if self.can_layer_update_state(tracker, StatusSignalLayer::ScreenAnchor, now_ms) {
            tracker.status.state = matched_state;
            tracker.status.authoritative_layer = Some(StatusSignalLayer::ScreenAnchor);
            true
        } else {
            false
        };
        tracker.status.completion_alerts_suppressed =
            tracker.status.session_boundary || tracker.status.restored_unconfirmed;
        let completion_alert_emitted = state_applied
            && prev_state != AgentRunState::Done
            && tracker.status.should_emit_completion_alert();

        let transition = AgentStatusTransition {
            session_id: session_id.to_string(),
            previous_state: prev_state,
            current_state: tracker.status.state,
            signal_layer: StatusSignalLayer::ScreenAnchor,
            state_applied,
            completion_alert_emitted,
            status: tracker.status.clone(),
        };
        drop(guard);
        let _ = self.tx.send(transition.clone());
        Some(transition)
    }

    /// Parses and applies Layer 3 OSC 7501 `programStatus` facets from terminal output.
    /// Metadata facets (`sessionBoundary`, `mode=monitoring`, `subagent`, `promptCacheTtlMs`,
    /// `interactivePrompt`) always merge cleanly; `state` obeys 3-layer precedence.
    pub fn apply_osc7501_stream(
        &self,
        session_id: &str,
        agent_id: &str,
        raw_stream: &str,
        now_ms: Option<i64>,
    ) -> Vec<AgentStatusTransition> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let parsed = parse_osc7501_sequences(raw_stream);
        let mut transitions = Vec::with_capacity(parsed.len());

        for facet in parsed {
            let mut guard = self.sessions.write();
            let tracker = guard
                .entry(session_id.to_string())
                .or_insert_with(|| SessionTracker {
                    status: empty_status(session_id, agent_id, now_ms),
                    layer_timestamps: [None; 4],
                });
            let prev_state = tracker.status.state;
            tracker.status.restored_unconfirmed = false;
            tracker.status.updated_at_ms = now_ms;
            tracker.layer_timestamps[usize::from(StatusSignalLayer::Osc7501.rank())] = Some(now_ms);

            if let Some(ref mode) = facet.mode {
                tracker.status.monitoring_mode = mode.eq_ignore_ascii_case("monitoring");
            }
            if let Some(boundary) = facet.session_boundary {
                tracker.status.session_boundary = boundary;
            }
            for (sub_id, sub_state) in facet.subagents {
                tracker.status.subagents.insert(
                    sub_id.clone(),
                    SubagentStatus {
                        subagent_id: sub_id,
                        state: sub_state,
                        label: None,
                        updated_at_ms: now_ms,
                    },
                );
            }
            if let Some(ttl) = facet.prompt_cache_ttl_ms {
                let ttl_i64 = i64::try_from(ttl).unwrap_or(i64::MAX);
                tracker.status.prompt_cache_expires_at_ms = Some(now_ms.saturating_add(ttl_i64));
            }
            if let Some(prompt) = facet.interactive_prompt {
                tracker.status.interactive_prompt = Some(prompt);
            }

            let state_applied = if let Some(next_state) = facet.state
                && self.can_layer_update_state(tracker, StatusSignalLayer::Osc7501, now_ms)
            {
                tracker.status.state = next_state;
                tracker.status.authoritative_layer = Some(StatusSignalLayer::Osc7501);
                true
            } else {
                false
            };

            tracker.status.completion_alerts_suppressed =
                tracker.status.session_boundary || tracker.status.restored_unconfirmed;
            let completion_alert_emitted = state_applied
                && prev_state != AgentRunState::Done
                && tracker.status.should_emit_completion_alert();

            let transition = AgentStatusTransition {
                session_id: session_id.to_string(),
                previous_state: prev_state,
                current_state: tracker.status.state,
                signal_layer: StatusSignalLayer::Osc7501,
                state_applied,
                completion_alert_emitted,
                status: tracker.status.clone(),
            };
            drop(guard);
            let _ = self.tx.send(transition.clone());
            transitions.push(transition);
        }
        transitions
    }
}

fn empty_status(session_id: &str, agent_id: &str, now_ms: i64) -> AgentSessionStatus {
    AgentSessionStatus {
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        state: AgentRunState::Idle,
        authoritative_layer: None,
        monitoring_mode: false,
        session_boundary: false,
        completion_alerts_suppressed: false,
        subagents: BTreeMap::new(),
        interactive_prompt: None,
        restored_unconfirmed: false,
        prompt_cache_expires_at_ms: None,
        last_hook_event: None,
        last_matched_rule: None,
        updated_at_ms: now_ms,
    }
}

/// Configuration for `AgentHookServer`.
#[derive(Debug, Clone)]
pub struct AgentHookServerConfig {
    /// Directory where restart-surviving `endpoint.env` and `endpoint.cmd` are atomically written.
    pub endpoint_dir: PathBuf,
    /// Optional explicit Bearer token; when `None`, 32 cryptographically random bytes are generated.
    pub token: Option<String>,
    /// Initial enabled state (can be toggled live via `AgentHookServer::set_enabled` without restart).
    pub initially_enabled: bool,
}

impl AgentHookServerConfig {
    pub fn new(endpoint_dir: impl Into<PathBuf>) -> Self {
        Self {
            endpoint_dir: endpoint_dir.into(),
            token: None,
            initially_enabled: true,
        }
    }
}

/// Genuine loopback HTTP/1.1 hook receiver (`127.0.0.1:0`) feeding Layer 1 events into
/// `AgentStatusEngine`, with constant-time Bearer auth, CSRF Origin/Host rejection,
/// live enable/disable toggle, and atomic `endpoint.env` / `endpoint.cmd` persistence.
pub struct AgentHookServer {
    addr: SocketAddr,
    token: String,
    endpoint_dir: PathBuf,
    enabled: Arc<AtomicBool>,
    shutdown_tx: watch::Sender<bool>,
    task: parking_lot::Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for AgentHookServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHookServer")
            .field("addr", &self.addr)
            .field("endpoint_dir", &self.endpoint_dir)
            .field("enabled", &self.is_enabled())
            .finish()
    }
}

impl Drop for AgentHookServer {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl AgentHookServer {
    /// Binds to `127.0.0.1:0`, atomically writes `endpoint.env` and `endpoint.cmd`, and
    /// spawns the async HTTP/1.1 accept loop.
    pub async fn bind(
        engine: Arc<AgentStatusEngine>,
        config: AgentHookServerConfig,
    ) -> Result<Self> {
        let token = match config.token.filter(|t| !t.trim().is_empty()) {
            Some(explicit) => explicit,
            None => generate_hook_token()?,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| {
            OrcaIdeRuntimeError::hook_server(format!("failed to bind 127.0.0.1:0: {e}"))
        })?;
        let addr = listener.local_addr()?;

        let enabled = Arc::new(AtomicBool::new(config.initially_enabled));
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        write_endpoint_files(&config.endpoint_dir, addr, &token)?;

        let task_token = token.clone();
        let task_enabled = Arc::clone(&enabled);
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((stream, _peer)) => {
                                let eng = Arc::clone(&engine);
                                let tok = task_token.clone();
                                let en = Arc::clone(&task_enabled);
                                tokio::spawn(async move {
                                    handle_hook_connection(stream, eng, tok, en).await;
                                });
                            }
                            Err(err) => {
                                tracing::warn!("agent hook server accept error: {err}");
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            addr,
            token,
            endpoint_dir: config.endpoint_dir,
            enabled,
            shutdown_tx,
            task: parking_lot::Mutex::new(Some(handle)),
        })
    }

    /// Returns the bound loopback socket address (`127.0.0.1:<ephemeral_port>`).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns the full loopback hook POST URL (`http://127.0.0.1:<port>/hook`).
    pub fn hook_url(&self) -> String {
        format!("http://{}/hook", self.addr)
    }

    /// Returns the Bearer token required in `Authorization: Bearer <token>`.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Returns path to the POSIX `endpoint.env` file.
    pub fn endpoint_env_path(&self) -> PathBuf {
        self.endpoint_dir.join("endpoint.env")
    }

    /// Returns path to the Windows `endpoint.cmd` file.
    pub fn endpoint_cmd_path(&self) -> PathBuf {
        self.endpoint_dir.join("endpoint.cmd")
    }

    /// Re-writes `endpoint.env` and `endpoint.cmd` atomically on disk.
    pub fn rewrite_endpoint_files(&self) -> Result<()> {
        write_endpoint_files(&self.endpoint_dir, self.addr, &self.token)
    }

    /// Toggles hook ingestion live without restarting the loopback TCP listener.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    /// Returns whether hook ingestion is currently enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Shuts down the loopback listener gracefully.
    pub async fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        let handle = self.task.lock().take();
        if let Some(h) = handle {
            let _ = h.await;
        }
    }
}

fn generate_hook_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| {
        OrcaIdeRuntimeError::hook_server(format!("failed to generate hook bearer token: {e}"))
    })?;
    Ok(hex::encode(bytes))
}

fn write_atomic_file(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_name = format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("endpoint"),
        Uuid::new_v4()
    );
    let tmp_path = path.with_file_name(tmp_name);
    std::fs::write(&tmp_path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn write_endpoint_files(dir: &Path, addr: SocketAddr, token: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let url = format!("http://{addr}/hook");
    let env_body = format!(
        "export ORCA_HOOK_URL='{url}'\nexport ORCA_HOOK_TOKEN='{token}'\n"
    );
    let cmd_body = format!(
        "@echo off\r\nset \"ORCA_HOOK_URL={url}\"\r\nset \"ORCA_HOOK_TOKEN={token}\"\r\n"
    );
    write_atomic_file(&dir.join("endpoint.env"), env_body.as_bytes())?;
    write_atomic_file(&dir.join("endpoint.cmd"), cmd_body.as_bytes())?;
    Ok(())
}

/// Validates whether a `Host` header value strictly names a loopback interface.
pub fn is_loopback_host(host: &str) -> bool {
    let trimmed = host.trim();
    if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains('@') {
        return false;
    }
    let host_only = if let Some(rest) = trimmed.strip_prefix('[') {
        let Some((ipv6, tail)) = rest.split_once(']') else {
            return false;
        };
        if !tail.is_empty() {
            let Some(port_str) = tail.strip_prefix(':') else {
                return false;
            };
            if port_str.parse::<u16>().is_err() {
                return false;
            }
        }
        ipv6
    } else if let Some((h, port_str)) = trimmed.rsplit_once(':') {
        if port_str.parse::<u16>().is_err() {
            return false;
        }
        h
    } else {
        trimmed
    };

    host_only.eq_ignore_ascii_case("localhost")
        || host_only == "127.0.0.1"
        || host_only == "::1"
}

fn verify_bearer_constant_time(headers: &HeaderMap, expected_token: &str) -> bool {
    let Some(auth_val) = headers.get(http::header::AUTHORIZATION) else {
        return false;
    };
    let Ok(auth_str) = auth_val.to_str() else {
        return false;
    };
    let Some(provided) = auth_str.strip_prefix("Bearer ") else {
        return false;
    };
    let provided_bytes = provided.trim().as_bytes();
    let expected_bytes = expected_token.as_bytes();
    if provided_bytes.len() != expected_bytes.len() {
        return false;
    }
    bool::from(provided_bytes.ct_eq(expected_bytes))
}

async fn write_json_response(
    stream: &mut TcpStream,
    status: StatusCode,
    body: &serde_json::Value,
) {
    let payload = body.to_string();
    let reason = status.canonical_reason().unwrap_or("Status");
    let raw = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status.as_u16(),
        reason,
        payload.len(),
        payload
    );
    let _ = stream.write_all(raw.as_bytes()).await;
    let _ = stream.flush().await;
}

async fn read_bounded_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    budget: &mut usize,
) -> std::result::Result<Option<String>, &'static str> {
    let mut buf = Vec::new();
    let n = reader
        .read_until(b'\n', &mut buf)
        .await
        .map_err(|_| "io error")?;
    if n == 0 {
        return Ok(None);
    }
    if n > *budget {
        return Err("request header too large");
    }
    *budget -= n;
    while buf.ends_with(b"\n") || buf.ends_with(b"\r") {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| "invalid utf-8 header")
}

async fn handle_hook_connection(
    stream: TcpStream,
    engine: Arc<AgentStatusEngine>,
    expected_token: String,
    enabled: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(stream);
    let mut header_budget = MAX_HOOK_HEADER_BYTES;

    let req_line = match tokio::time::timeout(
        HOOK_READ_TIMEOUT,
        read_bounded_line(&mut reader, &mut header_budget),
    )
    .await
    {
        Ok(Ok(Some(line))) => line,
        _ => return,
    };

    let mut parts = req_line.split_whitespace();
    let (Some(method_raw), Some(target_raw), Some(ver_raw)) =
        (parts.next(), parts.next(), parts.next())
    else {
        let mut s = reader.into_inner();
        write_json_response(
            &mut s,
            StatusCode::BAD_REQUEST,
            &serde_json::json!({"error":{"code":"bad_request","message":"malformed request line"}}),
        )
        .await;
        return;
    };
    if parts.next().is_some() || !ver_raw.starts_with("HTTP/1.") {
        let mut s = reader.into_inner();
        write_json_response(
            &mut s,
            StatusCode::BAD_REQUEST,
            &serde_json::json!({"error":{"code":"bad_request","message":"unsupported HTTP version"}}),
        )
        .await;
        return;
    }

    let Ok(method) = Method::from_bytes(method_raw.as_bytes()) else {
        let mut s = reader.into_inner();
        write_json_response(
            &mut s,
            StatusCode::BAD_REQUEST,
            &serde_json::json!({"error":{"code":"bad_request","message":"invalid HTTP method"}}),
        )
        .await;
        return;
    };

    let mut headers = HeaderMap::new();
    let mut has_origin = false;
    let mut host_values: Vec<String> = Vec::new();
    let mut header_count = 0usize;

    loop {
        let line = match tokio::time::timeout(
            HOOK_READ_TIMEOUT,
            read_bounded_line(&mut reader, &mut header_budget),
        )
        .await
        {
            Ok(Ok(Some(l))) => l,
            Ok(Ok(None)) => return,
            _ => {
                let mut s = reader.into_inner();
                write_json_response(
                    &mut s,
                    StatusCode::BAD_REQUEST,
                    &serde_json::json!({"error":{"code":"bad_request","message":"header read failed"}}),
                )
                .await;
                return;
            }
        };
        if line.is_empty() {
            break;
        }
        header_count += 1;
        if header_count > MAX_HOOK_HEADERS {
            let mut s = reader.into_inner();
            write_json_response(
                &mut s,
                StatusCode::BAD_REQUEST,
                &serde_json::json!({"error":{"code":"bad_request","message":"too many headers"}}),
            )
            .await;
            return;
        }
        let Some((name_raw, val_raw)) = line.split_once(':') else {
            let mut s = reader.into_inner();
            write_json_response(
                &mut s,
                StatusCode::BAD_REQUEST,
                &serde_json::json!({"error":{"code":"bad_request","message":"malformed header"}}),
            )
            .await;
            return;
        };
        let name_trimmed = name_raw.trim();
        let val_trimmed = val_raw.trim();
        if name_trimmed.eq_ignore_ascii_case("origin") {
            has_origin = true;
        }
        if name_trimmed.eq_ignore_ascii_case("host") {
            host_values.push(val_trimmed.to_string());
        }
        if let (Ok(hn), Ok(hv)) = (
            HeaderName::from_bytes(name_trimmed.as_bytes()),
            HeaderValue::from_str(val_trimmed),
        ) {
            headers.append(hn, hv);
        }
    }

    // Gate 1 & 2: CSRF Origin rejection & strict single loopback Host check -> 403 Forbidden.
    if has_origin || host_values.len() != 1 || !is_loopback_host(&host_values[0]) {
        let mut s = reader.into_inner();
        write_json_response(
            &mut s,
            StatusCode::FORBIDDEN,
            &serde_json::json!({"error":{"code":"forbidden","message":"Origin or non-loopback Host rejected"}}),
        )
        .await;
        return;
    }

    // Gate 3: Strict POST method check -> 405 Method Not Allowed.
    if method != Method::POST {
        let mut s = reader.into_inner();
        write_json_response(
            &mut s,
            StatusCode::METHOD_NOT_ALLOWED,
            &serde_json::json!({"error":{"code":"method_not_allowed","message":"only POST /hook is allowed"}}),
        )
        .await;
        return;
    }

    // Gate 4: Constant-time Bearer token check -> 401 Unauthorized.
    if !verify_bearer_constant_time(&headers, &expected_token) {
        let mut s = reader.into_inner();
        write_json_response(
            &mut s,
            StatusCode::UNAUTHORIZED,
            &serde_json::json!({"error":{"code":"unauthorized","message":"invalid bearer token"}}),
        )
        .await;
        return;
    }

    // Route check: only `/hook`.
    let path_only = target_raw.split('?').next().unwrap_or(target_raw);
    if path_only != "/hook" {
        let mut s = reader.into_inner();
        write_json_response(
            &mut s,
            StatusCode::NOT_FOUND,
            &serde_json::json!({"error":{"code":"not_found","message":"unknown hook route"}}),
        )
        .await;
        return;
    }

    let content_len: usize = headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if content_len > MAX_HOOK_BODY_BYTES {
        let mut s = reader.into_inner();
        write_json_response(
            &mut s,
            StatusCode::PAYLOAD_TOO_LARGE,
            &serde_json::json!({"error":{"code":"payload_too_large","message":"hook body exceeds limit"}}),
        )
        .await;
        return;
    }

    let mut body_bytes = vec![0u8; content_len];
    if content_len > 0 {
        let read_res = tokio::time::timeout(
            HOOK_READ_TIMEOUT,
            reader.read_exact(&mut body_bytes),
        )
        .await;
        if !matches!(read_res, Ok(Ok(_))) {
            let mut s = reader.into_inner();
            write_json_response(
                &mut s,
                StatusCode::BAD_REQUEST,
                &serde_json::json!({"error":{"code":"bad_request","message":"incomplete request body"}}),
            )
            .await;
            return;
        }
    }

    let mut s = reader.into_inner();
    if !enabled.load(Ordering::SeqCst) {
        write_json_response(
            &mut s,
            StatusCode::OK,
            &serde_json::json!({"ok": true, "applied": false, "enabled": false}),
        )
        .await;
        return;
    }

    let payload: HookEventPayload = match serde_json::from_slice(&body_bytes) {
        Ok(p) => p,
        Err(err) => {
            write_json_response(
                &mut s,
                StatusCode::BAD_REQUEST,
                &serde_json::json!({"error":{"code":"invalid_argument","message": err.to_string()}}),
            )
            .await;
            return;
        }
    };

    let transition = engine.apply_hook_event(payload, None);
    write_json_response(
        &mut s,
        StatusCode::OK,
        &serde_json::json!({
            "ok": true,
            "applied": true,
            "enabled": true,
            "state": transition.current_state.as_str(),
            "session_boundary": transition.status.session_boundary,
            "completion_alert_emitted": transition.completion_alert_emitted,
        }),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn send_raw_http(addr: SocketAddr, raw_request: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).await.expect("connect loopback");
        stream
            .write_all(raw_request.as_bytes())
            .await
            .expect("write request");
        stream.flush().await.expect("flush");
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await.expect("read response");
        let text = String::from_utf8(resp).expect("utf8 resp");
        let status_code: u16 = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .expect("http status code");
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status_code, body)
    }

    #[test]
    fn thirteen_rule_packs_and_three_layer_precedence_and_osc7501() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AgentStatusEngine>();
        assert_send_sync::<AgentHookServer>();

        let engine = AgentStatusEngine::new().expect("engine");
        assert_eq!(engine.rule_pack_count(), 13);

        // Verify all 13 built-in agent packs exist and evaluate cleanly.
        let expected_13 = [
            "claude", "codex", "grok", "opencode", "cursor", "copilot", "hermes", "kimi", "kiro",
            "goose", "cline", "aider", "pi",
        ];
        let packs = load_builtin_rule_packs().expect("packs");
        for agent in expected_13 {
            assert!(packs.contains_key(agent), "missing rule pack for {agent}");
        }

        // Hydrated session suppresses completion alerts until live signal confirms.
        let hydrated = engine.hydrate_restored_session("s-1", "claude", AgentRunState::Done, Some(1_000));
        assert!(hydrated.restored_unconfirmed);
        assert!(!hydrated.should_emit_completion_alert());

        // Layer 3 OSC 7501 with sessionBoundary=true suppresses completion alerts, tracks subagents,
        // promptCacheTtlMs, monitoring mode, and interactivePrompt JSON containing ';' and '='.
        let osc = concat!(
            "\x1b]7501;programStatus;state=done;mode=monitoring;sessionBoundary=true;",
            "subagent=sub-a:working;subagent=sub-b:done;promptCacheTtlMs=300000;",
            "interactivePrompt={\"question\":\"Pick A=1; B=2?\",\"options\":[{\"label\":\"A\"}],\"multi_select\":false}\x07"
        );
        let osc_transitions = engine.apply_osc7501_stream("s-1", "claude", osc, Some(2_000));
        assert_eq!(osc_transitions.len(), 1);
        let st = &osc_transitions[0].status;
        assert_eq!(st.state, AgentRunState::Done);
        assert!(st.session_boundary);
        assert!(st.monitoring_mode);
        assert!(!osc_transitions[0].completion_alert_emitted);
        assert!(!st.should_emit_completion_alert());
        assert_eq!(st.subagents.len(), 2);
        assert!(st.has_active_subagents());
        assert_eq!(st.prompt_cache_expires_at_ms, Some(302_000));
        assert_eq!(st.prompt_cache_remaining_ms(202_000), Some(100_000));
        assert_eq!(
            st.interactive_prompt.as_ref().map(|p| p.question.as_str()),
            Some("Pick A=1; B=2?")
        );

        // Layer 2 ScreenAnchor beats Layer 3 OSC 7501.
        let screen_t = engine
            .apply_screen_snapshot(
                "s-1",
                "claude",
                Some("⠋ Claude"),
                "Do you want to proceed with rm?",
                Some(2_100),
            )
            .expect("screen match");
        assert!(screen_t.state_applied);
        assert_eq!(screen_t.current_state, AgentRunState::Blocked);

        // Layer 3 OSC 7501 ST-terminated (`\x1b\\`) CANNOT override active Layer 2 ScreenAnchor!
        let lower_osc = engine.apply_osc7501_stream(
            "s-1",
            "claude",
            "\x1b]7501;state=working;sessionBoundary=false\x1b\\",
            Some(2_200),
        );
        assert!(!lower_osc[0].state_applied);
        assert_eq!(lower_osc[0].current_state, AgentRunState::Blocked);

        // Layer 1 Hook strictly beats Layer 2 ScreenAnchor.
        let hook_t = engine.apply_hook_event(
            HookEventPayload {
                session_id: "s-1".into(),
                agent_id: "claude".into(),
                event: "stop".into(),
                state: Some(AgentRunState::Done),
                session_boundary: Some(false),
                monitoring_mode: None,
                subagent_id: None,
                subagent_state: None,
                prompt_cache_ttl_ms: None,
                interactive_prompt: None,
            },
            Some(2_300),
        );
        assert!(hook_t.state_applied);
        assert_eq!(hook_t.current_state, AgentRunState::Done);
        assert!(hook_t.completion_alert_emitted);

        // Subsequent Layer 2 ScreenAnchor CANNOT override active Layer 1 Hook within window.
        let blocked_screen = engine
            .apply_screen_snapshot("s-1", "claude", None, "⠋ Working on task", Some(2_400))
            .expect("matched rule");
        assert!(!blocked_screen.state_applied);
        assert_eq!(blocked_screen.current_state, AgentRunState::Done);
    }

    #[tokio::test]
    async fn loopback_hook_server_enforces_security_and_persists_endpoint_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = Arc::new(AgentStatusEngine::new().expect("engine"));
        let server = AgentHookServer::bind(
            Arc::clone(&engine),
            AgentHookServerConfig::new(dir.path()),
        )
        .await
        .expect("bind hook server");

        // Verify atomic `endpoint.env` and `endpoint.cmd` contents.
        let env_content = std::fs::read_to_string(server.endpoint_env_path()).expect("read env");
        assert!(env_content.contains(&server.hook_url()));
        assert!(env_content.contains(server.token()));
        let cmd_content = std::fs::read_to_string(server.endpoint_cmd_path()).expect("read cmd");
        assert!(cmd_content.contains(&server.hook_url()));
        assert!(cmd_content.contains(server.token()));

        let addr = server.local_addr();
        let host = format!("127.0.0.1:{}", addr.port());
        let valid_body = serde_json::json!({
            "session_id": "s-http",
            "agent_id": "codex",
            "event": "pre_tool_use",
            "state": "working"
        })
        .to_string();

        // 1. CSRF Origin header -> 403 Forbidden.
        let (code, _) = send_raw_http(
            addr,
            &format!(
                "POST /hook HTTP/1.1\r\nHost: {host}\r\nOrigin: https://evil.example\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}",
                server.token(),
                valid_body.len(),
                valid_body
            ),
        )
        .await;
        assert_eq!(code, 403);

        // 2. Non-loopback Host header -> 403 Forbidden.
        let (code, _) = send_raw_http(
            addr,
            &format!(
                "POST /hook HTTP/1.1\r\nHost: attacker.com\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}",
                server.token(),
                valid_body.len(),
                valid_body
            ),
        )
        .await;
        assert_eq!(code, 403);

        // 3. Non-POST method -> 405 Method Not Allowed.
        let (code, _) = send_raw_http(
            addr,
            &format!(
                "GET /hook HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\n\r\n",
                server.token()
            ),
        )
        .await;
        assert_eq!(code, 405);

        // 4. Invalid Bearer token -> 401 Unauthorized.
        let (code, _) = send_raw_http(
            addr,
            &format!(
                "POST /hook HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer wrong-token\r\nContent-Length: {}\r\n\r\n{}",
                valid_body.len(),
                valid_body
            ),
        )
        .await;
        assert_eq!(code, 401);

        // 5. Valid POST /hook -> 200 OK & updates AgentStatusEngine.
        let (code, body) = send_raw_http(
            addr,
            &format!(
                "POST /hook HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}",
                server.token(),
                valid_body.len(),
                valid_body
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert!(body.contains("\"applied\":true"));
        assert_eq!(
            engine.get_status("s-http").expect("status").state,
            AgentRunState::Working
        );

        // 6. Live disable toggle without server restart ignores subsequent POSTs cleanly.
        server.set_enabled(false);
        let stop_body = serde_json::json!({
            "session_id": "s-http",
            "agent_id": "codex",
            "event": "stop",
            "state": "done"
        })
        .to_string();
        let (code, body) = send_raw_http(
            addr,
            &format!(
                "POST /hook HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}",
                server.token(),
                stop_body.len(),
                stop_body
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert!(body.contains("\"applied\":false"));
        assert_eq!(
            engine.get_status("s-http").expect("unchanged").state,
            AgentRunState::Working
        );

        server.shutdown().await;
    }
}
