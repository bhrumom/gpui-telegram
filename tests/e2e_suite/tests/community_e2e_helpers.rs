#![allow(dead_code)]

//! Shared Opaque-Box E2E Helper Harness for the Fused Reddit + Twitter (X) Community Suite.
//! Reused by Tiers 1, 2, 3, and 4 (`community_e2e_tier{1..4}.rs`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::{any, get, post};
use chrono::Utc;
use e2e_suite::browser_e2e_fixtures::no_proxy_http_client;
use e2e_suite::common::{EphemeralTestServer, create_test_png};
use e2e_suite::gpui_browser_harness::pump_until;
use gpui::{
    AppContext, AsyncApp, Entity, Keystroke, WindowBounds, WindowHandle, WindowOptions, bounds,
    point, px, size,
};
use gpui_telegram_agent::ApprovalMode;
use gpui_telegram_agent::hermes::toolsets::{HermesToolExecutor, HermesToolResult, ToolsetPreset};
use gpui_telegram_client::config::ClientConfig;
use gpui_telegram_client::{AuthResponse, CommunityClientFacade, TelegramClient};
use gpui_telegram_desktop::views::community::CommunityWorkspace;
use gpui_telegram_desktop::views::shell::{ActiveWorkspacePanel, AppShell};
use gpui_telegram_proto::community_contracts::CommunityActorDto;
use reqwest::Method;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Returns a clean index directory path inside a `TempDir` that NEVER contains
/// forbidden garbage-cleaner substrings (`tantivy`, `search_index`, `fts_index`).
pub fn safe_temp_idx_dir(temp: &TempDir) -> PathBuf {
    temp.path().join("community.idx")
}

/// Returns the current UTC timestamp in milliseconds.
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Pure-Rust RFC 4648 base64 encoder for uploading PNG media fixtures without extra crate deps.
pub fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Allocates an ephemeral loopback port, drops the listener immediately, and returns
/// an unreachable `http://127.0.0.1:<port>` URL for offline fallback tests.
pub fn closed_local_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral closed-port probe listener");
    let port = listener.local_addr().expect("probe local_addr").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

/// Authentication modes supported by `/api/v1/community/*` endpoints.
#[derive(Debug, Clone)]
pub enum CommunityAuthMode {
    Human(String),
    ActAsBot { owner_token: String, actor_id: String },
    DirectBot(String),
    DirectBotWithActorHeader { bot_token: String, actor_id: String },
    None,
}

#[derive(Debug, Clone)]
pub struct RegisteredHuman {
    pub user_id: i64,
    pub username: String,
    pub token: String,
    pub actor_id: String,
}

#[derive(Debug, Clone)]
pub struct RegisteredCommunityBot {
    pub bot_user_id: i64,
    pub username: String,
    pub handle: String,
    pub bot_token: String,
    pub actor_id: String,
    pub actor: CommunityActorDto,
}

/// Opaque-box HTTP driver built on `no_proxy_http_client()` covering all 3 Community auth modes,
/// Bot API split & concatenated routes, and media upload/download endpoints.
#[derive(Clone)]
pub struct CommunityHttpDriver {
    base_url: String,
    http: reqwest::Client,
}

