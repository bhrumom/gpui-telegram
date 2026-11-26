//! Telegram IM Bridge Fusion for Orca (`F21`):
//! - Links `OrcaBotRecord` instances to Telegram chats (`GET/POST /api/v1/orca/bots/{bot_id}/telegram/link`)
//! - Posts interactive inline-keyboard decision cards (`POST /api/v1/orca/bots/{bot_id}/telegram/card`)
//!   for Bot Ask Cards (`orca:ask:...`), Orchestration Gates (`orca:gate:...`), and Access Requests (`orca:access:...`)
//! - Dispatches Telegram inline callback clicks (`handle_orca_chat_callback`) directly into
//!   `BotItemsStore`, `OrchestrationEngine`, and `BotAccessHost`, editing the message, clearing
//!   its inline keyboard, and broadcasting PTS updates.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use gpui_telegram_core::types::{ChatId, MsgId, UserId};
use gpui_telegram_proto::message::{InlineKeyboardButton, MessageContent};
use gpui_telegram_proto::miniapp_contracts::{
    validate_bot_username, ChatCallbackRequest, PeerBotActionResult,
};
use gpui_telegram_proto::orca_contracts::{
    OrcaTelegramCardPostRequest, OrcaTelegramLinkRequest,
};
use orca_bots::{AnswerQuestionRequest, ConnectorAccessRequestStatus};
use orca_core::bot_items::{
    bot_item_ask_text, fold_ask_key, AskSelection, BotItemActor,
};
use orca_core::bot_types::OrcaBotRecord;
use orca_ide::orchestration_engine::ResolveGateAction;
use serde_json::json;

use crate::routes::bots_miniapps::send_bot_chat_reply;
use crate::routes::orca_error::{
    map_bots_err, map_ide_err, orca_err, parse_json_body, parse_optional_json_body,
};
use crate::routes::{broadcast_chat_pts, check_active_membership, map_telegram_err};
use crate::server::{AppState, AuthBearer};

fn now_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn derive_orca_tg_username(bot: &OrcaBotRecord, requested: Option<&str>, suffix: Option<&str>) -> String {
    if let Some(raw) = requested
        && let Ok(valid) = validate_bot_username(raw)
    {
        return valid;
    }
    let mut stem: String = bot
        .folder
        .as_str()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    if stem.is_empty() {
        stem.push_str("agent");
    }
    let suffix_part = suffix.unwrap_or("");
    let max_stem = 32usize.saturating_sub("orca_".len() + "_bot".len() + suffix_part.len());
    stem.truncate(max_stem.max(1));
    let candidate = format!("orca_{stem}{suffix_part}_bot");
    validate_bot_username(&candidate).unwrap_or_else(|_| "orca_agent_bot".to_string())
}

fn ensure_mirrored_telegram_bot(
    state: &AppState,
    owner_user_id: UserId,
    bot: &OrcaBotRecord,
    requested_username: Option<&str>,
) -> Result<(UserId, String), Response> {
    let primary_username = derive_orca_tg_username(bot, requested_username, None);
    if let Ok(Some(existing)) = state.db.get_bot_by_username(&primary_username) {
        return Ok((UserId(existing.bot_user_id as u64), existing.username));
    }

    let desc = format!("Orca Bot ({})", bot.agent.as_str());
    match state.db.create_registered_bot_with_options(
        owner_user_id.0 as i64,
        &primary_username,
        bot.name.as_str(),
        &desc,
        "Mirrored Orca Bot",
        "",
        "default",
        "",
        "",
        "[]",
        false,
        false,
        false,
    ) {
        Ok(reg) => Ok((UserId(reg.bot_user_id as u64), reg.username)),
        Err(_) => {
            let short_id: String = bot
                .id
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(6)
                .collect();
            let fallback_suffix = format!("_{}", if short_id.is_empty() { "1" } else { &short_id });
            let fallback_username = derive_orca_tg_username(bot, None, Some(&fallback_suffix));
            if let Ok(Some(existing)) = state.db.get_bot_by_username(&fallback_username) {
                return Ok((UserId(existing.bot_user_id as u64), existing.username));
            }
            state
                .db
                .create_registered_bot_with_options(
                    owner_user_id.0 as i64,
                    &fallback_username,
                    bot.name.as_str(),
                    &desc,
                    "Mirrored Orca Bot",
                    "",
                    "default",
                    "",
                    "",
                    "[]",
                    false,
                    false,
                    false,
                )
                .map(|reg| (UserId(reg.bot_user_id as u64), reg.username))
                .map_err(map_telegram_err)
        }
    }
}

