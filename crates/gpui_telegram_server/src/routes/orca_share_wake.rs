//! Orca Package Export/Import, E2EE Team Share, Cross-App Import Discovery,
//! Loopback Tool Relay Control, and Content-Free Wake WebSocket (`GET /api/v1/orca/wake`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use base64::Engine as _;
use futures::SinkExt;
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::orca_contracts::{
    OrcaImportDiscoverRequest, OrcaPackageExportRequest, OrcaPackageImportRequest,
    OrcaRelayRegisterGrantRequest, OrcaRelayRegisterToolRequest, OrcaRelayRevokeRequest,
    OrcaRelayStartResponse, OrcaRelayStatusResponse, OrcaRelayToolAuthDto, OrcaShareOpenRequest,
    OrcaShareSealRequest,
};
use orca_bots::{
    BotImportDiscoveryRoots, BotImportEngine, BotRelayGrant, ContentFreeWakeFrame,
    ExportPackageOptions, OAuthProviderConfig, OAuthTokenRecord, RelayToolAuth,
    RelayToolRegistration, WakeFrameKind, CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES,
    WS_CLOSE_MESSAGE_TOO_BIG, WS_CLOSE_SUPERSEDED,
};
use orca_core::bot_ecosystem::{
    TeamShareHeldMessagePayload, TeamShareOfferPayload, TeamSharePublicIdentity,
};
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::routes::orca_error::{map_bots_err, orca_err, parse_json_body, parse_optional_json_body};
use crate::server::{AppState, AuthBearer};

fn now_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn now_epoch_u64() -> u64 {
    now_epoch_ms().max(0) as u64
}

// ---------------------------------------------------------------------------
// 1. OrcaWakeHub & Content-Free Wake WebSocket (`GET /api/v1/orca/wake`)
// ---------------------------------------------------------------------------

enum WakeControl {
    Frame(String),
    Superseded,
}

struct ActiveWakeConn {
    conn_id: u64,
    tx: mpsc::UnboundedSender<WakeControl>,
}

/// Manages active Content-Free Wake WebSockets keyed by `(UserId, host_id)`.
/// Enforces immediate `4409` supersede close when a newer socket connects for the same key.
#[derive(Default)]
pub struct OrcaWakeHub {
    next_conn_id: AtomicU64,
    next_seq: AtomicU64,
    conns: RwLock<HashMap<(UserId, String), ActiveWakeConn>>,
}

impl std::fmt::Debug for OrcaWakeHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrcaWakeHub")
            .field("active_connections", &self.conns.read().len())
            .finish()
    }
}

impl OrcaWakeHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn register(
        &self,
        user_id: UserId,
        host_id: String,
    ) -> (u64, mpsc::UnboundedReceiver<WakeControl>) {
        let conn_id = self.next_conn_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::unbounded_channel();
        let prev = self
            .conns
            .write()
            .insert((user_id, host_id), ActiveWakeConn { conn_id, tx });
        if let Some(old) = prev {
            let _ = old.tx.send(WakeControl::Superseded);
        }
        (conn_id, rx)
    }

    fn unregister(&self, user_id: UserId, host_id: &str, conn_id: u64) {
        let mut guard = self.conns.write();
        if let Some(active) = guard.get(&(user_id, host_id.to_string()))
            && active.conn_id == conn_id
        {
            guard.remove(&(user_id, host_id.to_string()));
        }
    }

    /// Broadcasts a strictly content-free [`ContentFreeWakeFrame`] to all live wake sockets
    /// matching `host_id` (carrying ZERO message bodies, secrets, actor names, or URLs).
    pub fn push_wake(&self, host_id: &str, kind: WakeFrameKind) {
        let seq = self.next_sequence();
        let Ok(frame) = ContentFreeWakeFrame::new(host_id, kind, seq, now_epoch_u64()) else {
            return;
        };
        let Ok(encoded) = frame.encode_bounded_json() else {
            return;
        };
        let guard = self.conns.read();
        for ((_, conn_host), conn) in guard.iter() {
            if host_id == "*" || conn_host == host_id {
                let _ = conn.tx.send(WakeControl::Frame(encoded.clone()));
            }
        }
    }

    /// Alias for [`Self::push_wake`] allowing downstream modules to push content-free wake frames.
    pub fn notify_host(&self, host_id: &str, kind: WakeFrameKind) {
        self.push_wake(host_id, kind);
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct WakeQuery {
    #[serde(default)]
    pub host_id: Option<String>,
}

const WAKE_TRANSPORT_HARD_CAP_BYTES: usize = 2 * 1024 * 1024;

pub async fn orca_wake_ws(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<WakeQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let host_id = query
        .host_id
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            headers
                .get("x-orca-host-id")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| state.orca_ide.config().host_id.clone());

    ws.max_frame_size(WAKE_TRANSPORT_HARD_CAP_BYTES)
        .max_message_size(WAKE_TRANSPORT_HARD_CAP_BYTES)
        .on_upgrade(move |socket| handle_wake_socket(socket, state, auth.user_id, host_id))
}

