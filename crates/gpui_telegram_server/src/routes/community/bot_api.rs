//! Standard Telegram Bot API `/bot/{token}/community.*` and `/bot{token}/community.*` dispatcher.

use crate::db::miniapp_store::BotRegistryRecord;
use crate::routes::community::auth::ensure_bot_actor_synced;
use crate::routes::community::bot_dispatch::{
    check_bot_comment_loop_guard_tx, dispatch_comment_bot_effects, dispatch_post_bot_effects,
};
use crate::routes::community::lists_search::{
    execute_community_search, parse_search_sort, parse_search_tab,
};
use crate::routes::community::posts::{
    ensure_comment_publicly_interactable_conn, ensure_post_mod_target_allowed_conn,
    ensure_post_publicly_interactable_conn, is_post_publicly_interactable, load_post_detail_conn,
    sanitize_create_post_response_conn, sanitize_post_embedded_refs_conn,
    validate_create_post_references_conn,
};
use crate::routes::community::{
    authorized_media_user_ids_for_bot, broadcast_community_event, collect_post_media_ids,
    delete_post_in_search, sync_post_in_search, validate_community_media_access_db,
};
use crate::server::AppState;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use chrono::Utc;
use gpui_telegram_community::store::ops;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_proto::community_contracts::{
    ActorKind, CommentSort, CommunityNotificationDto, CommunityNotificationKind,
    CommunitySearchRequestDto, CreateCommentRequest, CreatePostRequest, FeedQueryParams,
    ModActionKind, ModActionRequest, NotificationsListResponse, PostFormat, PostStatus,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};
use std::sync::Arc;

fn bot_ok(result: Value) -> Response {
    (StatusCode::OK, Json(json!({ "ok": true, "result": result }))).into_response()
}

fn bot_err(status: StatusCode, error_code: i64, description: &str) -> Response {
    (
        status,
        Json(json!({
            "ok": false,
            "error_code": error_code,
            "description": description
        })),
    )
        .into_response()
}

fn bot_map_err(err: TelegramError) -> Response {
    match err {
        TelegramError::Auth(m) | TelegramError::AuthFailed(m) => {
            bot_err(StatusCode::UNAUTHORIZED, 401, &m)
        }
        TelegramError::NotFound(m) => bot_err(StatusCode::NOT_FOUND, 404, &m),
        TelegramError::PermissionDenied(m) => bot_err(StatusCode::FORBIDDEN, 403, &m),
        TelegramError::Validation(m) => {
            let lower = m.to_ascii_lowercase();
            if lower.contains("already exists") || lower.contains("already taken") {
                bot_err(StatusCode::CONFLICT, 409, &m)
            } else if lower.contains("poll is closed") || lower.contains("cannot edit a deleted") {
                bot_err(StatusCode::GONE, 410, &m)
            } else {
                bot_err(StatusCode::BAD_REQUEST, 400, &m)
            }
        }
        TelegramError::Database(m) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &m),
        other => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &other.to_string()),
    }
}

fn p_str<'a>(params: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some(s) = params
            .get(*k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(s);
        }
    }
    None
}

fn p_i64(params: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    for k in keys {
        if let Some(v) = params.get(*k) {
            if let Some(n) = v.as_i64() {
                return Some(n);
            }
            if let Some(s) = v.as_str().and_then(|s| s.trim().parse::<i64>().ok()) {
                return Some(s);
            }
        }
    }
    None
}

fn p_bool(params: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    for k in keys {
        if let Some(v) = params.get(*k) {
            if let Some(b) = v.as_bool() {
                return Some(b);
            }
            if let Some(n) = v.as_i64() {
                return Some(n != 0);
            }
            if let Some(s) = v.as_str() {
                match s.trim().to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" => return Some(true),
                    "false" | "0" | "no" => return Some(false),
                    _ => {}
                }
            }
        }
    }
    None
}

