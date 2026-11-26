//! Orca Bot Items CRUD, Multi-Decision Ask Cards, Snooze, Priority-Folded "Needs You" Inbox,
//! Proactive Read-Only Look Around, and Up Next REST routes.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use gpui_telegram_proto::orca_contracts::{
    OrcaCreateItemRequest, OrcaItemCommentRequest, OrcaItemSnoozeRequest, OrcaLookAroundRequest,
    OrcaUpNextActionRequest, OrcaUpdateItemRequest,
};
use orca_bots::{
    AnswerQuestionRequest, CreateBotItemRequest, ListBotItemsFilter, LookAroundProvider,
    LookAroundRunInput, LookAroundRunOutcome, UpdateBotItemFieldsRequest,
};
use orca_core::bot_items::{
    BotItem, BotItemActor, BotItemStatus, ItemCommentRequest, StatusTransitionRequest,
};
use orca_core::bot_look_around::{LookAroundPatch, Suggestion};
use orca_core::BotError;
use serde::Deserialize;
use serde_json::json;

use crate::routes::orca_error::{map_bots_err, orca_err, parse_json_body, parse_optional_json_body};
use crate::server::{AppState, AuthBearer};

fn now_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn resolve_item(state: &AppState, item_num_or_uid: &str) -> Result<BotItem, Response> {
    let store = state.orca_bots.items_store();
    if let Ok(num) = item_num_or_uid.trim().parse::<u64>() {
        match store.get_item_by_number(num) {
            Ok(it) => return Ok(it),
            Err(err) => return Err(map_bots_err(err)),
        }
    }
    store.get_item_by_uid(item_num_or_uid).map_err(map_bots_err)
}

// ---------------------------------------------------------------------------
// 1. Bot Items CRUD & Needs You Inbox
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListItemsQuery {
    #[serde(default)]
    pub include_closed: bool,
    #[serde(default)]
    pub include_snoozed: bool,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub now_ms: Option<i64>,
}