impl CommunityHttpDriver {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: no_proxy_http_client(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn apply_auth(&self, mut req: reqwest::RequestBuilder, auth: &CommunityAuthMode) -> reqwest::RequestBuilder {
        match auth {
            CommunityAuthMode::Human(tok) => req = req.header("Authorization", format!("Bearer {tok}")),
            CommunityAuthMode::ActAsBot { owner_token, actor_id } => {
                req = req
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .header("X-Community-Actor-Id", actor_id);
            }
            CommunityAuthMode::DirectBot(bot_tok) => {
                req = req.header("Authorization", format!("Bot {bot_tok}"));
            }
            CommunityAuthMode::DirectBotWithActorHeader { bot_token, actor_id } => {
                req = req
                    .header("Authorization", format!("Bot {bot_token}"))
                    .header("X-Community-Actor-Id", actor_id);
            }
            CommunityAuthMode::None => {}
        }
        req
    }

    pub async fn req(
        &self,
        method: Method,
        path: &str,
        auth: &CommunityAuthMode,
        body: Option<Value>,
    ) -> anyhow::Result<(u16, Value)> {
        let url = format!("{}{}", self.base_url, path);
        let mut builder = self.apply_auth(self.http.request(method, url), auth);
        if let Some(payload) = body {
            builder = builder.json(&payload);
        }
        let resp = builder.send().await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await?;
        let val = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes).into_owned() }));
        Ok((status, val))
    }

    pub async fn req_empty_body(
        &self,
        method: Method,
        path: &str,
        auth: &CommunityAuthMode,
    ) -> anyhow::Result<(u16, Value)> {
        self.req(method, path, auth, None).await
    }

    pub async fn req_raw(
        &self,
        method: Method,
        path: &str,
        auth: &CommunityAuthMode,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        let url = format!("{}{}", self.base_url, path);
        let mut builder = self.apply_auth(self.http.request(method, url), auth);
        if let Some(ct) = content_type {
            builder = builder.header("Content-Type", ct);
        }
        let resp = builder.body(body).send().await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await?.to_vec();
        Ok((status, bytes))
    }

    pub async fn register_human(&self, username: &str, password: &str) -> anyhow::Result<RegisteredHuman> {
        let (st, val) = self
            .req(
                Method::POST,
                "/api/v1/auth/register",
                &CommunityAuthMode::None,
                Some(json!({
                    "username": username,
                    "password": password,
                    "first_name": username,
                    "device_name": "CommunityE2E"
                })),
            )
            .await?;
        anyhow::ensure!(st == 201 || st == 200, "register_human({username}) failed {st}: {val}");
        let user_id = val["user_id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("missing user_id in {val}"))?;
        let token = val["token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing token in {val}"))?
            .to_string();
        Ok(RegisteredHuman {
            user_id,
            username: username.to_string(),
            token,
            actor_id: format!("u:{user_id}"),
        })
    }

    pub async fn register_community_bot(
        &self,
        owner_token: &str,
        handle: &str,
        display_name: &str,
        autonomy: &str,
    ) -> anyhow::Result<RegisteredCommunityBot> {
        let (st, val) = self
            .req(
                Method::POST,
                "/api/v1/community/identities/bot",
                &CommunityAuthMode::Human(owner_token.to_string()),
                Some(json!({
                    "handle": handle,
                    "display_name": display_name,
                    "bio": format!("Automated first-class citizen {display_name}"),
                    "bot_autonomy_mode": autonomy,
                    "bot_capabilities": ["rust", "analysis", "moderation"],
                    "bot_persona": format!("Helpful engineering citizen {display_name}")
                })),
            )
            .await?;
        anyhow::ensure!(st == 201, "register_community_bot({handle}) failed {st}: {val}");
        let actor: CommunityActorDto = serde_json::from_value(val["actor"].clone())?;
        let bot_token = val["bot_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing bot_token in {val}"))?
            .to_string();
        let bot_user_id = val["bot_user_id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("missing bot_user_id in {val}"))?;
        let username = val["username"].as_str().unwrap_or(handle).to_string();
        Ok(RegisteredCommunityBot {
            bot_user_id,
            username,
            handle: actor.handle.clone(),
            bot_token,
            actor_id: actor.actor_id.clone(),
            actor,
        })
    }

    pub async fn create_bot(
        &self,
        owner_token: &str,
        username: &str,
        display_name: &str,
    ) -> anyhow::Result<(i64, String, Value)> {
        let (st, val) = self
            .req(
                Method::POST,
                "/api/v1/bots",
                &CommunityAuthMode::Human(owner_token.to_string()),
                Some(json!({
                    "display_name": display_name,
                    "username": username,
                    "description": format!("Bot {display_name}")
                })),
            )
            .await?;
        anyhow::ensure!(st == 201 || st == 200, "create_bot({username}) failed {st}: {val}");
        let bot_user_id = val["bot"]["bot_user_id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("missing bot.bot_user_id in {val}"))?;
        let bot_token = val["bot_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing bot_token in {val}"))?
            .to_string();
        Ok((bot_user_id, bot_token, val))
    }

    pub async fn create_fabushi_bot(
        &self,
        owner_token: &str,
        display_name: &str,
        username: Option<&str>,
    ) -> anyhow::Result<(i64, String, Value)> {
        let mut payload = json!({
            "display_name": display_name,
            "description": format!("Fabushi AI citizen {display_name}"),
            "title": "Community AI Citizen"
        });
        if let Some(u) = username {
            payload["username"] = json!(u);
        }
        let (st, val) = self
            .req(
                Method::POST,
                "/api/v1/fabushi-bots",
                &CommunityAuthMode::Human(owner_token.to_string()),
                Some(payload),
            )
            .await?;
        anyhow::ensure!(st == 201 || st == 200, "create_fabushi_bot failed {st}: {val}");
        let bot_user_id = val["bot"]["bot_user_id"]
            .as_i64()
            .or_else(|| val["registry"]["bot_user_id"].as_i64())
            .ok_or_else(|| anyhow::anyhow!("missing bot_user_id in {val}"))?;
        let bot_token = val["bot_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing bot_token in {val}"))?
            .to_string();
        Ok((bot_user_id, bot_token, val))
    }

    /// Invokes standard Bot API route: `/bot/{token}/{method}` when `split_route == true`,
    /// or `/bot{token}/{method}` when `split_route == false`.
    pub async fn bot_api(
        &self,
        bot_token: &str,
        method_name: &str,
        split_route: bool,
        body: Option<Value>,
    ) -> anyhow::Result<(u16, Value)> {
        let path = if split_route {
            format!("/bot/{bot_token}/{method_name}")
        } else {
            format!("/bot{bot_token}/{method_name}")
        };
        self.req(Method::POST, &path, &CommunityAuthMode::None, body).await
    }

    pub async fn bot_api_get(
        &self,
        bot_token: &str,
        method_name: &str,
        split_route: bool,
        query: &str,
    ) -> anyhow::Result<(u16, Value)> {
        let base = if split_route {
            format!("/bot/{bot_token}/{method_name}")
        } else {
            format!("/bot{bot_token}/{method_name}")
        };
        let path = if query.is_empty() {
            base
        } else {
            format!("{base}?{query}")
        };
        self.req(Method::GET, &path, &CommunityAuthMode::None, None).await
    }

    pub async fn upload_png(
        &self,
        bearer_token: &str,
        filename: &str,
        rgba: (u8, u8, u8, u8),
        chat_id: Option<i64>,
    ) -> anyhow::Result<(String, Vec<u8>)> {
        let png = create_test_png(rgba.0, rgba.1, rgba.2, rgba.3);
        let path = match chat_id {
            Some(cid) => format!("/api/v1/media/upload?chat_id={cid}"),
            None => "/api/v1/media/upload".to_string(),
        };
        let (st, val) = self
            .req(
                Method::POST,
                &path,
                &CommunityAuthMode::Human(bearer_token.to_string()),
                Some(json!({
                    "filename": filename,
                    "mime_type": "image/png",
                    "data_base64": encode_base64(&png)
                })),
            )
            .await?;
        anyhow::ensure!(st == 201 || st == 200, "upload_png failed {st}: {val}");
        let media_id = val["media_id"]
            .as_str()
            .or_else(|| val["file_id"].as_str())
            .ok_or_else(|| anyhow::anyhow!("missing media_id in {val}"))?
            .to_string();
        Ok((media_id, png))
    }

    pub async fn get_media(&self, bearer_token: &str, media_id: &str) -> anyhow::Result<(u16, Vec<u8>)> {
        self.req_raw(
            Method::GET,
            &format!("/api/v1/media/{media_id}"),
            &CommunityAuthMode::Human(bearer_token.to_string()),
            None,
            Vec::new(),
        )
        .await
    }

    pub async fn get_media_meta(&self, bearer_token: &str, media_id: &str) -> anyhow::Result<(u16, Value)> {
        self.req(
            Method::GET,
            &format!("/api/v1/media/{media_id}/meta"),
            &CommunityAuthMode::Human(bearer_token.to_string()),
            None,
        )
        .await
    }
}

