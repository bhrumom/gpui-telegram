//! Modular Axum REST API for `/api/v1/community/*` (Reddit + X Fused Community).

pub mod actors;
pub mod auth;
pub mod bot_api;
pub mod bot_dispatch;
pub mod comments;
pub mod communities;
pub mod feed;
pub mod hermes_bridge;
pub mod lists_search;
pub mod posts;

use crate::db::miniapp_store::BotRegistryRecord;
use crate::db::ServerDatabase;
use crate::routes::{err_resp, map_telegram_err};
use crate::server::AppState;
use crate::ws::GatewayHub;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, patch, post, put};
use axum::Router;
use chrono::Utc;
use gpui_telegram_community::CommunitySearchIndexer;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::community_contracts::{
    CommunityActorDto, CommunityPostDto, CreatePostRequest, PostStatus,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use serde_json::Value;
use std::sync::Arc;

pub use auth::{
    ensure_bot_actor_synced, ensure_bot_actor_synced_tx, sync_telegram_identities_for_user,
    ActorAuth, ActorAuthMode,
};
pub use bot_dispatch::{
    check_bot_comment_loop_guard, dispatch_comment_bot_effects, dispatch_post_bot_effects,
    run_community_background_tick,
};
pub use lists_search::execute_community_search;
pub use posts::{
    ensure_comment_publicly_interactable_conn, ensure_post_mod_target_allowed_conn,
    ensure_post_publicly_interactable_conn, is_post_publicly_interactable, load_post_detail_conn,
    sanitize_create_post_response_conn, sanitize_post_embedded_refs_conn,
    validate_create_post_references_conn,
};

/// Mounts all 41 `/api/v1/community/*` REST endpoints.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        // Identities & Actors
        .route("/api/v1/community/identities", get(actors::get_identities))
        .route(
            "/api/v1/community/identities/human",
            post(actors::register_human_identity),
        )
        .route(
            "/api/v1/community/identities/bot",
            post(actors::register_bot_identity),
        )
        .route(
            "/api/v1/community/actors/{handle_or_id}",
            get(actors::get_actor_profile).patch(actors::patch_actor_profile),
        )
        .route(
            "/api/v1/community/actors/{actor_id}/follow",
            post(actors::follow_actor).delete(actors::unfollow_actor),
        )
        .route(
            "/api/v1/community/mutes-blocks",
            get(actors::list_mutes_blocks)
                .post(actors::set_mute_block)
                .delete(actors::delete_mute_block),
        )
        // Feed, Algorithm Preferences & Signals
        .route("/api/v1/community/feed", get(feed::get_feed))
        .route(
            "/api/v1/community/algo-prefs",
            get(feed::get_algo_prefs).put(feed::put_algo_prefs),
        )
        .route("/api/v1/community/signals", post(feed::record_signal))
        // Posts & Interactions
        .route("/api/v1/community/posts", post(posts::create_post))
        .route(
            "/api/v1/community/posts/{post_id}",
            get(posts::get_post)
                .patch(posts::update_post)
                .delete(posts::delete_post),
        )
        .route("/api/v1/community/posts/{post_id}/vote", post(posts::vote_post))
        .route("/api/v1/community/posts/{post_id}/like", post(posts::like_post))
        .route(
            "/api/v1/community/posts/{post_id}/repost",
            post(posts::repost_post),
        )
        .route(
            "/api/v1/community/posts/{post_id}/bookmark",
            post(posts::bookmark_post),
        )
        .route(
            "/api/v1/community/posts/{post_id}/poll-vote",
            post(posts::poll_vote_post),
        )
        .route(
            "/api/v1/community/posts/{post_id}/award",
            post(posts::award_post),
        )
        .route(
            "/api/v1/community/posts/{post_id}/summon-bot",
            post(posts::summon_bot_on_post),
        )
        // Comments
        .route(
            "/api/v1/community/posts/{post_id}/comments",
            get(comments::list_comments).post(comments::create_comment),
        )
        .route(
            "/api/v1/community/comments/{comment_id}",
            patch(comments::update_comment).delete(comments::delete_comment),
        )
        .route(
            "/api/v1/community/comments/{comment_id}/vote",
            post(comments::vote_comment),
        )
        .route(
            "/api/v1/community/comments/{comment_id}/like",
            post(comments::like_comment),
        )
        .route(
            "/api/v1/community/comments/{comment_id}/award",
            post(comments::award_comment),
        )
        // Communities, Wiki & Moderation
        .route(
            "/api/v1/community/communities",
            get(communities::list_communities).post(communities::create_community),
        )
        .route(
            "/api/v1/community/communities/{slug}",
            get(communities::get_community).patch(communities::update_community),
        )
        .route(
            "/api/v1/community/communities/{slug}/join",
            post(communities::join_community).delete(communities::leave_community),
        )
        .route(
            "/api/v1/community/communities/{slug}/wiki",
            put(communities::update_community_wiki),
        )
        .route(
            "/api/v1/community/communities/{slug}/mod-queue",
            get(communities::get_mod_queue),
        )
        .route(
            "/api/v1/community/communities/{slug}/mod-actions",
            post(communities::apply_mod_action),
        )
        .route(
            "/api/v1/community/communities/{slug}/mod-logs",
            get(communities::get_mod_logs),
        )
        // Lists, Trending, Search & Notifications
        .route(
            "/api/v1/community/lists",
            get(lists_search::list_lists).post(lists_search::create_list),
        )
        .route(
            "/api/v1/community/lists/{list_id}",
            patch(lists_search::update_list).delete(lists_search::delete_list),
        )
        .route("/api/v1/community/trending", get(lists_search::get_trending))
        .route("/api/v1/community/search", get(lists_search::search_community))
        .route(
            "/api/v1/community/notifications",
            get(lists_search::list_notifications),
        )
        .route(
            "/api/v1/community/notifications/read",
            post(lists_search::mark_notifications_read),
        )
}

