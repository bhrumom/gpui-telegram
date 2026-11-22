use crate::db::mcp_oauth_store::{random_hex, sha256_hex};
use crate::db::remote_assist_store::{
    is_valid_custom_assist_code, AssistVerifyOutcome, RemoteAssistDeviceRecord, RemoteSessionRecord,
};
use crate::routes::err_resp;
use crate::server::{AppState, AuthBearer};
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use chrono::Utc;
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_proto::remote_control_contracts::{
    AssistAllowRequest, AssistCodeMode, AssistSetCodeRequest, AssistStartRequest,
    AssistStartResponse, RemoteAssistConfigDto, RemoteControlMode, RemoteControllerKind,
    RemoteSessionDisconnectRequest, RemoteSessionDto, RemoteSessionsResponse,
};
use gpui_telegram_proto::sync::events::UpdateEvent;
use gpui_telegram_proto::tailnet_contracts::TailnetRelayMode;
use gpui_telegram_proto::ws::WsServerFrame;
use gpui_telegram_webrtc::signaling::SignalingFrame;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Debug, Clone)]
pub struct ActiveRemoteSessionEntry {
    pub owner_user_id: u64,
    pub controller_user_id: Option<u64>,
    pub dto: RemoteSessionDto,
    pub family_id: Option<String>,
    pub cancel_tx: watch::Sender<bool>,
}

#[derive(Debug, Clone)]
pub struct RemoteSessionCreate {
    pub session_id: String,
    pub owner_user_id: u64,
    pub controller_user_id: Option<u64>,
    pub controller_kind: RemoteControllerKind,
    pub controller_name: String,
    pub controller_platform: String,
    pub controller_id: String,
    pub target_node_id: String,
    pub target_device_id: String,
    pub mode: RemoteControlMode,
    pub active_tool: Option<String>,
    pub relay_mode: TailnetRelayMode,
    pub family_id: Option<String>,
    pub temp_relay_token_hash: Option<String>,
}

#[derive(Debug, Default)]
pub struct RemoteSessionRegistry {
    sessions: RwLock<HashMap<String, ActiveRemoteSessionEntry>>,
}