pub async fn list_bot_items(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    Query(query): Query<ListItemsQuery>,
) -> Response {
    if let Err(err) = state.orca_bots.controller().get_bot(&bot_id).await {
        return map_bots_err(err);
    }
    let statuses = match query.status.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => {
            let mut parsed = Vec::new();
            for part in raw.split(',') {
                let Some(st) = BotItemStatus::parse_cli(part.trim()) else {
                    return orca_err(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        format!("Unknown BotItemStatus '{}'", part.trim()),
                    );
                };
                parsed.push(st);
            }
            Some(parsed)
        }
        None => None,
    };
    let filter = ListBotItemsFilter {
        owner_bot_id: Some(bot_id),
        statuses,
        include_closed: query.include_closed,
        include_snoozed: query.include_snoozed,
        now_ms: Some(query.now_ms.unwrap_or_else(now_epoch_ms)),
        limit: query.limit,
        offset: query.offset.unwrap_or(0),
    };
    match state.orca_bots.items_store().list_items(filter) {
        Ok(items) => Json(items).into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn create_bot_item(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaCreateItemRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(err) = state.orca_bots.controller().get_bot(&bot_id).await {
        return map_bots_err(err);
    }
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let actor = req.actor.unwrap_or(BotItemActor::Owner);
    let mut create = CreateBotItemRequest::new(bot_id, actor, req.title);
    create.description = req.description;
    create.done_means = req.done_means;
    if let Some(st) = req.status {
        create.status = st;
    }
    create.ask = req.ask;
    create.wait_note = req.wait_note;
    create.chat_id = req.chat_id;
    create.group_id = req.group_id;
    create.source = req.source;
    create.made_by = req.made_by;
    create.links = req.links;

    match state.orca_bots.items_store().create_item(create, now_ms) {
        Ok((item, event)) => (
            StatusCode::CREATED,
            Json(json!({ "item": item, "event": event })),
        )
            .into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn get_item(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(item_num): Path<String>,
) -> Response {
    let item = match resolve_item(&state, &item_num) {
        Ok(it) => it,
        Err(resp) => return resp,
    };
    let store = state.orca_bots.items_store();
    let events = match store.list_events(&item.uid, None, None) {
        Ok(e) => e,
        Err(err) => return map_bots_err(err),
    };
    let links_deduped = match store.links_one_per_chain(&item.uid) {
        Ok(l) => l,
        Err(err) => return map_bots_err(err),
    };
    Json(json!({
        "item": item,
        "events": events,
        "links_deduped": links_deduped,
    }))
    .into_response()
}

pub async fn patch_item(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(item_num): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaUpdateItemRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let mut item = match resolve_item(&state, &item_num) {
        Ok(it) => it,
        Err(resp) => return resp,
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let actor = req.actor.unwrap_or(BotItemActor::Owner);
    let store = state.orca_bots.items_store();
    let mut expected_version = req.expected_version;
    let mut emitted_events = Vec::new();
    let mut unsnoozed_early = false;
    let mut mutated = false;

    if req.title.is_some() || req.description.is_some() || req.done_means.is_some() {
        match store.update_item_fields(
            UpdateBotItemFieldsRequest {
                item_uid: item.uid.clone(),
                actor: actor.clone(),
                expected_version,
                title: req.title,
                description: req.description,
                done_means: req.done_means,
            },
            now_ms,
        ) {
            Ok((updated, ev)) => {
                item = updated;
                expected_version = Some(item.version);
                emitted_events.push(ev);
                mutated = true;
            }
            Err(err) => return map_bots_err(err),
        }
    }

    if let Some(target_status) = req.status {
        let res = if target_status.is_closed() {
            state
                .orca_bots
                .close_item(
                    &item.uid,
                    actor.clone(),
                    target_status,
                    req.close_note.or(req.note.clone()),
                    expected_version,
                    None,
                    now_ms,
                )
                .await
        } else {
            store.set_status(
                &item.uid,
                StatusTransitionRequest {
                    actor: actor.clone(),
                    to_status: target_status,
                    expected_version,
                    wait_note: req.wait_note,
                    close_note: req.close_note,
                    note: req.note,
                    ask: req.ask,
                },
                now_ms,
            )
        };
        match res {
            Ok((updated, outcome)) => {
                item = updated;
                emitted_events.extend(outcome.emitted_events);
                unsnoozed_early |= outcome.unsnoozed_early;
                mutated = true;
            }
            Err(err) => return map_bots_err(err),
        }
    }

    if let Some(link) = req.link {
        match store.add_link(&item.uid, actor, link, now_ms) {
            Ok((updated, ev)) => {
                item = updated;
                emitted_events.push(ev);
                mutated = true;
            }
            Err(err) => return map_bots_err(err),
        }
    }

    if !mutated {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "PATCH item requires at least one of title, description, done_means, status, or link",
        );
    }

    Json(json!({
        "item": item,
        "emitted_events": emitted_events,
        "unsnoozed_early": unsnoozed_early,
    }))
    .into_response()
}

pub async fn comment_item(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(item_num): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaItemCommentRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let item = match resolve_item(&state, &item_num) {
        Ok(it) => it,
        Err(resp) => return resp,
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let actor = req.actor.unwrap_or(BotItemActor::Owner);
    let store = state.orca_bots.items_store();

    let res = if let Some(question_event) = req.question_event {
        store.answer_question(
            &item.uid,
            AnswerQuestionRequest {
                actor,
                question_event,
                selections: req.selections,
                reply_text: req.reply_text.or(req.text),
                files: req.files,
            },
            now_ms,
        )
    } else {
        let text = req.text.or(req.reply_text).unwrap_or_default();
        store.comment_item(
            &item.uid,
            ItemCommentRequest {
                actor,
                text,
                files: req.files,
                question_event: None,
            },
            now_ms,
        )
    };

    match res {
        Ok((updated, event)) => Json(json!({ "item": updated, "event": event })).into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn snooze_item(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(item_num): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaItemSnoozeRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let item = match resolve_item(&state, &item_num) {
        Ok(it) => it,
        Err(resp) => return resp,
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let actor = req.actor.unwrap_or(BotItemActor::Owner);
    let store = state.orca_bots.items_store();

    let res = if req.unsnooze || req.until_ms.is_none() {
        store.unsnooze_item(&item.uid, actor, now_ms)
    } else {
        store.snooze_item(&item.uid, actor, req.until_ms.unwrap_or(now_ms), now_ms)
    };

    match res {
        Ok((updated, event)) => Json(json!({ "item": updated, "event": event })).into_response(),
        Err(err) => map_bots_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct NeedsYouQuery {
    #[serde(default)]
    pub bot_id: Option<String>,
    #[serde(default, alias = "focused")]
    pub focused_uid: Option<String>,
    #[serde(default)]
    pub cap: Option<usize>,
    #[serde(default)]
    pub now_ms: Option<i64>,
}

pub async fn needs_you(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<NeedsYouQuery>,
) -> Response {
    let now_ms = query.now_ms.unwrap_or_else(now_epoch_ms);
    match state.orca_bots.items_store().needs_you_inbox(
        query.bot_id.as_deref(),
        query.focused_uid.as_deref(),
        query.cap,
        now_ms,
    ) {
        Ok(fold) => Json(json!({
            "visible": fold.visible,
            "folded": fold.folded,
            "hidden_count": fold.hidden_count,
            "approval_count": fold.approval_count,
            "focused_preserved": fold.focused_preserved,
        }))
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

// ---------------------------------------------------------------------------
// 2. Proactive Read-Only Look Around & Up Next Suggestions
// ---------------------------------------------------------------------------

struct ServerLookAroundProvider {
    suggestions: Vec<Suggestion>,
}

impl LookAroundProvider for ServerLookAroundProvider {
    fn gather_suggestions<'a>(
        &'a self,
        _input: LookAroundRunInput,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Vec<Suggestion>, BotError>> + Send + 'a>>
    {
        let out = self.suggestions.clone();
        Box::pin(async move { Ok(out) })
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct LookAroundQuery {
    #[serde(default)]
    pub now_ms: Option<i64>,
    #[serde(default)]
    pub utc_offset_seconds: Option<i32>,
}

pub async fn look_around_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    Query(query): Query<LookAroundQuery>,
) -> Response {
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let persisted = match state.orca_bots.up_next_store().get_state(&bot.id) {
        Ok(s) => s,
        Err(err) => return map_bots_err(err),
    };
    let suggestions = match state.orca_bots.up_next_store().list_suggestions(&bot.id) {
        Ok(s) => s,
        Err(err) => return map_bots_err(err),
    };
    let now_ms = query.now_ms.unwrap_or_else(now_epoch_ms);
    let utc_offset = query.utc_offset_seconds.unwrap_or(0);
    let evaluation = state
        .orca_bots
        .look_around()
        .evaluate_due(&bot.id, bot.agent, now_ms, utc_offset)
        .ok();
    Json(json!({
        "state": persisted,
        "suggestions": suggestions,
        "evaluation": evaluation,
    }))
    .into_response()
}

pub async fn look_around_post(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaLookAroundRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let utc_offset = req.utc_offset_seconds.unwrap_or(0);

    let mut schedule_opt = None;
    if let Some(patch_dto) = req.patch {
        let patch = LookAroundPatch {
            on: patch_dto.on,
            every: patch_dto.every,
            weekdays_only: patch_dto.weekdays_only,
            focus: patch_dto.focus,
        };
        match state
            .orca_bots
            .up_next_store()
            .patch_schedule(&bot.id, bot.agent, patch, now_ms)
        {
            Ok(sched) => schedule_opt = Some(sched),
            Err(err) => return map_bots_err(err),
        }
    }

    let should_run = req
        .run
        .unwrap_or(schedule_opt.is_none() || req.force || req.suggestions.is_some());
    if !should_run {
        let up_next = match state.orca_bots.up_next_store().list_suggestions(&bot.id) {
            Ok(s) => s,
            Err(err) => return map_bots_err(err),
        };
        return Json(json!({
            "schedule": schedule_opt,
            "outcome": "patched",
            "up_next": up_next,
        }))
        .into_response();
    }

    let provider = ServerLookAroundProvider {
        suggestions: req.suggestions.unwrap_or_default(),
    };
    match state
        .orca_bots
        .look_around()
        .run_look_around(&bot, &provider, now_ms, utc_offset, req.force)
        .await
    {
        Ok(LookAroundRunOutcome::SkippedNotDue) => {
            let up_next = state.orca_bots.up_next_store().list_suggestions(&bot.id).ok();
            Json(json!({
                "schedule": schedule_opt,
                "outcome": "skipped_not_due",
                "up_next": up_next,
            }))
            .into_response()
        }
        Ok(LookAroundRunOutcome::Completed {
            up_next,
            added_count,
            filtered_duplicates,
            stripped_credential_links,
        }) => Json(json!({
            "schedule": schedule_opt,
            "outcome": "completed",
            "up_next": up_next,
            "added_count": added_count,
            "filtered_duplicates": filtered_duplicates,
            "stripped_credential_links": stripped_credential_links,
        }))
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn up_next_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> Response {
    if let Err(err) = state.orca_bots.controller().get_bot(&bot_id).await {
        return map_bots_err(err);
    }
    match state.orca_bots.up_next_store().list_suggestions(&bot_id) {
        Ok(up_next) => Json(up_next).into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn up_next_post(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    if let Err(err) = state.orca_bots.controller().get_bot(&bot_id).await {
        return map_bots_err(err);
    }
    let req: OrcaUpNextActionRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    match req.action.trim().to_ascii_lowercase().as_str() {
        "start" => match state
            .orca_bots
            .up_next_store()
            .start_suggestion(&bot_id, &req.suggestion_id)
        {
            Ok(started) => Json(started).into_response(),
            Err(err) => map_bots_err(err),
        },
        "dismiss" => match state
            .orca_bots
            .up_next_store()
            .dismiss_suggestion(&bot_id, &req.suggestion_id, now_ms)
        {
            Ok(dismissed) => Json(dismissed).into_response(),
            Err(err) => map_bots_err(err),
        },
        other => orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("Unknown up-next action '{other}' (expected 'start' or 'dismiss')"),
        ),
    }
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/orca/bots/{bot_id}/items",
            get(list_bot_items).post(create_bot_item),
        )
        .route(
            "/api/v1/orca/items/{item_num}",
            get(get_item).patch(patch_item),
        )
        .route("/api/v1/orca/items/{item_num}/comment", post(comment_item))
        .route("/api/v1/orca/items/{item_num}/snooze", post(snooze_item))
        .route("/api/v1/orca/needs-you", get(needs_you))
        .route(
            "/api/v1/orca/bots/{bot_id}/look-around",
            get(look_around_get).post(look_around_post),
        )
        .route(
            "/api/v1/orca/bots/{bot_id}/up-next",
            get(up_next_get).post(up_next_post),
        )
}