/// Ephemeral `127.0.0.1:0` webhook receiver capturing `(X-Telegram-Bot-Api-Secret-Token, JSON body)`.
pub struct WebhookCaptureServer {
    addr: SocketAddr,
    pub rx: mpsc::UnboundedReceiver<(Option<String>, Value)>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl WebhookCaptureServer {
    pub async fn start() -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel::<(Option<String>, Value)>();
        let handler = |State(sender): State<mpsc::UnboundedSender<(Option<String>, Value)>>,
                       headers: HeaderMap,
                       body: Bytes| async move {
            let secret = headers
                .get("x-telegram-bot-api-secret-token")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let parsed = serde_json::from_slice::<Value>(&body)
                .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&body).into_owned() }));
            let _ = sender.send((secret, parsed));
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        };
        let router = Router::new()
            .route("/webhook", post(handler))
            .route("/webhook/{*rest}", post(handler))
            .with_state(tx);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self { addr, rx, shutdown_tx: Some(shutdown_tx), task: Some(task) })
    }

    pub fn url(&self) -> String {
        format!("http://{}/webhook", self.addr)
    }

    pub async fn recv_timeout(&mut self, timeout: Duration) -> Option<(Option<String>, Value)> {
        tokio::time::timeout(timeout, self.rx.recv()).await.ok().flatten()
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }
}