impl RemoteSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_or_touch(
        &self,
        state: &AppState,
        req: RemoteSessionCreate,
    ) -> (RemoteSessionDto, watch::Receiver<bool>) {
        let now_ms = Utc::now().timestamp_millis();

        {
            let mut guard = self.sessions.write();
            if let Some(existing) = guard.get_mut(&req.session_id) {
                existing.dto.last_activity_ms = now_ms;
                if let Some(ref tool) = req.active_tool {
                    existing.dto.active_tool = Some(tool.clone());
                }
                if !req.target_node_id.is_empty() {
                    existing.dto.target_node_id = req.target_node_id.clone();
                }
                let rx = existing.cancel_tx.subscribe();
                let dto_clone = existing.dto.clone();
                let owner_uid = existing.owner_user_id;
                drop(guard);
                let _ = state.db.update_remote_session_activity(
                    &req.session_id,
                    dto_clone.active_tool.as_deref(),
                    now_ms,
                );
                broadcast_control_activity(state, owner_uid, &dto_clone, true);
                return (dto_clone, rx);
            }
        }

        let dto = RemoteSessionDto {
            session_id: req.session_id.clone(),
            controller_kind: req.controller_kind.clone(),
            controller_name: req.controller_name.clone(),
            controller_platform: req.controller_platform.clone(),
            controller_id: req.controller_id.clone(),
            target_node_id: req.target_node_id.clone(),
            target_device_id: req.target_device_id.clone(),
            mode: req.mode.clone(),
            active_tool: req.active_tool.clone(),
            relay_mode: req.relay_mode,
            rtt_ms: None,
            fps: None,
            started_at_ms: now_ms,
            last_activity_ms: now_ms,
        };

        let db_rec = RemoteSessionRecord {
            session_id: req.session_id.clone(),
            owner_user_id: req.owner_user_id,
            controller_user_id: req.controller_user_id,
            controller_kind: controller_kind_to_str(&req.controller_kind).to_string(),
            controller_name: req.controller_name,
            controller_platform: req.controller_platform,
            controller_id: req.controller_id,
            target_node_id: req.target_node_id,
            target_device_id: req.target_device_id,
            mode: control_mode_to_str(&req.mode).to_string(),
            active_tool: req.active_tool,
            relay_mode: "direct".to_string(),
            rtt_ms: None,
            fps: None,
            temp_relay_token_hash: req.temp_relay_token_hash,
            started_at_ms: now_ms,
            last_activity_ms: now_ms,
            ended_at_ms: None,
            end_reason: None,
        };
        let _ = state.db.insert_remote_session(&db_rec);

        let (cancel_tx, cancel_rx) = watch::channel(false);
        {
            let mut guard = self.sessions.write();
            guard.insert(
                req.session_id,
                ActiveRemoteSessionEntry {
                    owner_user_id: req.owner_user_id,
                    controller_user_id: req.controller_user_id,
                    dto: dto.clone(),
                    family_id: req.family_id,
                    cancel_tx,
                },
            );
        }
        broadcast_control_activity(state, req.owner_user_id, &dto, true);
        (dto, cancel_rx)
    }

    pub fn update_active_tool(
        &self,
        state: &AppState,
        session_id: &str,
        active_tool: Option<&str>,
    ) {
        let now_ms = Utc::now().timestamp_millis();
        let dto_opt = {
            let mut guard = self.sessions.write();
            if let Some(entry) = guard.get_mut(session_id) {
                entry.dto.active_tool = active_tool.map(ToString::to_string);
                entry.dto.last_activity_ms = now_ms;
                Some((entry.owner_user_id, entry.dto.clone()))
            } else {
                None
            }
        };
        if let Some((owner_uid, dto)) = dto_opt {
            let _ = state
                .db
                .update_remote_session_activity(session_id, active_tool, now_ms);
            broadcast_control_activity(state, owner_uid, &dto, true);
        }
    }

    pub fn subscribe_cancel(&self, session_id: &str) -> Option<watch::Receiver<bool>> {
        self.sessions
            .read()
            .get(session_id)
            .map(|e| e.cancel_tx.subscribe())
    }

    pub fn list_active(&self, state: &AppState, owner_user_id: u64) -> Vec<RemoteSessionDto> {
        let mem: Vec<RemoteSessionDto> = self
            .sessions
            .read()
            .values()
            .filter(|e| e.owner_user_id == owner_user_id && !*e.cancel_tx.borrow())
            .map(|e| e.dto.clone())
            .collect();

        let db_list = state
            .db
            .list_active_remote_sessions(owner_user_id)
            .unwrap_or_default();

        let mut by_id: HashMap<String, RemoteSessionDto> = HashMap::new();
        for rec in db_list {
            by_id.insert(rec.session_id.clone(), db_session_to_dto(&rec));
        }
        for dto in mem {
            by_id.insert(dto.session_id.clone(), dto);
        }
        let mut out: Vec<RemoteSessionDto> = by_id.into_values().collect();
        out.sort_by(|a, b| b.started_at_ms.cmp(&a.started_at_ms));
        out
    }

    pub fn disconnect_session(
        &self,
        state: &AppState,
        owner_user_id: u64,
        session_id: &str,
        reason: &str,
    ) -> bool {
        let now_ms = Utc::now().timestamp_millis();
        let removed = {
            let mut guard = self.sessions.write();
            if guard
                .get(session_id)
                .is_some_and(|e| e.owner_user_id == owner_user_id)
            {
                guard.remove(session_id)
            } else {
                None
            }
        };
        let db_ended = state
            .db
            .end_remote_session(owner_user_id, session_id, reason, now_ms)
            .unwrap_or(false);

        state.mcp_sessions.terminate_session(session_id);

        if let Some(entry) = removed {
            let _ = entry.cancel_tx.send(true);
            broadcast_control_activity(state, entry.owner_user_id, &entry.dto, false);
            true
        } else {
            db_ended
        }
    }

    pub fn disconnect_all(&self, state: &AppState, owner_user_id: u64, reason: &str) -> usize {
        let now_ms = Utc::now().timestamp_millis();
        let removed: Vec<ActiveRemoteSessionEntry> = {
            let mut guard = self.sessions.write();
            let ids: Vec<String> = guard
                .iter()
                .filter(|(_, e)| e.owner_user_id == owner_user_id)
                .map(|(k, _)| k.clone())
                .collect();
            ids.into_iter().filter_map(|id| guard.remove(&id)).collect()
        };

        let db_ids = state
            .db
            .end_all_remote_sessions(owner_user_id, reason, now_ms)
            .unwrap_or_default();

        state.mcp_sessions.terminate_all_for_user(owner_user_id);
        let _ = state
            .computers
            .abort_user_pending_commands(owner_user_id, reason);

        let count = removed.len().max(db_ids.len());
        for entry in removed {
            let _ = entry.cancel_tx.send(true);
            state.mcp_sessions.terminate_session(&entry.dto.session_id);
            broadcast_control_activity(state, entry.owner_user_id, &entry.dto, false);
        }
        count
    }

    pub fn disconnect_by_family_id(
        &self,
        state: &AppState,
        owner_user_id: u64,
        family_id: &str,
        reason: &str,
    ) -> usize {
        let now_ms = Utc::now().timestamp_millis();
        let removed: Vec<ActiveRemoteSessionEntry> = {
            let mut guard = self.sessions.write();
            let ids: Vec<String> = guard
                .iter()
                .filter(|(_, e)| {
                    e.owner_user_id == owner_user_id
                        && e.family_id.as_deref() == Some(family_id)
                })
                .map(|(k, _)| k.clone())
                .collect();
            ids.into_iter().filter_map(|id| guard.remove(&id)).collect()
        };

        state.mcp_sessions.terminate_by_family_id(family_id);

        let mut count = 0usize;
        for entry in removed {
            count += 1;
            let _ = entry.cancel_tx.send(true);
            let _ = state.db.end_remote_session(
                owner_user_id,
                &entry.dto.session_id,
                reason,
                now_ms,
            );
            broadcast_control_activity(state, entry.owner_user_id, &entry.dto, false);
        }
        count
    }
}

