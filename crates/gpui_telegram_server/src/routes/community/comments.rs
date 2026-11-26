//! Community nested comments CRUD, voting, liking, and awards endpoints.

use crate::routes::community::bot_dispatch::{
    check_bot_comment_loop_guard_tx, dispatch_comment_bot_effects,
};
use crate::routes::community::posts::{
    ensure_comment_publicly_interactable_conn, is_post_publicly_interactable,
    is_post_visible_to_viewer_conn,
};
use crate::routes::community::{
    broadcast_community_event, map_community_err, validate_community_media_access_for_auth,
    ActorAuth,
};
use crate::server::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use chrono::Utc;
use gpui_telegram_community::store::ops;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_proto::community_contracts::{
    AwardResponse, CommentSort, CommentsListResponse, CommentsQueryParams, CommunityCommentDto,
    CommunityMemberRole, CreateCommentRequest, DeleteEntityResponse, GiveAwardRequest, PostStatus,
    UpdateCommentRequest, VoteRequest,
};
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use std::sync::Arc;

fn find_subtree(nodes: &[CommunityCommentDto], target_id: &str) -> Option<Vec<CommunityCommentDto>> {
    for node in nodes {
        if node.comment_id == target_id {
            return Some(node.children.clone());
        }
        if let Some(found) = find_subtree(&node.children, target_id) {
            return Some(found);
        }
    }
    None
}

