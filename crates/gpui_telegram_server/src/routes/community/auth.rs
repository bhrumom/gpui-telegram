//! Unified First-Class `ActorAuth` extractor supporting Human Bearer, Owner Act-As-Bot, and Direct Bot Token.

use crate::db::miniapp_store::{BotRegistryRecord, BOTFATHER_USER_ID};
use crate::db::ServerDatabase;
use crate::routes::err_resp;
use crate::server::{AppState, AuthBearer};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use gpui_telegram_community::identity::{
    bot_actor_id, can_user_act_as, human_actor_id, new_bot_actor, normalize_handle,
};
use gpui_telegram_community::store::ops;
use gpui_telegram_community::{TelegramBotSyncInput, TelegramHumanSyncInput};
use gpui_telegram_core::error::{Result as CoreResult, TelegramError};
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::community_contracts::{
    ActorKind, BotAutonomyMode, CommunityActorDto, RegisterBotActorRequest, VerifiedTier,
};
use rusqlite::Transaction;
use std::sync::Arc;

pub const HEADER_COMMUNITY_ACTOR_ID: &str = "x-community-actor-id";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorAuthMode {
    HumanBearer,
    ActAsOwnedActor,
    DirectBotToken,
}

#[derive(Debug, Clone)]
pub struct ActorAuth {
    pub mode: ActorAuthMode,
    pub actor_id: String,
    pub actor: CommunityActorDto,
    pub caller_user_id: Option<UserId>,
    pub owner_user_id: Option<i64>,
    pub bot_record: Option<BotRegistryRecord>,
}

fn non_empty(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Syncs a Telegram human user and all owned `bots_registry` rows into `community_actors`.
pub fn sync_telegram_identities_for_user(
    db: &ServerDatabase,
    user_id: UserId,
    now_ms: i64,
) -> CoreResult<Vec<CommunityActorDto>> {
    let user = db
        .get_user(user_id)?
        .ok_or_else(|| TelegramError::NotFound(format!("User {} not found", user_id.0)))?;
    let display_name = match user
        .last_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(last) if !user.first_name.trim().is_empty() => {
            format!("{} {}", user.first_name.trim(), last)
        }
        _ if !user.first_name.trim().is_empty() => user.first_name.trim().to_string(),
        _ => user
            .username
            .clone()
            .unwrap_or_else(|| format!("User {}", user_id.0)),
    };
    let human_in = TelegramHumanSyncInput {
        user_id: user_id.0 as i64,
        username: user.username,
        display_name,
        bio: user.bio,
        avatar_media_id: user.profile_photo_file_id,
    };

    let owned_bots = db.list_bots_by_owner(user_id.0 as i64)?;
    let bot_ins: Vec<TelegramBotSyncInput> = owned_bots
        .into_iter()
        .filter(|b| b.bot_user_id != BOTFATHER_USER_ID as i64)
        .map(|b| TelegramBotSyncInput {
            bot_user_id: b.bot_user_id,
            username: b.username,
            display_name: b.display_name,
            bio: non_empty(&b.description).or_else(|| non_empty(&b.about)),
            avatar_media_id: None,
            avatar_shape: non_empty(&b.avatar_shape),
            avatar_color: non_empty(&b.avatar_color),
            bot_model: non_empty(&b.preferred_model),
            bot_capabilities: vec!["chat".to_string(), "community".to_string()],
            bot_persona: non_empty(&b.ai_persona),
        })
        .collect();

    db.pool()
        .with_transaction(|tx| ops::sync_telegram_identities_tx(tx, &human_in, &bot_ins, now_ms))
}