fn broadcast_control_activity(
    state: &AppState,
    owner_user_id: u64,
    dto: &RemoteSessionDto,
    active: bool,
) {
    let label = match dto.controller_kind {
        RemoteControllerKind::AiMcp => "本机被控中 (AI MCP)",
        RemoteControllerKind::AssistGuest => "本机被控中 (远程协助)",
        _ => "本机被控中",
    };
    let status_state = if active {
        format!("{label}: {}", dto.controller_name)
    } else {
        "remote_control_ended".to_string()
    };
    let detail_json = json!({
        "session_id": dto.session_id,
        "controller_kind": controller_kind_to_str(&dto.controller_kind),
        "controller_name": dto.controller_name,
        "target_node_id": dto.target_node_id,
        "target_device_id": dto.target_device_id,
        "active": active,
        "active_tool": dto.active_tool,
        "banner": label,
    });
    let event = UpdateEvent::BotActivityChanged {
        chat_id: ChatId(0),
        bot_user_id: UserId(owner_user_id),
        state: status_state,
        detail: Some(detail_json.to_string()),
    };
    state
        .gateway_hub
        .broadcast_to_user(UserId(owner_user_id), WsServerFrame::Update(event));
}

fn controller_kind_to_str(kind: &RemoteControllerKind) -> &'static str {
    match kind {
        RemoteControllerKind::AssistGuest => "assist_guest",
        RemoteControllerKind::AiMcp => "ai_mcp",
        _ => "other",
    }
}

fn parse_controller_kind(raw: &str) -> RemoteControllerKind {
    match raw.trim().to_ascii_lowercase().as_str() {
        "assist_guest" => RemoteControllerKind::AssistGuest,
        _ => RemoteControllerKind::AiMcp,
    }
}

fn control_mode_to_str(mode: &RemoteControlMode) -> &'static str {
    match mode {
        RemoteControlMode::Desktop => "desktop",
        RemoteControlMode::Terminal => "terminal",
        _ => "desktop",
    }
}

fn parse_control_mode(raw: &str) -> RemoteControlMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "terminal" => RemoteControlMode::Terminal,
        _ => RemoteControlMode::Desktop,
    }
}

