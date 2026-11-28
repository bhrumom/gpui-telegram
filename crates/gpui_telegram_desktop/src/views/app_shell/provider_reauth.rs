//! OpenBot-aligned interactive Provider Re-Login, Native Codex OAuth PKCE Loopback Server,
//! and Managed CLI Browser Authentication Launcher.
//!
//! Covers every provider category with zero dead ends:
//! - **Codex / ChatGPT (`codex_chatgpt`)**: Native in-app OAuth2 PKCE S256 loopback callback server
//!   (mirroring `inherited/codex-rs/login/src/server.rs` on `127.0.0.1:1455` with `1457` fallback),
//!   opening the system browser directly and persisting refreshed tokens atomically to
//!   `~/.codex/auth.json` (`0600`), with automatic fallback to `codex login` CLI when ports are busy.
//! - **Google Vertex AI (`google_vertex`)**: Non-blocking managed `gcloud auth application-default login`
//!   launcher (opens Google OAuth consent in browser and updates `application_default_credentials.json`).
//! - **Claude Code (`claude_code`)**: Managed `claude auth login` launcher.
//! - **GitHub Copilot (`github_copilot`)**: Managed `gh auth login --web` launcher.
//! - **API Key / Local Providers**: Semantic recovery classification (`更新密钥` / `重试激活` / `刷新激活`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use magpie_gateway::{
    persist_codex_auth_json, SubscriptionKind, DEFAULT_CODEX_CLIENT_ID,
};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use uuid::Uuid;

pub const DEFAULT_CODEX_ISSUER: &str = "https://auth.openai.com";
pub const CODEX_DEFAULT_CALLBACK_PORT: u16 = 1455;
pub const CODEX_FALLBACK_CALLBACK_PORT: u16 = 1457;
const DEFAULT_REAUTH_TIMEOUT_SECS: u64 = 300;

/// Visual tone for an unavailable/expired provider status badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableBadgeTone {
    /// OAuth token expired, revoked (`invalid_grant`), or refresh failed.
    Expired,
    /// Invalid API key (`401`/`403`) or missing credentials.
    AuthError,
    /// Upstream network unreachable, timeout, or local service stopped.
    Network,
    /// Explicitly disabled by the user.
    Disabled,
}

/// Recovery strategy & localized UI copy for an unavailable provider or subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRecoveryPresentation {
    pub tone: UnavailableBadgeTone,
    pub badge_zh: &'static str,
    pub badge_en: &'static str,
    pub supports_browser_relogin: bool,
    pub primary_cta_zh: &'static str,
    pub primary_cta_en: &'static str,
    pub cli_command_hint: Option<&'static str>,
    pub prefer_open_editor: bool,
    pub subscription_kind: Option<SubscriptionKind>,
}

/// Infers `SubscriptionKind` from a provider/subscription ID or kind/icon hint.
pub fn infer_subscription_kind(
    provider_id: &str,
    kind_or_icon: Option<&str>,
) -> Option<SubscriptionKind> {
    for raw in [kind_or_icon.unwrap_or(""), provider_id] {
        let lower = raw.trim().to_ascii_lowercase();
        if lower.is_empty() {
            continue;
        }
        if lower.contains("google_vertex")
            || lower.contains("google-vertex")
            || lower.contains("vertex")
        {
            return Some(SubscriptionKind::GoogleVertex);
        }
        if lower.contains("codex") || lower == "chatgpt" {
            return Some(SubscriptionKind::CodexChatGpt);
        }
        if lower.contains("claude_code") || lower.starts_with("claude:") {
            return Some(SubscriptionKind::ClaudeCode);
        }
        if lower.contains("github_copilot") || lower.contains("copilot") {
            return Some(SubscriptionKind::GitHubCopilot);
        }
        if lower.contains("gemini_code_assist")
            || lower.contains("gemini_jetski")
            || lower.contains("magpie_gemini")
        {
            return Some(SubscriptionKind::GeminiCodeAssist);
        }
        if lower.contains("antigravity") {
            return Some(SubscriptionKind::Antigravity);
        }
        if lower.contains("grok_supergrok") || lower.contains("supergrok") {
            return Some(SubscriptionKind::GrokSuperGrok);
        }
    }
    None
}

/// Returns a copyable CLI command hint for re-authenticating a subscription kind manually if needed.
pub fn cli_relogin_command_for_kind(kind: SubscriptionKind) -> Option<&'static str> {
    match kind {
        SubscriptionKind::GoogleVertex => Some("gcloud auth application-default login"),
        SubscriptionKind::CodexChatGpt => Some("codex login"),
        SubscriptionKind::ClaudeCode => Some("claude auth login"),
        SubscriptionKind::GitHubCopilot => Some("gh auth login --web"),
        SubscriptionKind::GeminiCodeAssist | SubscriptionKind::Antigravity => {
            Some("gcloud auth application-default login")
        }
        SubscriptionKind::GrokSuperGrok => None,
    }
}