/// Dispatches all `/bot/{token}/community.<method>` and `/bot{token}/community.<method>` calls.
pub fn dispatch_community_bot_api(
    state: &Arc<AppState>,
    bot: &BotRegistryRecord,
    method_lc: &str,
    raw_method: &str,
    params: &Map<String, Value>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    let bot_actor = match ensure_bot_actor_synced(&state.db, bot, now_ms) {
        Ok(a) => a,
        Err(e) => return bot_map_err(e),
    };

    match method_lc {
        "community.getfeed" => handle_get_feed(state, &bot_actor.actor_id, params, now_ms),
        "community.getpost" => handle_get_post(state, &bot_actor.actor_id, params),
        "community.createpost" => handle_create_post(state, bot, &bot_actor, params, now_ms),
        "community.createcomment" => handle_create_comment(state, &bot_actor, params, now_ms),
        "community.vote" => handle_vote(state, &bot_actor.actor_id, params, now_ms),
        "community.like" => handle_like(state, &bot_actor.actor_id, params, now_ms),
        "community.repost" => handle_repost(state, &bot_actor.actor_id, params, now_ms),
        "community.quotepost" => handle_quote_post(state, bot, &bot_actor, params, now_ms),
        "community.search" => handle_search(state, &bot_actor.actor_id, params),
        "community.getmentions" => handle_get_mentions(state, &bot_actor.actor_id, params),
        "community.modaction" | "community.moderate" => {
            handle_mod_action(state, &bot_actor.actor_id, params, now_ms)
        }
        _ => bot_err(
            StatusCode::NOT_FOUND,
            404,
            &format!("Not Found: unknown method '{raw_method}'"),
        ),
    }
}

fn handle_get_feed(
    state: &Arc<AppState>,
    actor_id: &str,
    params: &Map<String, Value>,
    now_ms: i64,
) -> Response {
    let q = FeedQueryParams {
        tab: p_str(params, &["tab"]).map(str::to_string),
        sort: p_str(params, &["sort"]).map(str::to_string),
        time: p_str(params, &["time"]).map(str::to_string),
        actor_filter: p_str(params, &["actor_filter"]).map(str::to_string),
        community: p_str(params, &["community", "community_slug"]).map(str::to_string),
        list_id: p_str(params, &["list_id"]).map(str::to_string),
        actor: p_str(params, &["actor"]).map(str::to_string),
        flair: p_str(params, &["flair"]).map(str::to_string),
        cursor: p_str(params, &["cursor"]).map(str::to_string),
        limit: p_i64(params, &["limit"]).map(|n| n.clamp(1, 100) as usize),
    };
    let req = q.into_feed_request();
    match state.db.pool().with_reader(|conn| {
        let mut resp = ops::get_feed_conn(conn, actor_id, &req, now_ms)?;
        for item in &mut resp.items {
            sanitize_post_embedded_refs_conn(conn, &mut item.post);
        }
        Ok(resp)
    }) {
        Ok(resp) => bot_ok(serde_json::to_value(resp).unwrap_or_default()),
        Err(e) => bot_map_err(e),
    }
}

fn handle_get_post(
    state: &Arc<AppState>,
    actor_id: &str,
    params: &Map<String, Value>,
) -> Response {
    let Some(post_id) = p_str(params, &["post_id"]) else {
        return bot_err(StatusCode::BAD_REQUEST, 400, "Bad Request: post_id is required");
    };
    let sort = p_str(params, &["comment_sort", "sort"])
        .and_then(CommentSort::parse)
        .unwrap_or(CommentSort::Best);
    let max_depth = p_i64(params, &["max_depth"]).map(|d| d.clamp(1, 32) as u32);
    let limit = p_i64(params, &["limit", "comments_limit"])
        .unwrap_or(100)
        .clamp(1, 500) as usize;
    match state
        .db
        .pool()
        .with_reader(|conn| load_post_detail_conn(conn, post_id, Some(actor_id), sort, max_depth, limit))
    {
        Ok(resp) => bot_ok(serde_json::to_value(resp).unwrap_or_default()),
        Err(e) => bot_map_err(e),
    }
}

