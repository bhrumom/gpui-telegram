//! Loopback Credential-Isolating Bot Tool Relay (`BotToolRelay`) & `scrub_bot_tool_secrets`:
//! - Pure-Rust loopback HTTP/1.1 streaming server bound to `127.0.0.1:0` (ephemeral port).
//! - Strict security gates (`PROJECT.md §4`):
//!   1. Reject ANY `Origin` header -> `403 Forbidden` (`"Not allowed."`).
//!   2. Reject missing/duplicate/non-loopback `Host` header -> `403 Forbidden` (`"Not allowed."`).
//!   3. Allow ONLY `POST`, `GET`, `DELETE` -> `405 Method Not Allowed`.
//!   4. Constant-time per-bot Bearer token verification (`subtle::ConstantTimeEq`) -> `401 Unauthorized`.
//!   5. Folder/tool grant & active credential authorization -> `404 Not Found` (`CREDENTIAL_ENDED`).
//!   6. Per-owner concurrency (`Semaphore`) & rolling-window rate (`RelayAdmission`) -> `429 Too Many Requests`.
//! - 10s `header_read_timeout` with ZERO body/request timeout for live streaming SSE/MCP responses.
//! - Zero-secret-in-workspace credential injection (static headers, Composio headers, host OAuth 2.1).
//! - Single OAuth 401 refresh retry (`{"error":"sign-in-needed"}` if refresh fails or retry still 401).
//! - Bounded `mcp-session-id` -> upstream identity map (hard cap 1,000, deterministic oldest eviction).
//! - Immediate mid-flight stream cancellation via `tokio_util::sync::CancellationToken` on
//!   `revoke_bot(bot_id)` and `end_credential(credential_id)`.
//! - Atomic `scrub_bot_tool_secrets` stripping legacy mirrored secret headers from JSON configs on disk.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use http::header::{HeaderName, HeaderValue};
use http::{HeaderMap, Method, StatusCode, Uri};
use orca_core::bot_types::{BotToolName, is_valid_bot_tool_header_name, is_valid_bot_tool_url};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::tool_oauth::BotToolOAuth;
use crate::{OrcaBotsError, Result, now_epoch_ms};

/// Default HTTP/1.1 request header read timeout (10 seconds); zero body timeout afterwards.
pub const DEFAULT_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Hard cap on tracked `mcp-session-id` -> upstream identity bindings.
pub const MCP_SESSION_MAP_CAP: usize = 1_000;
/// Default max concurrent in-flight relay calls per owner.
pub const DEFAULT_OWNER_MAX_CONCURRENT: usize = 16;
/// Default max requests per rolling window per owner.
pub const DEFAULT_OWNER_RATE_LIMIT: usize = 120;
/// Default rolling-window duration for per-owner rate limiting.
pub const DEFAULT_OWNER_RATE_WINDOW: Duration = Duration::from_secs(60);

const MAX_REQUEST_HEADER_BYTES: usize = 64 * 1024;
const MAX_REQUEST_HEADERS: usize = 128;
const MAX_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;

const DEFAULT_SCRUB_SECRET_HEADERS: [&str; 7] = [
    "authorization",
    "x-api-key",
    "api-key",
    "x-auth-token",
    "proxy-authorization",
    "x-api-token",
    "x-secret-key",
];

/// Host-side credential injection mode for a registered tool (never stored in bot home files).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayToolAuth {
    None,
    StaticHeaders(BTreeMap<String, String>),
    ComposioHeaders(BTreeMap<String, String>),
    OAuth { credential_id: String },
}

/// Registration payload for a host-managed tool credential.
#[derive(Debug, Clone)]
pub struct RelayToolRegistration {
    pub credential_id: String,
    pub tool_handle: String,
    pub upstream_url: String,
    pub allowed_folders: HashSet<String>,
    pub auth: RelayToolAuth,
}

/// Registration payload for a per-bot loopback relay grant.
#[derive(Debug, Clone)]
pub struct BotRelayGrant {
    pub bot_id: String,
    pub owner_id: String,
    pub token: String,
    pub folder: String,
    pub allowed_folders: HashSet<String>,
    /// Empty set allows all tools whose `allowed_folders` include the bot's folder;
    /// non-empty set restricts this bot to the listed `tool_handle`s.
    pub allowed_tools: HashSet<String>,
}

/// Configuration for `BotToolRelay`.
#[derive(Debug, Clone)]
pub struct BotToolRelayConfig {
    pub header_read_timeout: Duration,
    pub owner_max_concurrent: usize,
    pub owner_rate_limit: usize,
    pub owner_rate_window: Duration,
    pub oauth: BotToolOAuth,
}

impl Default for BotToolRelayConfig {
    fn default() -> Self {
        Self {
            header_read_timeout: DEFAULT_HEADER_READ_TIMEOUT,
            owner_max_concurrent: DEFAULT_OWNER_MAX_CONCURRENT,
            owner_rate_limit: DEFAULT_OWNER_RATE_LIMIT,
            owner_rate_window: DEFAULT_OWNER_RATE_WINDOW,
            oauth: BotToolOAuth::new(),
        }
    }
}

/// Tracked `mcp-session-id` -> upstream binding stored in a bounded 1,000-entry map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSessionBinding {
    pub session_id: String,
    pub upstream_url: String,
    pub credential_id: String,
    pub tool_handle: String,
    pub bot_id: String,
    pub last_used_ms: i64,
}

/// Bounded `mcp-session-id` registry enforcing `MCP_SESSION_MAP_CAP = 1_000` with
/// deterministic oldest-entry eviction.
#[derive(Debug, Default)]
pub struct BoundedMcpSessionMap {
    cap: usize,
    entries: HashMap<String, McpSessionBinding>,
    order: VecDeque<String>,
}

impl BoundedMcpSessionMap {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, session_id: &str) -> Option<&McpSessionBinding> {
        self.entries.get(session_id)
    }

    /// Records or refreshes a binding; when inserting a new key at capacity,
    /// deterministically evicts the oldest key and returns it.
    pub fn record(&mut self, binding: McpSessionBinding) -> Option<String> {
        let key = binding.session_id.clone();
        if let Some(existing) = self.entries.get_mut(&key) {
            *existing = binding;
            return None;
        }
        let mut evicted = None;
        while self.entries.len() >= self.cap {
            if let Some(oldest) = self.order.pop_front() {
                if self.entries.remove(&oldest).is_some() {
                    evicted = Some(oldest);
                    break;
                }
            } else {
                break;
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, binding);
        evicted
    }

    pub fn remove_for_credential(&mut self, credential_id: &str) {
        self.entries.retain(|_, v| v.credential_id != credential_id);
        self.order.retain(|k| self.entries.contains_key(k));
    }

    pub fn remove_for_bot(&mut self, bot_id: &str) {
        self.entries.retain(|_, v| v.bot_id != bot_id);
        self.order.retain(|k| self.entries.contains_key(k));
    }
}

/// Per-owner concurrency (`Semaphore`) and rolling-window rate limiter (`429 Too Many Requests`).
#[derive(Debug)]
struct OwnerAdmissionBucket {
    semaphore: Arc<Semaphore>,
    timestamps: VecDeque<Instant>,
}

#[derive(Debug)]
struct RelayAdmission {
    max_concurrent: usize,
    rate_limit: usize,
    rate_window: Duration,
    owners: Mutex<HashMap<String, OwnerAdmissionBucket>>,
}

impl RelayAdmission {
    fn new(max_concurrent: usize, rate_limit: usize, rate_window: Duration) -> Self {
        Self {
            max_concurrent: max_concurrent.max(1),
            rate_limit: rate_limit.max(1),
            rate_window,
            owners: Mutex::new(HashMap::new()),
        }
    }

    fn try_admit(&self, owner_id: &str) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let mut guard = self.owners.lock().ok()?;
        let bucket = guard
            .entry(owner_id.to_string())
            .or_insert_with(|| OwnerAdmissionBucket {
                semaphore: Arc::new(Semaphore::new(self.max_concurrent)),
                timestamps: VecDeque::new(),
            });
        let now = Instant::now();
        while let Some(&front) = bucket.timestamps.front() {
            if now.saturating_duration_since(front) >= self.rate_window {
                bucket.timestamps.pop_front();
            } else {
                break;
            }
        }
        if bucket.timestamps.len() >= self.rate_limit {
            return None;
        }
        let permit = bucket.semaphore.clone().try_acquire_owned().ok()?;
        bucket.timestamps.push_back(now);
        Some(permit)
    }
}

