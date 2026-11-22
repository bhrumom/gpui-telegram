use crate::db::mcp_oauth_store::{random_hex, McpToolAuditRecord, McpVerifiedAccessToken};
use crate::db::remote_assist_store::is_valid_custom_assist_code;
use crate::routes::check_active_membership;
use crate::routes::mcp_oauth::{resolve_public_base_url, validate_target_resource};
use crate::routes::remote_assist::RemoteSessionCreate;
use crate::search::SearchQueryParams;
use crate::server::AppState;
use axum::{
    body::to_bytes,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json, Response,
    },
};
use chrono::Utc;
use gpui_telegram_agent::tasks::TaskFsm;
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_proto::computer_contracts::{RemoteComputerCommandRequest, RemoteComputerOp};
use gpui_telegram_proto::remote_control_contracts::{
    PowerActionKind, RemoteControlMode, RemoteControllerKind,
};
use gpui_telegram_proto::sync::events::UpdateEvent;
use gpui_telegram_proto::tailnet_contracts::TailnetRelayMode;
use gpui_telegram_proto::ws::WsServerFrame;
use parking_lot::RwLock;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub const SUPPORTED_MCP_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
pub const DEFAULT_MCP_VERSION: &str = "2025-11-25";

#[derive(Debug, Clone)]
pub struct McpSessionEntry {
    pub session_id: String,
    pub user_id: u64,
    pub family_id: String,
    pub client_id: String,
    pub client_name: String,
    pub protocol_version: String,
    pub created_at_ms: i64,
    pub terminated: bool,
    pub event_tx: broadcast::Sender<String>,
}

#[derive(Debug, Default)]
pub struct McpSessionRegistry {
    sessions: RwLock<HashMap<String, McpSessionEntry>>,
    user_seen_tokens: RwLock<HashMap<u64, HashSet<String>>>,
    terminated_tokens: RwLock<HashSet<String>>,
    terminated_families: RwLock<HashSet<String>>,
}

impl McpSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_token_seen(&self, user_id: u64, token_id: &str) {
        self.user_seen_tokens
            .write()
            .entry(user_id)
            .or_default()
            .insert(token_id.to_string());
    }

    pub fn create_session(
        &self,
        ctx: &McpVerifiedAccessToken,
        protocol_version: &str,
    ) -> McpSessionEntry {
        self.record_token_seen(ctx.user_id, &ctx.token_id);
        let now_ms = Utc::now().timestamp_millis();
        let session_id = format!("mcp_sess_{}", random_hex(16));
        let (event_tx, _) = broadcast::channel(64);
        let entry = McpSessionEntry {
            session_id: session_id.clone(),
            user_id: ctx.user_id,
            family_id: ctx.family_id.clone(),
            client_id: ctx.client_id.clone(),
            client_name: ctx.client_name.clone(),
            protocol_version: protocol_version.to_string(),
            created_at_ms: now_ms,
            terminated: false,
            event_tx,
        };
        self.sessions.write().insert(session_id, entry.clone());
        entry
    }

    pub fn get_session(&self, session_id: &str) -> Option<McpSessionEntry> {
        self.sessions.read().get(session_id).cloned()
    }

    pub fn is_terminated(&self, session_id: &str, token: &McpVerifiedAccessToken) -> bool {
        if self.terminated_families.read().contains(&token.family_id) {
            return true;
        }
        if !session_id.is_empty() && !session_id.starts_with("mcp_tok_") {
            if let Some(entry) = self.sessions.read().get(session_id) {
                if entry.terminated {
                    return true;
                }
            }
        }
        let tok_key = format!("mcp_tok_{}", token.token_id);
        self.terminated_tokens.read().contains(&tok_key)
    }

    pub fn terminate_session(&self, session_id: &str) {
        let mut guard = self.sessions.write();
        if let Some(entry) = guard.get_mut(session_id) {
            entry.terminated = true;
            let _ = entry.event_tx.send(
                json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/terminated",
                    "params": { "reason": "disconnected" }
                })
                .to_string(),
            );
        }
        self.terminated_tokens.write().insert(session_id.to_string());
    }

    pub fn terminate_by_family_id(&self, family_id: &str) {
        self.terminated_families
            .write()
            .insert(family_id.to_string());
        let mut guard = self.sessions.write();
        for entry in guard.values_mut() {
            if entry.family_id == family_id {
                entry.terminated = true;
            }
        }
    }

    pub fn terminate_all_for_user(&self, user_id: u64) {
        {
            let mut guard = self.sessions.write();
            for entry in guard.values_mut() {
                if entry.user_id == user_id {
                    entry.terminated = true;
                    let _ = entry.event_tx.send(
                        json!({
                            "jsonrpc": "2.0",
                            "method": "notifications/terminated",
                            "params": { "reason": "user_kill_switch" }
                        })
                        .to_string(),
                    );
                }
            }
        }
        if let Some(toks) = self.user_seen_tokens.read().get(&user_id) {
            let mut killed = self.terminated_tokens.write();
            for tid in toks {
                killed.insert(format!("mcp_tok_{tid}"));
            }
        }
    }
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn negotiate_protocol_version(requested: Option<&str>) -> String {
    if let Some(req) = requested.map(str::trim).filter(|s| !s.is_empty()) {
        if SUPPORTED_MCP_VERSIONS.contains(&req) {
            return req.to_string();
        }
    }
    DEFAULT_MCP_VERSION.to_string()
}

fn unauthorized_mcp_challenge(base_url: &str, rpc_id: Value, message: &str) -> Response {
    let www_auth = format!(
        r#"Bearer resource_metadata="{base_url}/.well-known/oauth-protected-resource", scope="devices:read devices:connect""#
    );
    let mut resp = (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "jsonrpc": "2.0",
            "id": rpc_id,
            "error": {
                "code": -32001,
                "message": message,
            }
        })),
    )
        .into_response();
    if let Ok(val) = HeaderValue::from_str(&www_auth) {
        resp.headers_mut().insert(header::WWW_AUTHENTICATE, val);
    }
    resp
}

fn insufficient_scope_challenge(
    base_url: &str,
    rpc_id: Value,
    required_scope: &str,
    message: &str,
) -> Response {
    let www_auth = format!(
        r#"Bearer error="insufficient_scope", scope="{required_scope}", resource_metadata="{base_url}/.well-known/oauth-protected-resource""#
    );
    let mut resp = (
        StatusCode::FORBIDDEN,
        Json(json!({
            "jsonrpc": "2.0",
            "id": rpc_id,
            "error": {
                "code": -32002,
                "message": message,
                "data": { "required_scope": required_scope }
            }
        })),
    )
        .into_response();
    if let Ok(val) = HeaderValue::from_str(&www_auth) {
        resp.headers_mut().insert(header::WWW_AUTHENTICATE, val);
    }
    resp
}

fn jsonrpc_ok(
    id: Value,
    result: Value,
    protocol_version: &str,
    session_id: Option<&str>,
) -> Response {
    let mut resp = (
        StatusCode::OK,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        })),
    )
        .into_response();
    if let Ok(pv) = HeaderValue::from_str(protocol_version) {
        resp.headers_mut().insert("MCP-Protocol-Version", pv);
    }
    if let Some(sid) = session_id {
        if let Ok(sv) = HeaderValue::from_str(sid) {
            resp.headers_mut().insert("Mcp-Session-Id", sv);
        }
    }
    resp
}

