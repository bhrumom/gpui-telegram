//! Community (`c/<slug>`) CRUD, join/leave, versioned wiki, mod queue, mod actions, and mod logs endpoints.

use crate::routes::community::{
    broadcast_community_event, delete_post_in_search, ensure_post_mod_target_allowed_conn,
    is_post_publicly_interactable, map_community_err, sanitize_post_embedded_refs_conn,
    sync_post_in_search, validate_community_media_access_for_auth, ActorAuth,
};
use crate::server::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use chrono::Utc;
use gpui_telegram_community::store::ops;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_proto::community_contracts::{
    CommunityDetailResponse, CommunityMemberRole, CreateCommunityRequest, JoinCommunityResponse,
    ListCommunitiesResponse, ModActionKind, ModActionRequest, ModLogsResponse, ModQueueResponse,
    ModQueueStatus, UpdateCommunityRequest, UpdateCommunityWikiRequest,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Default, Deserialize)]
pub struct ListCommunitiesQuery {
    pub category: Option<String>,
    pub limit: Option<usize>,
}

pub async fn list_communities(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListCommunitiesQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(100).clamp(1, 200);
    match state.db.pool().with_reader(|conn| {
        ops::list_communities_conn(conn, Some(&auth.actor_id), query.category.as_deref(), limit)
    }) {
        Ok(communities) => {
            (StatusCode::OK, Json(ListCommunitiesResponse { communities })).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn create_community(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateCommunityRequest>,
) -> Response {
    let media_refs: Vec<&str> = [req.banner_media_id.as_deref(), req.icon_media_id.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    if let Err(resp) = validate_community_media_access_for_auth(&state, &auth, &media_refs) {
        return resp;
    }

    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .pool()
        .with_transaction(|tx| ops::create_community_tx(tx, &auth.actor_id, &req, now_ms))
    {
        Ok(created) => {
            broadcast_community_event(
                &state,
                "community_created",
                Some(created.community_id.clone()),
                None,
                None,
                Some(auth.actor_id),
                &json!({ "community_id": created.community_id, "slug": created.slug }),
            );
            (StatusCode::CREATED, Json(created)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn get_community(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Response {
    let res = state.db.pool().with_reader(|conn| {
        let mut community = ops::get_community_conn(conn, &slug, Some(&auth.actor_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("Community '{slug}' not found")))?;

        let mut pinned_posts = Vec::new();
        for pid in &community.pinned_post_ids {
            if let Some(mut p) = ops::get_post_conn(conn, pid, Some(&auth.actor_id))? {
                if is_post_publicly_interactable(&p) {
                    sanitize_post_embedded_refs_conn(conn, &mut p);
                    pinned_posts.push(p);
                }
            }
        }
        community.pinned_post_ids = pinned_posts.iter().map(|p| p.post_id.clone()).collect();

        let mod_ids: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT actor_id FROM community_memberships
                     WHERE community_id = ?1 AND role IN ('owner', 'mod', 'bot_mod')
                     ORDER BY CASE role WHEN 'owner' THEN 0 WHEN 'mod' THEN 1 ELSE 2 END, joined_at_ms ASC",
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![community.community_id], |r| r.get::<_, String>(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| TelegramError::Database(e.to_string()))?);
            }
            out
        };
        let mut moderators = Vec::with_capacity(mod_ids.len());
        for mid in mod_ids {
            if let Some(actor) = ops::get_actor_conn(conn, &mid, Some(&auth.actor_id))? {
                moderators.push(actor);
            }
        }

        let wiki_revisions = ops::list_wiki_revisions_conn(conn, &community.community_id, 20)?;

        Ok(CommunityDetailResponse {
            community,
            pinned_posts,
            moderators,
            wiki_revisions,
        })
    });

    match res {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn update_community(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(req): Json<UpdateCommunityRequest>,
) -> Response {
    let media_refs: Vec<&str> = [req.banner_media_id.as_deref(), req.icon_media_id.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    if let Err(resp) = validate_community_media_access_for_auth(&state, &auth, &media_refs) {
        return resp;
    }

    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .pool()
        .with_transaction(|tx| ops::update_community_tx(tx, &slug, &auth.actor_id, &req, now_ms))
    {
        Ok(updated) => (StatusCode::OK, Json(updated)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn join_community(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .pool()
        .with_transaction(|tx| ops::join_community_tx(tx, &slug, &auth.actor_id, now_ms))
    {
        Ok(comm) => (
            StatusCode::OK,
            Json(JoinCommunityResponse {
                community_slug: comm.slug,
                is_joined: comm.is_joined,
                member_count: comm.member_count,
                viewer_role: comm.viewer_role,
            }),
        )
            .into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn leave_community(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Response {
    match state
        .db
        .pool()
        .with_transaction(|tx| ops::leave_community_tx(tx, &slug, &auth.actor_id))
    {
        Ok(comm) => (
            StatusCode::OK,
            Json(JoinCommunityResponse {
                community_slug: comm.slug,
                is_joined: comm.is_joined,
                member_count: comm.member_count,
                viewer_role: comm.viewer_role,
            }),
        )
            .into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn update_community_wiki(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(req): Json<UpdateCommunityWikiRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        ops::update_wiki_tx(tx, &slug, &auth.actor_id, &req.markdown, &req.summary, now_ms)
    }) {
        Ok(rev) => {
            broadcast_community_event(
                &state,
                "wiki_updated",
                Some(slug.clone()),
                None,
                None,
                Some(auth.actor_id),
                &json!({ "community_slug": slug, "revision": rev.revision }),
            );
            (StatusCode::OK, Json(rev)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ModQueueQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
}

pub async fn get_mod_queue(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Query(query): Query<ModQueueQuery>,
) -> Response {
    let status = query
        .status
        .as_deref()
        .map(str::trim)
        .and_then(ModQueueStatus::parse);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let res = state.db.pool().with_reader(|conn| {
        let comm = ops::get_community_conn(conn, &slug, Some(&auth.actor_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("Community '{slug}' not found")))?;
        if !comm.viewer_role.is_some_and(CommunityMemberRole::is_moderator) {
            return Err(TelegramError::PermissionDenied(format!(
                "Moderator privileges required to view mod queue for c/{}",
                comm.slug
            )));
        }
        ops::list_mod_queue_conn(conn, &comm.community_id, status, limit)
    });
    match res {
        Ok(items) => (StatusCode::OK, Json(ModQueueResponse { items })).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn apply_mod_action(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(mut req): Json<ModActionRequest>,
) -> Response {
    let target_kind = req.target_kind.trim().to_ascii_lowercase();
    req.target_kind = target_kind.clone();
    let now_ms = Utc::now().timestamp_millis();
    let res = state.db.pool().with_transaction(|tx| {
        if target_kind == "post" {
            ensure_post_mod_target_allowed_conn(tx, &req.target_id)?;
        }
        ops::apply_mod_action_tx(tx, &slug, &auth.actor_id, &req, now_ms)
    });

    match res {
        Ok(entry) => {
            if target_kind == "post" {
                match req.action {
                    ModActionKind::Remove => delete_post_in_search(&state, &req.target_id),
                    ModActionKind::Approve | ModActionKind::SetPostFlair => {
                        if let Ok(Some(post)) = state
                            .db
                            .pool()
                            .with_reader(|conn| ops::get_post_conn(conn, &req.target_id, None))
                        {
                            sync_post_in_search(&state, &post);
                        }
                    }
                    _ => {}
                }
            }
            broadcast_community_event(
                &state,
                "mod_action",
                Some(entry.community_id.clone()),
                (target_kind == "post").then(|| req.target_id.clone()),
                (target_kind == "comment").then(|| req.target_id.clone()),
                Some(auth.actor_id),
                &json!({
                    "log_id": entry.log_id,
                    "action": entry.action.as_str(),
                    "target_kind": entry.target_kind,
                    "target_id": entry.target_id,
                }),
            );
            (StatusCode::OK, Json(entry)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ModLogsQuery {
    pub limit: Option<usize>,
}

pub async fn get_mod_logs(
    _auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Query(query): Query<ModLogsQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    match state
        .db
        .pool()
        .with_reader(|conn| ops::list_mod_logs_conn(conn, &slug, limit))
    {
        Ok(logs) => (StatusCode::OK, Json(ModLogsResponse { logs })).into_response(),
        Err(e) => map_community_err(e),
    }
}