async fn send_close_and_flush(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
    let _ = SinkExt::flush(socket).await;
    let _ = SinkExt::close(socket).await;
}

async fn handle_wake_socket(
    mut socket: WebSocket,
    state: Arc<AppState>,
    user_id: UserId,
    host_id: String,
) {
    let (conn_id, mut ctrl_rx) = state.orca_wake_hub.register(user_id, host_id.clone());
    let mut shutdown_rx = state.gateway_hub.subscribe_shutdown();

    if *shutdown_rx.borrow() {
        let _ = socket.send(Message::Close(None)).await;
        let _ = SinkExt::close(&mut socket).await;
        state.orca_wake_hub.unregister(user_id, &host_id, conn_id);
        return;
    }

    loop {
        tokio::select! {
            biased;
            res = shutdown_rx.changed() => {
                if res.is_err() || *shutdown_rx.borrow() {
                    let _ = socket.send(Message::Close(None)).await;
                    let _ = SinkExt::close(&mut socket).await;
                    break;
                }
            }
            ctrl = ctrl_rx.recv() => {
                match ctrl {
                    Some(WakeControl::Superseded) => {
                        send_close_and_flush(&mut socket, WS_CLOSE_SUPERSEDED, "superseded").await;
                        break;
                    }
                    Some(WakeControl::Frame(text)) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        if text.len() > CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES {
                            send_close_and_flush(&mut socket, WS_CLOSE_MESSAGE_TOO_BIG, "message too big").await;
                            break;
                        }
                        if let Ok(frame) = ContentFreeWakeFrame::decode_bounded_json(&text)
                            && frame.kind == WakeFrameKind::Ping
                        {
                            let seq = state.orca_wake_hub.next_sequence();
                            if let Ok(encoded) = ContentFreeWakeFrame::new(
                                &host_id,
                                WakeFrameKind::Ping,
                                seq,
                                now_epoch_u64(),
                            )
                            .and_then(|f| f.encode_bounded_json())
                                && socket.send(Message::Text(encoded.into())).await.is_err()
                            {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(bin))) => {
                        if bin.len() > CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES {
                            send_close_and_flush(&mut socket, WS_CLOSE_MESSAGE_TOO_BIG, "message too big").await;
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if payload.len() > CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES {
                            send_close_and_flush(&mut socket, WS_CLOSE_MESSAGE_TOO_BIG, "message too big").await;
                            break;
                        }
                        let _ = socket.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => {
                        send_close_and_flush(&mut socket, WS_CLOSE_MESSAGE_TOO_BIG, "message too big").await;
                        break;
                    }
                }
            }
        }
    }

    state.orca_wake_hub.unregister(user_id, &host_id, conn_id);
}

// ---------------------------------------------------------------------------
// 2. Package Export / Import (v1 Deflate Zip + 5-Class Secret Scanner)
// ---------------------------------------------------------------------------

pub async fn package_export(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaPackageExportRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let include_mem_default = req.include_memory.unwrap_or(true);
    let opts = ExportPackageOptions {
        include_memory_index: req.include_memory_index.unwrap_or(include_mem_default),
        include_memory_topics: req.include_memory_topics.unwrap_or(include_mem_default),
        include_skills: req.include_skills.unwrap_or(true),
        include_avatar_png: req.include_avatar_png.unwrap_or(true),
        connectors_hint: req.connectors_hint,
    };
    match state.orca_bots.export_package(&bot_id, &opts).await {
        Ok(exported) => Json(json!({
            "manifest": exported.validated.manifest,
            "zip_size_bytes": exported.zip_bytes.len(),
            "zip_b64": BASE64_STD.encode(&exported.zip_bytes),
            "warnings": exported.warnings,
            "runnable_warnings": exported.runnable_warnings,
        }))
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct PackageImportQuery {
    #[serde(default, alias = "name")]
    pub custom_name: Option<String>,
}

pub async fn package_import(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<PackageImportQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Package import body cannot be empty",
        );
    }
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let looks_like_raw_zip = content_type.contains("application/zip")
        || content_type.contains("application/octet-stream")
        || body.starts_with(b"PK\x03\x04")
        || body.starts_with(b"PK\x05\x06");

    let (zip_bytes, custom_name) = if looks_like_raw_zip {
        (body.to_vec(), query.custom_name)
    } else {
        let req: OrcaPackageImportRequest = match parse_json_body(&body) {
            Ok(r) => r,
            Err(resp) => return resp,
        };
        let decoded = match BASE64_STD.decode(req.zip_b64.trim()) {
            Ok(b) => b,
            Err(err) => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Invalid base64 in zip_b64: {err}"),
                );
            }
        };
        (decoded, req.custom_name.or(query.custom_name))
    };

    match state
        .orca_bots
        .import_package(&zip_bytes, custom_name.as_deref())
        .await
    {
        Ok(imported) => (
            StatusCode::CREATED,
            Json(json!({
                "bot": imported.bot,
                "manifest": imported.validated.manifest,
                "warnings": imported.warnings,
                "restored_files": imported.restored_files,
            })),
        )
            .into_response(),
        Err(err) => map_bots_err(err),
    }
}