fn jsonrpc_err(status: StatusCode, id: Value, code: i64, message: &str) -> Response {
    (
        status,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message,
            }
        })),
    )
        .into_response()
}

fn authenticate_mcp_bearer(
    headers: &HeaderMap,
    state: &AppState,
    base_url: &str,
    rpc_id: Value,
) -> std::result::Result<McpVerifiedAccessToken, Response> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .unwrap_or("");
    let Some(token) = auth_header.strip_prefix("Bearer ").map(str::trim) else {
        return Err(unauthorized_mcp_challenge(
            base_url,
            rpc_id,
            "Missing or malformed Bearer token",
        ));
    };
    if token.is_empty() {
        return Err(unauthorized_mcp_challenge(
            base_url,
            rpc_id,
            "Empty Bearer token",
        ));
    }

    let now_ms = Utc::now().timestamp_millis();
    let verified = state
        .db
        .verify_mcp_access_token(token, now_ms)
        .map_err(|e| {
            jsonrpc_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                rpc_id.clone(),
                -32603,
                &e.to_string(),
            )
        })?;
    let Some(verified) = verified else {
        return Err(unauthorized_mcp_challenge(
            base_url,
            rpc_id,
            "Invalid, expired, or revoked MCP access token",
        ));
    };

    if !verified.resource.is_empty()
        && validate_target_resource(Some(&verified.resource), base_url).is_err()
    {
        return Err(unauthorized_mcp_challenge(
            base_url,
            rpc_id,
            "Access token resource audience does not match this MCP server",
        ));
    }

    state
        .mcp_sessions
        .record_token_seen(verified.user_id, &verified.token_id);

    Ok(verified)
}

/// `POST /mcp` — Streamable HTTP JSON-RPC 2.0 endpoint
pub async fn mcp_post(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Response {
    let base_url = resolve_public_base_url(&headers, &state);
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return jsonrpc_err(
                StatusCode::BAD_REQUEST,
                Value::Null,
                -32700,
                &format!("Parse error: {e}"),
            );
        }
    };
    let rpc_id = payload.get("id").cloned().unwrap_or(Value::Null);

    let verified = match authenticate_mcp_bearer(&headers, &state, &base_url, rpc_id.clone()) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if let Some(hdr_ver) = header_str(&headers, "MCP-Protocol-Version") {
        if !SUPPORTED_MCP_VERSIONS.contains(&hdr_ver.as_str()) {
            return jsonrpc_err(
                StatusCode::BAD_REQUEST,
                rpc_id,
                -32600,
                &format!("Unsupported MCP-Protocol-Version `{hdr_ver}`"),
            );
        }
    }

    let method = payload
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
    let session_hdr = header_str(&headers, "Mcp-Session-Id");

    if let Some(ref sid) = session_hdr {
        if let Some(entry) = state.mcp_sessions.get_session(sid) {
            if entry.user_id != verified.user_id {
                return jsonrpc_err(
                    StatusCode::FORBIDDEN,
                    rpc_id,
                    -32001,
                    "Mcp-Session-Id belongs to a different user",
                );
            }
            if entry.terminated {
                return jsonrpc_err(
                    StatusCode::GONE,
                    rpc_id,
                    -32001,
                    "MCP session disconnected by host user kill-switch",
                );
            }
        } else {
            return jsonrpc_err(
                StatusCode::NOT_FOUND,
                rpc_id,
                -32001,
                "Unknown or expired Mcp-Session-Id",
            );
        }
    }

    match method {
        "initialize" => {
            if state
                .mcp_sessions
                .terminated_families
                .read()
                .contains(&verified.family_id)
            {
                return unauthorized_mcp_challenge(
                    &base_url,
                    rpc_id,
                    "OAuth grant family has been revoked",
                );
            }
            let req_ver = params.get("protocolVersion").and_then(|v| v.as_str());
            let negotiated = negotiate_protocol_version(req_ver);
            let sess = state.mcp_sessions.create_session(&verified, &negotiated);
            let _ = state.remote_sessions.register_or_touch(
                &state,
                RemoteSessionCreate {
                    session_id: sess.session_id.clone(),
                    owner_user_id: verified.user_id,
                    controller_user_id: None,
                    controller_kind: RemoteControllerKind::AiMcp,
                    controller_name: verified.client_name.clone(),
                    controller_platform: "mcp".to_string(),
                    controller_id: verified.client_id.clone(),
                    target_node_id: verified
                        .allowed_node_ids
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "*".to_string()),
                    target_device_id: "mcp".to_string(),
                    mode: RemoteControlMode::Desktop,
                    active_tool: None,
                    relay_mode: TailnetRelayMode::default(),
                    family_id: Some(verified.family_id.clone()),
                    temp_relay_token_hash: None,
                },
            );
            jsonrpc_ok(
                rpc_id,
                json!({
                    "protocolVersion": negotiated,
                    "capabilities": {
                        "tools": { "listChanged": false },
                        "resources": { "subscribe": false, "listChanged": false }
                    },
                    "serverInfo": {
                        "name": "fabushi-official-mcp",
                        "version": env!("CARGO_PKG_VERSION"),
                        "title": "Fabushi Official MCP Server"
                    },
                    "instructions": "Fabushi Remote Control & Workspace MCP. Tools enforce OAuth 2.1 scopes and device allowlists."
                }),
                &negotiated,
                Some(&sess.session_id),
            )
        }
        "notifications/initialized" | "notifications/cancelled" => {
            (StatusCode::ACCEPTED, HeaderMap::new()).into_response()
        }
        "ping" => {
            let pv = header_str(&headers, "MCP-Protocol-Version")
                .unwrap_or_else(|| DEFAULT_MCP_VERSION.to_string());
            jsonrpc_ok(rpc_id, json!({}), &pv, session_hdr.as_deref())
        }
        "tools/list" => {
            let pv = header_str(&headers, "MCP-Protocol-Version")
                .unwrap_or_else(|| DEFAULT_MCP_VERSION.to_string());
            jsonrpc_ok(
                rpc_id,
                json!({ "tools": mcp_tool_definitions() }),
                &pv,
                session_hdr.as_deref(),
            )
        }
        "resources/list" => {
            let pv = header_str(&headers, "MCP-Protocol-Version")
                .unwrap_or_else(|| DEFAULT_MCP_VERSION.to_string());
            jsonrpc_ok(
                rpc_id,
                json!({
                    "resources": [
                        {
                            "uri": "fabushi://devices",
                            "name": "devices",
                            "title": "Authorized Remote Devices",
                            "mimeType": "application/json"
                        },
                        {
                            "uri": "fabushi://sessions",
                            "name": "sessions",
                            "title": "Active Remote Control Sessions",
                            "mimeType": "application/json"
                        },
                        {
                            "uri": "fabushi://dialogs",
                            "name": "dialogs",
                            "title": "User Chats & Dialogs",
                            "mimeType": "application/json"
                        }
                    ]
                }),
                &pv,
                session_hdr.as_deref(),
            )
        }
        "resources/read" => {
            handle_resources_read(
                &headers,
                &state,
                &base_url,
                &verified,
                rpc_id,
                &params,
                session_hdr.as_deref(),
            )
            .await
        }
        "tools/call" => {
            handle_tools_call(
                &headers,
                &state,
                &base_url,
                &verified,
                rpc_id,
                &params,
                session_hdr.as_deref(),
            )
            .await
        }
        "" => jsonrpc_err(StatusCode::BAD_REQUEST, rpc_id, -32600, "Missing JSON-RPC method"),
        other => jsonrpc_err(
            StatusCode::OK,
            rpc_id,
            -32601,
            &format!("Method not found: {other}"),
        ),
    }
}