fn build_create_post_from_params(
    params: &Map<String, Value>,
    force_quote: bool,
) -> Result<CreatePostRequest, String> {
    let mut normalized = params.clone();
    if !normalized.contains_key("body") {
        if let Some(t) = normalized.get("text").cloned() {
            normalized.insert("body".to_string(), t);
        }
    }
    if !normalized.contains_key("community_slug") {
        if let Some(c) = normalized.get("community").cloned() {
            normalized.insert("community_slug".to_string(), c);
        }
    }
    if force_quote {
        if !normalized.contains_key("quoted_post_id") {
            if let Some(pid) = normalized.get("post_id").cloned() {
                normalized.insert("quoted_post_id".to_string(), pid);
            }
        }
        normalized.insert("format".to_string(), json!("quote"));
    }
    let mut req: CreatePostRequest = serde_json::from_value(Value::Object(normalized))
        .map_err(|e| format!("Bad Request: invalid post parameters: {e}"))?;
    if force_quote {
        req.format = PostFormat::Quote;
        if req
            .quoted_post_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
        {
            return Err("Bad Request: quoted_post_id (or post_id) is required".to_string());
        }
    }
    Ok(req)
}

fn handle_create_post_inner(
    state: &Arc<AppState>,
    bot: &BotRegistryRecord,
    bot_actor: &gpui_telegram_proto::community_contracts::CommunityActorDto,
    params: &Map<String, Value>,
    force_quote: bool,
    now_ms: i64,
) -> Response {
    let req = match build_create_post_from_params(params, force_quote) {
        Ok(r) => r,
        Err(msg) => return bot_err(StatusCode::BAD_REQUEST, 400, &msg),
    };
    let media_ids = collect_post_media_ids(&req);
    let allowed_uids = authorized_media_user_ids_for_bot(bot, bot_actor);
    if let Err(e) = validate_community_media_access_db(&state.db, &allowed_uids, &media_ids) {
        return bot_map_err(e);
    }
    let res = state.db.pool().with_transaction(|tx| {
        validate_create_post_references_conn(tx, &req)?;
        let mut created = ops::create_post_tx(tx, &bot_actor.actor_id, &req, now_ms)?;
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
                broadcast_community_event(
                    state,
                    "post_created",
                    created.post.community_id.clone(),
                    Some(created.post.post_id.clone()),
                    None,
                    Some(bot_actor.actor_id.clone()),
                    &json!({
                        "post_id": created.post.post_id,
                        "community_slug": created.post.community_slug,
                        "author_actor_id": bot_actor.actor_id,
                        "author_kind": bot_actor.actor_kind.as_str(),
                        "via": "bot_api",
                    }),
                );
                dispatch_post_bot_effects(state, &created.post, bot_actor, now_ms);
            }
            bot_ok(serde_json::to_value(created).unwrap_or_default())
        }
        Err(e) => bot_map_err(e),
    }
}

fn handle_create_post(
    state: &Arc<AppState>,
    bot: &BotRegistryRecord,
    bot_actor: &gpui_telegram_proto::community_contracts::CommunityActorDto,
    params: &Map<String, Value>,
    now_ms: i64,
) -> Response {
    handle_create_post_inner(state, bot, bot_actor, params, false, now_ms)
}

fn handle_quote_post(
    state: &Arc<AppState>,
    bot: &BotRegistryRecord,
    bot_actor: &gpui_telegram_proto::community_contracts::CommunityActorDto,
    params: &Map<String, Value>,
    now_ms: i64,
) -> Response {
    handle_create_post_inner(state, bot, bot_actor, params, true, now_ms)
}