// ---------------------------------------------------------------------------
// 3. E2EE Team Share (Identity, Seal Offer/Held Message, Open Strict)
// ---------------------------------------------------------------------------

pub async fn share_identity(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    Json(state.orca_bots.team_share().public_identity()).into_response()
}

pub async fn share_seal(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaShareSealRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let team_share = state.orca_bots.team_share();
    let sender_identity = team_share.public_identity();
    let recipient = req
        .recipient_identity
        .unwrap_or_else(|| sender_identity.clone());
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let mode = req.mode.as_deref().unwrap_or("offer");

    match mode {
        "offer" => {
            let payload: TeamShareOfferPayload = match req.payload {
                Some(val) => match serde_json::from_value(val) {
                    Ok(p) => p,
                    Err(e) => {
                        return orca_err(
                            StatusCode::BAD_REQUEST,
                            "invalid_argument",
                            format!("Invalid TeamShareOfferPayload: {e}"),
                        );
                    }
                },
                None => TeamShareOfferPayload {
                    share_id: req
                        .share_id
                        .unwrap_or_else(|| format!("sh_{}", Uuid::new_v4().simple())),
                    bot_id: bot.id.clone(),
                    bot_name: bot.name.as_str().to_string(),
                    relay_url: req
                        .relay_url
                        .unwrap_or_else(|| state.orca_bots.config().webhook_base_url.clone()),
                    pinned_host_ed25519_b64: sender_identity.ed25519_public_b64.clone(),
                    pinned_chat_session_id: bot.chat_session_id.clone(),
                    sender_name: req.sender_name,
                },
            };
            let offer_id = req
                .offer_id
                .unwrap_or_else(|| format!("off_{}", Uuid::new_v4().simple()));
            let expires_at_ms = req.expires_at_ms.unwrap_or(now_ms.saturating_add(86_400_000));
            match team_share.create_offer(&recipient, &payload, offer_id, expires_at_ms) {
                Ok(sealed) => {
                    state
                        .orca_wake_hub
                        .push_wake(&recipient.host_id, WakeFrameKind::TeamShare);
                    Json(json!({
                        "mode": "offer",
                        "offer": sealed,
                        "sender_identity": sender_identity,
                    }))
                    .into_response()
                }
                Err(err) => map_bots_err(err),
            }
        }
        "held_message" => {
            let payload: TeamShareHeldMessagePayload = match req.payload {
                Some(val) => match serde_json::from_value(val) {
                    Ok(p) => p,
                    Err(e) => {
                        return orca_err(
                            StatusCode::BAD_REQUEST,
                            "invalid_argument",
                            format!("Invalid TeamShareHeldMessagePayload: {e}"),
                        );
                    }
                },
                None => TeamShareHeldMessagePayload {
                    share_id: req
                        .share_id
                        .unwrap_or_else(|| format!("sh_{}", Uuid::new_v4().simple())),
                    bot_id: bot.id.clone(),
                    client_message_id: req
                        .client_message_id
                        .unwrap_or_else(|| format!("cmsg_{}", Uuid::new_v4().simple())),
                    sender_name: req.sender_name.unwrap_or_else(|| "Owner".to_string()),
                    text: req.text.unwrap_or_default(),
                    sent_at_ms: now_ms,
                },
            };
            match team_share.seal_held_message(&recipient, &payload) {
                Ok(sealed) => {
                    state
                        .orca_wake_hub
                        .push_wake(&recipient.host_id, WakeFrameKind::TeamShare);
                    Json(json!({
                        "mode": "held_message",
                        "held_message": sealed,
                        "sender_identity": sender_identity,
                    }))
                    .into_response()
                }
                Err(err) => map_bots_err(err),
            }
        }
        other => orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("Unsupported share/seal mode '{other}' (expected 'offer' or 'held_message')"),
        ),
    }
}