pub async fn list_comments(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
    Query(query): Query<CommentsQueryParams>,
) -> Response {
    let sort = query.sort.unwrap_or(CommentSort::Best);
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let offset = query
        .cursor
        .as_deref()
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0);

    let res = state.db.pool().with_reader(|conn| {
        let post = ops::get_post_conn(conn, &post_id, Some(&auth.actor_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("Post '{post_id}' not found")))?;
        if !is_post_visible_to_viewer_conn(conn, &post, Some(&auth.actor_id)) {
            return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
        }

        let tree = ops::list_comment_tree_conn(
            conn,
            &post_id,
            Some(&auth.actor_id),
            sort,
            query.max_depth,
            limit.saturating_add(offset).saturating_add(50),
        )?;

        let scoped = if let Some(parent_id) = query
            .parent_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            find_subtree(&tree, parent_id).unwrap_or_default()
        } else {
            tree
        };

        let total = scoped.len();
        let page: Vec<CommunityCommentDto> =
            scoped.into_iter().skip(offset).take(limit).collect();
        let has_more = offset.saturating_add(page.len()) < total;
        let next_cursor = has_more.then(|| (offset + page.len()).to_string());

        Ok(CommentsListResponse {
            post_id,
            sort,
            comments: page,
            total_count: post.comments_count,
            has_more,
            next_cursor,
        })
    });

    match res {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn create_comment(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
    Json(req): Json<CreateCommentRequest>,
) -> Response {
    let media_ids: Vec<&str> = req.media.iter().map(|m| m.media_id.as_str()).collect();
    if let Err(resp) = validate_community_media_access_for_auth(&state, &auth, &media_ids) {
        return resp;
    }

    let res = state.db.pool().with_transaction(|tx| {
        let tx_now_ms = Utc::now().timestamp_millis();
        let post = ops::get_post_conn(tx, &post_id, Some(&auth.actor_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("Post '{post_id}' not found")))?;
        if !is_post_publicly_interactable(&post) {
            return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
        }

        check_bot_comment_loop_guard_tx(
            tx,
            &auth.actor,
            &post_id,
            req.parent_comment_id.as_deref(),
            false,
        )?;

        let reply_target_actor_id = if let Some(pid) = req
            .parent_comment_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            tx.query_row(
                "SELECT author_actor_id FROM community_comments WHERE comment_id = ?1",
                params![pid],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| TelegramError::Database(e.to_string()))?
        } else {
            Some(post.author_actor_id.clone())
        };

        let created = ops::create_comment_tx(tx, &auth.actor_id, &post_id, &req, tx_now_ms)?;
        Ok((post, created, reply_target_actor_id, tx_now_ms))
    });

    match res {
        Ok((post, created, reply_target_actor_id, tx_now_ms)) => {
            if !auth.actor.actor_kind.is_bot_like() {
                state.community_bot_guard.reset_on_human_turn(&post.post_id);
            }
            broadcast_community_event(
                &state,
                "comment_created",
                post.community_id.clone(),
                Some(post.post_id.clone()),
                Some(created.comment.comment_id.clone()),
                Some(auth.actor_id.clone()),
                &json!({
                    "comment_id": created.comment.comment_id,
                    "post_id": post.post_id,
                    "parent_comment_id": created.comment.parent_comment_id,
                    "author_actor_id": auth.actor_id,
                    "author_kind": auth.actor.actor_kind.as_str(),
                }),
            );
            dispatch_comment_bot_effects(
                &state,
                &post,
                &created.comment,
                &auth.actor,
                reply_target_actor_id.as_deref(),
                tx_now_ms,
            );
            (StatusCode::CREATED, Json(created)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn update_comment(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(comment_id): Path<String>,
    Json(req): Json<UpdateCommentRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let parent_meta: Option<(String, i64, Option<String>)> = tx
            .query_row(
                "SELECT p.status, p.is_removed, p.community_id
                 FROM community_comments c
                 JOIN community_posts p ON p.post_id = c.post_id
                 WHERE c.comment_id = ?1",
                params![&comment_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let Some((p_status_str, p_removed, comm_id)) = parent_meta else {
            return Err(TelegramError::NotFound(format!(
                "Comment '{comment_id}' not found"
            )));
        };
        let p_status = PostStatus::parse(&p_status_str).unwrap_or(PostStatus::Deleted);
        if p_status != PostStatus::Published || p_removed != 0 {
            return Err(TelegramError::NotFound(format!(
                "Comment '{comment_id}' not found"
            )));
        }
        let updated = ops::update_comment_tx(tx, &comment_id, &auth.actor_id, &req.body, now_ms)?;
        Ok((updated, comm_id))
    }) {
        Ok((updated, comm_id)) => {
            broadcast_community_event(
                &state,
                "comment_updated",
                comm_id,
                Some(updated.post_id.clone()),
                Some(updated.comment_id.clone()),
                Some(auth.actor_id),
                &json!({ "comment_id": updated.comment_id, "post_id": updated.post_id }),
            );
            (StatusCode::OK, Json(updated)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn delete_comment(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(comment_id): Path<String>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let meta: Option<(String, String, i64, i64, String, i64, Option<String>)> = tx
            .query_row(
                "SELECT c.post_id, c.author_actor_id, c.is_deleted, c.is_removed,
                        p.status, p.is_removed, p.community_id
                 FROM community_comments c
                 JOIN community_posts p ON p.post_id = c.post_id
                 WHERE c.comment_id = ?1",
                params![&comment_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let Some((post_id, c_author, c_del, c_rem, p_status_str, p_rem, comm_id)) = meta else {
            return Err(TelegramError::NotFound(format!(
                "Comment '{comment_id}' not found"
            )));
        };
        let p_status = PostStatus::parse(&p_status_str).unwrap_or(PostStatus::Deleted);
        if p_status == PostStatus::Deleted {
            return Err(TelegramError::NotFound(format!(
                "Comment '{comment_id}' not found"
            )));
        }
        let was_public =
            c_del == 0 && c_rem == 0 && p_status == PostStatus::Published && p_rem == 0;
        if !was_public && c_author != auth.actor_id {
            let is_mod = match comm_id.as_deref() {
                Some(cid) => ops::get_community_conn(tx, cid, Some(&auth.actor_id))?
                    .and_then(|c| c.viewer_role)
                    .is_some_and(CommunityMemberRole::is_moderator),
                None => false,
            };
            if !is_mod || matches!(p_status, PostStatus::Draft | PostStatus::Scheduled) {
                return Err(TelegramError::NotFound(format!(
                    "Comment '{comment_id}' not found"
                )));
            }
        }
        let deleted = ops::delete_comment_tx(tx, &comment_id, &auth.actor_id, now_ms)?;
        Ok((deleted, was_public, post_id, comm_id))
    }) {
        Ok((deleted, was_public, post_id, comm_id)) => {
            if deleted && was_public {
                broadcast_community_event(
                    &state,
                    "comment_deleted",
                    comm_id,
                    Some(post_id),
                    Some(comment_id.clone()),
                    Some(auth.actor_id),
                    &json!({ "comment_id": comment_id, "deleted": deleted }),
                );
            }
            (
                StatusCode::OK,
                Json(DeleteEntityResponse {
                    id: comment_id,
                    deleted,
                }),
            )
                .into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn vote_comment(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(comment_id): Path<String>,
    Json(req): Json<VoteRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let (post_id, comm_id) = ensure_comment_publicly_interactable_conn(tx, &comment_id)?;
        let resp = ops::vote_comment_tx(tx, &auth.actor_id, &comment_id, req.direction, now_ms)?;
        Ok((resp, post_id, comm_id))
    }) {
        Ok((resp, post_id, comm_id)) => {
            broadcast_community_event(
                &state,
                "vote_updated",
                comm_id,
                Some(post_id),
                Some(comment_id),
                Some(auth.actor_id),
                &json!({ "target_kind": "comment", "target_id": resp.target_id, "score": resp.score }),
            );
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn like_comment(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(comment_id): Path<String>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let (post_id, comm_id) = ensure_comment_publicly_interactable_conn(tx, &comment_id)?;
        let resp = ops::toggle_like_comment_tx(tx, &auth.actor_id, &comment_id, now_ms)?;
        Ok((resp, post_id, comm_id))
    }) {
        Ok((resp, post_id, comm_id)) => {
            broadcast_community_event(
                &state,
                "like_updated",
                comm_id,
                Some(post_id),
                Some(comment_id),
                Some(auth.actor_id),
                &json!({ "target_kind": "comment", "target_id": resp.target_id, "active": resp.active, "count": resp.count }),
            );
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn award_comment(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(comment_id): Path<String>,
    Json(req): Json<GiveAwardRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let (post_id, comm_id) = ensure_comment_publicly_interactable_conn(tx, &comment_id)?;
        let awards = ops::give_award_tx(tx, &auth.actor_id, "comment", &comment_id, &req, now_ms)?;
        Ok((awards, post_id, comm_id))
    }) {
        Ok((awards, post_id, comm_id)) => {
            broadcast_community_event(
                &state,
                "award_given",
                comm_id,
                Some(post_id),
                Some(comment_id.clone()),
                Some(auth.actor_id),
                &json!({ "target_kind": "comment", "target_id": comment_id }),
            );
            (
                StatusCode::OK,
                Json(AwardResponse {
                    target_id: comment_id,
                    awards,
                }),
            )
                .into_response()
        }
        Err(e) => map_community_err(e),
    }
}