fn handle_create_comment(
    state: &Arc<AppState>,
    bot_actor: &gpui_telegram_proto::community_contracts::CommunityActorDto,
    params: &Map<String, Value>,
    now_ms: i64,
) -> Response {
    let Some(post_id) = p_str(params, &["post_id"]) else {
        return bot_err(StatusCode::BAD_REQUEST, 400, "Bad Request: post_id is required");
    };
    let Some(body) = p_str(params, &["body", "text"]) else {
        return bot_err(StatusCode::BAD_REQUEST, 400, "Bad Request: body (or text) is required");
    };
    let parent_comment_id = p_str(params, &["parent_comment_id"]).map(str::to_string);
    let is_mod_distinguished = p_bool(params, &["is_mod_distinguished"]).unwrap_or(false);

    let req = CreateCommentRequest {
        parent_comment_id: parent_comment_id.clone(),
        body: body.to_string(),
        media: Vec::new(),
        is_mod_distinguished,
    };

    let res = state.db.pool().with_transaction(|tx| {
        let tx_now_ms = Utc::now().timestamp_millis().max(now_ms);
        let post = ops::get_post_conn(tx, post_id, Some(&bot_actor.actor_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("Post '{post_id}' not found")))?;
        if !is_post_publicly_interactable(&post) {
            return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
        }
        check_bot_comment_loop_guard_tx(
            tx,
            bot_actor,
            post_id,
            parent_comment_id.as_deref(),
            false,
        )?;
        let reply_target_actor_id = if let Some(pid) = parent_comment_id.as_deref() {
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
        let created = ops::create_comment_tx(tx, &bot_actor.actor_id, post_id, &req, tx_now_ms)?;
        Ok((post, created, reply_target_actor_id, tx_now_ms))
    });

    match res {
        Ok((post, created, reply_target_actor_id, tx_now_ms)) => {
            broadcast_community_event(
                state,
                "comment_created",
                post.community_id.clone(),
                Some(post.post_id.clone()),
                Some(created.comment.comment_id.clone()),
                Some(bot_actor.actor_id.clone()),
                &json!({
                    "comment_id": created.comment.comment_id,
                    "post_id": post.post_id,
                    "author_actor_id": bot_actor.actor_id,
                    "via": "bot_api",
                }),
            );
            dispatch_comment_bot_effects(
                state,
                &post,
                &created.comment,
                bot_actor,
                reply_target_actor_id.as_deref(),
                tx_now_ms,
            );
            bot_ok(serde_json::to_value(created).unwrap_or_default())
        }
        Err(e) => bot_map_err(e),
    }
}

fn resolve_target_post_or_comment<'a>(
    params: &'a Map<String, Value>,
) -> Result<(bool, &'a str), String> {
    if let Some(cid) = p_str(params, &["comment_id"]) {
        return Ok((true, cid));
    }
    if let Some(pid) = p_str(params, &["post_id"]) {
        return Ok((false, pid));
    }
    if let Some(tid) = p_str(params, &["target_id"]) {
        let is_comment = p_str(params, &["target_kind", "target_type"])
            .is_some_and(|k| k.trim().eq_ignore_ascii_case("comment"));
        return Ok((is_comment, tid));
    }
    Err("Bad Request: post_id, comment_id, or target_id is required".to_string())
}

fn handle_vote(
    state: &Arc<AppState>,
    actor_id: &str,
    params: &Map<String, Value>,
    now_ms: i64,
) -> Response {
    let Some(dir) = p_i64(params, &["direction"]).filter(|&d| (-1..=1).contains(&d)) else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: direction (-1, 0, or 1) is required",
        );
    };
    let (is_comment, target_id) = match resolve_target_post_or_comment(params) {
        Ok(v) => v,
        Err(msg) => return bot_err(StatusCode::BAD_REQUEST, 400, &msg),
    };

    let res = state.db.pool().with_transaction(|tx| {
        if is_comment {
            let _ = ensure_comment_publicly_interactable_conn(tx, target_id)?;
            ops::vote_comment_tx(tx, actor_id, target_id, dir as i8, now_ms)
        } else {
            let _ = ensure_post_publicly_interactable_conn(tx, target_id)?;
            ops::vote_post_tx(tx, actor_id, target_id, dir as i8, now_ms)
        }
    });
    match res {
        Ok(resp) => {
            if !is_comment {
                if let Ok(Some(refreshed)) = state
                    .db
                    .pool()
                    .with_reader(|conn| ops::get_post_conn(conn, target_id, None))
                {
                    sync_post_in_search(state, &refreshed);
                }
            }
            broadcast_community_event(
                state,
                "vote_updated",
                None,
                (!is_comment).then(|| target_id.to_string()),
                is_comment.then(|| target_id.to_string()),
                Some(actor_id.to_string()),
                &json!({ "target_id": resp.target_id, "score": resp.score }),
            );
            bot_ok(serde_json::to_value(resp).unwrap_or_default())
        }
        Err(e) => bot_map_err(e),
    }
}