impl Drop for WebhookCaptureServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// Ephemeral `127.0.0.1:0` server mimicking a legacy server without Community routes:
/// returns `200 OK` on `/health` and `404 {"error":"not_found"}` on `/api/v1/community/{*path}`.
pub struct Legacy404Server {
    addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl Legacy404Server {
    pub async fn start() -> anyhow::Result<Self> {
        let not_found = || async {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "message": "Not Found" })),
            )
        };
        let router = Router::new()
            .route("/health", get(|| async { (StatusCode::OK, Json(json!({ "ok": true }))) }))
            .route(
                "/api/v1/status",
                get(|| async {
                    (
                        StatusCode::OK,
                        Json(json!({ "status": "ok", "capabilities": ["messages"] })),
                    )
                }),
            )
            .route("/api/v1/community", any(not_found))
            .route("/api/v1/community/{*path}", any(not_found));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self { addr, shutdown_tx: Some(shutdown_tx), task: Some(task) })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }
}

impl Drop for Legacy404Server {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// Constructs an `AuthResponse` for binding a `TelegramClient` or `AppShell` session.
pub fn test_auth_response(user_id: u64, username: &str, token: &str) -> AuthResponse {
    AuthResponse {
        user_id,
        username: username.to_string(),
        session_id: format!("sess-e2e-{user_id}"),
        token: token.to_string(),
        first_name: Some(username.to_string()),
        created_at: None,
        expires_at: None,
    }
}

/// Safe async wrapper around `TelegramClient::open_with_dir` + RAII `TempDir`.
/// ALWAYS call `shutdown().await` (or `shutdown_keep_dir().await`) before returning from
/// `#[tokio::test]` to avoid Tokio's nested-runtime drop panic.
pub struct TestTelegramClient {
    client: Option<Arc<TelegramClient>>,
    temp_dir: Option<TempDir>,
    dir_path: PathBuf,
}

impl TestTelegramClient {
    pub fn open(server_url: &str) -> anyhow::Result<Self> {
        let dir = TempDir::new()?;
        Self::open_in_dir(server_url, dir)
    }

    pub fn open_in_dir(server_url: &str, dir: TempDir) -> anyhow::Result<Self> {
        let dir_path = dir.path().to_path_buf();
        let cfg = ClientConfig {
            server_url: server_url.to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        };
        let client = Arc::new(TelegramClient::open_with_dir(cfg, &dir_path)?);
        Ok(Self {
            client: Some(client),
            temp_dir: Some(dir),
            dir_path,
        })
    }

    pub fn open_at_path(server_url: &str, path: &Path) -> anyhow::Result<Self> {
        let cfg = ClientConfig {
            server_url: server_url.to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        };
        let client = Arc::new(TelegramClient::open_with_dir(cfg, path)?);
        Ok(Self {
            client: Some(client),
            temp_dir: None,
            dir_path: path.to_path_buf(),
        })
    }

    pub fn client(&self) -> &Arc<TelegramClient> {
        self.client.as_ref().expect("TestTelegramClient already shut down")
    }

    pub fn community(&self) -> &CommunityClientFacade {
        self.client().community()
    }

    pub fn dir_path(&self) -> &Path {
        &self.dir_path
    }

    pub fn bind_auth(&self, user_id: u64, username: &str, token: &str) -> AuthResponse {
        let auth = test_auth_response(user_id, username, token);
        self.client().set_auth_session(auth.clone());
        auth
    }