/// Classifies an unavailable provider/subscription into consistent OpenBot badges and primary actions.
pub fn classify_provider_recovery(
    provider_id: &str,
    kind_or_icon: Option<&str>,
    is_subscription: bool,
    unavailable_code: Option<&str>,
    unavailable_reason: Option<&str>,
) -> ProviderRecoveryPresentation {
    let sub_kind = infer_subscription_kind(provider_id, kind_or_icon);
    let code = unavailable_code
        .map(str::trim)
        .unwrap_or("")
        .to_ascii_lowercase();
    let reason_lower = unavailable_reason
        .map(str::trim)
        .unwrap_or("")
        .to_ascii_lowercase();

    let is_expired = matches!(
        code.as_str(),
        "oauth_invalid_grant" | "token_expired" | "oauth_refresh_failed"
    ) || reason_lower.contains("invalid_grant")
        || reason_lower.contains("已过期")
        || reason_lower.contains("expired")
        || reason_lower.contains("被撤销");
    let is_auth_err = matches!(
        code.as_str(),
        "auth_401" | "auth_403" | "oauth_invalid_client" | "missing_credentials" | "auth_failed"
    ) || reason_lower.contains("401")
        || reason_lower.contains("403")
        || reason_lower.contains("api key")
        || reason_lower.contains("未配置")
        || reason_lower.contains("未检测到有效凭据");
    let is_network = code == "unreachable"
        || reason_lower.contains("超时")
        || reason_lower.contains("无法连接")
        || reason_lower.contains("未启动")
        || reason_lower.contains("未运行")
        || reason_lower.contains("timed out")
        || reason_lower.contains("unreachable");
    let is_disabled = code == "disabled";

    let supports_browser_relogin = matches!(
        sub_kind,
        Some(
            SubscriptionKind::CodexChatGpt
                | SubscriptionKind::GoogleVertex
                | SubscriptionKind::ClaudeCode
                | SubscriptionKind::GitHubCopilot
                | SubscriptionKind::GeminiCodeAssist
                | SubscriptionKind::Antigravity
        )
    );
    let cli_command_hint = sub_kind.and_then(cli_relogin_command_for_kind);

    if is_disabled {
        return ProviderRecoveryPresentation {
            tone: UnavailableBadgeTone::Disabled,
            badge_zh: "已禁用",
            badge_en: "Disabled",
            supports_browser_relogin: false,
            primary_cta_zh: "重新启用",
            primary_cta_en: "Enable",
            cli_command_hint,
            prefer_open_editor: !is_subscription,
            subscription_kind: sub_kind,
        };
    }

    if is_network {
        return ProviderRecoveryPresentation {
            tone: UnavailableBadgeTone::Network,
            badge_zh: "无法连接",
            badge_en: "Unreachable",
            supports_browser_relogin,
            primary_cta_zh: "重试激活",
            primary_cta_en: "Retry",
            cli_command_hint,
            prefer_open_editor: false,
            subscription_kind: sub_kind,
        };
    }

    if is_expired || (supports_browser_relogin && (is_auth_err || is_subscription)) {
        let (badge_zh, badge_en, tone) = if is_expired {
            ("已过期", "Expired", UnavailableBadgeTone::Expired)
        } else {
            ("认证失败", "Auth Failed", UnavailableBadgeTone::AuthError)
        };
        if supports_browser_relogin {
            return ProviderRecoveryPresentation {
                tone,
                badge_zh,
                badge_en,
                supports_browser_relogin: true,
                primary_cta_zh: "重新登录",
                primary_cta_en: "Re-login",
                cli_command_hint,
                prefer_open_editor: false,
                subscription_kind: sub_kind,
            };
        }
        return ProviderRecoveryPresentation {
            tone,
            badge_zh,
            badge_en,
            supports_browser_relogin: false,
            primary_cta_zh: "更新密钥",
            primary_cta_en: "Update Key",
            cli_command_hint,
            prefer_open_editor: !is_subscription,
            subscription_kind: sub_kind,
        };
    }

    if is_auth_err {
        return ProviderRecoveryPresentation {
            tone: UnavailableBadgeTone::AuthError,
            badge_zh: "认证失败",
            badge_en: "Auth Failed",
            supports_browser_relogin: false,
            primary_cta_zh: "更新密钥",
            primary_cta_en: "Update Key",
            cli_command_hint,
            prefer_open_editor: !is_subscription,
            subscription_kind: sub_kind,
        };
    }

    ProviderRecoveryPresentation {
        tone: UnavailableBadgeTone::AuthError,
        badge_zh: "不可用",
        badge_en: "Unavailable",
        supports_browser_relogin,
        primary_cta_zh: if supports_browser_relogin {
            "重新登录"
        } else {
            "刷新激活"
        },
        primary_cta_en: if supports_browser_relogin {
            "Re-login"
        } else {
            "Refresh"
        },
        cli_command_hint,
        prefer_open_editor: false,
        subscription_kind: sub_kind,
    }
}

