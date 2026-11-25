//! Community feed, algorithm preferences, and engagement signal endpoints.

use crate::routes::community::{
    ensure_post_publicly_interactable_conn, map_community_err, sanitize_post_embedded_refs_conn,
    ActorAuth,
};
use crate::server::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use chrono::Utc;
use gpui_telegram_community::store::ops;
use gpui_telegram_proto::community_contracts::{
    AlgoPreferencesDto, EngagementSignalKind, FeedQueryParams, RecordSignalRequest,
};
use std::sync::Arc;

pub async fn get_feed(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<FeedQueryParams>,
) -> Response {
    let req = query.into_feed_request();
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_reader(|conn| {
        let mut resp = ops::get_feed_conn(conn, &auth.actor_id, &req, now_ms)?;
        for item in &mut resp.items {
            sanitize_post_embedded_refs_conn(conn, &mut item.post);
        }
        Ok(resp)
    }) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn get_algo_prefs(auth: ActorAuth, State(state): State<Arc<AppState>>) -> Response {
    match state
        .db
        .pool()
        .with_reader(|conn| ops::get_algo_prefs_conn(conn, &auth.actor_id))
    {
        Ok(prefs) => (StatusCode::OK, Json(prefs)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn put_algo_prefs(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Json(prefs): Json<AlgoPreferencesDto>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .pool()
        .with_transaction(|tx| ops::put_algo_prefs_tx(tx, &auth.actor_id, &prefs, now_ms))
    {
        Ok(saved) => (StatusCode::OK, Json(saved)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn record_signal(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Json(mut req): Json<RecordSignalRequest>,
) -> Response {
    req.post_id = req.post_id.trim().to_string();
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        if req.kind == EngagementSignalKind::Report {
            let _ = ensure_post_publicly_interactable_conn(tx, &req.post_id)?;
        }
        ops::record_signal_tx(tx, &auth.actor_id, &req, now_ms)
    }) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}