/// Ensures a single `BotRegistryRecord` exists in `community_actors` as `"b:{bot_user_id}"`.
pub fn ensure_bot_actor_synced_tx(
    tx: &Transaction<'_>,
    bot: &BotRegistryRecord,
    now_ms: i64,
) -> CoreResult<CommunityActorDto> {
    let cid = bot_actor_id(bot.bot_user_id);
    if let Some(existing) = ops::get_actor_conn(tx, &cid, None)? {
        return Ok(existing);
    }

    let base_handle = normalize_handle(&bot.username)
        .unwrap_or_else(|_| format!("bot_{}", bot.bot_user_id.abs()));
    let handle = if ops::get_actor_conn(tx, &base_handle, None)?.is_none() {
        base_handle
    } else {
        format!(
            "{}_{}",
            &base_handle[..base_handle.len().min(20)],
            bot.bot_user_id.abs()
        )
    };

    let req = RegisterBotActorRequest {
        bot_user_id: Some(bot.bot_user_id),
        actor_kind: Some(ActorKind::Bot),
        handle,
        display_name: if bot.display_name.trim().is_empty() {
            bot.username.clone()
        } else {
            bot.display_name.clone()
        },
        bio: non_empty(&bot.description).or_else(|| non_empty(&bot.about)),
        avatar_media_id: None,
        banner_media_id: None,
        avatar_shape: non_empty(&bot.avatar_shape),
        avatar_color: non_empty(&bot.avatar_color),
        bot_badge: Some(if bot.is_official {
            "Official Bot".to_string()
        } else {
            "AI Bot".to_string()
        }),
        bot_model: non_empty(&bot.preferred_model),
        bot_capabilities: vec!["chat".to_string(), "community".to_string()],
        bot_autonomy_mode: BotAutonomyMode::MentionOnly,
        bot_persona: non_empty(&bot.ai_persona),
        cooldown_secs: None,
        max_posts_per_hour: None,
    };
    let mut dto = new_bot_actor(bot.owner_user_id, bot.bot_user_id, &req, now_ms)?;
    if bot.is_official {
        dto.verified_tier = VerifiedTier::OfficialBot;
    }
    ops::upsert_actor_conn(tx, &dto, now_ms)?;
    Ok(dto)
}

pub fn ensure_bot_actor_synced(
    db: &ServerDatabase,
    bot: &BotRegistryRecord,
    now_ms: i64,
) -> CoreResult<CommunityActorDto> {
    let cid = bot_actor_id(bot.bot_user_id);
    if let Some(existing) = db
        .pool()
        .with_reader(|conn| ops::get_actor_conn(conn, &cid, None))?
    {
        return Ok(existing);
    }
    db.pool()
        .with_transaction(|tx| ensure_bot_actor_synced_tx(tx, bot, now_ms))
}

fn extract_bot_auth_header(parts: &Parts) -> Option<String> {
    let raw = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?;
    let trimmed = raw.trim();
    if trimmed.len() >= 4 && trimmed[..4].eq_ignore_ascii_case("bot ") {
        return Some(trimmed[4..].trim().to_string());
    }
    None
}