    pub async fn register_and_bind(
        &self,
        driver: &CommunityHttpDriver,
        username: &str,
        password: &str,
    ) -> anyhow::Result<RegisteredHuman> {
        let human = driver.register_human(username, password).await?;
        self.bind_auth(human.user_id as u64, &human.username, &human.token);
        Ok(human)
    }

    pub async fn shutdown(mut self) {
        if let Some(c) = self.client.take() {
            c.disconnect_ws();
            tokio::task::spawn_blocking(move || drop(c))
                .await
                .expect("blocking drop of TelegramClient");
        }
        drop(self.temp_dir.take());
    }

    pub async fn shutdown_keep_dir(mut self) -> Option<TempDir> {
        if let Some(c) = self.client.take() {
            c.disconnect_ws();
            tokio::task::spawn_blocking(move || drop(c))
                .await
                .expect("blocking drop of TelegramClient");
        }
        self.temp_dir.take()
    }
}

/// Executes a Hermes Community tool strictly as Bot citizen `b:{bot_user_id}` through
/// `AppState`'s live `ServerPlatformImBridge` (`user_id = None`, `bot_id = Some(bot_user_id)`).
pub async fn execute_bot_hermes_tool(
    server: &EphemeralTestServer,
    bot_user_id: i64,
    tool_name: &str,
    args: Value,
    preset: ToolsetPreset,
    approval_mode: ApprovalMode,
) -> HermesToolResult {
    let state = server.state().expect("EphemeralTestServer AppState active");
    let ctx = state
        .fabushi_core()
        .build_tool_context_with_user(
            &format!("e2e_bot_{bot_user_id}"),
            None,
            None,
            None,
            1,
            Some(preset),
            0,
            Arc::new(AtomicBool::new(false)),
        )
        .with_chat_and_bot(None, Some(bot_user_id))
        .with_user_id(None)
        .with_approval_mode(approval_mode);
    HermesToolExecutor::execute(tool_name, args, &ctx).await
}

/// Executes a Hermes Community tool as Human citizen `u:{user_id}` through `AppState`'s live bridge.
pub async fn execute_human_hermes_tool(
    server: &EphemeralTestServer,
    user_id: i64,
    tool_name: &str,
    args: Value,
    preset: ToolsetPreset,
    approval_mode: ApprovalMode,
) -> HermesToolResult {
    let state = server.state().expect("EphemeralTestServer AppState active");
    let uid_str = user_id.to_string();
    let ctx = state
        .fabushi_core()
        .build_tool_context_with_user(
            &format!("e2e_human_{user_id}"),
            None,
            None,
            Some(&uid_str),
            1,
            Some(preset),
            0,
            Arc::new(AtomicBool::new(false)),
        )
        .with_approval_mode(approval_mode);
    HermesToolExecutor::execute(tool_name, args, &ctx).await
}

/// Headless GPUI shell host supporting both macOS real offscreen windows and Linux
/// `HeadlessClient` fallback while executing real `AppShell` + `CommunityWorkspace` layout/render.
pub struct HeadlessCommunityHost {
    pub win: Option<WindowHandle<AppShell>>,
    pub shell: Entity<AppShell>,
    pub client: Arc<TelegramClient>,
    pub width: f32,
    pub height: f32,
    pub _dir: Option<TempDir>,
}

impl HeadlessCommunityHost {
    pub fn open_community_and_wait<'a>(&'a self, app: &'a mut AsyncApp) -> std::pin::Pin<Box<dyn std::future::Future<Output = Entity<CommunityWorkspace>> + 'a>> {
        Box::pin(async move {
            let _ = self.shell.update(app, |s, cx| {
                s.open_community_workspace(cx);
            });
            let cw = self
                .shell
                .read_with(app, |s, _| {
                    assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
                    s.community_workspace
                        .clone()
                        .expect("CommunityWorkspace must be mounted")
                })
                .expect("read AppShell community_workspace");
            assert!(
                pump_until(app, Duration::from_secs(6), |cx| {
                    let st = &cw.read(cx).state;
                    !st.feed_loading && !st.communities.is_empty() && st.identities.is_some()
                })
                .await,
                "CommunityWorkspace failed to complete initial bootstrap within 6s"
            );
            cw
        })
    }