/// `GET /mcp` — Streamable HTTP SSE notification channel
pub async fn mcp_get_sse(headers: HeaderMap, State(state): State<Arc<AppState>>) -> Response {
    let base_url = resolve_public_base_url(&headers, &state);
    let verified = match authenticate_mcp_bearer(&headers, &state, &base_url, Value::Null) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let sid = header_str(&headers, "Mcp-Session-Id");
    let rx = if let Some(ref s) = sid {
        let Some(entry) = state.mcp_sessions.get_session(s) else {
            return jsonrpc_err(
                StatusCode::NOT_FOUND,
                Value::Null,
                -32001,
                "Unknown Mcp-Session-Id",
            );
        };
        if entry.user_id != verified.user_id || entry.terminated {
            return jsonrpc_err(
                StatusCode::GONE,
                Value::Null,
                -32001,
                "MCP session unavailable or terminated",
            );
        }
        entry.event_tx.subscribe()
    } else {
        let (tx, rx) = broadcast::channel(8);
        let _ = tx.send(
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/ready",
                "params": { "server": "fabushi-official-mcp" }
            })
            .to_string(),
        );
        rx
    };

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    return Some((Ok::<Event, Infallible>(Event::default().data(msg)), rx));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(20))
                .text("ping"),
        )
        .into_response()
}

/// `DELETE /mcp` — Closes an explicit `Mcp-Session-Id`
pub async fn mcp_delete(headers: HeaderMap, State(state): State<Arc<AppState>>) -> Response {
    let base_url = resolve_public_base_url(&headers, &state);
    let verified = match authenticate_mcp_bearer(&headers, &state, &base_url, Value::Null) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let Some(sid) = header_str(&headers, "Mcp-Session-Id") else {
        return jsonrpc_err(
            StatusCode::BAD_REQUEST,
            Value::Null,
            -32602,
            "Mcp-Session-Id header is required",
        );
    };
    let Some(entry) = state.mcp_sessions.get_session(&sid) else {
        return jsonrpc_err(
            StatusCode::NOT_FOUND,
            Value::Null,
            -32001,
            "Unknown Mcp-Session-Id",
        );
    };
    if entry.user_id != verified.user_id {
        return jsonrpc_err(
            StatusCode::FORBIDDEN,
            Value::Null,
            -32001,
            "Mcp-Session-Id belongs to a different user",
        );
    }
    state
        .remote_sessions
        .disconnect_session(&state, verified.user_id, &sid, "mcp_client_delete");
    (
        StatusCode::OK,
        Json(json!({ "closed": true, "session_id": sid })),
    )
        .into_response()
}

async fn handle_resources_read(
    headers: &HeaderMap,
    state: &AppState,
    base_url: &str,
    verified: &McpVerifiedAccessToken,
    rpc_id: Value,
    params: &Value,
    session_id: Option<&str>,
) -> Response {
    let pv = header_str(headers, "MCP-Protocol-Version")
        .unwrap_or_else(|| DEFAULT_MCP_VERSION.to_string());
    let uri = params
        .get("uri")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");

    let (required_scope, payload_val) = match uri {
        "fabushi://devices" => {
            let now_ms = Utc::now().timestamp_millis();
            let nodes = state
                .db
                .list_computer_nodes(verified.user_id, None, now_ms)
                .unwrap_or_default()
                .into_iter()
                .filter(|n| is_node_allowed(verified, &n.node_id))
                .collect::<Vec<_>>();
            ("devices:read", json!({ "devices": nodes }))
        }
        "fabushi://sessions" => {
            let sessions = state.remote_sessions.list_active(state, verified.user_id);
            ("devices:read", json!({ "sessions": sessions }))
        }
        "fabushi://dialogs" => {
            let chats = state
                .db
                .list_user_chats(UserId(verified.user_id))
                .unwrap_or_default();
            ("chats:read", json!({ "dialogs": chats }))
        }
        _ => {
            return jsonrpc_err(
                StatusCode::NOT_FOUND,
                rpc_id,
                -32002,
                &format!("Resource not found: `{uri}`"),
            );
        }
    };

    if !has_scope(verified, required_scope) {
        return insufficient_scope_challenge(
            base_url,
            rpc_id,
            required_scope,
            &format!("Missing required scope `{required_scope}` for resource `{uri}`"),
        );
    }

    jsonrpc_ok(
        rpc_id,
        json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": payload_val.to_string(),
            }]
        }),
        &pv,
        session_id,
    )
}

fn has_scope(verified: &McpVerifiedAccessToken, required: &str) -> bool {
    verified.scopes.iter().any(|s| s == required)
}

fn is_node_allowed(verified: &McpVerifiedAccessToken, node_id: &str) -> bool {
    if verified.allowed_node_ids.is_empty()
        || verified.allowed_node_ids.iter().any(|allowed| allowed == "*")
    {
        return true;
    }
    verified
        .allowed_node_ids
        .iter()
        .any(|allowed| allowed == node_id)
}

fn required_scope_for_tool(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "list_devices" | "get_device_status" => Some("devices:read"),
        "request_human_takeover" => Some("devices:connect"),
        "capture_screen" | "get_accessibility_tree" => Some("screen:view"),
        "semantic_ui_action" | "mouse_action" | "keyboard_type" | "keyboard_hotkey" => {
            Some("input:control")
        }
        "terminal_list"
        | "terminal_open"
        | "terminal_exec"
        | "terminal_send"
        | "terminal_read"
        | "terminal_kill" => Some("terminal:exec"),
        "file_list" | "file_read" | "clipboard_get" => Some("files:read"),
        "file_write" | "clipboard_set" => Some("files:write"),
        "assist_get_info" | "assist_manage" => Some("assist:manage"),
        "device_power_action" => Some("power:control"),
        "list_dialogs" | "list_bots" => Some("chats:read"),
        "get_chat_history" | "search_messages" => Some("messaging:read"),
        "send_message" => Some("messaging:write"),
        "list_tasks" | "create_task" => Some("tasks:manage"),
        "miniapp_list" => Some("miniapp:control"),
        _ => None,
    }
}