fn extract_act_as_header(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get(HEADER_COMMUNITY_ACTOR_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl FromRequestParts<Arc<AppState>> for ActorAuth {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let now_ms = Utc::now().timestamp_millis();

        // Mode 3: Direct Bot Auth (`Authorization: Bot <bot_token>`)
        if let Some(bot_token) = extract_bot_auth_header(parts) {
            if bot_token.is_empty() {
                return Err(err_resp(
                    StatusCode::UNAUTHORIZED,
                    "missing_bot_token",
                    "Missing Bot authorization token",
                ));
            }
            let bot = state
                .db
                .get_bot_by_token(&bot_token)
                .map_err(|_| {
                    err_resp(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "database_error",
                        "Failed to verify bot token",
                    )
                })?
                .ok_or_else(|| {
                    err_resp(
                        StatusCode::UNAUTHORIZED,
                        "invalid_bot_token",
                        "Invalid bot authorization token",
                    )
                })?;

            let expected_actor_id = bot_actor_id(bot.bot_user_id);
            if let Some(requested) = extract_act_as_header(parts) {
                if requested != expected_actor_id {
                    return Err(err_resp(
                        StatusCode::FORBIDDEN,
                        "forbidden",
                        "Bot token cannot impersonate a different X-Community-Actor-Id",
                    ));
                }
            }

            let actor = ensure_bot_actor_synced(&state.db, &bot, now_ms).map_err(|e| {
                err_resp(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "actor_sync_failed",
                    &format!("Failed to sync bot actor: {e}"),
                )
            })?;

            return Ok(ActorAuth {
                mode: ActorAuthMode::DirectBotToken,
                actor_id: actor.actor_id.clone(),
                actor,
                caller_user_id: Some(UserId(bot.bot_user_id.max(0) as u64)),
                owner_user_id: (bot.owner_user_id > 0).then_some(bot.owner_user_id),
                bot_record: Some(bot),
            });
        }

        // Modes 1 & 2: Human Bearer (+ optional verified `X-Community-Actor-Id`)
        let bearer = AuthBearer::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let uid = bearer.user_id.0 as i64;
        let _ = sync_telegram_identities_for_user(&state.db, bearer.user_id, now_ms);

        let canonical_human_id = human_actor_id(uid);
        let requested_act_as = extract_act_as_header(parts);

        if let Some(requested) = requested_act_as.filter(|id| id != &canonical_human_id) {
            let mut target_opt = state
                .db
                .pool()
                .with_reader(|conn| ops::get_actor_conn(conn, &requested, None))
                .map_err(|_| {
                    err_resp(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "database_error",
                        "Failed to look up requested community actor",
                    )
                })?;

            let mut resolved_bot_record: Option<BotRegistryRecord> = None;

            if target_opt.is_none() {
                if let Some(raw_bid) = requested.strip_prefix("b:") {
                    if let Ok(bid) = raw_bid.parse::<i64>() {
                        if let Ok(Some(bot)) = state.db.get_bot_by_user_id(bid) {
                            if bot.owner_user_id != uid {
                                return Err(err_resp(
                                    StatusCode::FORBIDDEN,
                                    "forbidden",
                                    "Caller does not own requested bot actor",
                                ));
                            }
                            let synced = ensure_bot_actor_synced(&state.db, &bot, now_ms)
                                .map_err(|e| {
                                    err_resp(
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        "actor_sync_failed",
                                        &e.to_string(),
                                    )
                                })?;
                            resolved_bot_record = Some(bot);
                            target_opt = Some(synced);
                        }
                    }
                }
            }

            let target = target_opt.ok_or_else(|| {
                err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Requested X-Community-Actor-Id does not exist or is not owned by caller",
                )
            })?;

            if !can_user_act_as(uid, &target) {
                return Err(err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Caller does not own requested X-Community-Actor-Id",
                ));
            }

            if resolved_bot_record.is_none() {
                if let Some(bid) = target
                    .linked_user_id
                    .filter(|_| target.actor_kind.is_bot_like() || target.actor_id.starts_with("b:"))
                {
                    if let Ok(Some(bot)) = state.db.get_bot_by_user_id(bid) {
                        if bot.owner_user_id != uid {
                            return Err(err_resp(
                                StatusCode::FORBIDDEN,
                                "forbidden",
                                "Caller does not own requested bot in bots_registry",
                            ));
                        }
                        resolved_bot_record = Some(bot);
                    }
                }
            }

            return Ok(ActorAuth {
                mode: ActorAuthMode::ActAsOwnedActor,
                actor_id: target.actor_id.clone(),
                actor: target,
                caller_user_id: Some(bearer.user_id),
                owner_user_id: Some(uid),
                bot_record: resolved_bot_record,
            });
        }

        let human_actor = state
            .db
            .pool()
            .with_reader(|conn| ops::get_actor_conn(conn, &canonical_human_id, None))
            .map_err(|_| {
                err_resp(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database_error",
                    "Failed to load human community actor",
                )
            })?
            .ok_or_else(|| {
                err_resp(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "actor_sync_failed",
                    "Human community actor was not synced",
                )
            })?;

        Ok(ActorAuth {
            mode: ActorAuthMode::HumanBearer,
            actor_id: human_actor.actor_id.clone(),
            actor: human_actor,
            caller_user_id: Some(bearer.user_id),
            owner_user_id: Some(uid),
            bot_record: None,
        })
    }
}