/// Maps `TelegramError` to community HTTP status codes while preserving the canonical JSON envelope.
pub fn map_community_err(err: TelegramError) -> Response {
    match &err {
        TelegramError::Validation(m) | TelegramError::Database(m) => {
            let lower = m.to_ascii_lowercase();
            if lower.contains("already exists")
                || lower.contains("already taken")
                || lower.contains("unique constraint failed")
                || lower.contains("nonce conflict")
            {
                return err_resp(StatusCode::CONFLICT, "conflict", m);
            }
            if lower.contains("poll is closed") || lower.contains("cannot edit a deleted") {
                return err_resp(StatusCode::GONE, "gone", m);
            }
        }
        _ => {}
    }
    map_telegram_err(err)
}

/// Broadcasts `UpdateEvent::CommunityEvent` to all local WS sockets and `InProcessEventBus`.
pub fn broadcast_community_event_hub(
    hub: &GatewayHub,
    event_kind: &str,
    community_id: Option<String>,
    post_id: Option<String>,
    comment_id: Option<String>,
    actor_id: Option<String>,
    payload: &Value,
) {
    let ev = UpdateEvent::CommunityEvent {
        event_kind: event_kind.to_string(),
        community_id,
        post_id,
        comment_id,
        actor_id,
        payload_json: serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string()),
        timestamp_ms: Utc::now().timestamp_millis(),
    };
    hub.broadcast_all("community", WsServerFrame::Update(ev));
}

pub fn broadcast_community_event(
    state: &AppState,
    event_kind: &str,
    community_id: Option<String>,
    post_id: Option<String>,
    comment_id: Option<String>,
    actor_id: Option<String>,
    payload: &Value,
) {
    broadcast_community_event_hub(
        &state.gateway_hub,
        event_kind,
        community_id,
        post_id,
        comment_id,
        actor_id,
        payload,
    );
}

/// Synchronizes a single post into a `CommunitySearchIndexer`: indexes ONLY when
/// `Published && !is_removed`, otherwise deletes from Tantivy, then commits immediately.
pub fn sync_post_in_indexer(idx: &CommunitySearchIndexer, post: &CommunityPostDto) {
    if post.status == PostStatus::Published && !post.is_removed {
        let _ = idx.index_post(post);
    } else {
        let _ = idx.delete_post(&post.post_id);
    }
    let _ = idx.commit();
}

/// Synchronizes a single post into Tantivy and commits immediately so search reflects writes.
pub fn sync_post_in_search(state: &AppState, post: &CommunityPostDto) {
    sync_post_in_indexer(&state.community_search_indexer, post);
}

/// Removes a post from Tantivy and commits immediately.
pub fn delete_post_in_search(state: &AppState, post_id: &str) {
    let _ = state.community_search_indexer.delete_post(post_id);
    let _ = state.community_search_indexer.commit();
}

/// Collects all `media_id`s referenced by a post and all of its X thread parts.
pub fn collect_post_media_ids(req: &CreatePostRequest) -> Vec<&str> {
    let mut ids: Vec<&str> = req.media.iter().map(|m| m.media_id.as_str()).collect();
    for part in &req.thread_parts {
        for m in &part.media {
            ids.push(m.media_id.as_str());
        }
    }
    ids
}