#[derive(Debug, Clone)]
struct StoredBotGrant {
    bot_id: String,
    owner_id: String,
    token: String,
    folder: String,
    allowed_folders: HashSet<String>,
    allowed_tools: HashSet<String>,
    revoked: bool,
    cancel_token: CancellationToken,
}

#[derive(Debug, Clone)]
struct StoredToolEntry {
    credential_id: String,
    tool_handle: String,
    upstream_url: String,
    allowed_folders: HashSet<String>,
    auth: RelayToolAuth,
    ended: bool,
    cancel_token: CancellationToken,
}

struct RelayState {
    header_read_timeout: Duration,
    oauth: BotToolOAuth,
    admission: RelayAdmission,
    http_client: reqwest::Client,
    grants: Mutex<HashMap<String, StoredBotGrant>>,
    tools_by_handle: Mutex<HashMap<String, StoredToolEntry>>,
    handle_by_credential: Mutex<HashMap<String, String>>,
    mcp_sessions: Mutex<BoundedMcpSessionMap>,
    shutdown_token: CancellationToken,
}

/// Validates RFC 9110 `Host` header strictly against loopback literals:
/// `127.0.0.1[:port]`, `[::1][:port]`, or `localhost[:port]`.
pub fn is_allowed_loopback_host(host: &str) -> bool {
    let trimmed = host.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return false;
    }
    let check_port = |p: &str| -> bool {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n > 0)
    };
    if let Some(after_open) = trimmed.strip_prefix('[') {
        let Some((addr, rest)) = after_open.split_once(']') else {
            return false;
        };
        if addr != "::1" {
            return false;
        }
        return match rest.strip_prefix(':') {
            Some(port) => check_port(port),
            None => rest.is_empty(),
        };
    }
    let (hostname, port_opt) = match trimmed.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (trimmed, None),
    };
    let host_ok = hostname == "127.0.0.1" || hostname.eq_ignore_ascii_case("localhost");
    if !host_ok {
        return false;
    }
    match port_opt {
        Some(port) => check_port(port),
        None => true,
    }
}

/// Constant-time token comparison backed by `subtle::ConstantTimeEq`.
fn constant_time_token_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    bool::from(ab.ct_eq(bb))
}

fn parse_relay_route(uri: &Uri) -> Option<(String, String, Option<String>)> {
    let path = uri.path();
    let trimmed = path.strip_prefix('/')?;
    if trimmed.is_empty() {
        return None;
    }
    let rest_after_prefix = if let Some(after_tools) = trimmed.strip_prefix("tools/") {
        after_tools
    } else {
        trimmed
    };
    let (handle, subpath) = match rest_after_prefix.split_once('/') {
        Some((h, sub)) => (h, format!("/{sub}")),
        None => (rest_after_prefix, String::new()),
    };
    if handle.is_empty() || BotToolName::new(handle).is_err() {
        return None;
    }
    Some((
        handle.to_string(),
        subpath,
        uri.query().map(ToOwned::to_owned),
    ))
}

fn build_upstream_url(base_upstream: &str, subpath: &str, query: Option<&str>) -> Result<String> {
    let mut url = url::Url::parse(base_upstream)?;
    if !subpath.is_empty() {
        let base_path = url.path().trim_end_matches('/');
        url.set_path(&format!("{base_path}{subpath}"));
    }
    if let Some(q) = query {
        let combined = match url.query() {
            Some(existing) if !existing.is_empty() => format!("{existing}&{q}"),
            _ => q.to_string(),
        };
        url.set_query(Some(&combined));
    }
    Ok(url.to_string())
}

fn validate_tool_headers(headers: &BTreeMap<String, String>) -> Result<()> {
    for (k, v) in headers {
        if !is_valid_bot_tool_header_name(k) {
            return Err(OrcaBotsError::relay(
                400,
                "invalid_header_name",
                format!("Invalid HTTP header name '{k}'."),
            ));
        }
        if v.contains('\r') || v.contains('\n') || v.contains('\0') {
            return Err(OrcaBotsError::relay(
                400,
                "invalid_header_value",
                format!("Header value for '{k}' contains forbidden control characters."),
            ));
        }
    }
    Ok(())
}

/// Running loopback credential-isolating HTTP/1.1 streaming relay bound to `127.0.0.1:0`.
pub struct BotToolRelay {
    addr: SocketAddr,
    state: Arc<RelayState>,
    server_task: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for BotToolRelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotToolRelay")
            .field("addr", &self.addr)
            .finish()
    }
}

impl BotToolRelay {
    /// Starts the loopback relay on `127.0.0.1:0` with default configuration.
    pub async fn start_default() -> Result<Self> {
        Self::start(BotToolRelayConfig::default()).await
    }

    /// Starts the loopback relay on `127.0.0.1:0` (ephemeral port).
    pub async fn start(config: BotToolRelayConfig) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        // No body/request timeout so long-lived streaming SSE/MCP calls stay open;
        // only connect timeout is bounded.
        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()?;

        let shutdown_token = CancellationToken::new();
        let state = Arc::new(RelayState {
            header_read_timeout: config.header_read_timeout,
            oauth: config.oauth,
            admission: RelayAdmission::new(
                config.owner_max_concurrent,
                config.owner_rate_limit,
                config.owner_rate_window,
            ),
            http_client,
            grants: Mutex::new(HashMap::new()),
            tools_by_handle: Mutex::new(HashMap::new()),
            handle_by_credential: Mutex::new(HashMap::new()),
            mcp_sessions: Mutex::new(BoundedMcpSessionMap::new(MCP_SESSION_MAP_CAP)),
            shutdown_token: shutdown_token.clone(),
        });