fn handle_like(
    state: &Arc<AppState>,
    actor_id: &str,
    params: &Map<String, Value>,
    now_ms: i64,
) -> Response {
    let (is_comment, target_id) = match resolve_target_post_or_comment(params) {
        Ok(v) => v,
        Err(msg) => return bot_err(StatusCode::BAD_REQUEST, 400, &msg),
    };
    let res = state.db.pool().with_transaction(|tx| {
        if is_comment {
            let _ = ensure_comment_publicly_interactable_conn(tx, target_id)?;
            ops::toggle_like_comment_tx(tx, actor_id, target_id, now_ms)
        } else {
            let _ = ensure_post_publicly_interactable_conn(tx, target_id)?;
            ops::toggle_like_post_tx(tx, actor_id, target_id, now_ms)
        }
    });
    match res {
        Ok(resp) => {
            broadcast_community_event(
                state,
                "like_updated",
                None,
                (!is_comment).then(|| target_id.to_string()),
                is_comment.then(|| target_id.to_string()),
                Some(actor_id.to_string()),
                &json!({ "target_id": resp.target_id, "active": resp.active, "count": resp.count }),
            );
            bot_ok(serde_json::to_value(resp).unwrap_or_default())
        }
        Err(e) => bot_map_err(e),
    }
}

fn handle_repost(
    state: &Arc<AppState>,
    actor_id: &str,
    params: &Map<String, Value>,
    now_ms: i64,
) -> Response {
    let Some(post_id) = p_str(params, &["post_id"]) else {
        return bot_err(StatusCode::BAD_REQUEST, 400, "Bad Request: post_id is required");
    };
    match state.db.pool().with_transaction(|tx| {
        let _ = ensure_post_publicly_interactable_conn(tx, post_id)?;
        ops::toggle_repost_tx(tx, actor_id, post_id, now_ms)
    }) {
        Ok(resp) => {
            broadcast_community_event(
                state,
                "post_reposted",
                None,
                Some(post_id.to_string()),
                None,
                Some(actor_id.to_string()),
                &json!({ "post_id": resp.post_id, "reposted": resp.reposted, "reposts_count": resp.reposts_count }),
            );
            bot_ok(serde_json::to_value(resp).unwrap_or_default())
        }
        Err(e) => bot_map_err(e),
    }
}

fn handle_search(
    state: &Arc<AppState>,
    actor_id: &str,
    params: &Map<String, Value>,
) -> Response {
    let query = p_str(params, &["query", "q"]).unwrap_or("").to_string();
    let req = CommunitySearchRequestDto {
        query,
        tab: p_str(params, &["tab"])
            .map(parse_search_tab)
            .unwrap_or_default(),
        sort: p_str(params, &["sort"])
            .map(parse_search_sort)
            .unwrap_or_default(),
        community_slug: p_str(params, &["community_slug", "community"]).map(str::to_string),
        author_handle: p_str(params, &["author_handle", "author"]).map(str::to_string),
        flair: p_str(params, &["flair"]).map(str::to_string),
        actor_kind: p_str(params, &["actor_kind"]).and_then(ActorKind::parse),
        has_media: p_bool(params, &["has_media"]),
        has_poll: p_bool(params, &["has_poll"]),
        limit: p_i64(params, &["limit"]).unwrap_or(25).clamp(1, 50) as usize,
        offset: p_i64(params, &["offset"]).unwrap_or(0).max(0) as usize,
    };
    match execute_community_search(
        &state.db,
        Some(&state.community_search_indexer),
        Some(actor_id),
        &req,
    ) {
        Ok(resp) => bot_ok(serde_json::to_value(resp).unwrap_or_default()),
        Err(e) => bot_map_err(e),
    }
}