fn db_session_to_dto(rec: &RemoteSessionRecord) -> RemoteSessionDto {
    RemoteSessionDto {
        session_id: rec.session_id.clone(),
        controller_kind: parse_controller_kind(&rec.controller_kind),
        controller_name: rec.controller_name.clone(),
        controller_platform: rec.controller_platform.clone(),
        controller_id: rec.controller_id.clone(),
        target_node_id: rec.target_node_id.clone(),
        target_device_id: rec.target_device_id.clone(),
        mode: parse_control_mode(&rec.mode),
        active_tool: rec.active_tool.clone(),
        relay_mode: TailnetRelayMode::default(),
        rtt_ms: rec.rtt_ms,
        fps: rec.fps,
        started_at_ms: rec.started_at_ms,
        last_activity_ms: rec.last_activity_ms,
    }
}

fn assist_record_to_dto(rec: &RemoteAssistDeviceRecord, mask_temp: bool) -> RemoteAssistConfigDto {
    let mode = if rec.code_mode == "custom" {
        AssistCodeMode::Custom
    } else {
        AssistCodeMode::Temporary
    };
    let current_code = if mask_temp {
        None
    } else {
        rec.temp_code.clone()
    };
    RemoteAssistConfigDto {
        assist_id: rec.assist_id.clone(),
        allow_assist: rec.allow_assist,
        code_mode: mode,
        current_code,
        expires_at_ms: rec.expires_at_ms,
    }
}

fn extract_device_id(headers: &HeaderMap, query_dev: Option<&str>, body_dev: Option<&str>) -> String {
    if let Some(d) = body_dev.map(str::trim).filter(|s| !s.is_empty()) {
        return d.to_string();
    }
    if let Some(d) = query_dev.map(str::trim).filter(|s| !s.is_empty()) {
        return d.to_string();
    }
    if let Some(d) = headers
        .get("x-device-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return d.to_string();
    }
    "default".to_string()
}

#[derive(Debug, Default, Deserialize)]
pub struct AssistInfoQuery {
    pub device_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OptionalDeviceBody {
    pub device_id: Option<String>,
}

/// `GET /api/v1/remote/assist/info`
pub async fn assist_info(
    auth: AuthBearer,
    headers: HeaderMap,
    Query(q): Query<AssistInfoQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let device_id = extract_device_id(&headers, q.device_id.as_deref(), None);
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .get_or_init_assist_device(auth.user_id.0, &device_id, now_ms)
    {
        Ok(rec) => (StatusCode::OK, Json(assist_record_to_dto(&rec, false))).into_response(),
        Err(e) => err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            &e.to_string(),
        ),
    }
}

/// `POST /api/v1/remote/assist/reset-code`
pub async fn assist_reset_code(
    auth: AuthBearer,
    headers: HeaderMap,
    Query(q): Query<AssistInfoQuery>,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let parsed_dev = if body.is_empty() {
        None
    } else {
        serde_json::from_slice::<OptionalDeviceBody>(&body)
            .ok()
            .and_then(|b| b.device_id)
    };
    let device_id = extract_device_id(&headers, q.device_id.as_deref(), parsed_dev.as_deref());
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .reset_assist_temp_code(auth.user_id.0, &device_id, now_ms)
    {
        Ok(rec) => (StatusCode::OK, Json(assist_record_to_dto(&rec, false))).into_response(),
        Err(e) => err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            &e.to_string(),
        ),
    }
}

/// `POST /api/v1/remote/assist/set-code`
pub async fn assist_set_code(
    auth: AuthBearer,
    headers: HeaderMap,
    Query(q): Query<AssistInfoQuery>,
    State(state): State<Arc<AppState>>,
    Json(req): Json<AssistSetCodeRequest>,
) -> Response {
    if !is_valid_custom_assist_code(&req.code) {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_assist_code",
            "Custom assist code must be 8-16 ASCII alphanumeric characters containing both letters and digits",
        );
    }
    let device_id = extract_device_id(&headers, q.device_id.as_deref(), None);
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .set_assist_custom_code(auth.user_id.0, &device_id, &req.code, now_ms)
    {
        Ok(rec) => (StatusCode::OK, Json(assist_record_to_dto(&rec, true))).into_response(),
        Err(e) => err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_assist_code",
            &e.to_string(),
        ),
    }
}