fn summarize_tool_args(args: &Value) -> String {
    let mut clone = args.clone();
    if let Some(obj) = clone.as_object_mut() {
        for sensitive in ["code", "text", "content", "data", "password"] {
            if let Some(v) = obj.get_mut(sensitive) {
                if let Some(s) = v.as_str() {
                    *v = Value::String(format!("<{} chars>", s.chars().count()));
                }
            }
        }
    }
    let s = clone.to_string();
    if s.len() > 280 {
        format!("{}...", s.chars().take(277).collect::<String>())
    } else {
        s
    }
}

fn record_audit(
    state: &AppState,
    verified: &McpVerifiedAccessToken,
    session_id: &str,
    tool_name: &str,
    target_node_id: Option<&str>,
    args: &Value,
    ok: bool,
    error_code: Option<&str>,
    started: Instant,
) {
    let rec = McpToolAuditRecord {
        audit_id: format!("aud_{}", random_hex(12)),
        user_id: verified.user_id,
        client_id: verified.client_id.clone(),
        family_id: verified.family_id.clone(),
        session_id: session_id.to_string(),
        tool_name: tool_name.to_string(),
        target_node_id: target_node_id.map(ToString::to_string),
        args_summary: summarize_tool_args(args),
        ok,
        error_code: error_code.map(ToString::to_string),
        duration_ms: started.elapsed().as_millis() as i64,
        created_at_ms: Utc::now().timestamp_millis(),
    };
    let _ = state.db.record_mcp_tool_audit(&rec);
}

async fn handle_tools_call(
    headers: &HeaderMap,
    state: &Arc<AppState>,
    base_url: &str,
    verified: &McpVerifiedAccessToken,
    rpc_id: Value,
    params: &Value,
    session_hdr: Option<&str>,
) -> Response {
    let started = Instant::now();
    let pv = header_str(headers, "MCP-Protocol-Version")
        .unwrap_or_else(|| DEFAULT_MCP_VERSION.to_string());

    let tool_name = params
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let Some(req_scope) = required_scope_for_tool(tool_name) else {
        return jsonrpc_err(
            StatusCode::NOT_FOUND,
            rpc_id,
            -32601,
            &format!("Unknown MCP tool `{tool_name}`"),
        );
    };

    let target_node_opt = args
        .get("node_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let effective_session_id = session_hdr
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("mcp_tok_{}", verified.token_id));

    if !has_scope(verified, req_scope) {
        record_audit(
            state,
            verified,
            &effective_session_id,
            tool_name,
            target_node_opt.as_deref(),
            &args,
            false,
            Some("insufficient_scope"),
            started,
        );
        return insufficient_scope_challenge(
            base_url,
            rpc_id,
            req_scope,
            &format!("Tool `{tool_name}` requires OAuth scope `{req_scope}`"),
        );
    }

    if state
        .mcp_sessions
        .is_terminated(&effective_session_id, verified)
    {
        record_audit(
            state,
            verified,
            &effective_session_id,
            tool_name,
            target_node_opt.as_deref(),
            &args,
            false,
            Some("session_killed"),
            started,
        );
        return jsonrpc_err(
            StatusCode::GONE,
            rpc_id,
            -32001,
            "Remote control session was terminated by host user kill-switch",
        );
    }

    if let Some(ref nid) = target_node_opt {
        if !is_node_allowed(verified, nid) {
            record_audit(
                state,
                verified,
                &effective_session_id,
                tool_name,
                Some(nid),
                &args,
                false,
                Some("device_not_allowed"),
                started,
            );
            return jsonrpc_err(
                StatusCode::FORBIDDEN,
                rpc_id,
                -32003,
                &format!("Device `{nid}` is not in the authorized device allowlist"),
            );
        }
    }

    let (_, mut cancel_rx) = state.remote_sessions.register_or_touch(
        state,
        RemoteSessionCreate {
            session_id: effective_session_id.clone(),
            owner_user_id: verified.user_id,
            controller_user_id: None,
            controller_kind: RemoteControllerKind::AiMcp,
            controller_name: verified.client_name.clone(),
            controller_platform: "mcp".to_string(),
            controller_id: verified.client_id.clone(),
            target_node_id: target_node_opt.clone().unwrap_or_else(|| {
                verified
                    .allowed_node_ids
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "*".to_string())
            }),
            target_device_id: "mcp".to_string(),
            mode: RemoteControlMode::Desktop,
            active_tool: Some(tool_name.to_string()),
            relay_mode: TailnetRelayMode::default(),
            family_id: Some(verified.family_id.clone()),
            temp_relay_token_hash: None,
        },
    );

    if *cancel_rx.borrow() {
        return jsonrpc_err(
            StatusCode::GONE,
            rpc_id,
            -32001,
            "Remote control session was terminated by host user kill-switch",
        );
    }

    let exec_res = tokio::select! {
        biased;
        changed = cancel_rx.changed() => {
            let _ = changed;
            Err((
                StatusCode::GONE,
                -32001,
                "Tool execution aborted immediately by host user disconnect-all".to_string(),
                "kill_switch_aborted",
            ))
        }
        res = execute_mcp_tool(state, verified, &effective_session_id, tool_name, &args) => res,
    };

    state
        .remote_sessions
        .update_active_tool(state, &effective_session_id, None);

    match exec_res {
        Ok(call_result) => {
            record_audit(
                state,
                verified,
                &effective_session_id,
                tool_name,
                target_node_opt.as_deref(),
                &args,
                true,
                None,
                started,
            );
            jsonrpc_ok(rpc_id, call_result, &pv, session_hdr)
        }
        Err((status, code, msg, err_code)) => {
            record_audit(
                state,
                verified,
                &effective_session_id,
                tool_name,
                target_node_opt.as_deref(),
                &args,
                false,
                Some(err_code),
                started,
            );
            jsonrpc_err(status, rpc_id, code, &msg)
        }
    }
}

type ToolExecResult = std::result::Result<Value, (StatusCode, i64, String, &'static str)>;

fn resolve_target_node_id(
    state: &AppState,
    verified: &McpVerifiedAccessToken,
    args: &Value,
) -> std::result::Result<String, (StatusCode, i64, String, &'static str)> {
    if let Some(nid) = args
        .get("node_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !is_node_allowed(verified, nid) {
            return Err((
                StatusCode::FORBIDDEN,
                -32003,
                format!("Device `{nid}` is not in the authorized device allowlist"),
                "device_not_allowed",
            ));
        }
        return Ok(nid.to_string());
    }

    if verified.allowed_node_ids.len() == 1 && verified.allowed_node_ids[0] != "*" {
        return Ok(verified.allowed_node_ids[0].clone());
    }

    let now_ms = Utc::now().timestamp_millis();
    let nodes = state
        .db
        .list_computer_nodes(verified.user_id, None, now_ms)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| is_node_allowed(verified, &n.node_id))
        .collect::<Vec<_>>();

    if let Some(online) = nodes.iter().find(|n| n.online) {
        return Ok(online.node_id.clone());
    }
    if let Some(first) = nodes.first() {
        return Ok(first.node_id.clone());
    }

    Err((
        StatusCode::BAD_REQUEST,
        -32602,
        "node_id is required (no authorized device found)".to_string(),
        "missing_node_id",
    ))
}