pub async fn share_open(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaShareOpenRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let team_share = state.orca_bots.team_share();
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let mode = req.mode.as_deref().unwrap_or_else(|| {
        if req.held_message.is_some() {
            "held_message"
        } else {
            "offer"
        }
    });

    match mode {
        "offer" => {
            let Some(offer) = req.offer else {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "Missing 'offer' envelope for share/open",
                );
            };
            let sender_identity = req.sender_identity.unwrap_or_else(|| TeamSharePublicIdentity {
                host_id: offer.sender_host_id.clone(),
                x25519_public_b64: offer.ephemeral_x25519_public_b64.clone(),
                ed25519_public_b64: offer.sender_ed25519_public_b64.clone(),
            });
            match team_share.accept_incoming_offer(
                req.expected_sender_ed25519_b64.as_deref(),
                sender_identity,
                &offer,
                now_ms,
            ) {
                Ok(payload) => Json(json!({ "mode": "offer", "payload": payload })).into_response(),
                Err(err) => map_bots_err(err),
            }
        }
        "held_message" => {
            let Some(held) = req.held_message else {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "Missing 'held_message' envelope for share/open",
                );
            };
            let expected_sender = req
                .expected_sender_ed25519_b64
                .as_deref()
                .unwrap_or(&held.sender_ed25519_public_b64);
            let expected_share_id = req
                .expected_share_id
                .as_deref()
                .unwrap_or(&held.share_id);
            let expected_bot_id = req.expected_bot_id.as_deref().unwrap_or(&held.bot_id);
            match team_share.open_held_message_strict(
                expected_sender,
                expected_share_id,
                expected_bot_id,
                &held,
            ) {
                Ok(payload) => {
                    Json(json!({ "mode": "held_message", "payload": payload })).into_response()
                }
                Err(err) => map_bots_err(err),
            }
        }
        other => orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("Unsupported share/open mode '{other}' (expected 'offer' or 'held_message')"),
        ),
    }
}

// ---------------------------------------------------------------------------
// 4. Cross-App Import Discovery & Candidate Provisioning
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ImportDiscoverQuery {
    #[serde(default)]
    pub now_ms: Option<i64>,
}