/// `POST /api/v1/remote/assist/allow`
pub async fn assist_allow(
    auth: AuthBearer,
    headers: HeaderMap,
    Query(q): Query<AssistInfoQuery>,
    State(state): State<Arc<AppState>>,
    Json(req): Json<AssistAllowRequest>,
) -> Response {
    let device_id = extract_device_id(&headers, q.device_id.as_deref(), None);
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .set_assist_allow(auth.user_id.0, &device_id, req.allow_assist, now_ms)
    {
        Ok(rec) => {
            if !req.allow_assist {
                let sessions = state.remote_sessions.list_active(&state, auth.user_id.0);
                for s in sessions {
                    if s.controller_kind == RemoteControllerKind::AssistGuest
                        && (s.target_device_id == device_id || device_id == "default")
                    {
                        state.remote_sessions.disconnect_session(
                            &state,
                            auth.user_id.0,
                            &s.session_id,
                            "assist_disabled_by_host",
                        );
                    }
                }
            }
            (StatusCode::OK, Json(assist_record_to_dto(&rec, false))).into_response()
        }
        Err(e) => err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            &e.to_string(),
        ),
    }
}

fn resolve_optional_user(headers: &HeaderMap, state: &AppState) -> Option<u64> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?;
    let token = auth_header.strip_prefix("Bearer ").map(str::trim)?;
    if token.is_empty() {
        return None;
    }
    if let Some(sess) = state.session_manager.verify_token(token) {
        return Some(sess.user_id.0);
    }
    if let Ok(Some(tok)) = state
        .db
        .verify_mcp_access_token(token, Utc::now().timestamp_millis())
    {
        return Some(tok.user_id);
    }
    None
}

/// `POST /api/v1/remote/assist/start`
pub async fn assist_start(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(req): Json<AssistStartRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    let controller_user_id = resolve_optional_user(&headers, &state);

    let verified = match state
        .db
        .verify_assist_start(&req.assist_id, &req.access_code, now_ms)
    {
        Ok(outcome) => outcome,
        Err(e) => {
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                &e.to_string(),
            );
        }
    };

    let dev = match verified {
        AssistVerifyOutcome::Authorized(dev) => dev,
        AssistVerifyOutcome::NotFound => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "assist_not_found",
                "Remote assist device ID not found",
            );
        }
        AssistVerifyOutcome::AssistDisabled => {
            return err_resp(
                StatusCode::FORBIDDEN,
                "assist_disabled",
                "Remote assist is disabled on the target device",
            );
        }
        AssistVerifyOutcome::CodeExpired => {
            return err_resp(
                StatusCode::UNAUTHORIZED,
                "assist_code_expired",
                "Temporary remote assist code expired; ask host to refresh it",
            );
        }
        AssistVerifyOutcome::InvalidCode {
            remaining_attempts,
            locked_until_ms,
        } => {
            let status = if locked_until_ms.is_some() {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::UNAUTHORIZED
            };
            return (
                status,
                Json(json!({
                    "error": {
                        "code": if locked_until_ms.is_some() { "assist_rate_limited" } else { "invalid_assist_code" },
                        "message": "Invalid remote assist connection code",
                        "remaining_attempts": remaining_attempts,
                        "locked_until_ms": locked_until_ms,
                    }
                })),
            )
                .into_response();
        }
        AssistVerifyOutcome::RateLimited { locked_until_ms } => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": {
                        "code": "assist_rate_limited",
                        "message": "Too many failed remote assist attempts; locked temporarily",
                        "locked_until_ms": locked_until_ms,
                    }
                })),
            )
                .into_response();
        }
    };

    let nodes = state
        .db
        .list_computer_nodes(dev.user_id, None, now_ms)
        .unwrap_or_default();
    let target_node = nodes
        .iter()
        .find(|n| n.device_id.as_deref() == Some(dev.device_id.as_str()) && n.online)
        .or_else(|| {
            nodes
                .iter()
                .find(|n| n.device_id.as_deref() == Some(dev.device_id.as_str()))
        })
        .or_else(|| nodes.iter().find(|n| n.online))
        .or_else(|| nodes.first());

    let target_node_id = target_node
        .map(|n| n.node_id.clone())
        .unwrap_or_else(|| dev.device_id.clone());

    let session_id = format!("rassist_{}", random_hex(16));
    let temp_relay_token = format!("fbrelay_{}", random_hex(24));
    let temp_relay_token_hash = sha256_hex(&temp_relay_token);

    let controller_name = req
        .requester_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Remote Assist Guest")
        .to_string();
    let controller_platform = req
        .requester_platform
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("desktop")
        .to_string();
    let controller_id = req
        .requester_device_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            if let Some(u) = controller_user_id {
                format!("user:{u}")
            } else {
                "guest".to_string()
            }
        });

    let _ = state.remote_sessions.register_or_touch(
        &state,
        RemoteSessionCreate {
            session_id: session_id.clone(),
            owner_user_id: dev.user_id,
            controller_user_id,
            controller_kind: RemoteControllerKind::AssistGuest,
            controller_name,
            controller_platform,
            controller_id,
            target_node_id: target_node_id.clone(),
            target_device_id: dev.device_id.clone(),
            mode: req.mode,
            active_tool: Some("assist_connected".to_string()),
            relay_mode: TailnetRelayMode::default(),
            family_id: None,
            temp_relay_token_hash: Some(temp_relay_token_hash),
        },
    );

    let expires_at_ms = dev.expires_at_ms.unwrap_or_else(|| now_ms + 3_600_000);
    let resp = AssistStartResponse {
        session_id,
        target_node_id,
        target_device_id: dev.device_id,
        temp_relay_token,
        requires_host_confirm: false,
        expires_at_ms: Some(expires_at_ms),
    };
    (StatusCode::OK, Json(resp)).into_response()
}