        let accept_state = state.clone();
        let server_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_token.cancelled() => {
                        break;
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _peer)) => {
                                let conn_state = accept_state.clone();
                                tokio::spawn(async move {
                                    handle_connection(stream, conn_state).await;
                                });
                            }
                            Err(_) => {
                                if shutdown_token.is_cancelled() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            addr,
            state,
            server_task: Mutex::new(Some(server_task)),
        })
    }

    /// Returns the bound loopback `SocketAddr` (`127.0.0.1:<ephemeral_port>`).
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns the loopback base URL (`http://127.0.0.1:<port>`).
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Returns a clone of the underlying `BotToolOAuth` manager.
    pub fn oauth(&self) -> BotToolOAuth {
        self.state.oauth.clone()
    }

    /// Builds a zero-secret bot-home MCP tool reference pointing at this loopback relay.
    /// Real credentials remain strictly in host memory and NEVER appear in bot home files.
    pub fn build_bot_home_tool_stub(
        &self,
        tool_handle: &str,
        credential_handle: &str,
    ) -> Result<JsonValue> {
        BotToolName::new(tool_handle)?;
        Ok(serde_json::json!({
            "transport": "streamable-http",
            "url": format!("{}/tools/{}", self.base_url(), tool_handle),
            "credential": credential_handle
        }))
    }

    /// Registers or replaces a host-managed tool credential in the relay registry.
    pub fn register_tool(&self, reg: RelayToolRegistration) -> Result<()> {
        BotToolName::new(&reg.tool_handle)?;
        if reg.credential_id.trim().is_empty() {
            return Err(OrcaBotsError::relay(
                400,
                "invalid_credential_id",
                "credential_id must be non-empty.",
            ));
        }
        if !is_valid_bot_tool_url(&reg.upstream_url) {
            return Err(OrcaBotsError::relay(
                400,
                "invalid_upstream_url",
                format!("Upstream tool URL '{}' is invalid.", reg.upstream_url),
            ));
        }
        match &reg.auth {
            RelayToolAuth::StaticHeaders(h) | RelayToolAuth::ComposioHeaders(h) => {
                validate_tool_headers(h)?;
            }
            RelayToolAuth::OAuth { credential_id } if credential_id.trim().is_empty() => {
                return Err(OrcaBotsError::relay(
                    400,
                    "invalid_credential_id",
                    "OAuth credential_id must be non-empty.",
                ));
            }
            _ => {}
        }

        let entry = StoredToolEntry {
            credential_id: reg.credential_id.clone(),
            tool_handle: reg.tool_handle.clone(),
            upstream_url: reg.upstream_url,
            allowed_folders: reg.allowed_folders,
            auth: reg.auth,
            ended: false,
            cancel_token: CancellationToken::new(),
        };

        let mut tools = self.state.tools_by_handle.lock().map_err(|_| {
            OrcaBotsError::relay(500, "lock_poisoned", "Tool registry mutex poisoned.")
        })?;
        let mut by_cred = self.state.handle_by_credential.lock().map_err(|_| {
            OrcaBotsError::relay(500, "lock_poisoned", "Credential index mutex poisoned.")
        })?;
        if let Some(prev) = tools.insert(reg.tool_handle.clone(), entry) {
            prev.cancel_token.cancel();
        }
        by_cred.insert(reg.credential_id, reg.tool_handle);
        Ok(())
    }

    /// Registers or updates a per-bot loopback relay grant.
    pub fn register_bot_grant(&self, grant: BotRelayGrant) -> Result<()> {
        if grant.bot_id.trim().is_empty() || grant.token.trim().is_empty() {
            return Err(OrcaBotsError::relay(
                400,
                "invalid_grant",
                "Bot grant requires non-empty bot_id and token.",
            ));
        }
        let mut allowed_folders = grant.allowed_folders;
        if !grant.folder.trim().is_empty() {
            allowed_folders.insert(grant.folder.clone());
        }
        let owner_id = if grant.owner_id.trim().is_empty() {
            "local-owner".to_string()
        } else {
            grant.owner_id
        };

        let stored = StoredBotGrant {
            bot_id: grant.bot_id.clone(),
            owner_id,
            token: grant.token,
            folder: grant.folder,
            allowed_folders,
            allowed_tools: grant.allowed_tools,
            revoked: false,
            cancel_token: CancellationToken::new(),
        };

        let mut grants = self.state.grants.lock().map_err(|_| {
            OrcaBotsError::relay(500, "lock_poisoned", "Grant registry mutex poisoned.")
        })?;
        if let Some(prev) = grants.insert(grant.bot_id, stored) {
            prev.cancel_token.cancel();
        }
        Ok(())
    }

    /// Immediately cancels all in-flight streams for `bot_id` and marks its grant revoked
    /// so all subsequent calls with that bot's token return `404` (`CREDENTIAL_ENDED`).
    pub fn revoke_bot(&self, bot_id: &str) -> Result<bool> {
        let found = {
            let mut grants = self.state.grants.lock().map_err(|_| {
                OrcaBotsError::relay(500, "lock_poisoned", "Grant registry mutex poisoned.")
            })?;
            if let Some(grant) = grants.get_mut(bot_id) {
                grant.revoked = true;
                grant.cancel_token.cancel();
                true
            } else {
                false
            }
        };
        if let Ok(mut sessions) = self.state.mcp_sessions.lock() {
            sessions.remove_for_bot(bot_id);
        }
        Ok(found)
    }

    /// Immediately cancels all in-flight streams using `credential_id` and marks it ended
    /// so all subsequent calls return `404` (`CREDENTIAL_ENDED`).
    pub fn end_credential(&self, credential_id: &str) -> Result<bool> {
        let handle = {
            let by_cred = self.state.handle_by_credential.lock().map_err(|_| {
                OrcaBotsError::relay(500, "lock_poisoned", "Credential index mutex poisoned.")
            })?;
            by_cred.get(credential_id).cloned()
        };
        let Some(tool_handle) = handle else {
            return Ok(false);
        };
        let found = {
            let mut tools = self.state.tools_by_handle.lock().map_err(|_| {
                OrcaBotsError::relay(500, "lock_poisoned", "Tool registry mutex poisoned.")
            })?;
            if let Some(tool) = tools.get_mut(&tool_handle) {
                tool.ended = true;
                tool.cancel_token.cancel();
                true
            } else {
                false
            }
        };
        if let Ok(mut sessions) = self.state.mcp_sessions.lock() {
            sessions.remove_for_credential(credential_id);
        }
        Ok(found)
    }

    /// Returns the number of tracked `mcp-session-id` bindings (never exceeds `MCP_SESSION_MAP_CAP`).
    pub fn mcp_session_count(&self) -> usize {
        self.state
            .mcp_sessions
            .lock()
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Looks up a tracked `mcp-session-id` binding.
    pub fn get_mcp_session(&self, session_id: &str) -> Option<McpSessionBinding> {
        self.state
            .mcp_sessions
            .lock()
            .ok()
            .and_then(|m| m.get(session_id).cloned())
    }

    /// Cancels all active streams and shuts down the loopback listener cleanly.
    pub async fn shutdown(&self) {
        self.state.shutdown_token.cancel();
        if let Ok(grants) = self.state.grants.lock() {
            for g in grants.values() {
                g.cancel_token.cancel();
            }
        }
        if let Ok(tools) = self.state.tools_by_handle.lock() {
            for t in tools.values() {
                t.cancel_token.cancel();
            }
        }
        let handle = self
            .server_task
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(h) = handle {
            let _ = h.await;
        }
    }
}

impl Drop for BotToolRelay {
    fn drop(&mut self) {
        self.state.shutdown_token.cancel();
    }
}

struct ParsedHttp1Head {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    has_origin: bool,
    host_values: Vec<String>,
}

enum HeadReadError {
    Closed,
    BadRequest(&'static str),
}

async fn read_line_bounded<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    remaining_budget: &mut usize,
) -> std::result::Result<Option<String>, HeadReadError> {
    let mut line = Vec::new();
    let n = reader
        .read_until(b'\n', &mut line)
        .await
        .map_err(|_| HeadReadError::Closed)?;
    if n == 0 {
        return Ok(None);
    }
    if n > *remaining_budget {
        return Err(HeadReadError::BadRequest("Request header section too large."));
    }
    *remaining_budget -= n;
    while line.ends_with(b"\n") || line.ends_with(b"\r") {
        line.pop();
    }
    let text = String::from_utf8(line)
        .map_err(|_| HeadReadError::BadRequest("Invalid UTF-8 in HTTP header."))?;
    Ok(Some(text))
}

async fn read_http1_head<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::result::Result<ParsedHttp1Head, HeadReadError> {
    let mut budget = MAX_REQUEST_HEADER_BYTES;
    let Some(request_line) = read_line_bounded(reader, &mut budget).await? else {
        return Err(HeadReadError::Closed);
    };
    let mut parts = request_line.split_whitespace();
    let method_raw = parts
        .next()
        .ok_or(HeadReadError::BadRequest("Missing HTTP method."))?;
    let uri_raw = parts
        .next()
        .ok_or(HeadReadError::BadRequest("Missing request target."))?;
    let version_raw = parts
        .next()
        .ok_or(HeadReadError::BadRequest("Missing HTTP version."))?;
    if parts.next().is_some() || !version_raw.starts_with("HTTP/1.") {
        return Err(HeadReadError::BadRequest("Unsupported HTTP version."));
    }
    let method = Method::from_bytes(method_raw.as_bytes())
        .map_err(|_| HeadReadError::BadRequest("Invalid HTTP method."))?;
    let uri = uri_raw
        .parse::<Uri>()
        .map_err(|_| HeadReadError::BadRequest("Invalid request target URI."))?;

    let mut headers = HeaderMap::new();
    let mut has_origin = false;
    let mut host_values = Vec::new();
    let mut count = 0usize;

    loop {
        let Some(line) = read_line_bounded(reader, &mut budget).await? else {
            return Err(HeadReadError::Closed);
        };
        if line.is_empty() {
            break;
        }
        count += 1;
        if count > MAX_REQUEST_HEADERS {
            return Err(HeadReadError::BadRequest("Too many request headers."));
        }
        let Some((raw_name, raw_val)) = line.split_once(':') else {
            return Err(HeadReadError::BadRequest("Malformed header line."));
        };
        let trimmed_name = raw_name.trim();
        let trimmed_val = raw_val.trim();
        if trimmed_name.eq_ignore_ascii_case("origin") {
            has_origin = true;
        }
        if trimmed_name.eq_ignore_ascii_case("host") {
            host_values.push(trimmed_val.to_string());
        }
        let name = HeaderName::from_bytes(trimmed_name.as_bytes())
            .map_err(|_| HeadReadError::BadRequest("Invalid header name."))?;
        let val = HeaderValue::from_str(trimmed_val)
            .map_err(|_| HeadReadError::BadRequest("Invalid header value."))?;
        headers.append(name, val);
    }

    Ok(ParsedHttp1Head {
        method,
        uri,
        headers,
        has_origin,
        host_values,
    })
}