pub async fn import_discover_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ImportDiscoverQuery>,
) -> Response {
    let now_ms = query.now_ms.unwrap_or_else(now_epoch_ms);
    match state.orca_bots.discover_imports(now_ms) {
        Ok(candidates) => Json(json!({
            "candidates": candidates,
            "export_prompt": BotImportEngine::export_prompt(),
        }))
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn import_discover_post(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaImportDiscoverRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);

    if let Some(candidate) = req.import_candidate {
        return match state
            .orca_bots
            .import_candidate(
                &candidate,
                req.custom_name.as_deref(),
                req.raw_export_reply.as_deref(),
            )
            .await
        {
            Ok(bot) => (StatusCode::CREATED, Json(json!({ "bot": bot, "imported": true })))
                .into_response(),
            Err(err) => map_bots_err(err),
        };
    }

    if let Some(export_text) = req.export_text {
        return match BotImportEngine::parse_generic_export(&export_text, now_ms) {
            Ok(candidates) => Json(json!({
                "candidates": candidates,
                "export_prompt": BotImportEngine::export_prompt(),
            }))
            .into_response(),
            Err(err) => map_bots_err(err),
        };
    }

    if let Some(roots_dto) = req.roots {
        let roots = BotImportDiscoveryRoots {
            openclaw_dirs: roots_dto.openclaw_dirs.into_iter().map(PathBuf::from).collect(),
            hermes_dirs: roots_dto.hermes_dirs.into_iter().map(PathBuf::from).collect(),
            openbot_dirs: roots_dto.openbot_dirs.into_iter().map(PathBuf::from).collect(),
        };
        let engine = BotImportEngine::new(roots);
        return match engine.discover_all(now_ms) {
            Ok(candidates) => Json(json!({
                "candidates": candidates,
                "export_prompt": BotImportEngine::export_prompt(),
            }))
            .into_response(),
            Err(err) => map_bots_err(err),
        };
    }

    match state.orca_bots.discover_imports(now_ms) {
        Ok(candidates) => Json(json!({
            "candidates": candidates,
            "export_prompt": BotImportEngine::export_prompt(),
        }))
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

// ---------------------------------------------------------------------------
// 5. Loopback Tool Relay Control (`127.0.0.1:0`)
// ---------------------------------------------------------------------------

pub async fn relay_start(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state.orca_bots.start_tool_relay().await {
        Ok(relay) => Json(OrcaRelayStartResponse {
            running: true,
            addr: relay.addr().to_string(),
            base_url: relay.base_url(),
        })
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn relay_status(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state.orca_bots.tool_relay().await {
        Some(relay) => Json(OrcaRelayStatusResponse {
            running: true,
            addr: Some(relay.addr().to_string()),
            base_url: Some(relay.base_url()),
            mcp_session_count: relay.mcp_session_count(),
        })
        .into_response(),
        None => Json(OrcaRelayStatusResponse {
            running: false,
            addr: None,
            base_url: None,
            mcp_session_count: 0,
        })
        .into_response(),
    }
}

pub async fn relay_register_tool(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaRelayRegisterToolRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let relay = match state.orca_bots.start_tool_relay().await {
        Ok(r) => r,
        Err(err) => return map_bots_err(err),
    };

    if let Some(seed) = req.oauth_seed {
        let cfg = OAuthProviderConfig {
            issuer: seed.issuer,
            authorization_endpoint: seed.authorization_endpoint,
            token_endpoint: seed.token_endpoint,
            revocation_endpoint: seed.revocation_endpoint,
            registration_endpoint: None,
            client_id: seed.client_id,
            client_secret: seed.client_secret,
            redirect_uri: seed.redirect_uri,
            resource: seed.resource.clone(),
            scopes: seed.scopes,
        };
        let token_rec = OAuthTokenRecord {
            credential_id: req.credential_id.clone(),
            access_token: seed.access_token,
            refresh_token: seed.refresh_token,
            token_type: "Bearer".to_string(),
            expires_at_ms: seed
                .expires_at_ms
                .unwrap_or_else(|| now_epoch_ms().saturating_add(3_600_000)),
            scope: None,
            resource: seed.resource,
        };
        if let Err(err) = state
            .orca_bots
            .tool_oauth()
            .store_token_for_credential(cfg, token_rec)
        {
            return map_bots_err(err);
        }
    }

    let auth = match req.auth {
        OrcaRelayToolAuthDto::None => RelayToolAuth::None,
        OrcaRelayToolAuthDto::StaticHeaders { headers } => RelayToolAuth::StaticHeaders(headers),
        OrcaRelayToolAuthDto::ComposioHeaders { headers } => {
            RelayToolAuth::ComposioHeaders(headers)
        }
        OrcaRelayToolAuthDto::OAuth { credential_id } => RelayToolAuth::OAuth { credential_id },
    };

    let tool_handle = req.tool_handle.clone();
    let credential_id = req.credential_id.clone();
    if let Err(err) = relay.register_tool(RelayToolRegistration {
        credential_id: req.credential_id,
        tool_handle: req.tool_handle,
        upstream_url: req.upstream_url,
        allowed_folders: req.allowed_folders,
        auth,
    }) {
        return map_bots_err(err);
    }

    let bot_home_stub = match relay.build_bot_home_tool_stub(&tool_handle, &credential_id) {
        Ok(s) => s,
        Err(err) => return map_bots_err(err),
    };
    (
        StatusCode::CREATED,
        Json(json!({
            "registered": true,
            "tool_handle": tool_handle,
            "credential_id": credential_id,
            "relay_url": format!("{}/tools/{}", relay.base_url(), tool_handle),
            "base_url": relay.base_url(),
            "bot_home_stub": bot_home_stub,
        })),
    )
        .into_response()
}

pub async fn relay_register_grant(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaRelayRegisterGrantRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let relay = match state.orca_bots.start_tool_relay().await {
        Ok(r) => r,
        Err(err) => return map_bots_err(err),
    };
    let folder = match req.folder {
        Some(f) if !f.trim().is_empty() => f,
        _ => match state.orca_bots.controller().get_bot(&req.bot_id).await {
            Ok(bot) => bot.folder.as_str().to_string(),
            Err(_) => String::new(),
        },
    };
    let owner_id = req
        .owner_id
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("user_{}", auth.user_id.0));
    let bot_id = req.bot_id.clone();
    if let Err(err) = relay.register_bot_grant(BotRelayGrant {
        bot_id: req.bot_id,
        owner_id,
        token: req.token,
        folder,
        allowed_folders: req.allowed_folders,
        allowed_tools: req.allowed_tools,
    }) {
        return map_bots_err(err);
    }
    (
        StatusCode::CREATED,
        Json(json!({
            "granted": true,
            "bot_id": bot_id,
            "base_url": relay.base_url(),
            "tool_url_prefix": format!("{}/tools", relay.base_url()),
        })),
    )
        .into_response()
}

pub async fn relay_revoke(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaRelayRevokeRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if req.bot_id.is_none() && req.credential_id.is_none() {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Provide at least one of 'bot_id' or 'credential_id' to revoke",
        );
    }
    let Some(relay) = state.orca_bots.tool_relay().await else {
        return Json(json!({
            "revoked": false,
            "bot_revoked": false,
            "credential_ended": false,
        }))
        .into_response();
    };
    let bot_revoked = match req.bot_id.as_deref() {
        Some(bid) => match relay.revoke_bot(bid) {
            Ok(v) => v,
            Err(err) => return map_bots_err(err),
        },
        None => false,
    };
    let credential_ended = match req.credential_id.as_deref() {
        Some(cid) => match relay.end_credential(cid) {
            Ok(v) => v,
            Err(err) => return map_bots_err(err),
        },
        None => false,
    };
    Json(json!({
        "revoked": bot_revoked || credential_ended,
        "bot_revoked": bot_revoked,
        "credential_ended": credential_ended,
    }))
    .into_response()
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/orca/wake", get(orca_wake_ws))
        .route("/api/v1/orca/bots/{bot_id}/package/export", post(package_export))
        .route("/api/v1/orca/bots/package/import", post(package_import))
        .route("/api/v1/orca/bots/share/identity", get(share_identity))
        .route("/api/v1/orca/bots/{bot_id}/share/seal", post(share_seal))
        .route("/api/v1/orca/bots/share/open", post(share_open))
        .route(
            "/api/v1/orca/bots/import/discover",
            get(import_discover_get).post(import_discover_post),
        )
        .route("/api/v1/orca/relay/start", post(relay_start))
        .route("/api/v1/orca/relay/status", get(relay_status))
        .route("/api/v1/orca/relay/tools", post(relay_register_tool))
        .route("/api/v1/orca/relay/grants", post(relay_register_grant))
        .route("/api/v1/orca/relay/revoke", post(relay_revoke))
}
