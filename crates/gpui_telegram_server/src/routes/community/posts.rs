//! Community post CRUD, voting, engagement, polls, awards, and summon-bot endpoints.

use crate::routes::community::bot_dispatch::{
    backfill_post_mention_notifications_tx, check_bot_comment_loop_guard,
    check_bot_comment_loop_guard_tx, dispatch_post_bot_effects, enqueue_bot_mention_updates,
    filter_eligible_bots_for_community_conn, generate_bot_reply_text,
};
use crate::routes::community::{
    broadcast_community_event, collect_post_media_ids, delete_post_in_search, map_community_err,
    sync_post_in_search, validate_community_media_access_for_auth, ActorAuth,
};
use crate::routes::err_resp;
use crate::server::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use chrono::Utc;
use gpui_telegram_community::bot_runtime::BotGuardRejection;
use gpui_telegram_community::store::ops;
use gpui_telegram_community::thread::ThreadAssembler;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_proto::community_contracts::{
    ActorKind, AwardResponse, BookmarkRequest, BotAutonomyMode, CommentSort, CommunityActorDto,
    CommunityMemberRole, CommunityPostDto, CommunityPollVoteRequest, CommentsQueryParams,
    CreateCommentRequest, CreatePostRequest, CreatePostResponse, DeleteEntityResponse,
    GiveAwardRequest, PostDetailResponse, PostStatus, RepostRequest, SummonBotRequest,
    SummonBotResponse, UpdatePostRequest, VoteRequest,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

/// Checks if `viewer_actor_id` is allowed to read `post` (hides Draft/Scheduled/Deleted/Removed from outsiders).
pub fn is_post_visible_to_viewer_conn(
    conn: &Connection,
    post: &CommunityPostDto,
    viewer_actor_id: Option<&str>,
) -> bool {
    if post.status == PostStatus::Deleted {
        return false;
    }
    let is_author = viewer_actor_id.is_some_and(|v| v == post.author_actor_id);
    if matches!(post.status, PostStatus::Draft | PostStatus::Scheduled) && !is_author {
        return false;
    }
    if post.is_removed && !is_author {
        let is_mod = match (post.community_id.as_deref(), viewer_actor_id) {
            (Some(cid), Some(vid)) => ops::get_community_conn(conn, cid, Some(vid))
                .ok()
                .flatten()
                .and_then(|c| c.viewer_role)
                .is_some_and(CommunityMemberRole::is_moderator),
            _ => false,
        };
        if !is_mod {
            return false;
        }
    }
    true
}

/// Returns `true` iff a post is publicly published and not removed.
pub fn is_post_publicly_interactable(post: &CommunityPostDto) -> bool {
    post.status == PostStatus::Published && !post.is_removed
}

/// Fast PK check inside a reader or write `tx` (eliminates TOCTOU).
/// Returns `Ok(community_id)` iff post exists with `status == 'published'` AND `is_removed == 0`.
/// Returns `Err(TelegramError::NotFound)` for Draft, Scheduled, Deleted, Removed, or nonexistent.
pub fn ensure_post_publicly_interactable_conn(
    conn: &Connection,
    post_id: &str,
) -> Result<Option<String>, TelegramError> {
    let row: Option<(String, i64, Option<String>)> = conn
        .query_row(
            "SELECT status, is_removed, community_id FROM community_posts WHERE post_id = ?1",
            params![post_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    let Some((status_str, is_removed, comm_id)) = row else {
        return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
    };
    let status = PostStatus::parse(&status_str).unwrap_or(PostStatus::Deleted);
    if status != PostStatus::Published || is_removed != 0 {
        return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
    }
    Ok(comm_id)
}

/// Rejects any `quoted_post_id`, `reposted_post_id`, or `crosspost_of_id` targeting a non-public post.
pub fn validate_create_post_references_conn(
    conn: &Connection,
    req: &CreatePostRequest,
) -> Result<(), TelegramError> {
    for raw_ref in [
        req.quoted_post_id.as_deref(),
        req.reposted_post_id.as_deref(),
        req.crosspost_of_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if !raw_ref.trim().is_empty() {
            ensure_post_publicly_interactable_conn(conn, raw_ref)?;
        }
    }
    Ok(())
}

/// Rejects voting/liking/awarding a deleted/removed comment OR any comment on a non-public post.
/// Returns `Ok((post_id, community_id))`.
pub fn ensure_comment_publicly_interactable_conn(
    conn: &Connection,
    comment_id: &str,
) -> Result<(String, Option<String>), TelegramError> {
    let row: Option<(String, i64, i64, String, i64, Option<String>)> = conn
        .query_row(
            "SELECT c.post_id, c.is_deleted, c.is_removed, p.status, p.is_removed, p.community_id
             FROM community_comments c
             JOIN community_posts p ON p.post_id = c.post_id
             WHERE c.comment_id = ?1",
            params![comment_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    let Some((post_id, c_del, c_rem, p_status_str, p_rem, comm_id)) = row else {
        return Err(TelegramError::NotFound(format!(
            "Comment '{comment_id}' not found"
        )));
    };
    let p_status = PostStatus::parse(&p_status_str).unwrap_or(PostStatus::Deleted);
    if c_del != 0 || c_rem != 0 || p_status != PostStatus::Published || p_rem != 0 {
        return Err(TelegramError::NotFound(format!(
            "Comment '{comment_id}' not found"
        )));
    }
    Ok((post_id, comm_id))
}

/// Strips embedded quote/repost/crosspost summaries if the referenced post was later deleted/removed/unpublished.
pub fn sanitize_post_embedded_refs_conn(conn: &Connection, post: &mut CommunityPostDto) {
    if post.quoted_post.is_some() {
        let keep = post
            .quoted_post_id
            .as_deref()
            .or_else(|| post.quoted_post.as_ref().map(|e| e.post_id.as_str()))
            .is_some_and(|qid| ensure_post_publicly_interactable_conn(conn, qid).is_ok());
        if !keep {
            post.quoted_post = None;
        }
    }
    if post.reposted_post.is_some() {
        let keep = post
            .reposted_post_id
            .as_deref()
            .or_else(|| post.reposted_post.as_ref().map(|e| e.post_id.as_str()))
            .is_some_and(|rid| ensure_post_publicly_interactable_conn(conn, rid).is_ok());
        if !keep {
            post.reposted_post = None;
        }
    }
    if post.crosspost_origin.is_some() {
        let keep = post
            .crosspost_of_id
            .as_deref()
            .or_else(|| post.crosspost_origin.as_ref().map(|e| e.post_id.as_str()))
            .is_some_and(|xid| ensure_post_publicly_interactable_conn(conn, xid).is_ok());
        if !keep {
            post.crosspost_origin = None;
        }
    }
}

/// Sanitizes embedded post references across a root created post and all thread parts.
pub fn sanitize_create_post_response_conn(conn: &Connection, resp: &mut CreatePostResponse) {
    sanitize_post_embedded_refs_conn(conn, &mut resp.post);
    for tp in &mut resp.thread_posts {
        sanitize_post_embedded_refs_conn(conn, tp);
    }
}

/// Blocks mod actions (`Pin`, `Approve`, `SetPostFlair`, etc.) on Draft/Scheduled/Deleted posts
/// while STILL allowing `Published` posts with `is_removed = 1` (so mods can `Approve` removed posts).
pub fn ensure_post_mod_target_allowed_conn(
    conn: &Connection,
    post_id: &str,
) -> Result<(), TelegramError> {
    let status_str: Option<String> = conn
        .query_row(
            "SELECT status FROM community_posts WHERE post_id = ?1",
            params![post_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    let Some(st) = status_str.as_deref().and_then(PostStatus::parse) else {
        return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
    };
    if st != PostStatus::Published {
        return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
    }
    Ok(())
}

/// Loads `PostDetailResponse` including thread chain, quote chain, and nested comment tree.
pub fn load_post_detail_conn(
    conn: &Connection,
    post_id: &str,
    viewer_actor_id: Option<&str>,
    sort: CommentSort,
    max_depth: Option<u32>,
    limit: usize,
) -> Result<PostDetailResponse, TelegramError> {
    let mut post = ops::get_post_conn(conn, post_id, viewer_actor_id)?
        .ok_or_else(|| TelegramError::NotFound(format!("Post '{post_id}' not found")))?;
    if !is_post_visible_to_viewer_conn(conn, &post, viewer_actor_id) {
        return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
    }
    sanitize_post_embedded_refs_conn(conn, &mut post);

    let mut pool_ids = Vec::new();
    let root_id = post.thread_root_id.as_deref().unwrap_or(&post.post_id);
    {
        let mut stmt = conn
            .prepare(
                "SELECT post_id FROM community_posts
                 WHERE post_id = ?1 OR thread_root_id = ?1
                 ORDER BY thread_index ASC",
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![root_id], |r| r.get::<_, String>(0))
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        for r in rows {
            pool_ids.push(r.map_err(|e| TelegramError::Database(e.to_string()))?);
        }
    }

    let mut cur_q = post.quoted_post_id.clone();
    let mut walked = HashMap::<String, bool>::new();
    for _ in 0..8 {
        let Some(qid) = cur_q else { break };
        if walked.insert(qid.clone(), true).is_some() {
            break;
        }
        pool_ids.push(qid.clone());
        cur_q = conn
            .query_row(
                "SELECT quote_post_id FROM community_posts WHERE post_id = ?1",
                params![qid],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|e| TelegramError::Database(e.to_string()))?
            .flatten();
    }

    let mut pool = Vec::new();
    let mut seen = HashMap::<String, bool>::new();
    for pid in pool_ids {
        if seen.insert(pid.clone(), true).is_none() {
            if let Some(mut p) = ops::get_post_conn(conn, &pid, viewer_actor_id)? {
                if is_post_visible_to_viewer_conn(conn, &p, viewer_actor_id) {
                    sanitize_post_embedded_refs_conn(conn, &mut p);
                    pool.push(p);
                }
            }
        }
    }

    let mut thread_chain = ThreadAssembler::build_linear_thread(&post, &pool);
    for p in &mut thread_chain {
        sanitize_post_embedded_refs_conn(conn, p);
    }
    let mut quote_chain = ThreadAssembler::build_quote_chain(&post, &pool, 8);
    for p in &mut quote_chain {
        sanitize_post_embedded_refs_conn(conn, p);
    }
    let comments =
        ops::list_comment_tree_conn(conn, &post.post_id, viewer_actor_id, sort, max_depth, limit)?;
    let total_comments = post.comments_count;

    Ok(PostDetailResponse {
        post,
        thread_chain,
        quote_chain,
        comments,
        total_comments,
    })
}

pub async fn create_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreatePostRequest>,
) -> Response {
    let media_ids = collect_post_media_ids(&req);
    if let Err(resp) = validate_community_media_access_for_auth(&state, &auth, &media_ids) {
        return resp;
    }

    let now_ms = Utc::now().timestamp_millis();
    let res = state.db.pool().with_transaction(|tx| {
        validate_create_post_references_conn(tx, &req)?;
        let mut created = ops::create_post_tx(tx, &auth.actor_id, &req, now_ms)?;
        sanitize_create_post_response_conn(tx, &mut created);
        Ok(created)
    });

    match res {
        Ok(created) => {
            if created.post.status == PostStatus::Published && !created.post.is_removed {
                let _ = state.community_search_indexer.index_post(&created.post);
                for tp in &created.thread_posts {
                    if tp.status == PostStatus::Published && !tp.is_removed {
                        let _ = state.community_search_indexer.index_post(tp);
                    }
                }
                let _ = state.community_search_indexer.commit();

                if !auth.actor.actor_kind.is_bot_like() {
                    state
                        .community_bot_guard
                        .reset_on_human_turn(&created.post.post_id);
                }

                broadcast_community_event(
                    &state,
                    "post_created",
                    created.post.community_id.clone(),
                    Some(created.post.post_id.clone()),
                    None,
                    Some(auth.actor_id.clone()),
                    &json!({
                        "post_id": created.post.post_id,
                        "community_slug": created.post.community_slug,
                        "author_actor_id": auth.actor_id,
                        "author_kind": auth.actor.actor_kind.as_str(),
                        "format": created.post.format.as_str(),
                    }),
                );
                dispatch_post_bot_effects(&state, &created.post, &auth.actor, now_ms);
            }
            (StatusCode::CREATED, Json(created)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn get_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
    Query(query): Query<CommentsQueryParams>,
) -> Response {
    let sort = query.sort.unwrap_or(CommentSort::Best);
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    match state.db.pool().with_reader(|conn| {
        load_post_detail_conn(
            conn,
            &post_id,
            Some(&auth.actor_id),
            sort,
            query.max_depth,
            limit,
        )
    }) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn update_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
    Json(req): Json<UpdatePostRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let prior: Option<(String, i64, String)> = tx
            .query_row(
                "SELECT status, is_removed, author_actor_id FROM community_posts WHERE post_id = ?1",
                params![&post_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let Some((prior_status_str, prior_removed, prior_author)) = prior else {
            return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
        };
        let prior_status = PostStatus::parse(&prior_status_str).unwrap_or(PostStatus::Deleted);
        if matches!(prior_status, PostStatus::Draft | PostStatus::Scheduled)
            && prior_author != auth.actor_id
        {
            return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
        }
        let was_public = prior_status == PostStatus::Published && prior_removed == 0;
        let mut updated = ops::update_post_tx(tx, &post_id, &auth.actor_id, &req, now_ms)?;
        sanitize_post_embedded_refs_conn(tx, &mut updated);
        if !was_public && is_post_publicly_interactable(&updated) {
            backfill_post_mention_notifications_tx(tx, &updated, now_ms)?;
        }
        Ok((updated, was_public))
    }) {
        Ok((updated, was_public)) => {
            sync_post_in_search(&state, &updated);
            if is_post_publicly_interactable(&updated) {
                broadcast_community_event(
                    &state,
                    "post_updated",
                    updated.community_id.clone(),
                    Some(updated.post_id.clone()),
                    None,
                    Some(auth.actor_id.clone()),
                    &json!({ "post_id": updated.post_id, "status": updated.status.as_str() }),
                );
                if !was_public {
                    if !auth.actor.actor_kind.is_bot_like() {
                        state
                            .community_bot_guard
                            .reset_on_human_turn(&updated.post_id);
                    }
                    dispatch_post_bot_effects(&state, &updated, &auth.actor, now_ms);
                }
            }
            (StatusCode::OK, Json(updated)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn delete_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let prior: Option<(String, i64, String, Option<String>)> = tx
            .query_row(
                "SELECT status, is_removed, author_actor_id, community_id
                 FROM community_posts WHERE post_id = ?1",
                params![&post_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let Some((prior_status_str, prior_removed, prior_author, community_id)) = prior else {
            return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
        };
        let prior_status = PostStatus::parse(&prior_status_str).unwrap_or(PostStatus::Deleted);
        if matches!(prior_status, PostStatus::Draft | PostStatus::Scheduled)
            && prior_author != auth.actor_id
        {
            return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
        }
        let was_public = prior_status == PostStatus::Published && prior_removed == 0;
        let deleted = ops::delete_post_tx(tx, &post_id, &auth.actor_id, now_ms)?;
        Ok((deleted, was_public, community_id))
    }) {
        Ok((deleted, was_public, community_id)) => {
            delete_post_in_search(&state, &post_id);
            if deleted && was_public {
                broadcast_community_event(
                    &state,
                    "post_deleted",
                    community_id,
                    Some(post_id.clone()),
                    None,
                    Some(auth.actor_id),
                    &json!({ "post_id": post_id, "deleted": deleted }),
                );
            }
            (
                StatusCode::OK,
                Json(DeleteEntityResponse {
                    id: post_id,
                    deleted,
                }),
            )
                .into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn vote_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
    Json(req): Json<VoteRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let comm_id = ensure_post_publicly_interactable_conn(tx, &post_id)?;
        let resp = ops::vote_post_tx(tx, &auth.actor_id, &post_id, req.direction, now_ms)?;
        Ok((resp, comm_id))
    }) {
        Ok((resp, comm_id)) => {
            if let Ok(Some(refreshed)) = state
                .db
                .pool()
                .with_reader(|conn| ops::get_post_conn(conn, &post_id, None))
            {
                sync_post_in_search(&state, &refreshed);
            }
            broadcast_community_event(
                &state,
                "vote_updated",
                comm_id,
                Some(post_id),
                None,
                Some(auth.actor_id),
                &json!({ "target_kind": "post", "target_id": resp.target_id, "score": resp.score }),
            );
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn like_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let comm_id = ensure_post_publicly_interactable_conn(tx, &post_id)?;
        let resp = ops::toggle_like_post_tx(tx, &auth.actor_id, &post_id, now_ms)?;
        Ok((resp, comm_id))
    }) {
        Ok((resp, comm_id)) => {
            broadcast_community_event(
                &state,
                "like_updated",
                comm_id,
                Some(post_id),
                None,
                Some(auth.actor_id),
                &json!({ "target_kind": "post", "target_id": resp.target_id, "active": resp.active, "count": resp.count }),
            );
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn repost_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
    body: Bytes,
) -> Response {
    let _req: RepostRequest = if body.is_empty() {
        RepostRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_json",
                    &format!("Invalid repost payload: {e}"),
                )
            }
        }
    };
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let comm_id = ensure_post_publicly_interactable_conn(tx, &post_id)?;
        let resp = ops::toggle_repost_tx(tx, &auth.actor_id, &post_id, now_ms)?;
        Ok((resp, comm_id))
    }) {
        Ok((resp, comm_id)) => {
            broadcast_community_event(
                &state,
                "post_reposted",
                comm_id,
                Some(post_id),
                None,
                Some(auth.actor_id),
                &json!({ "post_id": resp.post_id, "reposted": resp.reposted, "reposts_count": resp.reposts_count }),
            );
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn bookmark_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: BookmarkRequest = if body.is_empty() {
        BookmarkRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_json",
                    &format!("Invalid bookmark payload: {e}"),
                )
            }
        }
    };
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let already_bookmarked: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM community_bookmarks WHERE actor_id = ?1 AND post_id = ?2)",
                params![&auth.actor_id, &post_id],
                |r| r.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let is_removing_existing =
            already_bookmarked && req.remove.unwrap_or(req.folder.is_none());
        if !is_removing_existing {
            ensure_post_publicly_interactable_conn(tx, &post_id)?;
        }
        ops::toggle_bookmark_tx(
            tx,
            &auth.actor_id,
            &post_id,
            req.folder.as_deref(),
            req.remove,
            now_ms,
        )
    }) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn poll_vote_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
    Json(req): Json<CommunityPollVoteRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let comm_id = ensure_post_publicly_interactable_conn(tx, &post_id)?;
        let poll = ops::vote_poll_tx(tx, &auth.actor_id, &post_id, &req.option_ids, now_ms)?;
        Ok((poll, comm_id))
    }) {
        Ok((poll, comm_id)) => {
            broadcast_community_event(
                &state,
                "poll_voted",
                comm_id,
                Some(post_id.clone()),
                None,
                Some(auth.actor_id),
                &json!({ "post_id": post_id, "total_votes": poll.total_votes }),
            );
            (StatusCode::OK, Json(poll)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn award_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
    Json(req): Json<GiveAwardRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.pool().with_transaction(|tx| {
        let comm_id = ensure_post_publicly_interactable_conn(tx, &post_id)?;
        let awards = ops::give_award_tx(tx, &auth.actor_id, "post", &post_id, &req, now_ms)?;
        Ok((awards, comm_id))
    }) {
        Ok((awards, comm_id)) => {
            broadcast_community_event(
                &state,
                "award_given",
                comm_id,
                Some(post_id.clone()),
                None,
                Some(auth.actor_id),
                &json!({ "target_kind": "post", "target_id": post_id }),
            );
            (
                StatusCode::OK,
                Json(AwardResponse {
                    target_id: post_id,
                    awards,
                }),
            )
                .into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn summon_bot_on_post(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(post_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: SummonBotRequest = if body.is_empty() {
        SummonBotRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_json",
                    &format!("Invalid summon-bot payload: {e}"),
                )
            }
        }
    };

    let prep = state.db.pool().with_reader(|conn| {
        let post = ops::get_post_conn(conn, &post_id, Some(&auth.actor_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("Post '{post_id}' not found")))?;
        if !is_post_publicly_interactable(&post) {
            return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
        }
        if post.is_locked {
            return Err(TelegramError::PermissionDenied(
                "Cannot summon a bot onto a locked post".into(),
            ));
        }

        let community = if let Some(cid) = post.community_id.as_deref() {
            let comm = ops::get_community_conn(conn, cid, None)?;
            if let Some(ref c) = comm {
                if !c.allow_bots {
                    return Err(TelegramError::PermissionDenied(format!(
                        "Bots are disabled in c/{}",
                        c.slug
                    )));
                }
            }
            comm
        } else {
            None
        };

        let target_bot: CommunityActorDto = if let Some(ref id_or_h) = req
            .bot_actor_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| req.bot_handle.as_deref().filter(|s| !s.trim().is_empty()))
        {
            let found = ops::get_actor_conn(conn, id_or_h, None)?
                .ok_or_else(|| TelegramError::NotFound(format!("Bot '{id_or_h}' not found")))?;
            if !found.actor_kind.is_bot_like() {
                return Err(TelegramError::Validation(format!(
                    "Actor '{}' is not a Bot citizen",
                    found.handle
                )));
            }
            let is_suspended: i64 = conn
                .query_row(
                    "SELECT is_suspended FROM community_actors WHERE actor_id = ?1",
                    params![&found.actor_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| TelegramError::Database(e.to_string()))?
                .unwrap_or(0);
            if is_suspended != 0 {
                return Err(TelegramError::PermissionDenied(format!(
                    "Bot @{} is suspended and cannot be summoned",
                    found.handle
                )));
            }
            found
        } else {
            let mut bots = ops::list_actors_conn(conn, Some(ActorKind::Bot), 50)?;
            bots.extend(ops::list_actors_conn(conn, Some(ActorKind::HybridAgent), 50)?);
            let eligible = filter_eligible_bots_for_community_conn(conn, bots, community.as_ref())?;
            eligible
                .into_iter()
                .find(|b| {
                    b.actor_id != auth.actor_id
                        && b.bot_autonomy_mode != BotAutonomyMode::Manual
                })
                .ok_or_else(|| TelegramError::NotFound("No eligible bots available to summon".into()))?
        };

        if target_bot.bot_autonomy_mode == BotAutonomyMode::Manual {
            return Err(TelegramError::PermissionDenied(format!(
                "Bot @{} is in Manual mode and cannot be summoned automatically",
                target_bot.handle
            )));
        }

        if let Some(ref c) = community {
            let comm_for_bot = ops::get_community_conn(conn, &c.community_id, Some(&target_bot.actor_id))?;
            if matches!(
                comm_for_bot.and_then(|cm| cm.viewer_role),
                Some(CommunityMemberRole::Banned | CommunityMemberRole::Muted)
            ) {
                return Err(TelegramError::PermissionDenied(format!(
                    "Bot @{} is banned or muted in c/{}",
                    target_bot.handle, c.slug
                )));
            }
        }

        let comments =
            ops::list_comment_tree_conn(conn, &post.post_id, None, CommentSort::New, None, 100)?;
        Ok((post, target_bot, comments))
    });

    let (post, target_bot, comments) = match prep {
        Ok(v) => v,
        Err(e) => return map_community_err(e),
    };

    let explicit_human_summon = !auth.actor.actor_kind.is_bot_like();
    if let Err(e) = check_bot_comment_loop_guard(
        &state,
        &target_bot,
        &post.post_id,
        req.parent_comment_id.as_deref(),
        explicit_human_summon,
    ) {
        return map_community_err(e);
    }

    if !explicit_human_summon && target_bot.actor_id == auth.actor_id {
        return (
            StatusCode::OK,
            Json(SummonBotResponse {
                summoned_bot_actor_id: target_bot.actor_id,
                triggered: false,
                reason: "Guard declined summon: SelfTriggerBlocked".into(),
                created_comment: None,
            }),
        )
            .into_response();
    }

    let trigger_text = req
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|p| format!("@{} {p}", target_bot.handle))
        .unwrap_or_else(|| format!("@{} {}", target_bot.handle, post.body));

    // Strictly respect Stage-2 outbound turn review & duplicate-echo guard (zero fallback bypass).
    let Some(body_text) =
        generate_bot_reply_text(&state, &target_bot, &post, &comments, &trigger_text).await
    else {
        return (
            StatusCode::OK,
            Json(SummonBotResponse {
                summoned_bot_actor_id: target_bot.actor_id,
                triggered: false,
                reason: "Bot reply suppressed by outbound review or duplicate-echo guard".into(),
                created_comment: None,
            }),
        )
            .into_response();
    };

    let comment_req = CreateCommentRequest {
        parent_comment_id: req.parent_comment_id.clone(),
        body: body_text,
        media: Vec::new(),
        is_mod_distinguished: false,
    };

    let mut guard_rejection: Option<BotGuardRejection> = None;
    let commit_res = state.db.pool().with_transaction(|tx| {
        let tx_now_ms = Utc::now().timestamp_millis();
        let fresh_post = ops::get_post_conn(tx, &post.post_id, Some(&auth.actor_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("Post '{}' not found", post.post_id)))?;
        if !is_post_publicly_interactable(&fresh_post) {
            return Err(TelegramError::NotFound(format!(
                "Post '{}' not found",
                post.post_id
            )));
        }
        if fresh_post.is_locked {
            return Err(TelegramError::PermissionDenied(
                "Cannot summon a bot onto a locked post".into(),
            ));
        }
        check_bot_comment_loop_guard_tx(
            tx,
            &target_bot,
            &post.post_id,
            comment_req.parent_comment_id.as_deref(),
            explicit_human_summon,
        )?;
        let created =
            ops::create_comment_tx(tx, &target_bot.actor_id, &post.post_id, &comment_req, tx_now_ms)?;
        if let Err(rej) = state.community_bot_guard.try_acquire_turn(
            &post.post_id,
            &target_bot.actor_id,
            &auth.actor_id,
            auth.actor.actor_kind,
            explicit_human_summon,
            tx_now_ms,
        ) {
            guard_rejection = Some(rej);
            return Err(TelegramError::Validation("bot_guard_declined".into()));
        }
        Ok((created, tx_now_ms))
    });

    if let Some(rej) = guard_rejection {
        return (
            StatusCode::OK,
            Json(SummonBotResponse {
                summoned_bot_actor_id: target_bot.actor_id,
                triggered: false,
                reason: format!("Guard declined summon: {rej:?}"),
                created_comment: None,
            }),
        )
            .into_response();
    }

    match commit_res {
        Ok((created, tx_now_ms)) => {
            enqueue_bot_mention_updates(
                &state,
                &trigger_text,
                &auth.actor,
                &post.post_id,
                req.parent_comment_id.as_deref(),
                post.community_slug.as_deref(),
                Some(&target_bot.actor_id),
                tx_now_ms,
            );
            broadcast_community_event(
                &state,
                "comment_created",
                post.community_id.clone(),
                Some(post.post_id.clone()),
                Some(created.comment.comment_id.clone()),
                Some(target_bot.actor_id.clone()),
                &json!({
                    "comment_id": created.comment.comment_id,
                    "post_id": post.post_id,
                    "author_actor_id": target_bot.actor_id,
                    "summoned_by": auth.actor_id,
                }),
            );
            (
                StatusCode::OK,
                Json(SummonBotResponse {
                    summoned_bot_actor_id: target_bot.actor_id,
                    triggered: true,
                    reason: "Summoned bot replied".into(),
                    created_comment: Some(created.comment),
                }),
            )
                .into_response()
        }
        Err(e) => map_community_err(e),
    }
}