async fn run_device_op(
    state: &Arc<AppState>,
    verified: &McpVerifiedAccessToken,
    args: &Value,
    op: RemoteComputerOp,
) -> std::result::Result<(String, Value), (StatusCode, i64, String, &'static str)> {
    let node_id = resolve_target_node_id(state, verified, args)?;
    let req = RemoteComputerCommandRequest {
        node_id: node_id.clone(),
        caller_device_id: Some(format!("mcp:{}", verified.client_id)),
        caller_bot_id: None,
        caller_label: verified.client_name.clone(),
        operation: op,
    };

    let res = state
        .computers
        .execute_remote_command(verified.user_id, req, None)
        .await
        .map_err(|e| {
            (
                StatusCode::OK,
                -32000,
                format!("Device command failed: {e}"),
                "device_command_error",
            )
        })?;
    let val = serde_json::to_value(&res).unwrap_or_else(|_| json!({}));
    Ok((node_id, val))
}

fn text_tool_result(val: &Value) -> Value {
    let text = serde_json::to_string_pretty(val).unwrap_or_else(|_| val.to_string());
    json!({
        "content": [{
            "type": "text",
            "text": text,
        }],
        "structuredContent": val,
        "isError": false,
    })
}

fn extract_arg_str(
    args: &Value,
    key: &str,
) -> std::result::Result<String, (StatusCode, i64, String, &'static str)> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                -32602,
                format!("Missing required argument `{key}`"),
                "invalid_params",
            )
        })
}