async fn read_http1_body<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    headers: &HeaderMap,
) -> std::result::Result<Bytes, &'static str> {
    let is_chunked = headers
        .get_all(http::header::TRANSFER_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|s| s.to_ascii_lowercase().contains("chunked"));

    if is_chunked {
        let mut out = Vec::new();
        loop {
            let mut size_line = String::new();
            let n = reader
                .read_line(&mut size_line)
                .await
                .map_err(|_| "Failed reading chunk size.")?;
            if n == 0 {
                return Err("Unexpected EOF in chunked body.");
            }
            let size_hex = size_line
                .trim()
                .split(';')
                .next()
                .unwrap_or("")
                .trim();
            let chunk_len = usize::from_str_radix(size_hex, 16)
                .map_err(|_| "Invalid chunk length.")?;
            if chunk_len == 0 {
                // Consume trailing headers / final CRLF.
                loop {
                    let mut trailer = String::new();
                    let t = reader
                        .read_line(&mut trailer)
                        .await
                        .map_err(|_| "Failed reading chunk trailer.")?;
                    if t == 0 || trailer == "\r\n" || trailer == "\n" {
                        break;
                    }
                }
                break;
            }
            if out.len().saturating_add(chunk_len) > MAX_REQUEST_BODY_BYTES {
                return Err("Request body too large.");
            }
            let start = out.len();
            out.resize(start + chunk_len, 0);
            reader
                .read_exact(&mut out[start..])
                .await
                .map_err(|_| "Unexpected EOF inside chunk data.")?;
            let mut crlf = [0u8; 2];
            reader
                .read_exact(&mut crlf)
                .await
                .map_err(|_| "Missing CRLF after chunk.")?;
        }
        return Ok(Bytes::from(out));
    }

    let mut content_len: Option<usize> = None;
    for val in headers.get_all(http::header::CONTENT_LENGTH) {
        let s = val.to_str().map_err(|_| "Invalid Content-Length.")?;
        let parsed = s
            .trim()
            .parse::<usize>()
            .map_err(|_| "Invalid Content-Length.")?;
        if let Some(prev) = content_len
            && prev != parsed
        {
            return Err("Conflicting Content-Length headers.");
        }
        content_len = Some(parsed);
    }

    let len = content_len.unwrap_or(0);
    if len > MAX_REQUEST_BODY_BYTES {
        return Err("Request body too large.");
    }
    if len == 0 {
        return Ok(Bytes::new());
    }
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .await
        .map_err(|_| "Unexpected EOF reading request body.")?;
    Ok(Bytes::from(buf))
}

async fn write_simple_response<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    status: StatusCode,
    content_type: &str,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) {
    let reason = status.canonical_reason().unwrap_or("");
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        status.as_u16(),
        body.len()
    );
    for (k, v) in extra_headers {
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let _ = writer.write_all(head.as_bytes()).await;
    if !body.is_empty() {
        let _ = writer.write_all(body).await;
    }
    let _ = writer.flush().await;
    let _ = writer.shutdown().await;
}

fn is_hop_by_hop_or_stripped_req_header(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "host"
            | "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "upgrade"
            | "proxy-authorization"
            | "proxy-connection"
            | "content-length"
            | "authorization"
            | "origin"
            | "x-orca-bot-folder"
    )
}

fn is_hop_by_hop_resp_header(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "upgrade"
            | "content-length"
    )
}

fn authenticate_bot_grant(state: &RelayState, headers: &HeaderMap) -> Option<StoredBotGrant> {
    let auth_val = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let presented = auth_val.strip_prefix("Bearer ")?.trim();
    if presented.is_empty() {
        return None;
    }
    let grants = state.grants.lock().ok()?;
    let mut matched: Option<StoredBotGrant> = None;
    for grant in grants.values() {
        if constant_time_token_eq(presented, &grant.token) {
            matched = Some(grant.clone());
        }
    }
    matched
}

async fn send_upstream_once(
    state: &RelayState,
    method: &Method,
    upstream_url: &str,
    req_headers: &HeaderMap,
    body: &Bytes,
    tool: &StoredToolEntry,
    oauth_bearer: Option<&str>,
) -> std::result::Result<reqwest::Response, reqwest::Error> {
    let req_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::POST);
    let mut builder = state.http_client.request(req_method, upstream_url);

    for (name, val) in req_headers {
        let lower = name.as_str().to_ascii_lowercase();
        if is_hop_by_hop_or_stripped_req_header(&lower) {
            continue;
        }
        builder = builder.header(name.as_str(), val.as_bytes());
    }

    match &tool.auth {
        RelayToolAuth::None => {}
        RelayToolAuth::StaticHeaders(map) | RelayToolAuth::ComposioHeaders(map) => {
            for (k, v) in map {
                builder = builder.header(k.as_str(), v.as_str());
            }
        }
        RelayToolAuth::OAuth { .. } => {
            if let Some(tok) = oauth_bearer {
                builder = builder.header(http::header::AUTHORIZATION, format!("Bearer {tok}"));
            }
        }
    }

    if !body.is_empty() {
        builder = builder.body(body.clone());
    }
    builder.send().await
}

async fn wait_cancelled(
    shutdown: &CancellationToken,
    bot_cancel: &CancellationToken,
    tool_cancel: &CancellationToken,
) {
    tokio::select! {
        _ = shutdown.cancelled() => {}
        _ = bot_cancel.cancelled() => {}
        _ = tool_cancel.cancelled() => {}
    }
}

