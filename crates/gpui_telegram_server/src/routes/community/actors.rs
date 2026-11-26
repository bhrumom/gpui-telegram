//! Community identity, actor profile, follow graph, and mute/block endpoints.

use crate::db::ServerDatabase;
use crate::routes::community::{
    authorized_media_user_ids_for_auth, broadcast_community_event, map_community_err,
    sanitize_post_embedded_refs_conn, validate_community_media_access_db,
    validate_community_media_access_for_auth, ActorAuth, ActorAuthMode,
};
use crate::routes::err_resp;
use crate::server::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use chrono::Utc;
use gpui_telegram_community::identity::{
    can_user_act_as, human_actor_id, new_bot_actor, new_human_actor,
};
use gpui_telegram_community::store::ops;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::community_contracts::{
    ActorProfileResponse, CommunityActorDto, CommunityCommentDto, CommunityIdentitiesResponse,
    DeleteEntityResponse, FollowActionResponse, MuteBlockActionKind, MuteBlockListResponse,
    MuteBlockRequest, MuteBlockTargetKind, RegisterBotActorRequest, RegisterHumanActorRequest,
    UpdateCommunityActorRequest,
};
use gpui_telegram_proto::miniapp_contracts::validate_bot_username;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub async fn get_identities(auth: ActorAuth, State(state): State<Arc<AppState>>) -> Response {
    if auth.mode == ActorAuthMode::DirectBotToken {
        let resp = CommunityIdentitiesResponse {
            active_actor_id: auth.actor_id.clone(),
            human_actor: None,
            bot_actors: vec![auth.actor.clone()],
            all_actors: vec![auth.actor],
        };
        return (StatusCode::OK, Json(resp)).into_response();
    }

    let owner_uid = auth.owner_user_id.unwrap_or(0);
    let human_id = human_actor_id(owner_uid);
    match state
        .db
        .pool()
        .with_reader(|conn| ops::list_identities_for_user_conn(conn, owner_uid))
    {
        Ok(all_actors) => {
            let human_actor = all_actors
                .iter()
                .find(|a| a.actor_id == human_id)
                .cloned();
            let bot_actors: Vec<CommunityActorDto> = all_actors
                .iter()
                .filter(|a| a.actor_id != human_id)
                .cloned()
                .collect();
            let resp = CommunityIdentitiesResponse {
                active_actor_id: auth.actor_id,
                human_actor,
                bot_actors,
                all_actors,
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

fn reindex_actor_posts_in_search(state: &AppState, actor_id: &str) {
    let posts = state.db.pool().with_reader(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT post_id FROM community_posts
                 WHERE author_actor_id = ?1 AND status = 'published' AND is_removed = 0
                 ORDER BY created_at_ms DESC LIMIT 500",
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let ids: Vec<String> = stmt
            .query_map(params![actor_id], |r| r.get::<_, String>(0))
            .map_err(|e| TelegramError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let mut out = Vec::with_capacity(ids.len());
        for pid in ids {
            if let Some(p) = ops::get_post_conn(conn, &pid, None)? {
                out.push(p);
            }
        }
        Ok(out)
    });
    if let Ok(posts) = posts {
        if !posts.is_empty() {
            for p in &posts {
                let _ = state.community_search_indexer.index_post(p);
            }
            let _ = state.community_search_indexer.commit();
        }
    }
}

pub async fn register_human_identity(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterHumanActorRequest>,
) -> Response {
    if auth.mode == ActorAuthMode::DirectBotToken {
        return err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Bot token cannot register a human identity",
        );
    }
    let Some(owner_uid) = auth.owner_user_id else {
        return err_resp(StatusCode::UNAUTHORIZED, "unauthorized", "Missing human user");
    };

    let media_refs: Vec<&str> = [
        req.avatar_media_id.as_deref(),
        req.banner_media_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Err(resp) = validate_community_media_access_for_auth(&state, &auth, &media_refs) {
        return resp;
    }

    let now_ms = Utc::now().timestamp_millis();
    let res = state.db.pool().with_transaction(|tx| {
        let dto = new_human_actor(owner_uid, &req, now_ms)?;
        let handle_owner: Option<String> = tx
            .query_row(
                "SELECT actor_id FROM community_actors WHERE handle = ?1 COLLATE NOCASE",
                params![&dto.handle],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        if let Some(existing_aid) = handle_owner {
            if existing_aid != dto.actor_id {
                return Err(TelegramError::Validation(format!(
                    "Handle @{} is already taken",
                    dto.handle
                )));
            }
        }

        let existing_handle: Option<String> = tx
            .query_row(
                "SELECT handle FROM community_actors WHERE actor_id = ?1",
                params![&dto.actor_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| TelegramError::Database(e.to_string()))?;

        if existing_handle.is_some() {
            let bio_opt = req.bio.as_deref().map(|s| s.trim().to_string());
            let shape_opt = req
                .avatar_shape
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|_| dto.avatar_shape.clone());
            let color_opt = req
                .avatar_color
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|_| dto.avatar_color.clone());
            tx.execute(
                "UPDATE community_actors SET
                    actor_kind = 'human',
                    handle = ?2,
                    display_name = ?3,
                    bio = COALESCE(?4, bio),
                    location = COALESCE(?5, location),
                    website = COALESCE(?6, website),
                    avatar_media_id = COALESCE(?7, avatar_media_id),
                    banner_media_id = COALESCE(?8, banner_media_id),
                    avatar_shape = COALESCE(?9, avatar_shape),
                    avatar_color = COALESCE(?10, avatar_color),
                    updated_at_ms = ?11
                 WHERE actor_id = ?1",
                params![
                    dto.actor_id,
                    dto.handle,
                    dto.display_name,
                    bio_opt,
                    dto.location,
                    dto.website,
                    dto.avatar_media_id,
                    dto.banner_media_id,
                    shape_opt,
                    color_opt,
                    now_ms,
                ],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        } else {
            ops::upsert_actor_conn(tx, &dto, now_ms)?;
        }

        let persisted = ops::get_actor_conn(tx, &dto.actor_id, None)?.ok_or_else(|| {
            TelegramError::Database(format!("Failed to reload actor '{}'", dto.actor_id))
        })?;
        let handle_changed = existing_handle
            .as_deref()
            .is_some_and(|old| !old.eq_ignore_ascii_case(&persisted.handle));
        Ok((persisted, handle_changed))
    });

    match res {
        Ok((persisted, handle_changed)) => {
            if handle_changed {
                reindex_actor_posts_in_search(&state, &persisted.actor_id);
            }
            (StatusCode::CREATED, Json(persisted)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

fn derive_compliant_bot_username_in_tx(
    tx: &rusqlite::Transaction<'_>,
    normalized_handle: &str,
) -> Result<String, TelegramError> {
    if let Ok(valid) = validate_bot_username(normalized_handle) {
        return Ok(valid);
    }
    let mut base = normalized_handle.to_string();
    if !base.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        base = format!("b_{base}");
    }
    let suffix = "_bot";
    let stem_max = 32 - suffix.len();
    let stem: String = base.chars().take(stem_max).collect();
    let first = format!("{stem}{suffix}");
    if let Ok(valid) = validate_bot_username(&first) {
        let taken: Option<i64> = tx
            .query_row(
                "SELECT user_id FROM users WHERE LOWER(username) = LOWER(?1)",
                params![valid],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        if taken.is_none() {
            return Ok(valid);
        }
    }
    for n in 2..=9999u32 {
        let extra = format!("_{n}_bot");
        let stem_lim = 32usize.saturating_sub(extra.len());
        let candidate = format!("{}{extra}", base.chars().take(stem_lim).collect::<String>());
        if let Ok(valid) = validate_bot_username(&candidate) {
            let taken: Option<i64> = tx
                .query_row(
                    "SELECT user_id FROM users WHERE LOWER(username) = LOWER(?1)",
                    params![valid],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            if taken.is_none() {
                return Ok(valid);
            }
        }
    }
    Err(TelegramError::Validation(
        "Unable to allocate a unique Telegram bot username".into(),
    ))
}

pub async fn register_bot_identity(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterBotActorRequest>,
) -> Response {
    if auth.mode == ActorAuthMode::DirectBotToken {
        return err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Direct Bot token cannot mint new bots without a human owner session",
        );
    }
    let Some(owner_uid) = auth.owner_user_id else {
        return err_resp(StatusCode::UNAUTHORIZED, "unauthorized", "Missing owner session");
    };

    let media_refs: Vec<&str> = [
        req.avatar_media_id.as_deref(),
        req.banner_media_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Err(resp) = validate_community_media_access_for_auth(&state, &auth, &media_refs) {
        return resp;
    }

    let is_admin = auth
        .caller_user_id
        .is_some_and(|u| crate::routes::hermes::caller_is_admin(&state, u));
    let now_ms = Utc::now().timestamp_millis();

    let tx_res = state.db.pool().with_transaction(|tx| {
        let (bot_rec, target_actor_id, handle_changed) = if let Some(existing_bot_id) =
            req.bot_user_id
        {
            let bot = ServerDatabase::get_bot_by_user_id_in_tx(tx, existing_bot_id)?
                .ok_or_else(|| {
                    TelegramError::NotFound(format!("Bot user {existing_bot_id} not found"))
                })?;
            if bot.owner_user_id != owner_uid && !is_admin {
                return Err(TelegramError::PermissionDenied(
                    "Caller does not own the requested bot_user_id".into(),
                ));
            }
            let dto = new_bot_actor(bot.owner_user_id, bot.bot_user_id, &req, now_ms)?;
            let handle_owner: Option<String> = tx
                .query_row(
                    "SELECT actor_id FROM community_actors WHERE handle = ?1 COLLATE NOCASE",
                    params![&dto.handle],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            if let Some(existing_aid) = handle_owner {
                if existing_aid != dto.actor_id {
                    return Err(TelegramError::Validation(format!(
                        "Handle @{} is already taken",
                        dto.handle
                    )));
                }
            }
            let existing_handle: Option<String> = tx
                .query_row(
                    "SELECT handle FROM community_actors WHERE actor_id = ?1",
                    params![&dto.actor_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| TelegramError::Database(e.to_string()))?;

            if existing_handle.is_some() {
                let actor_kind_opt = req.actor_kind.map(|k| k.as_str());
                let bio_opt = req.bio.as_deref().map(|s| s.trim().to_string());
                let shape_opt = req
                    .avatar_shape
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|_| dto.avatar_shape.clone());
                let color_opt = req
                    .avatar_color
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|_| dto.avatar_color.clone());
                let badge_opt = req
                    .bot_badge
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .and_then(|_| dto.bot_badge.clone());
                let caps_json = serde_json::to_string(&dto.bot_capabilities)
                    .map_err(|e| TelegramError::Internal(e.to_string()))?;
                tx.execute(
                    "UPDATE community_actors SET
                        actor_kind = COALESCE(?2, actor_kind),
                        handle = ?3,
                        display_name = ?4,
                        bio = COALESCE(?5, bio),
                        avatar_media_id = COALESCE(?6, avatar_media_id),
                        banner_media_id = COALESCE(?7, banner_media_id),
                        avatar_shape = COALESCE(?8, avatar_shape),
                        avatar_color = COALESCE(?9, avatar_color),
                        bot_badge = COALESCE(?10, bot_badge),
                        bot_model = COALESCE(?11, bot_model),
                        bot_capabilities_json = ?12,
                        bot_autonomy_mode = ?13,
                        bot_persona = COALESCE(?14, bot_persona),
                        updated_at_ms = ?15
                     WHERE actor_id = ?1",
                    params![
                        dto.actor_id,
                        actor_kind_opt,
                        dto.handle,
                        dto.display_name,
                        bio_opt,
                        dto.avatar_media_id,
                        dto.banner_media_id,
                        shape_opt,
                        color_opt,
                        badge_opt,
                        dto.bot_model,
                        caps_json,
                        dto.bot_autonomy_mode.as_str(),
                        dto.bot_persona,
                        now_ms,
                    ],
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            } else {
                ops::upsert_actor_conn(tx, &dto, now_ms)?;
            }
            let changed = existing_handle
                .as_deref()
                .is_some_and(|old| !old.eq_ignore_ascii_case(&dto.handle));
            (bot, dto.actor_id, changed)
        } else {
            let preview = new_bot_actor(owner_uid, 0, &req, now_ms)?;
            let handle_taken: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM community_actors WHERE handle = ?1 COLLATE NOCASE)",
                    params![&preview.handle],
                    |r| r.get(0),
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            if handle_taken {
                return Err(TelegramError::Validation(format!(
                    "Handle @{} is already taken",
                    preview.handle
                )));
            }
            let bot_username = derive_compliant_bot_username_in_tx(tx, &preview.handle)?;
            let bot = ServerDatabase::insert_registered_bot_in_tx(
                tx,
                owner_uid,
                &bot_username,
                &req.display_name,
                req.bio.as_deref().unwrap_or(""),
                req.bot_persona.as_deref().unwrap_or(""),
                req.avatar_shape.as_deref(),
                req.avatar_color.as_deref(),
                req.bot_model.as_deref(),
                now_ms,
            )?;
            let dto = new_bot_actor(owner_uid, bot.bot_user_id, &req, now_ms)?;
            ops::upsert_actor_conn(tx, &dto, now_ms)?;
            (bot, dto.actor_id, false)
        };

        if let Some(max_hr) = req.max_posts_per_hour {
            tx.execute(
                "UPDATE community_actors SET bot_rate_limit_per_hour = ?1 WHERE actor_id = ?2",
                params![max_hr.clamp(1, 500) as i64, target_actor_id],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        }
        let persisted = ops::get_actor_conn(tx, &target_actor_id, None)?.ok_or_else(|| {
            TelegramError::Database(format!("Failed to reload actor '{target_actor_id}'"))
        })?;
        Ok((persisted, bot_rec, handle_changed))
    });

    match tx_res {
        Ok((persisted, bot_rec, handle_changed)) => {
            if handle_changed {
                reindex_actor_posts_in_search(&state, &persisted.actor_id);
            }
            crate::routes::bots_miniapps::provision_fabushi_bot_with_host(
                &state.db,
                &state.fabushi_bots,
                &bot_rec,
            );
            (
                StatusCode::CREATED,
                Json(json!({
                    "actor": persisted,
                    "bot_token": bot_rec.bot_token,
                    "bot_user_id": bot_rec.bot_user_id,
                    "username": bot_rec.username,
                })),
            )
                .into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn get_actor_profile(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(handle_or_id): Path<String>,
) -> Response {
    let viewer_id = auth.actor_id.clone();
    let res = state.db.pool().with_reader(|conn| {
        let actor = ops::get_actor_conn(conn, &handle_or_id, Some(&viewer_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("Actor '{handle_or_id}' not found")))?;

        let post_ids: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT post_id FROM community_posts
                     WHERE author_actor_id = ?1 AND status = 'published' AND is_removed = 0
                     ORDER BY created_at_ms DESC LIMIT 20",
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![actor.actor_id], |r| r.get::<_, String>(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| TelegramError::Database(e.to_string()))?);
            }
            out
        };
        let mut recent_posts = Vec::with_capacity(post_ids.len());
        for pid in post_ids {
            if let Some(mut p) = ops::get_post_conn(conn, &pid, Some(&viewer_id))? {
                sanitize_post_embedded_refs_conn(conn, &mut p);
                recent_posts.push(p);
            }
        }

        let recent_comments: Vec<CommunityCommentDto> = {
            let mut stmt = conn
                .prepare(
                    "SELECT c.comment_id, c.post_id, c.parent_comment_id, c.depth, c.path,
                            p.author_actor_id, c.body_markdown, c.media_json,
                            c.upvotes, c.downvotes, c.score, c.likes_count,
                            c.is_stickied, c.is_distinguished_mod, c.is_removed, c.is_deleted,
                            c.created_at_ms, c.updated_at_ms
                     FROM community_comments c
                     JOIN community_posts p ON p.post_id = c.post_id
                     WHERE c.author_actor_id = ?1
                       AND c.is_removed = 0 AND c.is_deleted = 0
                       AND p.status = 'published' AND p.is_removed = 0
                     ORDER BY c.created_at_ms DESC LIMIT 20",
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![actor.actor_id], |r| {
                    let post_author_id: String = r.get(5)?;
                    let media_json: String = r.get(7)?;
                    let media = serde_json::from_str(&media_json).unwrap_or_default();
                    Ok(CommunityCommentDto {
                        comment_id: r.get(0)?,
                        post_id: r.get(1)?,
                        parent_comment_id: r.get(2)?,
                        depth: r.get::<_, i64>(3)?.max(0) as u32,
                        path: r.get(4)?,
                        author_actor_id: actor.actor_id.clone(),
                        author_handle: actor.handle.clone(),
                        author_display_name: actor.display_name.clone(),
                        author_kind: actor.actor_kind,
                        author_verified_tier: actor.verified_tier,
                        author_bot_badge: actor.bot_badge.clone(),
                        author_avatar_shape: actor.avatar_shape.clone(),
                        author_avatar_color: actor.avatar_color.clone(),
                        is_op: actor.actor_id == post_author_id,
                        body: r.get(6)?,
                        media,
                        upvotes: r.get(8)?,
                        downvotes: r.get(9)?,
                        score: r.get(10)?,
                        likes_count: r.get::<_, i64>(11)?.max(0) as u64,
                        awards: Vec::new(),
                        viewer_vote: 0,
                        viewer_liked: false,
                        is_stickied: r.get::<_, i64>(12)? != 0,
                        is_mod_distinguished: r.get::<_, i64>(13)? != 0,
                        is_removed: r.get::<_, i64>(14)? != 0,
                        is_deleted: r.get::<_, i64>(15)? != 0,
                        children: Vec::new(),
                        more_children_count: 0,
                        created_at_ms: r.get(16)?,
                        updated_at_ms: r.get(17)?,
                    })
                })
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| TelegramError::Database(e.to_string()))?);
            }
            out
        };

        let moderated_communities: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT c.slug FROM community_memberships m
                     JOIN communities c ON c.community_id = m.community_id
                     WHERE m.actor_id = ?1 AND m.role IN ('owner', 'mod', 'bot_mod')
                     ORDER BY c.slug ASC",
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![actor.actor_id], |r| r.get::<_, String>(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| TelegramError::Database(e.to_string()))?);
            }
            out
        };

        Ok(ActorProfileResponse {
            actor,
            recent_posts,
            recent_comments,
            moderated_communities,
        })
    });

    match res {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn patch_actor_profile(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(actor_id): Path<String>,
    Json(req): Json<UpdateCommunityActorRequest>,
) -> Response {
    let target = match state
        .db
        .pool()
        .with_reader(|conn| ops::get_actor_conn(conn, &actor_id, None))
    {
        Ok(Some(a)) => a,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "not_found", "Actor not found"),
        Err(e) => return map_community_err(e),
    };

    let allowed = auth.actor_id == target.actor_id
        || (auth.mode != ActorAuthMode::DirectBotToken
            && auth
                .owner_user_id
                .is_some_and(|uid| can_user_act_as(uid, &target)));
    if !allowed {
        return err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Only the actor itself or its owner can modify this actor profile",
        );
    }

    let media_refs: Vec<&str> = [
        req.avatar_media_id.as_deref(),
        req.banner_media_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut allowed_uids = authorized_media_user_ids_for_auth(&state.db, &auth);
    if let Some(linked) = target.linked_user_id.filter(|&id| id > 0) {
        let uid = UserId(linked as u64);
        if !allowed_uids.contains(&uid) {
            allowed_uids.push(uid);
        }
    }
    if let Err(e) = validate_community_media_access_db(&state.db, &allowed_uids, &media_refs) {
        return map_community_err(e);
    }

    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .pool()
        .with_transaction(|tx| ops::update_actor_tx(tx, &target.actor_id, &req, now_ms))
    {
        Ok(updated) => (StatusCode::OK, Json(updated)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn follow_actor(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(actor_id): Path<String>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    let res = state.db.pool().with_transaction(|tx| {
        let target = ops::get_actor_conn(tx, &actor_id, None)?
            .ok_or_else(|| TelegramError::NotFound(format!("Actor '{actor_id}' not found")))?;
        ops::follow_actor_tx(tx, &auth.actor_id, &target.actor_id, now_ms)?;
        let refreshed = ops::get_actor_conn(tx, &target.actor_id, Some(&auth.actor_id))?
            .unwrap_or(target);
        Ok(FollowActionResponse {
            actor_id: refreshed.actor_id,
            is_following: true,
            followers_count: refreshed.followers_count,
        })
    });

    match res {
        Ok(resp) => {
            broadcast_community_event(
                &state,
                "actor_followed",
                None,
                None,
                None,
                Some(auth.actor_id),
                &json!({ "target_actor_id": resp.actor_id, "followers_count": resp.followers_count }),
            );
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_community_err(e),
    }
}

pub async fn unfollow_actor(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(actor_id): Path<String>,
) -> Response {
    let res = state.db.pool().with_transaction(|tx| {
        let target = ops::get_actor_conn(tx, &actor_id, None)?
            .ok_or_else(|| TelegramError::NotFound(format!("Actor '{actor_id}' not found")))?;
        ops::unfollow_actor_tx(tx, &auth.actor_id, &target.actor_id)?;
        let refreshed = ops::get_actor_conn(tx, &target.actor_id, Some(&auth.actor_id))?
            .unwrap_or(target);
        Ok(FollowActionResponse {
            actor_id: refreshed.actor_id,
            is_following: false,
            followers_count: refreshed.followers_count,
        })
    });

    match res {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn list_mutes_blocks(auth: ActorAuth, State(state): State<Arc<AppState>>) -> Response {
    match state
        .db
        .pool()
        .with_reader(|conn| ops::list_mutes_blocks_conn(conn, &auth.actor_id))
    {
        Ok(items) => (StatusCode::OK, Json(MuteBlockListResponse { items })).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn set_mute_block(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<MuteBlockRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .pool()
        .with_transaction(|tx| ops::set_mute_block_tx(tx, &auth.actor_id, &req, now_ms))
    {
        Ok(saved) => (StatusCode::OK, Json(saved)).into_response(),
        Err(e) => map_community_err(e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct DeleteMuteBlockQuery {
    pub target_kind: Option<String>,
    pub target_value: Option<String>,
    pub action: Option<String>,
}

pub async fn delete_mute_block(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DeleteMuteBlockQuery>,
    body: Bytes,
) -> Response {
    let parsed_body: Option<MuteBlockRequest> = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => Some(v),
            Err(e) => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_json",
                    &format!("Invalid mute/block body: {e}"),
                )
            }
        }
    };

    let (target_kind, target_value, action) = if let Some(b) = parsed_body {
        (b.target_kind, b.target_value, b.action)
    } else {
        let Some(tk) = query
            .target_kind
            .as_deref()
            .map(str::trim)
            .and_then(MuteBlockTargetKind::parse)
        else {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "target_kind (actor|community|keyword) is required",
            );
        };
        let Some(tv) = query
            .target_value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "target_value is required",
            );
        };
        let Some(act) = query
            .action
            .as_deref()
            .map(str::trim)
            .and_then(MuteBlockActionKind::parse)
        else {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "action (mute|block) is required",
            );
        };
        (tk, tv.to_string(), act)
    };

    match state.db.pool().with_transaction(|tx| {
        ops::remove_mute_block_tx(tx, &auth.actor_id, target_kind, &target_value, action)
    }) {
        Ok(deleted) => (
            StatusCode::OK,
            Json(DeleteEntityResponse {
                id: format!(
                    "{}:{}:{}",
                    target_kind.as_str(),
                    action.as_str(),
                    target_value
                ),
                deleted,
            }),
        )
            .into_response(),
        Err(e) => map_community_err(e),
    }
}