/// Computes authorized uploader `UserId`s for a resolved community actor plus optional verified human session.
/// Always filters `id > 0` so `UserId(0)` is never authorized.
pub fn authorized_media_user_ids_for_actor(
    db: &ServerDatabase,
    actor: &CommunityActorDto,
    session_user_id: Option<UserId>,
) -> Vec<UserId> {
    let mut ids = Vec::with_capacity(3);
    let mut push = |uid: UserId| {
        if uid.0 > 0 && !ids.contains(&uid) {
            ids.push(uid);
        }
    };
    if let Some(u) = session_user_id {
        push(u);
    }
    if let Some(linked) = actor.linked_user_id.filter(|&id| id > 0) {
        push(UserId(linked as u64));
        if actor.actor_kind.is_bot_like() || actor.actor_id.starts_with("b:") {
            if let Ok(Some(bot)) = db.get_bot_by_user_id(linked) {
                if bot.owner_user_id > 0 {
                    push(UserId(bot.owner_user_id as u64));
                }
            }
        }
    }
    if let Some(owner) = actor.owner_user_id.filter(|&id| id > 0) {
        push(UserId(owner as u64));
    }
    ids
}

/// Computes authorized uploader `UserId`s for a REST `ActorAuth` caller (Human, Act-As, or Direct Bot).
pub fn authorized_media_user_ids_for_auth(db: &ServerDatabase, auth: &ActorAuth) -> Vec<UserId> {
    let session_uid = match auth.mode {
        ActorAuthMode::HumanBearer | ActorAuthMode::ActAsOwnedActor => auth.caller_user_id,
        ActorAuthMode::DirectBotToken => None,
    };
    let mut ids = authorized_media_user_ids_for_actor(db, &auth.actor, session_uid);
    if let Some(ref bot) = auth.bot_record {
        for raw in [bot.bot_user_id, bot.owner_user_id] {
            if raw > 0 {
                let uid = UserId(raw as u64);
                if !ids.contains(&uid) {
                    ids.push(uid);
                }
            }
        }
    }
    ids
}

/// Computes authorized uploader `UserId`s for a Bot API caller (`[bot_user_id, owner_user_id]`).
pub fn authorized_media_user_ids_for_bot(
    bot: &BotRegistryRecord,
    bot_actor: &CommunityActorDto,
) -> Vec<UserId> {
    let mut ids = Vec::with_capacity(2);
    for raw in [
        Some(bot.bot_user_id),
        Some(bot.owner_user_id),
        bot_actor.linked_user_id,
        bot_actor.owner_user_id,
    ]
    .into_iter()
    .flatten()
    {
        if raw > 0 {
            let uid = UserId(raw as u64);
            if !ids.contains(&uid) {
                ids.push(uid);
            }
        }
    }
    ids
}

/// Fail-closed DB-backed media-hijack defense across REST, Bot API, and Hermes Bridge.
/// Rejects (`PermissionDenied` -> HTTP 403):
///   (a) any `media_id` in `media_files` with `chat_id IS NOT NULL` (even for the uploader), OR
///   (b) any `media_id` in `media_files` whose `uploader_id` is NOT in `caller_user_ids`.
/// Fail-closed on DB errors (`TelegramError::Database` -> 500).
pub fn validate_community_media_access_db(
    db: &ServerDatabase,
    caller_user_ids: &[UserId],
    media_ids: &[&str],
) -> Result<(), TelegramError> {
    for raw_id in media_ids {
        let mid = raw_id.trim();
        if mid.is_empty() {
            continue;
        }
        let Some((_, uploader, chat_opt)) = db.get_media_file(mid)? else {
            continue;
        };
        if chat_opt.is_some() {
            return Err(TelegramError::PermissionDenied(
                "Private chat media cannot be attached to public community content".into(),
            ));
        }
        if !caller_user_ids.contains(&uploader) {
            return Err(TelegramError::PermissionDenied(
                "Caller is not authorized to attach this private or unattached media file".into(),
            ));
        }
    }
    Ok(())
}

/// REST convenience wrapper mapping `PermissionDenied` -> `403 Forbidden` JSON envelope.
pub fn validate_community_media_access_for_auth(
    state: &AppState,
    auth: &ActorAuth,
    media_ids: &[&str],
) -> Result<(), Response> {
    let allowed = authorized_media_user_ids_for_auth(&state.db, auth);
    validate_community_media_access_db(&state.db, &allowed, media_ids).map_err(map_community_err)
}