// ---------------------------------------------------------------------------
// 1. Link Orca Bot <-> Telegram Chat (`GET/POST .../telegram/link`)
// ---------------------------------------------------------------------------

/// `GET /api/v1/orca/bots/{bot_id}/telegram/link` (`200 OK`).
pub async fn get_telegram_link(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> Response {
    if let Err(err) = state.orca_bots.controller().get_bot(&bot_id).await {
        return map_bots_err(err);
    }
    let chat_id = state.orca_telegram_links.read().get(&bot_id).copied();
    Json(json!({
        "bot_id": bot_id,
        "linked": chat_id.is_some(),
        "chat_id": chat_id,
    }))
    .into_response()
}

/// `POST /api/v1/orca/bots/{bot_id}/telegram/link` (`200 OK`).
/// Links an `OrcaBotRecord` to a Telegram `ChatId` (registering/resolving a mirrored
/// Telegram bot user and companion DM via `ensure_companion_bot_chat_record` when `chat_id` is omitted).
pub async fn link_telegram_chat(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let req: OrcaTelegramLinkRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let (bot_user_id, bot_username) = match ensure_mirrored_telegram_bot(
        &state,
        auth.user_id,
        &bot,
        req.bot_username.as_deref(),
    ) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };

    let chat_id = match req.chat_id {
        Some(explicit_cid) => {
            let cid = ChatId(explicit_cid);
            if let Err(resp) = check_active_membership(&state, cid, auth.user_id) {
                return resp;
            }
            cid
        }
        None => match state
            .db
            .ensure_companion_bot_chat_record(auth.user_id, bot_user_id)
        {
            Ok((cid, _created)) => cid,
            Err(err) => return map_telegram_err(err),
        },
    };

    state.orca_telegram_links.write().insert(bot_id.clone(), chat_id.0);

    let mut welcome_message_id: Option<i64> = None;
    if let Some(welcome) = req
        .welcome_text
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && let Ok(mid) = send_bot_chat_reply(&state, chat_id, bot_user_id, welcome, Vec::new())
    {
        welcome_message_id = Some(mid.0);
    }

    Json(json!({
        "ok": true,
        "bot_id": bot_id,
        "chat_id": chat_id.0,
        "bot_user_id": bot_user_id.0,
        "bot_username": bot_username,
        "welcome_message_id": welcome_message_id,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// 2. Interactive Telegram Decision Card Poster (`POST .../telegram/card`)
// ---------------------------------------------------------------------------

fn cb_button(text: impl Into<String>, callback_data: impl Into<String>) -> InlineKeyboardButton {
    InlineKeyboardButton {
        text: text.into(),
        callback_data: Some(callback_data.into()),
        url: None,
        web_app_url: None,
    }
}

/// `POST /api/v1/orca/bots/{bot_id}/telegram/card` (`200 OK`).
/// Posts an interactive Telegram message with `InlineKeyboardButton` rows via
/// `routes::bots_miniapps::send_bot_chat_reply` for Bot Ask Cards, Orchestration Gates,
/// or Access Requests.
pub async fn post_telegram_card(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let req: OrcaTelegramCardPostRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let chat_id_raw = req
        .chat_id
        .or_else(|| state.orca_telegram_links.read().get(&bot_id).copied());
    let Some(cid_raw) = chat_id_raw else {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Missing chat_id and bot is not linked to a Telegram chat.",
        );
    };
    let chat_id = ChatId(cid_raw);
    if let Err(resp) = check_active_membership(&state, chat_id, auth.user_id) {
        return resp;
    }

    let sender_bot_uid = ensure_mirrored_telegram_bot(&state, auth.user_id, &bot, None)
        .map(|(uid, _)| uid)
        .unwrap_or(auth.user_id);

    let kind_norm = req.card_kind.trim().to_ascii_lowercase();
    let (card_text, kb_rows): (String, Vec<Vec<InlineKeyboardButton>>) = match kind_norm.as_str() {
        "ask" | "bot_ask" | "item_ask" => {
            let Some(item_num) = req.item_number else {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "Missing required 'item_number' for ask card.",
                );
            };
            let item = match state.orca_bots.items_store().get_item_by_number(item_num) {
                Ok(it) => it,
                Err(err) => return map_bots_err(err),
            };
            let Some(ref ask) = item.ask else {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Item #{item_num} has no structured ask payload."),
                );
            };
            let mut rows = Vec::new();
            for (q_idx, q) in ask.questions.iter().enumerate() {
                let mut row = Vec::new();
                for (opt_idx, opt) in q.options.iter().enumerate() {
                    let option_id = if q_idx == 0 {
                        opt_idx.to_string()
                    } else {
                        format!("q{q_idx}_{opt_idx}")
                    };
                    let label = if q.recommended == Some(opt_idx) {
                        format!("★ {}", opt.label.trim())
                    } else {
                        opt.label.trim().to_string()
                    };
                    row.push(cb_button(
                        label,
                        format!("orca:ask:{}:{}", item.number, option_id),
                    ));
                }
                if !row.is_empty() {
                    rows.push(row);
                }
            }
            let text = req.text.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
                format!(
                    "🤖 {} — Item #{}: {}\n\n{}",
                    bot.name.as_str(),
                    item.number,
                    item.title,
                    bot_item_ask_text(ask)
                )
            });
            (text, rows)
        }
        "gate" | "orchestration_gate" | "decision_gate" => {
            let Some(gate_id) = req
                .gate_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "Missing required 'gate_id' for gate card.",
                );
            };
            let rows = vec![vec![
                cb_button("✅ Approve", format!("orca:gate:{gate_id}:approve")),
                cb_button("❌ Reject", format!("orca:gate:{gate_id}:reject")),
            ]];
            let text = req.text.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
                format!(
                    "🚦 {} — Orchestration Gate `{gate_id}` requires approval.",
                    bot.name.as_str()
                )
            });
            (text, rows)
        }
        "access" | "access_request" => {
            let Some(request_id) = req
                .request_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "Missing required 'request_id' for access card.",
                );
            };
            let requester_name = state
                .orca_bots
                .access_host()
                .list_requests(None, None)
                .ok()
                .and_then(|list| {
                    list.into_iter()
                        .find(|r| r.request_id == request_id)
                        .map(|r| r.member.name)
                })
                .unwrap_or_else(|| "External requester".to_string());
            let rows = vec![vec![
                cb_button("✅ Approve Access", format!("orca:access:{request_id}:approve")),
                cb_button("🚫 Decline", format!("orca:access:{request_id}:decline")),
            ]];
            let text = req.text.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
                format!(
                    "🔐 {} — Access request `{request_id}` from **{requester_name}**.",
                    bot.name.as_str()
                )
            });
            (text, rows)
        }
        other => {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Unsupported card_kind '{other}' (expected ask|gate|access)."),
            );
        }
    };

    let msg_id = match send_bot_chat_reply(&state, chat_id, sender_bot_uid, &card_text, kb_rows.clone()) {
        Ok(mid) => mid,
        Err(err) => return map_telegram_err(err),
    };

    Json(json!({
        "ok": true,
        "bot_id": bot_id,
        "chat_id": chat_id.0,
        "message_id": msg_id.0,
        "card_kind": kind_norm,
        "text": card_text,
        "inline_keyboard": kb_rows,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// 3. Telegram Callback Dispatch (`orca:ask:...`, `orca:gate:...`, `orca:access:...`)
// ---------------------------------------------------------------------------

fn resolve_ask_option(
    ask: &orca_core::bot_items::BotItemAsk,
    option_id: &str,
) -> Option<(usize, String)> {
    let trimmed = option_id.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(opt_idx) = trimmed.parse::<usize>() {
        return ask
            .questions
            .first()
            .and_then(|q| q.options.get(opt_idx))
            .map(|o| (0, o.label.clone()));
    }
    if let Some(rest) = trimmed.strip_prefix('q')
        && let Some((q_str, o_str)) = rest.split_once('_')
        && let (Ok(q_idx), Ok(opt_idx)) = (q_str.parse::<usize>(), o_str.parse::<usize>())
    {
        return ask
            .questions
            .get(q_idx)
            .and_then(|q| q.options.get(opt_idx))
            .map(|o| (q_idx, o.label.clone()));
    }
    if let Some((q_str, o_str)) = trimmed.split_once(':')
        && let (Ok(q_idx), Ok(opt_idx)) = (q_str.parse::<usize>(), o_str.parse::<usize>())
    {
        return ask
            .questions
            .get(q_idx)
            .and_then(|q| q.options.get(opt_idx))
            .map(|o| (q_idx, o.label.clone()));
    }
    let key = fold_ask_key(trimmed);
    for (q_idx, q) in ask.questions.iter().enumerate() {
        if let Some(opt) = q.options.iter().find(|o| fold_ask_key(&o.label) == key) {
            return Some((q_idx, opt.label.clone()));
        }
    }
    None
}

fn finalize_telegram_card_update(
    state: &Arc<AppState>,
    chat_id: ChatId,
    req: &ChatCallbackRequest,
    updated_text: String,
    toast: String,
) -> Response {
    if req.message_id > 0
        && let Ok(pts) = state.message_pipeline.edit_message(
            chat_id,
            MsgId(req.message_id),
            MessageContent::plain_text(&updated_text),
        )
    {
        let _ = state
            .db
            .save_message_extras(chat_id, MsgId(req.message_id), Some("[]"), None);
        broadcast_chat_pts(state, chat_id, pts);
    }
    (
        StatusCode::OK,
        Json(PeerBotActionResult {
            ok: true,
            companion_chat_id: Some(chat_id.0),
            toast: Some(toast),
            updated_text: Some(updated_text),
            inline_keyboard: Some(Vec::new()),
            ..Default::default()
        }),
    )
        .into_response()
}

/// Dispatches Telegram inline callback clicks prefixed with `"orca:"` (`rest` is the
/// substring after `"orca:"`).
///
/// Supported formats:
/// - `ask:<item_num>:<option_id>` -> resolves open Ask question via `BotItemsStore::answer_question`
/// - `gate:<gate_id>:<approve|reject>` -> resolves via `OrchestrationEngine::resolve_gate`
/// - `access:<req_id>:<approve|decline>` -> resolves via `BotAccessHost::decide_request`
pub async fn handle_orca_chat_callback(
    state: &Arc<AppState>,
    chat_id: ChatId,
    user_id: UserId,
    req: &ChatCallbackRequest,
    rest: &str,
) -> Response {
    let now_ms = now_epoch_ms();
    let actor_name = state
        .db
        .get_user(user_id)
        .ok()
        .flatten()
        .and_then(|u| u.username)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("user_{}", user_id.0));

    if let Some(ask_rest) = rest.strip_prefix("ask:") {
        let Some((num_str, option_id)) = ask_rest.split_once(':') else {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "Expected callback format 'orca:ask:<item_num>:<option_id>'.",
            );
        };
        let Ok(item_num) = num_str.trim().parse::<u64>() else {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Invalid item number '{num_str}' in orca:ask callback."),
            );
        };
        let item = match state.orca_bots.items_store().get_item_by_number(item_num) {
            Ok(it) => it,
            Err(err) => return map_bots_err(err),
        };
        let Some(question_event) = item.question_event else {
            return orca_err(
                StatusCode::CONFLICT,
                "question_closed",
                format!("Item #{item_num} has no open Ask question."),
            );
        };
        let (selections, reply_text, chosen_label) = if let Some(ref ask) = item.ask {
            match resolve_ask_option(ask, option_id) {
                Some((question_index, label)) => (
                    Some(vec![AskSelection {
                        question_index,
                        labels: vec![label.clone()],
                    }]),
                    None,
                    label,
                ),
                None => {
                    return orca_err(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        format!("Unknown ask option '{option_id}' for item #{item_num}."),
                    );
                }
            }
        } else {
            (None, Some(option_id.trim().to_string()), option_id.trim().to_string())
        };
        let answer_req = AnswerQuestionRequest {
            actor: BotItemActor::Owner,
            question_event,
            selections,
            reply_text,
            files: Vec::new(),
        };
        return match state
            .orca_bots
            .items_store()
            .answer_question(&item.uid, answer_req, now_ms)
        {
            Ok((updated_item, _ev)) => finalize_telegram_card_update(
                state,
                chat_id,
                req,
                format!(
                    "✅ Item #{} ({}) answered: **{}**",
                    updated_item.number, updated_item.title, chosen_label
                ),
                format!("Answered item #{}: {}", updated_item.number, chosen_label),
            ),
            Err(err) => map_bots_err(err),
        };
    }

    if let Some(gate_rest) = rest.strip_prefix("gate:") {
        let Some((gate_id, decision)) = gate_rest.rsplit_once(':') else {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "Expected callback format 'orca:gate:<gate_id>:<approve|reject>'.",
            );
        };
        let action = match decision.trim().to_ascii_lowercase().as_str() {
            "approve" => ResolveGateAction::Approve {
                actor: actor_name,
                note: Some("Resolved via Telegram inline card".to_string()),
            },
            "reject" | "deny" => ResolveGateAction::Reject {
                actor: actor_name,
                reason: Some("Rejected via Telegram inline card".to_string()),
            },
            other => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unsupported gate decision '{other}' (expected approve|reject)."),
                );
            }
        };
        return match state
            .orca_ide
            .orchestration()
            .resolve_gate(gate_id.trim(), action, Some(now_ms))
        {
            Ok(gate) => {
                let status_str = format!("{:?}", gate.status).to_ascii_lowercase();
                finalize_telegram_card_update(
                    state,
                    chat_id,
                    req,
                    format!("🚦 Gate `{}` resolved: **{}**", gate.gate_id, status_str),
                    format!("Gate {} -> {}", gate.gate_id, status_str),
                )
            }
            Err(err) => map_ide_err(err),
        };
    }

    if let Some(access_rest) = rest.strip_prefix("access:") {
        let Some((req_id, decision)) = access_rest.rsplit_once(':') else {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "Expected callback format 'orca:access:<req_id>:<approve|decline>'.",
            );
        };
        let approve = match decision.trim().to_ascii_lowercase().as_str() {
            "approve" => true,
            "decline" | "deny" | "reject" => false,
            other => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unsupported access decision '{other}' (expected approve|decline)."),
                );
            }
        };
        return match state
            .orca_bots
            .access_host()
            .decide_request(
                req_id.trim(),
                approve,
                now_ms,
                state.orca_bots.controller(),
            )
            .await
        {
            Ok(outcome) => {
                let label = match outcome.request.status {
                    ConnectorAccessRequestStatus::Granted => "granted",
                    ConnectorAccessRequestStatus::Declined => "declined",
                    ConnectorAccessRequestStatus::Pending => "pending",
                };
                finalize_telegram_card_update(
                    state,
                    chat_id,
                    req,
                    format!(
                        "🔐 Access request `{}` ({}) -> **{}**",
                        outcome.request.request_id, outcome.request.member.name, label
                    ),
                    format!("Access request {}: {}", outcome.request.request_id, label),
                )
            }
            Err(err) => map_bots_err(err),
        };
    }

    orca_err(
        StatusCode::BAD_REQUEST,
        "invalid_argument",
        format!("Unknown orca callback payload 'orca:{rest}'."),
    )
}

/// Router exposing Orca Telegram IM Bridge Fusion routes.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/orca/bots/{bot_id}/telegram/link",
            get(get_telegram_link).post(link_telegram_chat),
        )
        .route(
            "/api/v1/orca/bots/{bot_id}/telegram/card",
            post(post_telegram_card),
        )
}