/// Opens an `http://` or `https://` URL in the user's default system browser.
pub fn open_external_url_in_system_browser(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("Only http:// and https:// URLs can be opened externally".to_string());
    }
    #[cfg(target_os = "macos")]
    let spawn_res = std::process::Command::new("open").arg(trimmed).spawn();
    #[cfg(target_os = "linux")]
    let spawn_res = std::process::Command::new("xdg-open").arg(trimmed).spawn();
    #[cfg(target_os = "windows")]
    let spawn_res = std::process::Command::new("cmd")
        .args(["/C", "start", "", trimmed])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let spawn_res: std::io::Result<std::process::Child> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unsupported OS for external browser launch",
    ));

    spawn_res
        .map(|_| ())
        .map_err(|e| format!("无法打开系统浏览器: {e}"))
}

/// Resolves a CLI binary across `$PATH` plus standard macOS / Linux / Windows SDK locations
/// (important when the GUI desktop app is launched from Finder/Dock with a minimal `$PATH`).
pub fn resolve_cli_binary(program: &str) -> Option<PathBuf> {
    let trimmed = program.trim();
    if trimmed.is_empty() {
        return None;
    }
    let direct = PathBuf::from(trimmed);
    if direct.components().count() > 1 && direct.is_file() {
        return Some(direct);
    }

    let mut search_dirs: Vec<PathBuf> = Vec::new();
    if let Some(path_var) = std::env::var_os("PATH") {
        search_dirs.extend(std::env::split_paths(&path_var));
    }
    let home = std::env::var_os("HOME")
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|s| !s.is_empty()))
        .map(PathBuf::from);
    if let Some(ref h) = home {
        search_dirs.push(h.join(".local").join("bin"));
        search_dirs.push(h.join(".cargo").join("bin"));
        search_dirs.push(h.join(".npm-global").join("bin"));
        search_dirs.push(h.join("google-cloud-sdk").join("bin"));
        search_dirs.push(h.join("Library").join("Application Support").join("google-cloud-sdk").join("bin"));
        search_dirs.push(h.join("bin"));
    }
    #[cfg(unix)]
    {
        search_dirs.push(PathBuf::from("/opt/homebrew/bin"));
        search_dirs.push(PathBuf::from("/opt/homebrew/share/google-cloud-sdk/bin"));
        search_dirs.push(PathBuf::from("/usr/local/bin"));
        search_dirs.push(PathBuf::from("/usr/local/share/google-cloud-sdk/bin"));
        search_dirs.push(PathBuf::from("/usr/lib/google-cloud-sdk/bin"));
        search_dirs.push(PathBuf::from("/snap/bin"));
        search_dirs.push(PathBuf::from("/usr/bin"));
    }

    #[cfg(windows)]
    let candidates: Vec<String> = vec![
        trimmed.to_string(),
        format!("{trimmed}.exe"),
        format!("{trimmed}.cmd"),
        format!("{trimmed}.bat"),
    ];
    #[cfg(not(windows))]
    let candidates: Vec<String> = vec![trimmed.to_string()];

    for dir in search_dirs {
        for cand in &candidates {
            let full = dir.join(cand);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

/// Generated OAuth2 PKCE (S256) pair.
#[derive(Debug, Clone)]
pub struct PkcePair {
    pub code_verifier: String,
    pub code_challenge: String,
}

/// Generates a cryptographically random PKCE S256 verifier & challenge pair.
pub fn generate_pkce_pair() -> PkcePair {
    let mut raw = [0u8; 64];
    for chunk in raw.chunks_exact_mut(16) {
        chunk.copy_from_slice(Uuid::new_v4().as_bytes());
    }
    let code_verifier = URL_SAFE_NO_PAD.encode(raw);
    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(digest);
    PkcePair {
        code_verifier,
        code_challenge,
    }
}

fn generate_oauth_state() -> String {
    let mut raw = [0u8; 32];
    for chunk in raw.chunks_exact_mut(16) {
        chunk.copy_from_slice(Uuid::new_v4().as_bytes());
    }
    URL_SAFE_NO_PAD.encode(raw)
}

fn url_encode_query_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn url_decode_query_component(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &input[i + 1..i + 3];
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse_request_path_and_query(raw_http: &str) -> Option<(String, HashMap<String, String>)> {
    let first_line = raw_http.lines().next()?.trim();
    let mut parts = first_line.split_whitespace();
    let _method = parts.next()?;
    let target = parts.next()?;
    let (path, query_str) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q),
        None => (target.to_string(), ""),
    };
    let mut params = HashMap::new();
    for pair in query_str.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (url_decode_query_component(k), url_decode_query_component(v)),
            None => (url_decode_query_component(pair), String::new()),
        };
        params.insert(k, v);
    }
    Some((path, params))
}