    pub fn cw(&self, app: &AsyncApp) -> Entity<CommunityWorkspace> {
        self.shell
            .read_with(app, |s, _| {
                s.community_workspace
                    .clone()
                    .expect("CommunityWorkspace not mounted")
            })
            .expect("read AppShell")
    }

    pub fn draw(&self, app: &mut AsyncApp) {
        self.draw_at(app, self.width, self.height);
    }

    pub fn draw_at(&self, app: &mut AsyncApp, width: f32, height: f32) {
        if let Some(win) = self.win {
            let _ = app.update_window(win.into(), |_, window, cx| {
                let _ = window.draw(cx);
            });
        }
        let _ = self.shell.update(app, |s, cx| {
            s.app.sync_viewport_size(width, height);
            let sb_w = s.sidebar.read(cx).effective_width();
            if let Some(cw) = s.community_workspace.clone() {
                cw.update(cx, |cw, wcx| {
                    cw.set_host_sidebar_width(sb_w);
                    let _el = cw.render_for_viewport_width(width, wcx);
                });
            }
        });
    }

    pub fn key(&self, app: &mut AsyncApp, spec: &str) {
        if let Some(win) = self.win {
            let _ = app.update_window(win.into(), |_, window, cx| {
                let _ = window.dispatch_keystroke(Keystroke::parse(spec).unwrap(), cx);
            });
            return;
        }
        let _ = self.shell.update(app, |s, cx| match spec {
            "escape" => {
                let _ = s.handle_non_char_key("escape", false, cx);
            }
            "enter" => {
                let _ = s.handle_non_char_key("enter", false, cx);
            }
            "shift-enter" => {
                let _ = s.handle_non_char_key("enter", true, cx);
            }
            "cmd-shift-c" => {
                let _ = s.handle_shortcut_key("c", true, cx);
            }
            "cmd-c" => {
                let _ = s.handle_shortcut_key("c", false, cx);
            }
            "cmd-b" => {
                let _ = s.handle_shortcut_key("b", false, cx);
            }
            "cmd-d" => {
                let _ = s.handle_shortcut_key("d", false, cx);
            }
            "cmd-l" => {
                let _ = s.handle_shortcut_key("l", false, cx);
            }
            "cmd-k" => {
                let _ = s.handle_shortcut_key("k", false, cx);
            }
            other => panic!("unsupported headless key spec: {other}"),
        });
    }
}

/// Opens an authenticated `AppShell` bound to `client` (either live server or offline local mode).
pub fn open_shell_for_community(
    app: &mut AsyncApp,
    client: Arc<TelegramClient>,
    user_id: u64,
    username: &str,
    token: &str,
    width: f32,
    height: f32,
    offline_force_local: bool,
) -> HeadlessCommunityHost {
    if offline_force_local {
        client.community().set_force_local(true);
    }
    let auth = test_auth_response(user_id, username, token);
    client.set_auth_session(auth.clone());

    let client_for_win = Arc::clone(&client);
    let auth_for_win = auth.clone();
    let (win, shell) = app
        .update(|cx| {
            let _rt = client_for_win.runtime().enter();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds(
                        point(px(0.0), px(0.0)),
                        size(px(width), px(height)),
                    ))),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                |_window, cx| {
                    cx.new(|cx| {
                        let mut shell = AppShell::new_with_client(client_for_win.clone(), cx);
                        shell.apply_authenticated_session(auth_for_win.clone(), cx);
                        shell.app.sync_viewport_size(width, height);
                        shell
                    })
                },
            );
            match opened {
                Ok(w) => {
                    let ent = w.entity(cx).expect("AppShell entity from window");
                    (Some(w), ent)
                }
                Err(err)
                    if cfg!(target_os = "linux")
                        && err.to_string().contains("headless mode") =>
                {
                    let ent = cx.new(|cx| {
                        let mut shell = AppShell::new_with_client(client_for_win.clone(), cx);
                        shell.apply_authenticated_session(auth_for_win.clone(), cx);
                        shell.app.sync_viewport_size(width, height);
                        shell
                    });
                    (None, ent)
                }
                Err(err) => panic!("open headless GPUI window failed unexpectedly: {err:#}"),
            }
        })
        .expect("app.update open_shell_for_community");

    HeadlessCommunityHost {
        win,
        shell,
        client,
        width,
        height,
        _dir: None,
    }
}