fn handle_get_mentions(
    state: &Arc<AppState>,
    actor_id: &str,
    params: &Map<String, Value>,
) -> Response {
    let limit = p_i64(params, &["limit"]).unwrap_or(50).clamp(1, 200);
    let unread_only = p_bool(params, &["unread_only"]).unwrap_or(false);
    let include_replies = p_bool(params, &["include_replies"]).unwrap_or(true);

    let list_sql: &'static str = match (include_replies, unread_only) {
        (true, false) => {
            "SELECT n.notif_id, n.recipient_actor_id, n.actor_id, a.handle, a.display_name,
                    a.actor_kind, n.kind, n.target_post_id, n.target_comment_id,
                    n.preview_text, n.is_read, n.created_at_ms
             FROM community_notifications n
             JOIN community_actors a ON a.actor_id = n.actor_id
             WHERE n.recipient_actor_id = ?1
               AND n.kind IN ('mention', 'bot_summon', 'reply', 'bot_reply')
             ORDER BY n.created_at_ms DESC, n.rowid DESC
             LIMIT ?2"
        }
        (true, true) => {
            "SELECT n.notif_id, n.recipient_actor_id, n.actor_id, a.handle, a.display_name,
                    a.actor_kind, n.kind, n.target_post_id, n.target_comment_id,
                    n.preview_text, n.is_read, n.created_at_ms
             FROM community_notifications n
             JOIN community_actors a ON a.actor_id = n.actor_id
             WHERE n.recipient_actor_id = ?1
               AND n.kind IN ('mention', 'bot_summon', 'reply', 'bot_reply')
               AND n.is_read = 0
             ORDER BY n.created_at_ms DESC, n.rowid DESC
             LIMIT ?2"
        }
        (false, false) => {
            "SELECT n.notif_id, n.recipient_actor_id, n.actor_id, a.handle, a.display_name,
                    a.actor_kind, n.kind, n.target_post_id, n.target_comment_id,
                    n.preview_text, n.is_read, n.created_at_ms
             FROM community_notifications n
             JOIN community_actors a ON a.actor_id = n.actor_id
             WHERE n.recipient_actor_id = ?1
               AND n.kind IN ('mention', 'bot_summon')
             ORDER BY n.created_at_ms DESC, n.rowid DESC
             LIMIT ?2"
        }
        (false, true) => {
            "SELECT n.notif_id, n.recipient_actor_id, n.actor_id, a.handle, a.display_name,
                    a.actor_kind, n.kind, n.target_post_id, n.target_comment_id,
                    n.preview_text, n.is_read, n.created_at_ms
             FROM community_notifications n
             JOIN community_actors a ON a.actor_id = n.actor_id
             WHERE n.recipient_actor_id = ?1
               AND n.kind IN ('mention', 'bot_summon')
               AND n.is_read = 0
             ORDER BY n.created_at_ms DESC, n.rowid DESC
             LIMIT ?2"
        }
    };
    let count_sql: &'static str = if include_replies {
        "SELECT COUNT(*) FROM community_notifications
         WHERE recipient_actor_id = ?1 AND is_read = 0
           AND kind IN ('mention', 'bot_summon', 'reply', 'bot_reply')"
    } else {
        "SELECT COUNT(*) FROM community_notifications
         WHERE recipient_actor_id = ?1 AND is_read = 0
           AND kind IN ('mention', 'bot_summon')"
    };

    match state.db.pool().with_reader(|conn| {
        let unread_count: i64 = conn
            .query_row(count_sql, params![actor_id], |r| r.get(0))
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let mut stmt = conn
            .prepare(list_sql)
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![actor_id, limit], |r| {
                let ak: String = r.get(5)?;
                let nk: String = r.get(6)?;
                Ok(CommunityNotificationDto {
                    notif_id: r.get(0)?,
                    recipient_actor_id: r.get(1)?,
                    actor_id: r.get(2)?,
                    actor_handle: r.get(3)?,
                    actor_display_name: r.get(4)?,
                    actor_kind: ActorKind::parse(&ak).unwrap_or(ActorKind::Human),
                    kind: CommunityNotificationKind::parse(&nk)
                        .unwrap_or(CommunityNotificationKind::System),
                    target_post_id: r.get(7)?,
                    target_comment_id: r.get(8)?,
                    preview_text: r.get(9)?,
                    is_read: r.get::<_, i64>(10)? != 0,
                    created_at_ms: r.get(11)?,
                })
            })
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let mut notifications = Vec::new();
        for r in rows {
            notifications.push(r.map_err(|e| TelegramError::Database(e.to_string()))?);
        }
        Ok(NotificationsListResponse {
            notifications,
            unread_count: unread_count.max(0) as u64,
        })
    }) {
        Ok(out) => bot_ok(serde_json::to_value(out).unwrap_or_default()),
        Err(e) => bot_map_err(e),
    }
}