async fn execute_mcp_tool(
    state: &Arc<AppState>,
    verified: &McpVerifiedAccessToken,
    session_id: &str,
    tool_name: &str,
    args: &Value,
) -> ToolExecResult {
    let now_ms = Utc::now().timestamp_millis();
    let uid = UserId(verified.user_id);

    match tool_name {
        "list_devices" => {
            let nodes = state
                .db
                .list_computer_nodes(verified.user_id, None, now_ms)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        -32603,
                        e.to_string(),
                        "db_error",
                    )
                })?
                .into_iter()
                .filter(|n| is_node_allowed(verified, &n.node_id))
                .collect::<Vec<_>>();
            Ok(text_tool_result(&json!({ "devices": nodes })))
        }
        "get_device_status" => {
            let node_id = resolve_target_node_id(state, verified, args)?;
            let node = state
                .db
                .list_computer_nodes(verified.user_id, None, now_ms)
                .unwrap_or_default()
                .into_iter()
                .find(|n| n.node_id == node_id);
            let displays = if node.as_ref().is_some_and(|n| n.online) {
                state
                    .computers
                    .execute_remote_command(
                        verified.user_id,
                        RemoteComputerCommandRequest {
                            node_id: node_id.clone(),
                            caller_device_id: Some("mcp".to_string()),
                            caller_bot_id: None,
                            caller_label: verified.client_name.clone(),
                            operation: RemoteComputerOp::DisplayList,
                        },
                        None,
                    )
                    .await
                    .ok()
            } else {
                None
            };
            Ok(text_tool_result(&json!({
                "node_id": node_id,
                "node": node,
                "displays": displays,
            })))
        }
        "capture_screen" => {
            let (node_id, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::CaptureScreen,
            )
            .await?;

            let mut content = Vec::new();
            if let Some(data_url) = val.get("frame_data_url").and_then(|v| v.as_str()) {
                if let Some((meta, b64)) = data_url.split_once(";base64,") {
                    let mime = meta.strip_prefix("data:").unwrap_or("image/png");
                    content.push(json!({
                        "type": "image",
                        "data": b64,
                        "mimeType": mime,
                    }));
                }
            }
            content.push(json!({
                "type": "text",
                "text": json!({
                    "node_id": node_id,
                    "ok": val.get("ok").and_then(|v| v.as_bool()).unwrap_or(true),
                    "stdout": val.get("stdout"),
                }).to_string(),
            }));
            Ok(json!({
                "content": content,
                "structuredContent": val,
                "isError": false,
            }))
        }
        "get_accessibility_tree" => {
            let max_depth = args
                .get("max_depth")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::AccessibilityTree { max_depth },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "semantic_ui_action" => {
            let action = extract_arg_str(args, "action")?;
            let element_id = args
                .get("element_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let value = args
                .get("value")
                .or_else(|| args.get("text"))
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::SemanticAction {
                    element_id,
                    action,
                    value,
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "mouse_action" => {
            let action = extract_arg_str(args, "action")?;
            let x = args.get("x").and_then(|v| v.as_i64()).map(|v| v as i32);
            let y = args.get("y").and_then(|v| v.as_i64()).map(|v| v as i32);
            let dx = args
                .get("dx")
                .or_else(|| args.get("delta_x"))
                .and_then(|v| v.as_i64())
                .map(|v| v as i32);
            let dy = args
                .get("dy")
                .or_else(|| args.get("delta_y"))
                .and_then(|v| v.as_i64())
                .map(|v| v as i32);
            let button = args
                .get("button")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let clicks = args.get("clicks").and_then(|v| v.as_u64()).map(|v| v as u8);
            let scroll_x = args
                .get("scroll_x")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32);
            let scroll_y = args
                .get("scroll_y")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32);
            let relative = args
                .get("relative")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::MouseAction {
                    action,
                    x,
                    y,
                    dx,
                    dy,
                    button,
                    clicks,
                    scroll_x,
                    scroll_y,
                    relative,
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "keyboard_type" => {
            let text = extract_arg_str(args, "text")?;
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::KeyboardAction {
                    action: "type".to_string(),
                    text: Some(text),
                    key: None,
                    modifiers: vec![],
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "keyboard_hotkey" => {
            let keys: Vec<String> = args
                .get("keys")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| item.as_str().map(ToString::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if keys.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    -32602,
                    "`keys` array is required".to_string(),
                    "invalid_params",
                ));
            }
            let (modifiers, key) = if keys.len() > 1 {
                (
                    keys[..keys.len() - 1]
                        .iter()
                        .map(|k| k.to_ascii_lowercase())
                        .collect(),
                    keys[keys.len() - 1].clone(),
                )
            } else {
                (vec![], keys[0].clone())
            };
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::KeyboardAction {
                    action: "hotkey".to_string(),
                    text: None,
                    key: Some(key),
                    modifiers,
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "terminal_list" => {
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::TerminalOp {
                    op: "ls".to_string(),
                    session_name: None,
                    shell: None,
                    input: None,
                    command: None,
                    cols: None,
                    rows: None,
                    timeout_ms: None,
                    new_name: None,
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "terminal_open" => {
            let session_name = args
                .get("session_name")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::TerminalOp {
                    op: "open".to_string(),
                    session_name,
                    shell: args.get("shell").and_then(|v| v.as_str()).map(ToString::to_string),
                    input: None,
                    command: None,
                    cols: args.get("cols").and_then(|v| v.as_u64()).map(|v| v as u16),
                    rows: args.get("rows").and_then(|v| v.as_u64()).map(|v| v as u16),
                    timeout_ms: None,
                    new_name: None,
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "terminal_exec" => {
            let command = extract_arg_str(args, "command")?;
            let cwd = args.get("cwd").and_then(|v| v.as_str()).map(ToString::to_string);
            let timeout_ms = args
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    args.get("timeout_secs")
                        .and_then(|v| v.as_u64())
                        .map(|s| s * 1000)
                });
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::ShellExec {
                    command,
                    cwd,
                    timeout_ms,
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "terminal_send" => {
            let tid = args
                .get("session_name")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        -32602,
                        "Missing `session_id` / `session_name`".to_string(),
                        "invalid_params",
                    )
                })?;
            let input = args
                .get("input")
                .or_else(|| args.get("data"))
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        -32602,
                        "`input` string is required".to_string(),
                        "invalid_params",
                    )
                })?;
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::TerminalOp {
                    op: "send".to_string(),
                    session_name: Some(tid),
                    shell: None,
                    input: Some(input),
                    command: None,
                    cols: None,
                    rows: None,
                    timeout_ms: None,
                    new_name: None,
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "terminal_read" => {
            let tid = args
                .get("session_name")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        -32602,
                        "Missing `session_id` / `session_name`".to_string(),
                        "invalid_params",
                    )
                })?;
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::TerminalOp {
                    op: "read".to_string(),
                    session_name: Some(tid),
                    shell: None,
                    input: None,
                    command: None,
                    cols: None,
                    rows: None,
                    timeout_ms: None,
                    new_name: None,
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "terminal_kill" => {
            let tid = args
                .get("session_name")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        -32602,
                        "Missing `session_id` / `session_name`".to_string(),
                        "invalid_params",
                    )
                })?;
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::TerminalOp {
                    op: "kill".to_string(),
                    session_name: Some(tid),
                    shell: None,
                    input: None,
                    command: None,
                    cols: None,
                    rows: None,
                    timeout_ms: None,
                    new_name: None,
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "file_list" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .to_string();
            let (_, val) =
                run_device_op(state, verified, args, RemoteComputerOp::ListDir { path }).await?;
            Ok(text_tool_result(&val))
        }
        "file_read" => {
            let path = extract_arg_str(args, "path")?;
            let max_bytes = args
                .get("max_bytes")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::ReadFile { path, max_bytes },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "file_write" => {
            let path = extract_arg_str(args, "path")?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        -32602,
                        "`content` string is required".to_string(),
                        "invalid_params",
                    )
                })?;
            let create_dirs = args
                .get("create_dirs")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::WriteFile {
                    path,
                    content,
                    create_dirs,
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "clipboard_get" => {
            let (_, val) =
                run_device_op(state, verified, args, RemoteComputerOp::ClipboardGet).await?;
            Ok(text_tool_result(&val))
        }
        "clipboard_set" => {
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        -32602,
                        "`text` is required".to_string(),
                        "invalid_params",
                    )
                })?;
            let (_, val) = run_device_op(
                state,
                verified,
                args,
                RemoteComputerOp::ClipboardSet {
                    text: Some(text),
                    image_b64: None,
                    mime_type: Some("text/plain".to_string()),
                },
            )
            .await?;
            Ok(text_tool_result(&val))
        }
        "assist_get_info" => {
            let dev = args
                .get("device_id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("default");
            let rec = state
                .db
                .get_or_init_assist_device(verified.user_id, dev, now_ms)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        -32603,
                        e.to_string(),
                        "db_error",
                    )
                })?;
            Ok(text_tool_result(&json!({
                "assist_id": rec.assist_id,
                "device_id": rec.device_id,
                "allow_assist": rec.allow_assist,
                "code_mode": rec.code_mode,
                "temp_code": rec.temp_code,
                "expires_at_ms": rec.expires_at_ms,
            })))
        }
        "assist_manage" => {
            let action = extract_arg_str(args, "action")?;
            let dev = args
                .get("device_id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("default");
            let rec = match action.as_str() {
                "allow" => {
                    let allow = args
                        .get("allow_assist")
                        .and_then(|v| v.as_bool())
                        .ok_or_else(|| {
                            (
                                StatusCode::BAD_REQUEST,
                                -32602,
                                "`allow_assist` boolean is required".to_string(),
                                "invalid_params",
                            )
                        })?;
                    state
                        .db
                        .set_assist_allow(verified.user_id, dev, allow, now_ms)
                        .map_err(|e| {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                -32603,
                                e.to_string(),
                                "db_error",
                            )
                        })?
                }
                "reset_code" => state
                    .db
                    .reset_assist_temp_code(verified.user_id, dev, now_ms)
                    .map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            -32603,
                            e.to_string(),
                            "db_error",
                        )
                    })?,
                "set_code" => {
                    let code = extract_arg_str(args, "code")?;
                    if !is_valid_custom_assist_code(&code) {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            -32602,
                            "Custom code must be 8-16 alphanumeric characters with letters and digits"
                                .to_string(),
                            "invalid_params",
                        ));
                    }
                    state
                        .db
                        .set_assist_custom_code(verified.user_id, dev, &code, now_ms)
                        .map_err(|e| (StatusCode::BAD_REQUEST, -32602, e.to_string(), "invalid_params"))?
                }
                _ => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        -32602,
                        "action must be `allow`, `reset_code`, or `set_code`".to_string(),
                        "invalid_params",
                    ));
                }
            };
            Ok(text_tool_result(&json!({
                "assist_id": rec.assist_id,
                "device_id": rec.device_id,
                "allow_assist": rec.allow_assist,
                "code_mode": rec.code_mode,
                "temp_code": rec.temp_code,
                "expires_at_ms": rec.expires_at_ms,
            })))
        }
        "device_power_action" => {
            if args.get("confirm").and_then(|v| v.as_bool()) != Some(true) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    -32602,
                    "Power action requires `confirm: true`".to_string(),
                    "confirmation_required",
                ));
            }
            let raw_action = extract_arg_str(args, "action")?;
            let action = match raw_action.to_ascii_lowercase().as_str() {
                "lock" | "lock_screen" => PowerActionKind::Lock,
                "sleep" => PowerActionKind::Sleep,
                "restart" | "reboot" => PowerActionKind::Reboot,
                "shutdown" | "power_off" => PowerActionKind::Shutdown,
                _ => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        -32602,
                        "Unsupported power action; use lock, sleep, restart, or shutdown".to_string(),
                        "invalid_params",
                    ));
                }
            };
            let (_, val) =
                run_device_op(state, verified, args, RemoteComputerOp::PowerAction { action })
                    .await?;
            Ok(text_tool_result(&val))
        }
        "request_human_takeover" => {
            let node_id = resolve_target_node_id(state, verified, args)?;
            let reason = extract_arg_str(args, "reason")?;
            let instructions = args
                .get("instructions")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let event = UpdateEvent::BotActivityChanged {
                chat_id: ChatId(0),
                bot_user_id: uid,
                state: format!("human_takeover_requested:{node_id}"),
                detail: Some(
                    json!({
                        "session_id": session_id,
                        "node_id": node_id,
                        "client_name": verified.client_name,
                        "reason": reason,
                        "instructions": instructions,
                    })
                    .to_string(),
                ),
            };
            state
                .gateway_hub
                .broadcast_to_user(uid, WsServerFrame::Update(event));
            Ok(text_tool_result(&json!({
                "status": "takeover_requested",
                "node_id": node_id,
                "reason": reason,
            })))
        }
        "list_dialogs" => {
            let chats = state.db.list_user_chats(uid).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    -32603,
                    e.to_string(),
                    "db_error",
                )
            })?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let sliced: Vec<_> = chats.into_iter().take(limit.clamp(1, 200)).collect();
            Ok(text_tool_result(&json!({ "dialogs": sliced })))
        }
        "get_chat_history" => {
            let chat_id_raw = args.get("chat_id").and_then(|v| v.as_i64()).ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    -32602,
                    "`chat_id` integer is required".to_string(),
                    "invalid_params",
                )
            })?;
            let chat_id = ChatId(chat_id_raw);
            check_active_membership(state, chat_id, uid).map_err(|_| {
                (
                    StatusCode::FORBIDDEN,
                    -32003,
                    "User is not an active member of this chat".to_string(),
                    "forbidden",
                )
            })?;
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(30)
                .clamp(1, 100) as u32;
            let before_id = args.get("before_id").and_then(|v| v.as_i64());
            let msgs = state
                .db
                .list_messages_with_topic(chat_id, None, before_id, limit)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        -32603,
                        e.to_string(),
                        "db_error",
                    )
                })?;
            let formatted: Vec<Value> = msgs
                .iter()
                .map(|m| crate::routes::messages::format_message_json(state, m, uid))
                .collect();
            Ok(text_tool_result(&json!({
                "chat_id": chat_id_raw,
                "messages": formatted,
            })))
        }
        "send_message" => {
            let chat_id_raw = args.get("chat_id").and_then(|v| v.as_i64()).ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    -32602,
                    "`chat_id` integer is required".to_string(),
                    "invalid_params",
                )
            })?;
            let text = extract_arg_str(args, "text")?;
            let mut body = json!({ "text": text });
            if let Some(reply_to) = args.get("reply_to_msg_id").and_then(|v| v.as_i64()) {
                body["reply_to_msg_id"] = json!(reply_to);
            }
            let resp = crate::routes::messages::send_for_chat(
                state,
                uid,
                chat_id_raw,
                body,
                StatusCode::CREATED,
            )
            .await;
            let status = resp.status();
            let bytes = to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap_or_default();
            let parsed: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
            if !status.is_success() {
                return Err((
                    status,
                    -32000,
                    parsed
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Failed to send message")
                        .to_string(),
                    "send_message_failed",
                ));
            }
            Ok(text_tool_result(&parsed))
        }
        "search_messages" => {
            let query_str = extract_arg_str(args, "query")?;
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(20)
                .clamp(1, 100) as usize;
            let member_chat_ids = state.db.list_user_member_chat_ids(uid).unwrap_or_default();
            let member_set: HashSet<i64> = member_chat_ids.iter().map(|c| c.0).collect();

            let mut search_params = SearchQueryParams::new(&query_str);
            if let Some(cid) = args.get("chat_id").and_then(|v| v.as_i64()) {
                if !member_set.contains(&cid) {
                    return Err((
                        StatusCode::FORBIDDEN,
                        -32003,
                        "User is not an active member of this chat".to_string(),
                        "forbidden",
                    ));
                }
                search_params = search_params.with_chat_id(cid.to_string());
            }

            let mut results: Vec<Value> = state
                .search_indexer
                .search_hits(&search_params)
                .unwrap_or_default()
                .into_iter()
                .filter(|h| {
                    h.chat_id
                        .parse::<i64>()
                        .is_ok_and(|cid| member_set.contains(&cid))
                })
                .take(limit)
                .map(|h| json!(h))
                .collect();

            if results.is_empty() {
                let q_lower = query_str.to_ascii_lowercase();
                let target_chats: Vec<ChatId> =
                    if let Some(cid) = args.get("chat_id").and_then(|v| v.as_i64()) {
                        vec![ChatId(cid)]
                    } else {
                        member_chat_ids
                    };
                for cid in target_chats {
                    if let Ok(msgs) = state.db.list_messages_with_topic(cid, None, None, 50) {
                        for m in msgs {
                            if m.content_json.to_ascii_lowercase().contains(&q_lower) {
                                results.push(crate::routes::messages::format_message_json(
                                    state, &m, uid,
                                ));
                                if results.len() >= limit {
                                    break;
                                }
                            }
                        }
                    }
                    if results.len() >= limit {
                        break;
                    }
                }
            }
            Ok(text_tool_result(&json!({ "hits": results })))
        }
        "list_tasks" => {
            let guard = state.task_registry.read().await;
            let tasks: Vec<Value> = guard
                .values()
                .map(|fsm| {
                    json!({
                        "task_id": fsm.task_id,
                        "state": fsm.state.as_str(),
                        "abort_reason": fsm.abort_reason,
                    })
                })
                .collect();
            Ok(text_tool_result(&json!({ "tasks": tasks })))
        }
        "create_task" => {
            let title = extract_arg_str(args, "title")?;
            let description = args
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let task_id = format!("task_{}", random_hex(8));
            let fsm = TaskFsm::new(task_id.clone());
            state
                .task_registry
                .write()
                .await
                .insert(task_id.clone(), fsm);
            Ok(text_tool_result(&json!({
                "task_id": task_id,
                "title": title,
                "description": description,
                "state": "pending",
            })))
        }
        "list_bots" => {
            let bots = state
                .db
                .list_all_bots()
                .unwrap_or_default()
                .into_iter()
                .filter(|b| b.owner_user_id == verified.user_id as i64 || b.is_official)
                .collect::<Vec<_>>();
            Ok(text_tool_result(&json!({ "bots": bots })))
        }
        "miniapp_list" => {
            let installed = state
                .db
                .list_user_installed_mini_apps(verified.user_id as i64)
                .unwrap_or_default();
            let published = state
                .db
                .search_published_mini_apps("", None)
                .unwrap_or_default();
            Ok(text_tool_result(&json!({
                "installed": installed,
                "published": published,
            })))
        }
        _ => Err((
            StatusCode::NOT_FOUND,
            -32601,
            format!("Unknown MCP tool `{tool_name}`"),
            "unknown_tool",
        )),
    }
}