/// Builds the exact Codex / ChatGPT OAuth2 PKCE authorization URL matching `codex login`.
pub fn build_codex_authorize_url(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkcePair,
    state: &str,
) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("scope", "openid profile email offline_access"),
        ("code_challenge", pkce.code_challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
        ("originator", "codex_cli_rs"),
    ];
    let qs = params
        .iter()
        .map(|(k, v)| format!("{k}={}", url_encode_query_component(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}/oauth/authorize?{qs}", issuer.trim_end_matches('/'))
}

/// Options for running the native in-app Codex OAuth2 PKCE loopback login flow.
#[derive(Debug, Clone)]
pub struct CodexOAuthLoginOptions {
    pub codex_home: PathBuf,
    pub issuer: String,
    pub client_id: String,
    pub candidate_ports: Vec<u16>,
    pub open_browser: bool,
    pub timeout: Duration,
}

impl CodexOAuthLoginOptions {
    pub fn default_for_home(home: &Path) -> Self {
        let codex_home = std::env::var_os("CODEX_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let issuer = std::env::var("CODEX_OAUTH_ISSUER")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CODEX_ISSUER.to_string());
        Self {
            codex_home,
            issuer,
            client_id: DEFAULT_CODEX_CLIENT_ID.to_string(),
            candidate_ports: vec![CODEX_DEFAULT_CALLBACK_PORT, CODEX_FALLBACK_CALLBACK_PORT],
            open_browser: true,
            timeout: Duration::from_secs(DEFAULT_REAUTH_TIMEOUT_SECS),
        }
    }
}

/// Result of a completed interactive re-authentication flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReauthSuccess {
    pub kind: SubscriptionKind,
    pub auth_file_path: Option<PathBuf>,
    pub detail_zh: String,
    pub detail_en: String,
}

/// Structured failure from interactive re-authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReauthError {
    Cancelled,
    TimedOut,
    CliNotInstalled {
        binary: String,
        command_hint: String,
    },
    Failed(String),
}

impl std::fmt::Display for ReauthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "已取消重新登录"),
            Self::TimedOut => write!(f, "浏览器登录超时，请重试"),
            Self::CliNotInstalled {
                binary,
                command_hint,
            } => write!(
                f,
                "未检测到 `{binary}` CLI，请先安装或在终端运行 `{command_hint}` 后点击「刷新激活」"
            ),
            Self::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

async fn send_http_html(
    stream: &mut tokio::net::TcpStream,
    status_line: &str,
    body_html: &str,
) {
    let resp = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_html}",
        body_html.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