fn handle_mod_action(
    state: &Arc<AppState>,
    actor_id: &str,
    params: &Map<String, Value>,
    now_ms: i64,
) -> Response {
    let Some(slug) = p_str(params, &["community_slug", "slug", "community"]) else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: community_slug (or slug) is required",
        );
    };
    let Some(action) = p_str(params, &["action"]).and_then(ModActionKind::parse) else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: valid moderation 'action' is required",
        );
    };
    let Some(target_kind) = p_str(params, &["target_kind"]) else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: target_kind is required",
        );
    };
    let Some(target_id) = p_str(params, &["target_id"]) else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: target_id is required",
        );
    };
    let target_kind = target_kind.trim().to_ascii_lowercase();
    let req = ModActionRequest {
        action,
        target_kind: target_kind.clone(),
        target_id: target_id.to_string(),
        reason: p_str(params, &["reason"]).map(str::to_string),
        flair: p_str(params, &["flair"]).map(str::to_string),
        queue_item_id: p_str(params, &["queue_item_id"]).map(str::to_string),
    };

    match state.db.pool().with_transaction(|tx| {
        if target_kind == "post" {
            ensure_post_mod_target_allowed_conn(tx, &req.target_id)?;
        }
        ops::apply_mod_action_tx(tx, slug, actor_id, &req, now_ms)
    }) {
        Ok(entry) => {
            if target_kind == "post" {
                match req.action {
                    ModActionKind::Remove => delete_post_in_search(state, &req.target_id),
                    ModActionKind::Approve | ModActionKind::SetPostFlair => {
                        if let Ok(Some(post)) = state
                            .db
                            .pool()
                            .with_reader(|conn| ops::get_post_conn(conn, &req.target_id, None))
                        {
                            sync_post_in_search(state, &post);
                        }
                    }
                    _ => {}
                }
            }
            broadcast_community_event(
                state,
                "mod_action",
                Some(entry.community_id.clone()),
                (target_kind == "post").then(|| req.target_id.clone()),
                (target_kind == "comment").then(|| req.target_id.clone()),
                Some(actor_id.to_string()),
                &json!({
                    "log_id": entry.log_id,
                    "action": entry.action.as_str(),
                    "target_kind": entry.target_kind,
                    "target_id": entry.target_id,
                }),
            );
            bot_ok(serde_json::to_value(entry).unwrap_or_default())
        }
        Err(e) => bot_map_err(e),
    }
}