/// `GET /api/v1/remote/sessions`
pub async fn remote_sessions_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let sessions = state.remote_sessions.list_active(&state, auth.user_id.0);
    (StatusCode::OK, Json(RemoteSessionsResponse { sessions })).into_response()
}

/// `POST /api/v1/remote/sessions/disconnect`
pub async fn remote_sessions_disconnect(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<RemoteSessionDisconnectRequest>,
) -> Response {
    let disconnected = state.remote_sessions.disconnect_session(
        &state,
        auth.user_id.0,
        &req.session_id,
        "disconnected_by_host",
    );
    (
        StatusCode::OK,
        Json(json!({
            "disconnected": disconnected,
            "session_id": req.session_id,
        })),
    )
        .into_response()
}

/// `POST /api/v1/remote/sessions/disconnect-all`
pub async fn remote_sessions_disconnect_all(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let count =
        state
            .remote_sessions
            .disconnect_all(&state, auth.user_id.0, "user_kill_switch");
    (
        StatusCode::OK,
        Json(json!({
            "disconnected_count": count,
            "kill_switch": true,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeviceWebRtcSignalRequest {
    pub sender_device_id: String,
    pub target_device_id: Option<String>,
    pub session_id: Option<String>,
    pub frame: SignalingFrame,
}

/// `POST /api/v1/remote/webrtc/signal`
pub async fn remote_webrtc_signal(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeviceWebRtcSignalRequest>,
) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");
    if token.is_empty() {
        return err_resp(StatusCode::UNAUTHORIZED, "unauthorized", "Missing Bearer token");
    }

    let sender_user = if let Some(sess) = state.session_manager.verify_token(token) {
        sess.user_id
    } else if token.starts_with("fbrelay_") {
        let Some(ref sid) = req.session_id else {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "missing_session_id",
                "session_id is required with assist relay token",
            );
        };
        match state.db.verify_assist_relay_token(token, sid) {
            Ok(Some(rec)) => UserId(rec.controller_user_id.unwrap_or(rec.owner_user_id)),
            _ => {
                return err_resp(
                    StatusCode::UNAUTHORIZED,
                    "invalid_relay_token",
                    "Invalid or ended assist relay session",
                );
            }
        }
    } else {
        return err_resp(StatusCode::UNAUTHORIZED, "unauthorized", "Invalid Bearer token");
    };

    match state
        .signaling_hub
        .dispatch_device_frame(
            sender_user,
            &req.sender_device_id,
            req.target_device_id.as_deref(),
            req.frame,
        )
        .await
    {
        Ok(()) => (StatusCode::OK, Json(json!({ "relayed": true }))).into_response(),
        Err(e) => crate::routes::map_telegram_err(e),
    }
}