fn render_callback_html(ok: bool, title: &str, subtitle: &str) -> String {
    let accent = if ok { "#10B981" } else { "#EF4444" };
    let icon = if ok { "✓" } else { "!" };
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>{title}</title>
<style>
  body {{ margin:0; font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif; background:#0F1115; color:#F3F4F6; display:flex; align-items:center; justify-content:center; min-height:100vh; }}
  .card {{ background:#171A21; border:1px solid #262B36; border-radius:16px; padding:32px 36px; max-width:420px; text-align:center; box-shadow:0 16px 40px rgba(0,0,0,.45); }}
  .badge {{ width:48px; height:48px; border-radius:999px; margin:0 auto 16px; display:flex; align-items:center; justify-content:center; font-size:22px; font-weight:700; color:#fff; background:{accent}; }}
  h1 {{ margin:0 0 10px; font-size:19px; font-weight:600; }}
  p {{ margin:0; font-size:13.5px; line-height:1.55; color:#9CA3AF; }}
</style>
</head>
<body>
  <div class="card">
    <div class="badge">{icon}</div>
    <h1>{title}</h1>
    <p>{subtitle}</p>
  </div>
</body>
</html>"#
    )
}

async fn try_cancel_existing_loopback(port: u16) {
    if let Ok(Ok(mut stream)) = tokio::time::timeout(
        Duration::from_millis(600),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    {
        let req = format!(
            "GET /cancel HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(req.as_bytes()).await;
        let _ = stream.flush().await;
    }
}

async fn bind_codex_callback_listener(candidate_ports: &[u16]) -> Result<(TcpListener, u16), String> {
    let ports: Vec<u16> = if candidate_ports.is_empty() {
        vec![CODEX_DEFAULT_CALLBACK_PORT, CODEX_FALLBACK_CALLBACK_PORT]
    } else {
        candidate_ports.to_vec()
    };

    for (idx, port) in ports.iter().copied().enumerate() {
        let addr = format!("127.0.0.1:{port}");
        match TcpListener::bind(&addr).await {
            Ok(listener) => {
                let actual = listener
                    .local_addr()
                    .map_err(|e| format!("Failed reading bound OAuth callback port: {e}"))?
                    .port();
                return Ok((listener, actual));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse && port != 0 => {
                if idx == 0 {
                    try_cancel_existing_loopback(port).await;
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    if let Ok(listener) = TcpListener::bind(&addr).await {
                        let actual = listener
                            .local_addr()
                            .map_err(|e| format!("Failed reading bound OAuth callback port: {e}"))?
                            .port();
                        return Ok((listener, actual));
                    }
                }
            }
            Err(err) => {
                return Err(format!("无法启动本地 OAuth 回调端口 {addr}: {err}"));
            }
        }
    }

    Err("本地 OAuth 回调端口 1455/1457 已被占用".to_string())
}

/// Runs the full Codex / ChatGPT browser OAuth2 PKCE loopback login flow and atomically persists
/// fresh credentials into `opts.codex_home/auth.json`.
///
/// The optional `on_authorize_url` hook receives `(actual_port, authorize_url)` as soon as the
/// loopback listener is bound (used by tests and optional UI status display).
pub async fn run_codex_pkce_oauth_login(
    opts: CodexOAuthLoginOptions,
    mut cancel_rx: watch::Receiver<bool>,
    on_authorize_url: Option<Box<dyn FnOnce(u16, String) + Send>>,
) -> Result<ReauthSuccess, ReauthError> {
    if *cancel_rx.borrow() {
        return Err(ReauthError::Cancelled);
    }
    let pkce = generate_pkce_pair();
    let state = generate_oauth_state();
    let (listener, actual_port) = bind_codex_callback_listener(&opts.candidate_ports)
        .await
        .map_err(ReauthError::Failed)?;
    let redirect_uri = format!("http://localhost:{actual_port}/auth/callback");
    let auth_url = build_codex_authorize_url(
        &opts.issuer,
        &opts.client_id,
        &redirect_uri,
        &pkce,
        &state,
    );

    if let Some(cb) = on_authorize_url {
        cb(actual_port, auth_url.clone());
    }
    if opts.open_browser {
        open_external_url_in_system_browser(&auth_url).map_err(ReauthError::Failed)?;
    }

    magpie_gateway::invalidate_system_proxy_cache();
    let deadline = tokio::time::Instant::now() + opts.timeout;
    let http = magpie_gateway::build_proxy_aware_http_client(
        Some(Duration::from_secs(10)),
        Some(Duration::from_secs(20)),
    );

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(ReauthError::TimedOut);
        }
        let remaining = deadline.saturating_duration_since(now);
        let accept_res = tokio::select! {
            _ = cancel_rx.changed() => {
                if *cancel_rx.borrow() {
                    return Err(ReauthError::Cancelled);
                }
                continue;
            }
            _ = tokio::time::sleep(remaining) => {
                return Err(ReauthError::TimedOut);
            }
            accepted = listener.accept() => accepted,
        };

        let Ok((mut stream, _peer)) = accept_res else {
            continue;
        };

        let mut buf = vec![0u8; 8192];
        let n = match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => n,
            _ => continue,
        };
        let raw_http = String::from_utf8_lossy(&buf[..n]);
        let Some((path, params)) = parse_request_path_and_query(&raw_http) else {
            send_http_html(
                &mut stream,
                "400 Bad Request",
                &render_callback_html(false, "请求无效", "无法解析浏览器回调参数。"),
            )
            .await;
            continue;
        };

        match path.as_str() {
            "/favicon.ico" => {
                let _ = stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                    .await;
                continue;
            }
            "/cancel" => {
                send_http_html(
                    &mut stream,
                    "200 OK",
                    &render_callback_html(false, "登录已取消", "您可以关闭此标签页并返回应用。"),
                )
                .await;
                return Err(ReauthError::Cancelled);
            }
            "/auth/callback" => {
                let got_state = params.get("state").map(String::as_str);
                if got_state != Some(state.as_str()) {
                    send_http_html(
                        &mut stream,
                        "400 Bad Request",
                        &render_callback_html(
                            false,
                            "登录状态校验失败",
                            "OAuth state 不匹配，请返回应用重新点击「重新登录」。",
                        ),
                    )
                    .await;
                    continue;
                }
                if let Some(err_code) = params.get("error").filter(|s| !s.trim().is_empty()) {
                    let desc = params
                        .get("error_description")
                        .map(String::as_str)
                        .unwrap_or(err_code.as_str());
                    let msg = format!("ChatGPT / Codex 授权失败: {desc}");
                    send_http_html(
                        &mut stream,
                        "200 OK",
                        &render_callback_html(false, "授权未完成", &msg),
                    )
                    .await;
                    return Err(ReauthError::Failed(msg));
                }
                let Some(code) = params.get("code").filter(|s| !s.trim().is_empty()) else {
                    send_http_html(
                        &mut stream,
                        "400 Bad Request",
                        &render_callback_html(
                            false,
                            "缺少授权码",
                            "浏览器回调缺少 authorization code，请返回应用重试。",
                        ),
                    )
                    .await;
                    return Err(ReauthError::Failed("OAuth callback missing code".to_string()));
                };

                let token_endpoint = format!("{}/oauth/token", opts.issuer.trim_end_matches('/'));
                let body = format!(
                    "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
                    url_encode_query_component(code),
                    url_encode_query_component(&redirect_uri),
                    url_encode_query_component(&opts.client_id),
                    url_encode_query_component(&pkce.code_verifier),
                );
                let resp = http
                    .post(&token_endpoint)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| ReauthError::Failed(format!("令牌交换网络错误: {e}")))?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let raw_err = resp.text().await.unwrap_or_default();
                    let clean = magpie_gateway::redact_oauth_error_body(&raw_err);
                    let msg = format!("令牌交换失败 ({status}): {clean}");
                    send_http_html(
                        &mut stream,
                        "200 OK",
                        &render_callback_html(false, "登录失败", &msg),
                    )
                    .await;
                    return Err(ReauthError::Failed(msg));
                }

                let val: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ReauthError::Failed(format!("解析登录令牌失败: {e}")))?;
                let access_token = val
                    .get("access_token")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ReauthError::Failed("登录响应缺少 access_token".to_string()))?;
                let refresh_token = val
                    .get("refresh_token")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ReauthError::Failed("登录响应缺少 refresh_token".to_string()))?;
                let id_token = val
                    .get("id_token")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(access_token);

                let auth_path = persist_codex_auth_json(
                    &opts.codex_home,
                    id_token,
                    access_token,
                    refresh_token,
                    None,
                )
                .map_err(|e| ReauthError::Failed(format!("保存 Codex 凭据失败: {e}")))?;

                send_http_html(
                    &mut stream,
                    "200 OK",
                    &render_callback_html(
                        true,
                        "ChatGPT / Codex 登录成功",
                        "凭据已安全保存并正在自动重新激活，您可以关闭此页面返回应用。",
                    ),
                )
                .await;

                return Ok(ReauthSuccess {
                    kind: SubscriptionKind::CodexChatGpt,
                    auth_file_path: Some(auth_path),
                    detail_zh: "ChatGPT / Codex 登录成功".to_string(),
                    detail_en: "ChatGPT / Codex sign-in completed".to_string(),
                });
            }
            _ => {
                send_http_html(
                    &mut stream,
                    "404 Not Found",
                    &render_callback_html(false, "页面不存在", "请返回应用完成登录流程。"),
                )
                .await;
            }
        }
    }
}

async fn run_managed_cli_login(
    kind: SubscriptionKind,
    binary: &str,
    args: &[&str],
    command_hint: &str,
    mut cancel_rx: watch::Receiver<bool>,
    timeout: Duration,
) -> Result<ReauthSuccess, ReauthError> {
    if *cancel_rx.borrow() {
        return Err(ReauthError::Cancelled);
    }
    let Some(bin_path) = resolve_cli_binary(binary) else {
        return Err(ReauthError::CliNotInstalled {
            binary: binary.to_string(),
            command_hint: command_hint.to_string(),
        });
    };

    let mut cmd = tokio::process::Command::new(&bin_path);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| ReauthError::Failed(format!("无法启动 `{command_hint}`: {e}")))?;

    let status_res = tokio::select! {
        _ = cancel_rx.changed() => {
            if *cancel_rx.borrow() {
                let _ = child.kill().await;
                return Err(ReauthError::Cancelled);
            }
            child.wait().await
        }
        _ = tokio::time::sleep(timeout) => {
            let _ = child.kill().await;
            return Err(ReauthError::TimedOut);
        }
        res = child.wait() => res,
    };

    let status = status_res
        .map_err(|e| ReauthError::Failed(format!("等待 `{command_hint}` 退出失败: {e}")))?;
    if status.success() {
        return Ok(ReauthSuccess {
            kind,
            auth_file_path: None,
            detail_zh: format!("{} 重新登录成功", kind.display_name()),
            detail_en: format!("{} sign-in completed", kind.display_name()),
        });
    }

    let mut stderr_buf = String::new();
    if let Some(mut err_pipe) = child.stderr.take() {
        let _ = tokio::io::AsyncReadExt::read_to_string(&mut err_pipe, &mut stderr_buf).await;
    }
    let tail = stderr_buf
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(str::trim)
        .unwrap_or("");
    let code_str = status
        .code()
        .map(|c| format!("exit {c}"))
        .unwrap_or_else(|| "terminated".to_string());
    if tail.is_empty() {
        Err(ReauthError::Failed(format!(
            "`{command_hint}` 未完成 ({code_str})，请在浏览器完成授权后重试"
        )))
    } else {
        let truncated: String = tail.chars().take(160).collect();
        Err(ReauthError::Failed(format!(
            "`{command_hint}` 失败 ({code_str}): {truncated}"
        )))
    }
}

/// Runs the appropriate interactive browser/CLI re-authentication flow for `kind`.
pub async fn run_interactive_subscription_reauth(
    kind: SubscriptionKind,
    home: &Path,
    cancel_rx: watch::Receiver<bool>,
) -> Result<ReauthSuccess, ReauthError> {
    let timeout = Duration::from_secs(DEFAULT_REAUTH_TIMEOUT_SECS);
    match kind {
        SubscriptionKind::CodexChatGpt => {
            let opts = CodexOAuthLoginOptions::default_for_home(home);
            match run_codex_pkce_oauth_login(opts, cancel_rx.clone(), None).await {
                Ok(ok) => Ok(ok),
                Err(ReauthError::Cancelled) => Err(ReauthError::Cancelled),
                Err(ReauthError::TimedOut) => Err(ReauthError::TimedOut),
                Err(first_err) => {
                    // Fallback: if local loopback port bind failed and `codex` CLI is installed, invoke `codex login`.
                    if resolve_cli_binary("codex").is_some()
                        && matches!(&first_err, ReauthError::Failed(m) if m.contains("端口"))
                    {
                        run_managed_cli_login(
                            SubscriptionKind::CodexChatGpt,
                            "codex",
                            &["login"],
                            "codex login",
                            cancel_rx,
                            timeout,
                        )
                        .await
                    } else {
                        Err(first_err)
                    }
                }
            }
        }
        SubscriptionKind::GoogleVertex
        | SubscriptionKind::GeminiCodeAssist
        | SubscriptionKind::Antigravity => {
            run_managed_cli_login(
                kind,
                "gcloud",
                &["auth", "application-default", "login"],
                "gcloud auth application-default login",
                cancel_rx,
                timeout,
            )
            .await
        }
        SubscriptionKind::ClaudeCode => {
            run_managed_cli_login(
                SubscriptionKind::ClaudeCode,
                "claude",
                &["auth", "login"],
                "claude auth login",
                cancel_rx,
                timeout,
            )
            .await
        }
        SubscriptionKind::GitHubCopilot => {
            run_managed_cli_login(
                SubscriptionKind::GitHubCopilot,
                "gh",
                &["auth", "login", "--web"],
                "gh auth login --web",
                cancel_rx,
                timeout,
            )
            .await
        }
        SubscriptionKind::GrokSuperGrok => Err(ReauthError::Failed(
            "Grok 请在服务商配置页更新 API Key 或重新获取会话凭据".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_classification_maps_expired_auth_and_network_accurately() {
        let vertex = classify_provider_recovery(
            "google_vertex:user@example.com",
            Some("google_vertex"),
            true,
            Some("oauth_invalid_grant"),
            Some("Google Vertex AI OAuth 授权已过期 (invalid_grant)"),
        );
        assert_eq!(vertex.tone, UnavailableBadgeTone::Expired);
        assert_eq!(vertex.badge_zh, "已过期");
        assert!(vertex.supports_browser_relogin);
        assert_eq!(vertex.primary_cta_zh, "重新登录");
        assert_eq!(
            vertex.cli_command_hint,
            Some("gcloud auth application-default login")
        );

        let codex = classify_provider_recovery(
            "codex:dev@example.com",
            Some("codex_chatgpt"),
            true,
            Some("token_expired"),
            Some("Codex token expired"),
        );
        assert_eq!(codex.tone, UnavailableBadgeTone::Expired);
        assert!(codex.supports_browser_relogin);
        assert_eq!(codex.primary_cta_zh, "重新登录");
        assert_eq!(codex.cli_command_hint, Some("codex login"));

        let api_401 = classify_provider_recovery(
            "deepseek",
            Some("deepseek"),
            false,
            Some("auth_401"),
            Some("认证失败 (401)"),
        );
        assert_eq!(api_401.tone, UnavailableBadgeTone::AuthError);
        assert_eq!(api_401.badge_zh, "认证失败");
        assert!(!api_401.supports_browser_relogin);
        assert!(api_401.prefer_open_editor);
        assert_eq!(api_401.primary_cta_zh, "更新密钥");

        let ollama_down = classify_provider_recovery(
            "ollama",
            Some("ollama"),
            false,
            Some("unreachable"),
            Some("本地服务未启动或无法连接"),
        );
        assert_eq!(ollama_down.tone, UnavailableBadgeTone::Network);
        assert_eq!(ollama_down.badge_zh, "无法连接");
        assert_eq!(ollama_down.primary_cta_zh, "重试激活");
        assert!(!ollama_down.prefer_open_editor);
    }

    #[test]
    fn codex_pkce_loopback_login_exchanges_code_and_writes_private_auth_json() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let tmp = tempfile::TempDir::new().expect("tempdir");
            let codex_home = tmp.path().join(".codex");

            // Mock OpenAI OAuth token endpoint
            let token_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
            let token_addr = token_listener.local_addr().expect("addr");
            let jwt_hdr = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
            let future_exp = chrono::Utc::now().timestamp() + 3600;
            let jwt_claims = URL_SAFE_NO_PAD.encode(
                serde_json::json!({
                    "email": "fresh-codex@example.com",
                    "exp": future_exp,
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": "acct_relogin_77",
                        "chatgpt_plan_type": "pro"
                    }
                })
                .to_string()
                .as_bytes(),
            );
            let fresh_jwt = format!("{jwt_hdr}.{jwt_claims}.sig");
            let fresh_jwt_srv = fresh_jwt.clone();
            tokio::spawn(async move {
                if let Ok((mut sock, _)) = token_listener.accept().await {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req_str = String::from_utf8_lossy(&buf[..n]);
                    assert!(req_str.contains("grant_type=authorization_code"));
                    assert!(req_str.contains("code=test_auth_code_99"));
                    assert!(req_str.contains("code_verifier="));
                    let body = serde_json::json!({
                        "access_token": fresh_jwt_srv,
                        "refresh_token": "rt_fresh_codex_99",
                        "id_token": fresh_jwt_srv,
                        "token_type": "Bearer",
                        "expires_in": 3600
                    })
                    .to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                }
            });

            let (url_tx, url_rx) = tokio::sync::oneshot::channel::<(u16, String)>();
            let (_cancel_tx, cancel_rx) = watch::channel(false);
            let opts = CodexOAuthLoginOptions {
                codex_home: codex_home.clone(),
                issuer: format!("http://{token_addr}"),
                client_id: DEFAULT_CODEX_CLIENT_ID.to_string(),
                candidate_ports: vec![0],
                open_browser: false,
                timeout: Duration::from_secs(10),
            };

            let login_task = tokio::spawn(run_codex_pkce_oauth_login(
                opts,
                cancel_rx,
                Some(Box::new(move |port, url| {
                    let _ = url_tx.send((port, url));
                })),
            ));

            let (port, auth_url) = url_rx.await.expect("authorize url emitted");
            assert!(auth_url.contains("code_challenge_method=S256"));
            let state = auth_url
                .split("state=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .expect("state param");

            // Simulate browser redirect back to localhost callback
            let cb_url = format!(
                "http://127.0.0.1:{port}/auth/callback?code=test_auth_code_99&state={state}"
            );
            let cb_resp = reqwest::get(&cb_url).await.expect("callback GET");
            assert_eq!(cb_resp.status(), 200);
            let html = cb_resp.text().await.expect("html");
            assert!(html.contains("登录成功"));

            let outcome = login_task.await.expect("join").expect("login ok");
            assert_eq!(outcome.kind, SubscriptionKind::CodexChatGpt);
            let auth_path = codex_home.join("auth.json");
            assert!(auth_path.is_file());

            // Verify magpie_gateway immediately discovers the newly persisted Codex login as active
            let reg = magpie_gateway::discover_subscriptions_in_home(tmp.path(), Some(&codex_home));
            let acc = reg
                .accounts
                .iter()
                .find(|a| a.kind == SubscriptionKind::CodexChatGpt)
                .expect("discovered fresh Codex account");
            assert!(acc.active);
            assert_eq!(acc.email.as_deref(), Some("fresh-codex@example.com"));
            assert_eq!(acc.account_id.as_deref(), Some("acct_relogin_77"));
        });
    }
}