async fn handle_connection(stream: TcpStream, state: Arc<RelayState>) {
    let _ = stream.set_nodelay(true);
    let (read_half, mut writer) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Enforce header_read_timeout (default 10s), with ZERO body timeout afterwards.
    let head = match tokio::time::timeout(state.header_read_timeout, read_http1_head(&mut reader))
        .await
    {
        Ok(Ok(h)) => h,
        Ok(Err(HeadReadError::Closed)) => return,
        Err(_) => {
            write_simple_response(
                &mut writer,
                StatusCode::REQUEST_TIMEOUT,
                "text/plain; charset=utf-8",
                &[],
                b"Header timeout.",
            )
            .await;
            return;
        }
        Ok(Err(HeadReadError::BadRequest(msg))) => {
            write_simple_response(
                &mut writer,
                StatusCode::BAD_REQUEST,
                "text/plain; charset=utf-8",
                &[],
                msg.as_bytes(),
            )
            .await;
            return;
        }
    };

    // Gate 1: Reject ANY Origin header (browser CSRF protection).
    if head.has_origin {
        write_simple_response(
            &mut writer,
            StatusCode::FORBIDDEN,
            "text/plain; charset=utf-8",
            &[],
            b"Not allowed.",
        )
        .await;
        return;
    }

    // Gate 2: Require exactly one loopback Host header (DNS rebinding protection).
    if head.host_values.len() != 1 || !is_allowed_loopback_host(&head.host_values[0]) {
        write_simple_response(
            &mut writer,
            StatusCode::FORBIDDEN,
            "text/plain; charset=utf-8",
            &[],
            b"Not allowed.",
        )
        .await;
        return;
    }

    // Gate 3: Allow ONLY POST, GET, DELETE.
    if !matches!(head.method, Method::POST | Method::GET | Method::DELETE) {
        write_simple_response(
            &mut writer,
            StatusCode::METHOD_NOT_ALLOWED,
            "text/plain; charset=utf-8",
            &[("Allow", "GET, POST, DELETE")],
            b"Method not allowed.",
        )
        .await;
        return;
    }

    // Gate 4: Constant-time per-bot Bearer authentication.
    let Some(grant) = authenticate_bot_grant(&state, &head.headers) else {
        write_simple_response(
            &mut writer,
            StatusCode::UNAUTHORIZED,
            "application/json",
            &[("WWW-Authenticate", "Bearer")],
            br#"{"error":"unauthorized"}"#,
        )
        .await;
        return;
    };

    // Gate 5: Tool route, active grant/credential, and folder scope authorization.
    let Some((tool_handle, subpath, query)) = parse_relay_route(&head.uri) else {
        write_simple_response(
            &mut writer,
            StatusCode::NOT_FOUND,
            "application/json",
            &[],
            br#"{"error":"CREDENTIAL_ENDED"}"#,
        )
        .await;
        return;
    };

    let tool_opt = state
        .tools_by_handle
        .lock()
        .ok()
        .and_then(|map| map.get(&tool_handle).cloned());
    let Some(tool) = tool_opt else {
        write_simple_response(
            &mut writer,
            StatusCode::NOT_FOUND,
            "application/json",
            &[],
            br#"{"error":"CREDENTIAL_ENDED"}"#,
        )
        .await;
        return;
    };

    let requested_folder = head
        .headers
        .get("x-orca-bot-folder")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let folder_authorized = match requested_folder {
        Some(f) => grant.allowed_folders.contains(f) && tool.allowed_folders.contains(f),
        None => {
            (!grant.folder.is_empty() && tool.allowed_folders.contains(&grant.folder))
                || grant
                    .allowed_folders
                    .iter()
                    .any(|f| tool.allowed_folders.contains(f))
        }
    };
    let tool_allowed_for_bot =
        grant.allowed_tools.is_empty() || grant.allowed_tools.contains(&tool_handle);

    if grant.revoked
        || tool.ended
        || grant.cancel_token.is_cancelled()
        || tool.cancel_token.is_cancelled()
        || !folder_authorized
        || !tool_allowed_for_bot
    {
        write_simple_response(
            &mut writer,
            StatusCode::NOT_FOUND,
            "application/json",
            &[],
            br#"{"error":"CREDENTIAL_ENDED"}"#,
        )
        .await;
        return;
    }

    // Gate 6: Per-owner concurrency & rolling-window admission control (429).
    let Some(_permit) = state.admission.try_admit(&grant.owner_id) else {
        write_simple_response(
            &mut writer,
            StatusCode::TOO_MANY_REQUESTS,
            "application/json",
            &[],
            br#"{"error":"rate_limited"}"#,
        )
        .await;
        return;
    };

    // Read request body (retained in memory for potential single OAuth 401 retry).
    let req_body = match read_http1_body(&mut reader, &head.headers).await {
        Ok(b) => b,
        Err(msg) => {
            write_simple_response(
                &mut writer,
                StatusCode::BAD_REQUEST,
                "text/plain; charset=utf-8",
                &[],
                msg.as_bytes(),
            )
            .await;
            return;
        }
    };

    let Ok(upstream_url) = build_upstream_url(&tool.upstream_url, &subpath, query.as_deref())
    else {
        write_simple_response(
            &mut writer,
            StatusCode::BAD_GATEWAY,
            "application/json",
            &[],
            br#"{"error":"invalid_upstream_url"}"#,
        )
        .await;
        return;
    };

    if let Some(sid) = head
        .headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && let Ok(mut map) = state.mcp_sessions.lock()
    {
        map.record(McpSessionBinding {
            session_id: sid.to_string(),
            upstream_url: upstream_url.clone(),
            credential_id: tool.credential_id.clone(),
            tool_handle: tool.tool_handle.clone(),
            bot_id: grant.bot_id.clone(),
            last_used_ms: now_epoch_ms(),
        });
    }

    proxy_with_oauth_retry_and_stream(
        &state,
        &mut writer,
        &head.method,
        &head.headers,
        &req_body,
        &upstream_url,
        &grant,
        &tool,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn proxy_with_oauth_retry_and_stream<W: AsyncWriteExt + Unpin>(
    state: &RelayState,
    writer: &mut W,
    method: &Method,
    req_headers: &HeaderMap,
    req_body: &Bytes,
    upstream_url: &str,
    grant: &StoredBotGrant,
    tool: &StoredToolEntry,
) {
    let initial_oauth_token = match &tool.auth {
        RelayToolAuth::OAuth { credential_id } => {
            match state.oauth.get_valid_access_token(credential_id).await {
                Ok(tok) => Some(tok),
                Err(_) => {
                    write_simple_response(
                        writer,
                        StatusCode::UNAUTHORIZED,
                        "application/json",
                        &[],
                        br#"{"error":"sign-in-needed"}"#,
                    )
                    .await;
                    return;
                }
            }
        }
        _ => None,
    };

    let first_resp = tokio::select! {
        biased;
        _ = wait_cancelled(&state.shutdown_token, &grant.cancel_token, &tool.cancel_token) => {
            return;
        }
        res = send_upstream_once(
            state,
            method,
            upstream_url,
            req_headers,
            req_body,
            tool,
            initial_oauth_token.as_deref(),
        ) => res,
    };

    let mut upstream_resp = match first_resp {
        Ok(r) => r,
        Err(_) => {
            write_simple_response(
                writer,
                StatusCode::BAD_GATEWAY,
                "application/json",
                &[],
                br#"{"error":"upstream_unreachable"}"#,
            )
            .await;
            return;
        }
    };

    // Single OAuth 401 refresh & retry (NEVER infinite retry).
    if upstream_resp.status() == reqwest::StatusCode::UNAUTHORIZED
        && let RelayToolAuth::OAuth { credential_id } = &tool.auth
    {
        drop(upstream_resp);
        let refreshed = tokio::select! {
            biased;
            _ = wait_cancelled(&state.shutdown_token, &grant.cancel_token, &tool.cancel_token) => {
                return;
            }
            res = state.oauth.force_refresh(credential_id) => res,
        };
        let Ok(fresh_token) = refreshed else {
            write_simple_response(
                writer,
                StatusCode::UNAUTHORIZED,
                "application/json",
                &[],
                br#"{"error":"sign-in-needed"}"#,
            )
            .await;
            return;
        };

        let retry_resp = tokio::select! {
            biased;
            _ = wait_cancelled(&state.shutdown_token, &grant.cancel_token, &tool.cancel_token) => {
                return;
            }
            res = send_upstream_once(
                state,
                method,
                upstream_url,
                req_headers,
                req_body,
                tool,
                Some(fresh_token.as_str()),
            ) => res,
        };
        match retry_resp {
            Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
                write_simple_response(
                    writer,
                    StatusCode::UNAUTHORIZED,
                    "application/json",
                    &[],
                    br#"{"error":"sign-in-needed"}"#,
                )
                .await;
                return;
            }
            Ok(r) => {
                upstream_resp = r;
            }
            Err(_) => {
                write_simple_response(
                    writer,
                    StatusCode::BAD_GATEWAY,
                    "application/json",
                    &[],
                    br#"{"error":"upstream_unreachable"}"#,
                )
                .await;
                return;
            }
        }
    }

    // Track response mcp-session-id in bounded 1,000-entry map.
    if let Some(sid) = upstream_resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && let Ok(mut map) = state.mcp_sessions.lock()
    {
        map.record(McpSessionBinding {
            session_id: sid.to_string(),
            upstream_url: upstream_url.to_string(),
            credential_id: tool.credential_id.clone(),
            tool_handle: tool.tool_handle.clone(),
            bot_id: grant.bot_id.clone(),
            last_used_ms: now_epoch_ms(),
        });
    }

    // Write HTTP/1.1 streaming chunked response headers immediately without buffering body.
    let status = StatusCode::from_u16(upstream_resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let reason = status.canonical_reason().unwrap_or("");
    let mut head = format!("HTTP/1.1 {} {reason}\r\n", status.as_u16());
    for (name, val) in upstream_resp.headers() {
        let lower = name.as_str().to_ascii_lowercase();
        if is_hop_by_hop_resp_header(&lower) {
            continue;
        }
        if let Ok(val_str) = val.to_str() {
            head.push_str(name.as_str());
            head.push_str(": ");
            head.push_str(val_str);
            head.push_str("\r\n");
        }
    }
    head.push_str("Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
    if writer.write_all(head.as_bytes()).await.is_err() || writer.flush().await.is_err() {
        return;
    }

    // Stream upstream chunks live, aborting immediately mid-flight if revoked or ended.
    let mut body_stream = upstream_resp.bytes_stream();
    loop {
        tokio::select! {
            biased;
            _ = wait_cancelled(&state.shutdown_token, &grant.cancel_token, &tool.cancel_token) => {
                // Deliberately drop stream & close socket without 0-length terminator so
                // downstream immediately observes mid-flight abort.
                return;
            }
            next = body_stream.next() => {
                match next {
                    Some(Ok(chunk)) => {
                        if chunk.is_empty() {
                            continue;
                        }
                        let prefix = format!("{:x}\r\n", chunk.len());
                        if writer.write_all(prefix.as_bytes()).await.is_err()
                            || writer.write_all(&chunk).await.is_err()
                            || writer.write_all(b"\r\n").await.is_err()
                            || writer.flush().await.is_err()
                        {
                            return;
                        }
                    }
                    Some(Err(_)) => {
                        return;
                    }
                    None => {
                        let _ = writer.write_all(b"0\r\n\r\n").await;
                        let _ = writer.flush().await;
                        let _ = writer.shutdown().await;
                        return;
                    }
                }
            }
        }
    }
}

/// Record of a single secret header removed by `scrub_bot_tool_secrets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrubbedSecretHeader {
    pub file: PathBuf,
    pub json_path: String,
    pub header_name: String,
}

/// Summary report returned by `scrub_bot_tool_secrets`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretScrubReport {
    pub files_scanned: usize,
    pub files_modified: usize,
    pub files_skipped_missing: usize,
    pub removed_headers: Vec<ScrubbedSecretHeader>,
}

fn scrub_json_value(
    file: &Path,
    path_prefix: &str,
    value: &mut JsonValue,
    secret_headers_lower: &HashSet<String>,
    removed: &mut Vec<ScrubbedSecretHeader>,
) {
    match value {
        JsonValue::Object(map) => {
            if let Some(JsonValue::Object(headers_obj)) = map.get_mut("headers") {
                let to_remove: Vec<String> = headers_obj
                    .keys()
                    .filter(|k| secret_headers_lower.contains(&k.to_ascii_lowercase()))
                    .cloned()
                    .collect();
                for key in to_remove {
                    headers_obj.remove(&key);
                    let jp = if path_prefix.is_empty() {
                        format!("headers.{key}")
                    } else {
                        format!("{path_prefix}.headers.{key}")
                    };
                    removed.push(ScrubbedSecretHeader {
                        file: file.to_path_buf(),
                        json_path: jp,
                        header_name: key,
                    });
                }
            }
            for (k, v) in map.iter_mut() {
                if k == "headers" {
                    continue;
                }
                let next_prefix = if path_prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{path_prefix}.{k}")
                };
                scrub_json_value(file, &next_prefix, v, secret_headers_lower, removed);
            }
        }
        JsonValue::Array(items) => {
            for (idx, item) in items.iter_mut().enumerate() {
                let next_prefix = format!("{path_prefix}[{idx}]");
                scrub_json_value(file, &next_prefix, item, secret_headers_lower, removed);
            }
        }
        _ => {}
    }
}