fn mcp_tool_definitions() -> Value {
    json!([
        {
            "name": "list_devices",
            "description": "List online and registered desktop/server devices owned by the user (filtered by authorized device allowlist).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_device_status",
            "description": "Fetch live telemetry, OS/architecture, displays, and online status for a specific device.",
            "inputSchema": { "type": "object", "properties": { "node_id": { "type": "string" } } }
        },
        {
            "name": "capture_screen",
            "description": "Capture a screenshot of the target device display and return an MCP ImageContent block.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": { "type": "string" }
                }
            }
        },
        {
            "name": "get_accessibility_tree",
            "description": "Inspect the target device accessibility UI tree for semantic UI automation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": { "type": "string" },
                    "max_depth": { "type": "integer" }
                }
            }
        },
        {
            "name": "semantic_ui_action",
            "description": "Trigger an accessibility semantic action (click, focus, set_text, scroll) on a UI element.",
            "inputSchema": {
                "type": "object",
                "required": ["action"],
                "properties": {
                    "node_id": { "type": "string" },
                    "action": { "type": "string" },
                    "element_id": { "type": "string" },
                    "value": { "type": "string" }
                }
            }
        },
        {
            "name": "mouse_action",
            "description": "Perform a mouse move, click, double_click, right_click, scroll, or drag action on the target device.",
            "inputSchema": {
                "type": "object",
                "required": ["action"],
                "properties": {
                    "node_id": { "type": "string" },
                    "action": { "type": "string" },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "dx": { "type": "integer" },
                    "dy": { "type": "integer" },
                    "button": { "type": "string" },
                    "clicks": { "type": "integer" },
                    "scroll_x": { "type": "integer" },
                    "scroll_y": { "type": "integer" },
                    "relative": { "type": "boolean" }
                }
            }
        },
        {
            "name": "keyboard_type",
            "description": "Type UTF-8 text into the currently focused window on the target device.",
            "inputSchema": {
                "type": "object",
                "required": ["text"],
                "properties": { "node_id": { "type": "string" }, "text": { "type": "string" } }
            }
        },
        {
            "name": "keyboard_hotkey",
            "description": "Press a keyboard chord/hotkey combination (e.g. [\"Cmd\", \"C\"]).",
            "inputSchema": {
                "type": "object",
                "required": ["keys"],
                "properties": {
                    "node_id": { "type": "string" },
                    "keys": { "type": "array", "items": { "type": "string" } }
                }
            }
        },
        {
            "name": "terminal_list",
            "description": "List active interactive PTY terminal sessions on the target device.",
            "inputSchema": { "type": "object", "properties": { "node_id": { "type": "string" } } }
        },
        {
            "name": "terminal_open",
            "description": "Open a new persistent PTY terminal session on the target device.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": { "type": "string" },
                    "session_name": { "type": "string" },
                    "shell": { "type": "string" },
                    "cols": { "type": "integer" },
                    "rows": { "type": "integer" }
                }
            }
        },
        {
            "name": "terminal_exec",
            "description": "Execute a shell command on the target device and return stdout/stderr/exit_code.",
            "inputSchema": {
                "type": "object",
                "required": ["command"],
                "properties": {
                    "node_id": { "type": "string" },
                    "command": { "type": "string" },
                    "cwd": { "type": "string" },
                    "timeout_ms": { "type": "integer" }
                }
            }
        },
        {
            "name": "terminal_send",
            "description": "Send stdin text or control sequences to an open PTY terminal session.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id", "input"],
                "properties": {
                    "node_id": { "type": "string" },
                    "session_id": { "type": "string" },
                    "input": { "type": "string" }
                }
            }
        },
        {
            "name": "terminal_read",
            "description": "Read buffered output from an open PTY terminal session.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "node_id": { "type": "string" },
                    "session_id": { "type": "string" }
                }
            }
        },
        {
            "name": "terminal_kill",
            "description": "Terminate an open PTY terminal session on the target device.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "node_id": { "type": "string" },
                    "session_id": { "type": "string" }
                }
            }
        },
        {
            "name": "file_list",
            "description": "List files and directories at `path` on the target device.",
            "inputSchema": {
                "type": "object",
                "properties": { "node_id": { "type": "string" }, "path": { "type": "string" } }
            }
        },
        {
            "name": "file_read",
            "description": "Read file contents from `path` on the target device.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "node_id": { "type": "string" },
                    "path": { "type": "string" },
                    "max_bytes": { "type": "integer" }
                }
            }
        },
        {
            "name": "file_write",
            "description": "Write file contents to `path` on the target device.",
            "inputSchema": {
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "node_id": { "type": "string" },
                    "path": { "type": "string" },
                    "content": { "type": "string" },
                    "create_dirs": { "type": "boolean" }
                }
            }
        },
        {
            "name": "clipboard_get",
            "description": "Read the current system clipboard text on the target device.",
            "inputSchema": { "type": "object", "properties": { "node_id": { "type": "string" } } }
        },
        {
            "name": "clipboard_set",
            "description": "Write text to the system clipboard on the target device.",
            "inputSchema": {
                "type": "object",
                "required": ["text"],
                "properties": { "node_id": { "type": "string" }, "text": { "type": "string" } }
            }
        },
        {
            "name": "assist_get_info",
            "description": "Get permanent 9-digit Remote Assist ID and connection code settings for the user's device.",
            "inputSchema": { "type": "object", "properties": { "device_id": { "type": "string" } } }
        },
        {
            "name": "assist_manage",
            "description": "Manage Remote Assist (`allow`, `reset_code`, `set_code`) on the user's device.",
            "inputSchema": {
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string" },
                    "device_id": { "type": "string" },
                    "allow_assist": { "type": "boolean" },
                    "code": { "type": "string" }
                }
            }
        },
        {
            "name": "device_power_action",
            "description": "Trigger lock, sleep, restart, or shutdown on the target device (requires confirm: true).",
            "inputSchema": {
                "type": "object",
                "required": ["action", "confirm"],
                "properties": {
                    "node_id": { "type": "string" },
                    "action": { "type": "string" },
                    "confirm": { "type": "boolean" }
                }
            }
        },
        {
            "name": "request_human_takeover",
            "description": "Pause automation and notify the host human user to complete CAPTCHA/2FA/manual confirmation.",
            "inputSchema": {
                "type": "object",
                "required": ["reason"],
                "properties": {
                    "node_id": { "type": "string" },
                    "reason": { "type": "string" },
                    "instructions": { "type": "string" }
                }
            }
        },
        {
            "name": "list_dialogs",
            "description": "List Fabushi chats/dialogs accessible to the authenticated user.",
            "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer" } } }
        },
        {
            "name": "get_chat_history",
            "description": "Fetch recent message history from a Fabushi chat.",
            "inputSchema": {
                "type": "object",
                "required": ["chat_id"],
                "properties": {
                    "chat_id": { "type": "integer" },
                    "limit": { "type": "integer" },
                    "before_id": { "type": "integer" }
                }
            }
        },
        {
            "name": "send_message",
            "description": "Send a message to a Fabushi chat.",
            "inputSchema": {
                "type": "object",
                "required": ["chat_id", "text"],
                "properties": {
                    "chat_id": { "type": "integer" },
                    "text": { "type": "string" },
                    "reply_to_msg_id": { "type": "integer" }
                }
            }
        },
        {
            "name": "search_messages",
            "description": "Search Fabushi messages across chats or within a single chat.",
            "inputSchema": {
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string" },
                    "chat_id": { "type": "integer" },
                    "limit": { "type": "integer" }
                }
            }
        },
        {
            "name": "list_tasks",
            "description": "List Fabushi collaborative agent/user tasks.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "create_task",
            "description": "Create a new Fabushi task.",
            "inputSchema": {
                "type": "object",
                "required": ["title"],
                "properties": {
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "chat_id": { "type": "integer" }
                }
            }
        },
        {
            "name": "list_bots",
            "description": "List Fabushi bots owned or accessible by the user.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "miniapp_list",
            "description": "List registered and installed Fabushi MiniApps.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}