/// Convenience helper: opens an isolated TempDir offline-local `AppShell` and mounts `CommunityWorkspace`.
pub async fn open_offline_community_host(
    app: &mut AsyncApp,
    width: f32,
    height: f32,
    user_id: u64,
    username: &str,
    mount_community: bool,
) -> (HeadlessCommunityHost, Option<Entity<CommunityWorkspace>>) {
    let dir = TempDir::new().expect("create TempDir for headless GPUI client");
    let cfg = ClientConfig {
        server_url: closed_local_url(),
        max_reconnect_attempts: 0,
        ..Default::default()
    };
    let client = Arc::new(
        TelegramClient::open_with_dir(cfg, dir.path()).expect("TelegramClient::open_with_dir"),
    );
    let mut host = open_shell_for_community(
        app,
        client,
        user_id,
        username,
        &format!("tok-e2e-{user_id}"),
        width,
        height,
        true,
    );
    host._dir = Some(dir);
    let cw = if mount_community {
        Some(host.open_community_and_wait(app).await)
    } else {
        None
    };
    (host, cw)
}

/// Declares cross-platform (Linux + macOS `__DATA,__mod_init_func`) headless GPUI subprocess
/// dispatchers for a test binary reusing `e2e_suite::gpui_browser_harness`.
#[allow(unused_macros)]
macro_rules! define_community_gpui_harness {
    (
        child_env: $child_env:expr,
        target_env: $target_env:expr,
        after_ctors_env: $after_ctors_env:expr,
        targets: {
            $($name:literal => $body:path),* $(,)?
        }
    ) => {
        const GPUI_CHILD_ENV: &str = $child_env;
        const GPUI_CHILD_TARGET_ENV: &str = $target_env;
        const GPUI_CHILD_AFTER_CTORS_ENV: &str = $after_ctors_env;

        fn dispatch_gpui_target(target: &str, use_underscore_exit: bool) -> ! {
            type AsyncBody = fn(gpui::AsyncApp) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>;
            let body_fn: AsyncBody = match target {
                $($name => |app| Box::pin($body(app)),)*
                other => {
                    eprintln!("unknown community E2E headless GPUI target: {other}");
                    e2e_suite::gpui_browser_harness::exit_headless_child(101, use_underscore_exit);
                }
            };
            e2e_suite::gpui_browser_harness::run_headless_gpui_body(target, use_underscore_exit, body_fn);
        }

        #[cfg(target_os = "macos")]
        #[used]
        #[unsafe(link_section = "__DATA,__mod_init_func")]
        static MACOS_COMMUNITY_E2E_GPUI_ENTRY: extern "C" fn() = macos_mod_init_community_gpui_entry;

        #[cfg(target_os = "macos")]
        extern "C" fn macos_atexit_community_gpui_entry() {
            if let Ok(target) = std::env::var(GPUI_CHILD_TARGET_ENV) {
                dispatch_gpui_target(&target, true);
            }
        }

        #[cfg(target_os = "macos")]
        extern "C" fn macos_mod_init_community_gpui_entry() {
            if std::env::var(GPUI_CHILD_ENV).as_deref() == Ok("1") {
                if std::env::var(GPUI_CHILD_AFTER_CTORS_ENV).as_deref() == Ok("1") {
                    unsafe extern "C" {
                        fn atexit(func: extern "C" fn()) -> i32;
                    }
                    unsafe {
                        atexit(macos_atexit_community_gpui_entry);
                    }
                    return;
                }
                if let Ok(target) = std::env::var(GPUI_CHILD_TARGET_ENV) {
                    dispatch_gpui_target(&target, false);
                }
            }
        }

        fn run_community_gpui_test(test_fn_name: &str) {
            if std::env::var(GPUI_CHILD_ENV).as_deref() == Ok("1") {
                dispatch_gpui_target(test_fn_name, false);
            }
            e2e_suite::gpui_browser_harness::spawn_headless_subprocess(
                GPUI_CHILD_ENV,
                GPUI_CHILD_TARGET_ENV,
                GPUI_CHILD_AFTER_CTORS_ENV,
                test_fn_name,
                false,
                None,
                None,
            );
        }
    };
}
#[allow(unused_imports)]
pub(crate) use define_community_gpui_harness;