fn atomic_write_json_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp_name = format!(
        ".{}.scrub-tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("cfg"),
        Uuid::new_v4()
    );
    let tmp_path = path.with_file_name(tmp_name);
    {
        let mut f = File::create(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(OrcaBotsError::Io(err));
    }
    Ok(())
}

/// Atomically strips legacy mirrored secret headers (`Authorization`, `x-api-key`, `api-key`,
/// `x-auth-token`, plus `extra_secret_headers`) from Claude/agent JSON config files on disk.
pub fn scrub_bot_tool_secrets(
    paths: &[PathBuf],
    extra_secret_headers: &[String],
) -> Result<SecretScrubReport> {
    let mut secret_headers: HashSet<String> = DEFAULT_SCRUB_SECRET_HEADERS
        .iter()
        .map(|s| s.to_string())
        .collect();
    for custom in extra_secret_headers {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            secret_headers.insert(trimmed.to_ascii_lowercase());
        }
    }

    let mut report = SecretScrubReport::default();
    for path in paths {
        let raw = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                report.files_skipped_missing += 1;
                continue;
            }
            Err(err) => return Err(OrcaBotsError::Io(err)),
        };
        report.files_scanned += 1;
        let mut doc: JsonValue = serde_json::from_str(&raw)?;
        let before_count = report.removed_headers.len();
        scrub_json_value(path, "", &mut doc, &secret_headers, &mut report.removed_headers);
        if report.removed_headers.len() > before_count {
            let mut serialized = serde_json::to_vec_pretty(&doc)?;
            serialized.push(b'\n');
            atomic_write_json_file(path, &serialized)?;
            report.files_modified += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_oauth::{OAuthProviderConfig, OAuthSecurityPolicy, OAuthTokenRecord};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    async fn raw_http_exchange(addr: SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
        let mut out = Vec::new();
        let _ = stream.read_to_end(&mut out).await;
        String::from_utf8_lossy(&out).to_string()
    }

    #[tokio::test]
    async fn relay_security_gates_403_405_401_404_429_and_static_header_injection() {
        // Mock upstream echoing received headers + session id.
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            while let Ok((mut sock, _)) = upstream.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req_txt = String::from_utf8_lossy(&buf[..n]).to_string();
                    let has_secret = req_txt
                        .to_ascii_lowercase()
                        .contains("x-api-key: host-only-secret-xyz");
                    let body = if has_secret {
                        r#"{"ok":true,"injected":true}"#
                    } else {
                        r#"{"ok":false,"injected":false}"#
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nmcp-session-id: sess-up-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });

        let relay = BotToolRelay::start(BotToolRelayConfig {
            owner_max_concurrent: 4,
            owner_rate_limit: 2,
            owner_rate_window: Duration::from_secs(60),
            ..BotToolRelayConfig::default()
        })
        .await
        .unwrap();

        let mut static_headers = BTreeMap::new();
        static_headers.insert("x-api-key".into(), "host-only-secret-xyz".into());
        let mut allowed_folders = HashSet::new();
        allowed_folders.insert("alpha-bot".to_string());
        relay
            .register_tool(RelayToolRegistration {
                credential_id: "cred-static-1".into(),
                tool_handle: "linear-mcp".into(),
                upstream_url: format!("http://{upstream_addr}/mcp"),
                allowed_folders: allowed_folders.clone(),
                auth: RelayToolAuth::StaticHeaders(static_headers),
            })
            .unwrap();
        relay
            .register_bot_grant(BotRelayGrant {
                bot_id: "bot-alpha".into(),
                owner_id: "owner-1".into(),
                token: "bot-secret-bearer-token-1".into(),
                folder: "alpha-bot".into(),
                allowed_folders,
                allowed_tools: HashSet::new(),
            })
            .unwrap();

        // Zero-secret stub never leaks upstream secret.
        let stub = relay
            .build_bot_home_tool_stub("linear-mcp", "cred-static-1")
            .unwrap();
        assert!(!stub.to_string().contains("host-only-secret-xyz"));

        let addr = relay.addr();
        // 1. Origin header -> 403 Not allowed.
        let r_origin = raw_http_exchange(
            addr,
            &format!(
                "POST /tools/linear-mcp HTTP/1.1\r\nHost: {addr}\r\nOrigin: https://evil.example\r\nAuthorization: Bearer bot-secret-bearer-token-1\r\n\r\n"
            ),
        )
        .await;
        assert!(r_origin.starts_with("HTTP/1.1 403 "));
        assert!(r_origin.ends_with("Not allowed."));

        // 2. Non-loopback Host -> 403 Not allowed.
        let r_host = raw_http_exchange(
            addr,
            "POST /tools/linear-mcp HTTP/1.1\r\nHost: evil.example:8080\r\nAuthorization: Bearer bot-secret-bearer-token-1\r\n\r\n",
        )
        .await;
        assert!(r_host.starts_with("HTTP/1.1 403 "));

        // 3. Disallowed method PUT -> 405 Method Not Allowed.
        let r_put = raw_http_exchange(
            addr,
            &format!(
                "PUT /tools/linear-mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer bot-secret-bearer-token-1\r\n\r\n"
            ),
        )
        .await;
        assert!(r_put.starts_with("HTTP/1.1 405 "));

        // 4. Missing/bad Bearer token -> 401 Unauthorized.
        let r_401 = raw_http_exchange(
            addr,
            &format!("POST /tools/linear-mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer wrong\r\n\r\n"),
        )
        .await;
        assert!(r_401.starts_with("HTTP/1.1 401 "));

        // 5. Unauthorized folder -> 404 CREDENTIAL_ENDED.
        let r_404 = raw_http_exchange(
            addr,
            &format!(
                "POST /tools/linear-mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer bot-secret-bearer-token-1\r\nx-orca-bot-folder: foreign-bot\r\n\r\n"
            ),
        )
        .await;
        assert!(r_404.starts_with("HTTP/1.1 404 "));
        assert!(r_404.contains("CREDENTIAL_ENDED"));

        // 6. Valid calls inject static header & track mcp-session-id; 3rd call hits 429 rate cap!
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = format!("{}/tools/linear-mcp", relay.base_url());
        let ok1 = client
            .post(&url)
            .bearer_auth("bot-secret-bearer-token-1")
            .body(r#"{"jsonrpc":"2.0","id":1}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(ok1.status(), reqwest::StatusCode::OK);
        let ok1_body = ok1.text().await.unwrap();
        assert!(ok1_body.contains(r#""injected":true"#));
        assert!(relay.get_mcp_session("sess-up-1").is_some());

        let ok2 = client
            .get(&url)
            .bearer_auth("bot-secret-bearer-token-1")
            .send()
            .await
            .unwrap();
        assert_eq!(ok2.status(), reqwest::StatusCode::OK);

        let r_429 = client
            .get(&url)
            .bearer_auth("bot-secret-bearer-token-1")
            .send()
            .await
            .unwrap();
        assert_eq!(r_429.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

        relay.shutdown().await;
        upstream_task.abort();
    }

    #[tokio::test]
    async fn relay_oauth_401_single_retry_and_sign_in_needed_on_second_401() {
        // Mock OAuth token refresh endpoint.
        let oauth_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let oauth_addr = oauth_listener.local_addr().unwrap();
        let refresh_hits = Arc::new(AtomicUsize::new(0));
        let rh = refresh_hits.clone();
        let oauth_task = tokio::spawn(async move {
            while let Ok((mut sock, _)) = oauth_listener.accept().await {
                let rh_c = rh.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let n = rh_c.fetch_add(1, Ordering::SeqCst) + 1;
                    let body = format!(
                        r#"{{"access_token":"fresh-tok-{n}","refresh_token":"ref-{n}","token_type":"Bearer","expires_in":3600}}"#
                    );
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });

        // Mock upstream MCP server: /recover accepts "fresh-tok-1", /always401 always returns 401.
        let up_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up_addr = up_listener.local_addr().unwrap();
        let recover_hits = Arc::new(AtomicUsize::new(0));
        let always401_hits = Arc::new(AtomicUsize::new(0));
        let r_hits = recover_hits.clone();
        let a_hits = always401_hits.clone();
        let up_task = tokio::spawn(async move {
            while let Ok((mut sock, _)) = up_listener.accept().await {
                let r_c = r_hits.clone();
                let a_c = a_hits.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let first_line = req.lines().next().unwrap_or("");
                    let (status, body) = if first_line.contains("/recover") {
                        r_c.fetch_add(1, Ordering::SeqCst);
                        if req.contains("Bearer fresh-tok-1") {
                            ("200 OK", r#"{"recovered":true}"#)
                        } else {
                            ("401 Unauthorized", r#"{"error":"expired"}"#)
                        }
                    } else {
                        a_c.fetch_add(1, Ordering::SeqCst);
                        ("401 Unauthorized", r#"{"error":"revoked"}"#)
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });

        let oauth = BotToolOAuth::with_policy(OAuthSecurityPolicy::allow_loopback_for_tests());
        let far_future = now_epoch_ms() + 3_600_000;
        for cid in ["cred-recover", "cred-always401"] {
            oauth
                .store_token_for_credential(
                    OAuthProviderConfig {
                        issuer: format!("http://{oauth_addr}"),
                        authorization_endpoint: format!("http://{oauth_addr}/auth"),
                        token_endpoint: format!("http://{oauth_addr}/token"),
                        revocation_endpoint: None,
                        registration_endpoint: None,
                        client_id: "cid".into(),
                        client_secret: None,
                        redirect_uri: "http://127.0.0.1:9999/cb".into(),
                        resource: "https://mcp.example.com".into(),
                        scopes: vec![],
                    },
                    OAuthTokenRecord {
                        credential_id: cid.into(),
                        access_token: "stale-tok".into(),
                        refresh_token: Some("ref-0".into()),
                        token_type: "Bearer".into(),
                        expires_at_ms: far_future,
                        scope: None,
                        resource: "https://mcp.example.com".into(),
                    },
                )
                .unwrap();
        }

        let relay = BotToolRelay::start(BotToolRelayConfig {
            oauth,
            ..BotToolRelayConfig::default()
        })
        .await
        .unwrap();

        let mut folders = HashSet::new();
        folders.insert("bot-a".to_string());
        relay
            .register_tool(RelayToolRegistration {
                credential_id: "cred-recover".into(),
                tool_handle: "tool-recover".into(),
                upstream_url: format!("http://{up_addr}/recover"),
                allowed_folders: folders.clone(),
                auth: RelayToolAuth::OAuth {
                    credential_id: "cred-recover".into(),
                },
            })
            .unwrap();
        relay
            .register_tool(RelayToolRegistration {
                credential_id: "cred-always401".into(),
                tool_handle: "tool-always401".into(),
                upstream_url: format!("http://{up_addr}/always401"),
                allowed_folders: folders.clone(),
                auth: RelayToolAuth::OAuth {
                    credential_id: "cred-always401".into(),
                },
            })
            .unwrap();
        relay
            .register_bot_grant(BotRelayGrant {
                bot_id: "bot-a".into(),
                owner_id: "owner-1".into(),
                token: "tok-a".into(),
                folder: "bot-a".into(),
                allowed_folders: folders,
                allowed_tools: HashSet::new(),
            })
            .unwrap();

        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        // Case A: 401 -> single refresh -> retry -> 200 OK
        let res_ok = client
            .post(format!("{}/tools/tool-recover", relay.base_url()))
            .bearer_auth("tok-a")
            .body("ping")
            .send()
            .await
            .unwrap();
        assert_eq!(res_ok.status(), reqwest::StatusCode::OK);
        assert!(res_ok.text().await.unwrap().contains(r#""recovered":true"#));
        assert_eq!(recover_hits.load(Ordering::SeqCst), 2);

        // Case B: 401 -> single refresh -> retry still 401 -> 401 {"error":"sign-in-needed"} (NEVER infinite!)
        let res_fail = client
            .post(format!("{}/tools/tool-always401", relay.base_url()))
            .bearer_auth("tok-a")
            .body("ping")
            .send()
            .await
            .unwrap();
        assert_eq!(res_fail.status(), reqwest::StatusCode::UNAUTHORIZED);
        let body_fail = res_fail.text().await.unwrap();
        assert_eq!(body_fail, r#"{"error":"sign-in-needed"}"#);
        assert_eq!(always401_hits.load(Ordering::SeqCst), 2);
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 2);

        relay.shutdown().await;
        oauth_task.abort();
        up_task.abort();
    }

    #[tokio::test]
    async fn relay_mid_stream_sse_abort_on_revoke_and_bounded_sessions_and_secret_scrub() {
        // Streaming SSE mock upstream that emits chunk 1 immediately, then stays open indefinitely.
        let up_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up_addr = up_listener.local_addr().unwrap();
        let up_task = tokio::spawn(async move {
            if let Ok((mut sock, _)) = up_listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
                let _ = sock.write_all(head.as_bytes()).await;
                let chunk1 = b"data: hello-sse\n\n";
                let frame = format!("{:x}\r\n", chunk1.len());
                let _ = sock.write_all(frame.as_bytes()).await;
                let _ = sock.write_all(chunk1).await;
                let _ = sock.write_all(b"\r\n").await;
                let _ = sock.flush().await;
                // Keep SSE stream open until relay aborts.
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });

        let relay = BotToolRelay::start_default().await.unwrap();
        let mut folders = HashSet::new();
        folders.insert("sse-bot".to_string());
        relay
            .register_tool(RelayToolRegistration {
                credential_id: "cred-sse".into(),
                tool_handle: "events".into(),
                upstream_url: format!("http://{up_addr}/sse"),
                allowed_folders: folders.clone(),
                auth: RelayToolAuth::None,
            })
            .unwrap();
        relay
            .register_bot_grant(BotRelayGrant {
                bot_id: "bot-sse".into(),
                owner_id: "owner-sse".into(),
                token: "sse-bearer-token".into(),
                folder: "sse-bot".into(),
                allowed_folders: folders,
                allowed_tools: HashSet::new(),
            })
            .unwrap();

        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut resp = client
            .get(format!("{}/events", relay.base_url()))
            .bearer_auth("sse-bearer-token")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);

        // Read live first SSE chunk while upstream is still open.
        let first = tokio::time::timeout(Duration::from_secs(3), resp.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(&first[..], b"data: hello-sse\n\n");

        // Revoke bot mid-stream: next chunk read MUST abort immediately (well under 30s).
        assert!(relay.revoke_bot("bot-sse").unwrap());
        let next_res = tokio::time::timeout(Duration::from_secs(2), resp.chunk())
            .await
            .expect("mid-stream abort must not hang");
        assert!(next_res.is_err(), "aborted chunked stream must error on client");

        // Subsequent call from revoked bot returns 404 CREDENTIAL_ENDED.
        let after = client
            .get(format!("{}/events", relay.base_url()))
            .bearer_auth("sse-bearer-token")
            .send()
            .await
            .unwrap();
        assert_eq!(after.status(), reqwest::StatusCode::NOT_FOUND);
        assert!(after.text().await.unwrap().contains("CREDENTIAL_ENDED"));

        relay.shutdown().await;
        up_task.abort();

        // Bounded mcp-session-id map cap 1,000 deterministic oldest eviction.
        let mut smap = BoundedMcpSessionMap::new(MCP_SESSION_MAP_CAP);
        for i in 0..(MCP_SESSION_MAP_CAP + 5) {
            smap.record(McpSessionBinding {
                session_id: format!("sid-{i}"),
                upstream_url: "https://mcp.example.com".into(),
                credential_id: "c1".into(),
                tool_handle: "t1".into(),
                bot_id: "b1".into(),
                last_used_ms: i as i64,
            });
        }
        assert_eq!(smap.len(), MCP_SESSION_MAP_CAP);
        assert!(smap.get("sid-0").is_none());
        assert!(smap.get("sid-4").is_none());
        assert!(smap.get("sid-5").is_some());
        assert!(smap.get(&format!("sid-{}", MCP_SESSION_MAP_CAP + 4)).is_some());

        // scrub_bot_tool_secrets atomically removes secret headers while keeping safe headers.
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("claude_config.json");
        let missing_path = tmp.path().join("missing.json");
        fs::write(
            &cfg_path,
            serde_json::json!({
                "mcpServers": {
                    "linear": {
                        "url": "http://127.0.0.1:4321/tools/linear",
                        "headers": {
                            "Authorization": "Bearer secret-1",
                            "X-API-Key": "secret-2",
                            "api-key": "secret-3",
                            "x-auth-token": "secret-4",
                            "X-Custom-Secret": "secret-5",
                            "Accept": "application/json"
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let report = scrub_bot_tool_secrets(
            &[cfg_path.clone(), missing_path],
            &["X-Custom-Secret".to_string()],
        )
        .unwrap();
        assert_eq!(report.files_scanned, 1);
        assert_eq!(report.files_modified, 1);
        assert_eq!(report.files_skipped_missing, 1);
        assert_eq!(report.removed_headers.len(), 5);

        let cleaned: JsonValue = serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
        let hdrs = cleaned["mcpServers"]["linear"]["headers"].as_object().unwrap();
        assert_eq!(hdrs.len(), 1);
        assert_eq!(hdrs.get("Accept").and_then(JsonValue::as_str), Some("application/json"));
    }

    #[tokio::test]
    async fn relay_header_read_timeout_408_and_end_credential_mid_stream_abort() {
        let up_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up_addr = up_listener.local_addr().unwrap();
        let up_task = tokio::spawn(async move {
            if let Ok((mut sock, _)) = up_listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
                let _ = sock.write_all(head.as_bytes()).await;
                let chunk1 = b"data: tick-1\n\n";
                let _ = sock
                    .write_all(format!("{:x}\r\n", chunk1.len()).as_bytes())
                    .await;
                let _ = sock.write_all(chunk1).await;
                let _ = sock.write_all(b"\r\n").await;
                let _ = sock.flush().await;
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });

        // Short 150ms header timeout proves header_read_timeout fires 408 on slow headers,
        // while live SSE streams remain open past 150ms (zero body timeout)!
        let relay = BotToolRelay::start(BotToolRelayConfig {
            header_read_timeout: Duration::from_millis(150),
            ..BotToolRelayConfig::default()
        })
        .await
        .unwrap();

        let slow_resp = {
            let mut s = TcpStream::connect(relay.addr()).await.unwrap();
            s.write_all(b"GET /events HTTP/1.1\r\n").await.unwrap();
            s.flush().await.unwrap();
            let mut out = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(2), s.read_to_end(&mut out))
                .await
                .unwrap();
            String::from_utf8_lossy(&out).to_string()
        };
        assert!(slow_resp.starts_with("HTTP/1.1 408 "));

        let mut folders = HashSet::new();
        folders.insert("bot-cred".to_string());
        relay
            .register_tool(RelayToolRegistration {
                credential_id: "cred-end-me".into(),
                tool_handle: "stream-tool".into(),
                upstream_url: format!("http://{up_addr}/sse"),
                allowed_folders: folders.clone(),
                auth: RelayToolAuth::None,
            })
            .unwrap();
        relay
            .register_bot_grant(BotRelayGrant {
                bot_id: "bot-cred".into(),
                owner_id: "owner-cred".into(),
                token: "cred-bearer-token".into(),
                folder: "bot-cred".into(),
                allowed_folders: folders,
                allowed_tools: HashSet::new(),
            })
            .unwrap();

        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut resp = client
            .get(format!("{}/tools/stream-tool", relay.base_url()))
            .bearer_auth("cred-bearer-token")
            .send()
            .await
            .unwrap();
        let first = resp.chunk().await.unwrap().unwrap();
        assert_eq!(&first[..], b"data: tick-1\n\n");

        // Wait longer than header_read_timeout (150ms) to prove ZERO body timeout on SSE!
        tokio::time::sleep(Duration::from_millis(220)).await;
        assert!(relay.end_credential("cred-end-me").unwrap());
        let aborted = tokio::time::timeout(Duration::from_secs(2), resp.chunk())
            .await
            .expect("end_credential must abort open SSE stream immediately");
        assert!(aborted.is_err());

        let after = client
            .get(format!("{}/tools/stream-tool", relay.base_url()))
            .bearer_auth("cred-bearer-token")
            .send()
            .await
            .unwrap();
        assert_eq!(after.status(), reqwest::StatusCode::NOT_FOUND);
        assert!(after.text().await.unwrap().contains("CREDENTIAL_ENDED"));

        relay.shutdown().await;
        up_task.abort();
    }
}
