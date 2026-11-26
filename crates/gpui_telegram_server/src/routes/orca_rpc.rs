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
use orca_core::orchestration_protocol::{RpcRequestEnvelope, RpcResponseEnvelope};

use crate::server::{AppState, AuthBearer};

/// Dispatches a Unified Runtime RPC v3 request (`POST /api/v1/orca/rpc`).
///
/// - Requires valid `AuthBearer` (`401 Unauthorized` if missing/invalid).
/// - Malformed/empty JSON -> HTTP `400 Bad Request` with `RpcResponseEnvelope::err("invalid_argument", ...)`.
/// - All dispatched RPC calls (both `ok: true` and `ok: false`) -> HTTP `200 OK` with `Json(RpcResponseEnvelope)`.
/// - NEVER returns HTTP `501 Not Implemented`.
pub async fn handle_orca_rpc(
    _auth: AuthBearer,
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
    let resp = state.orca_ide.dispatch_rpc(req).await;
    (StatusCode::OK, Json(resp)).into_response()
}

/// Router exposing solely `POST /api/v1/orca/rpc`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/orca/rpc", post(handle_orca_rpc))
}

/// Aggregated Orca router merging all M4.1 Orca route modules (extensible by M4.2).
pub fn orca_router() -> Router<Arc<AppState>> {
    Router::new()
        .merge(crate::routes::orca_bots::router())
        .merge(crate::routes::orca_bots_items::router())
        .merge(crate::routes::orca_bots_groups::router())
        .merge(crate::routes::orca_share_wake::router())
        .merge(router())
}
