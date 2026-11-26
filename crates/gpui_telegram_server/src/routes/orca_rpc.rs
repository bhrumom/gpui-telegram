//! Unified Orca Runtime Protocol v3 RPC endpoint (`POST /api/v1/orca/rpc`)
//! and top-level Orca router aggregator (`orca_router()`).

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::post,
    Router,
};
use orca_core::RUNTIME_PROTOCOL_VERSION;
use orca_core::orchestration_protocol::{
    RpcRequestEnvelope, RpcResponseEnvelope, negotiate_runtime_protocol,
};
use serde_json::{Value, json};

use crate::server::{AppState, AuthBearer};

fn parse_optional_i64(val: Option<&Value>) -> Option<i64> {
    let v = val?;
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|u| i64::try_from(u).ok()))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

/// Dispatches a Unified Runtime RPC v3 request (`POST /api/v1/orca/rpc`).
///
/// - Requires valid `AuthBearer` (`401 Unauthorized` if missing/invalid).
/// - Malformed/empty JSON -> HTTP `400 Bad Request` with `RpcResponseEnvelope::err("invalid_argument", ...)`.
/// - All dispatched RPC calls (both `ok: true` and `ok: false`) -> HTTP `200 OK` with `Json(RpcResponseEnvelope)`.
/// - NEVER returns HTTP `501 Not Implemented`.
pub async fn handle_orca_rpc(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(RpcResponseEnvelope::err(
                "invalid_argument",
                "RPC request body cannot be empty",
            )),
        )
            .into_response();
    }
    let req: RpcRequestEnvelope = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(RpcResponseEnvelope::err(
                    "invalid_argument",
                    format!("Malformed RPC request envelope: {err}"),
                )),
            )
                .into_response();
        }
    };

    if matches!(
        req.method.trim(),
        "bots.telegram.bindChat" | "bot.telegram.bindChat" | "bots.telegram.bind_chat"
    ) {
        if let Err(err) = negotiate_runtime_protocol(req.protocol_version, RUNTIME_PROTOCOL_VERSION)
        {
            return (
                StatusCode::OK,
                Json(RpcResponseEnvelope::err("protocol_version", err.to_string())),
            )
                .into_response();
        }
        let Some(bot_id) = req
            .params
            .get("botId")
            .or_else(|| req.params.get("bot_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return (
                StatusCode::OK,
                Json(RpcResponseEnvelope::err(
                    "invalid_argument",
                    "Missing required 'botId' for bots.telegram.bindChat",
                )),
            )
                .into_response();
        };
        let explicit_chat_id =
            parse_optional_i64(req.params.get("chatId").or_else(|| req.params.get("chat_id")));
        let resp = match crate::routes::orca_telegram_bridge::bind_bot_to_telegram_chat(
            &state,
            auth.user_id,
            bot_id,
            explicit_chat_id,
        )
        .await
        {
            Ok((chat_id, bot_user_id, bot_username)) => RpcResponseEnvelope::ok(json!({
                "botId": bot_id,
                "bot_id": bot_id,
                "chatId": chat_id,
                "chat_id": chat_id,
                "botUserId": bot_user_id,
                "botUsername": bot_username,
                "linked": true,
            })),
            Err((code, msg)) => RpcResponseEnvelope::err(code, msg),
        };
        return (StatusCode::OK, Json(resp)).into_response();
    }

    let caller_profile = format!("user:{}", auth.user_id.0);
    let resp = state
        .orca_ide
        .dispatch_rpc_for_profile(req, &caller_profile)
        .await;
    (StatusCode::OK, Json(resp)).into_response()
}

/// Router exposing solely `POST /api/v1/orca/rpc`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/orca/rpc", post(handle_orca_rpc))
}

/// Aggregated Orca router merging all Orca REST, WebSocket, Webhook, Orchestration, IDE,
/// Telegram IM Fusion, and Unified RPC v3 route modules.
pub fn orca_router() -> Router<Arc<AppState>> {
    Router::new()
        .merge(crate::routes::orca_bots::router())
        .merge(crate::routes::orca_bots_items::router())
        .merge(crate::routes::orca_bots_groups::router())
        .merge(crate::routes::orca_share_wake::router())
        .merge(crate::routes::orca_webhooks::router())
        .merge(crate::routes::orca_orchestration::router())
        .merge(crate::routes::orca_ide::router())
        .merge(crate::routes::orca_telegram_bridge::router())
        .merge(router())
}
