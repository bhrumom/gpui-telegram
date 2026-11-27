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
use orca_bots::wire_json::{
    format_ms_rfc3339, item_link_to_wire_json, item_to_wire_json, parse_flexible_item_link,
    parse_rfc3339_or_ms,
};
use orca_bots::{
    AnswerQuestionRequest, CreateBotItemRequest, ListBotItemsFilter, LookAroundProvider,
    LookAroundRunInput, LookAroundRunOutcome, UpdateBotItemFieldsRequest,
};
use orca_core::bot_items::{
    BotItem, BotItemActor, BotItemStatus, ItemCommentRequest, StatusTransitionRequest,
};
use orca_core::bot_look_around::{LookAroundEvery, LookAroundPatch, Suggestion};
use orca_core::bot_memory::{JournalAsker, JournalCloseAttribution};
use orca_core::BotError;
use serde::Deserialize;
use serde_json::{json, Value};

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
    pub include_closed: Option<bool>,
    #[serde(default)]
    pub include_snoozed: Option<bool>,
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
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let stale_policy = bot.items_stale.unwrap_or_default();
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
        include_closed: query.include_closed.unwrap_or(true),
        include_snoozed: query.include_snoozed.unwrap_or(true),
        now_ms: Some(query.now_ms.unwrap_or_else(now_epoch_ms)),
        limit: query.limit,
        offset: query.offset.unwrap_or(0),
    };
    match state.orca_bots.items_store().list_items(filter) {
        Ok(items) => {
            let out: Vec<Value> = items
                .iter()
                .map(|it| item_to_wire_json(it, stale_policy, None))
                .collect();
            Json(out).into_response()
        }
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
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let stale_policy = bot.items_stale.unwrap_or_default();
    let mut parsed_links = Vec::with_capacity(req.links.len());
    for lv in &req.links {
        match parse_flexible_item_link(lv) {
            Ok(l) => parsed_links.push(l),
            Err(err) => return map_bots_err(err),
        }
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
    create.chat_id = req.chat_id.or_else(|| {
        req.telegram_chat_id.as_ref().and_then(|v| {
            v.as_str()
                .map(|s| s.trim().to_string())
                .or_else(|| v.as_i64().map(|n| n.to_string()))
                .filter(|s| !s.is_empty())
        })
    });
    create.group_id = req.group_id;
    let is_approval = req
        .kind
        .as_deref()
        .is_some_and(|k| k.trim().eq_ignore_ascii_case("approval"))
        || req
            .priority
            .as_deref()
            .is_some_and(|p| p.trim().eq_ignore_ascii_case("approval"));
    create.source = if is_approval {
        Some("approval".to_string())
    } else {
        req.source
    };
    create.made_by = req.made_by;
    create.links = parsed_links;

    match state.orca_bots.items_store().create_item(create, now_ms) {
        Ok((item, event)) => (
            StatusCode::CREATED,
            Json(json!({
                "item": item_to_wire_json(&item, stale_policy, None),
                "event": event,
            })),
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
    let stale_policy = state
        .orca_bots
        .controller()
        .get_bot(&item.owner_bot_id)
        .await
        .ok()
        .and_then(|b| b.items_stale)
        .unwrap_or_default();
    let store = state.orca_bots.items_store();
    let events = match store.list_events(&item.uid, None, None) {
        Ok(e) => e,
        Err(err) => return map_bots_err(err),
    };
    let links_deduped = match store.links_one_per_chain(&item.uid) {
        Ok(l) => l,
        Err(err) => return map_bots_err(err),
    };
    let item_wire = item_to_wire_json(&item, stale_policy, Some(&links_deduped));
    let wire_links: Vec<Value> = links_deduped.iter().map(item_link_to_wire_json).collect();
    let fallback = item_wire.get("plainTextFallback").cloned();
    let stale_days = item_wire.get("staleAfterDays").cloned();
    Json(json!({
        "item": item_wire,
        "events": events,
        "links": wire_links,
        "links_deduped": wire_links,
        "ask": item.ask,
        "plainTextFallback": fallback,
        "staleAfterDays": stale_days,
    }))
    .into_response()
}

pub async fn patch_item(
    auth: AuthBearer,
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
            let attribution = match item.made_by.as_ref() {
                Some(BotItemActor::Person {
                    source,
                    user_id,
                    name,
                }) => JournalCloseAttribution::ExternalPeople(vec![JournalAsker {
                    source_name: source.clone(),
                    user_id: user_id.clone(),
                    name: name.clone(),
                }]),
                _ => JournalCloseAttribution::Owner,
            };
            state
                .orca_bots
                .close_item(
                    &item.uid,
                    actor.clone(),
                    target_status,
                    req.close_note.or(req.note.clone()),
                    expected_version,
                    Some(attribution),
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

    if let Some(link_val) = req.link {
        let link = match parse_flexible_item_link(&link_val) {
            Ok(l) => l,
            Err(err) => return map_bots_err(err),
        };
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

    let mut posted_card_msg_id: Option<i64> = None;
    if item.status == BotItemStatus::NeedsYou && item.ask.is_some() {
        let target_chat_id = req
            .telegram_chat_id
            .as_ref()
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
            })
            .or_else(|| {
                item.chat_id
                    .as_deref()
                    .and_then(|s| s.trim().parse::<i64>().ok())
            })
            .or_else(|| state.orca_telegram_links.read().get(&item.owner_bot_id).copied());
        if let Some(cid_raw) = target_chat_id {
            posted_card_msg_id =
                crate::routes::orca_telegram_bridge::post_ask_decision_card_to_chat(
                    &state,
                    auth.user_id,
                    &item,
                    cid_raw,
                )
                .await;
        }
    }

    let stale_policy = state
        .orca_bots
        .controller()
        .get_bot(&item.owner_bot_id)
        .await
        .ok()
        .and_then(|b| b.items_stale)
        .unwrap_or_default();
    Json(json!({
        "item": item_to_wire_json(&item, stale_policy, None),
        "emitted_events": emitted_events,
        "unsnoozed_early": unsnoozed_early,
        "telegram_card_message_id": posted_card_msg_id,
    }))
    .into_response()
}

fn answers_value_to_selections(
    ask: Option<&orca_core::bot_items::BotItemAsk>,
    answers: &Value,
) -> Option<Vec<orca_core::bot_items::AskSelection>> {
    let ask = ask?;
    let obj = answers.as_object()?;
    let mut out = Vec::new();
    for (q_idx, q) in ask.questions.iter().enumerate() {
        let matched_val = obj
            .get(&q.prompt)
            .or_else(|| obj.get(&q_idx.to_string()));
        let Some(v) = matched_val else {
            continue;
        };
        let labels: Vec<String> = if let Some(arr) = v.as_array() {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else if let Some(s) = v.as_str() {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![trimmed.to_string()]
            }
        } else {
            Vec::new()
        };
        if !labels.is_empty() {
            out.push(orca_core::bot_items::AskSelection {
                question_index: q_idx,
                labels,
            });
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

pub async fn comment_item(
    auth: AuthBearer,
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
    let bound_chat_num: Option<i64> = item
        .chat_id
        .as_deref()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .or_else(|| {
            req.chat_id.as_ref().and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
            })
        });
    if let Some(cid_num) = bound_chat_num
        && let Ok(Some(chat)) = state.db.get_chat(gpui_telegram_core::types::ChatId(cid_num))
    {
        let is_member = chat.creator_id == auth.user_id
            || state
                .db
                .get_chat_member(gpui_telegram_core::types::ChatId(cid_num), auth.user_id)
                .ok()
                .flatten()
                .is_some();
        if !is_member {
            return orca_err(
                StatusCode::FORBIDDEN,
                "forbidden",
                "You do not have access to this Telegram-bound item",
            );
        }
    }
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let actor = req.actor.unwrap_or(BotItemActor::Owner);
    let store = state.orca_bots.items_store();

    let res = if let Some(question_event) = req.question_event {
        let selections = req.selections.or_else(|| {
            req.answers
                .as_ref()
                .and_then(|ans| answers_value_to_selections(item.ask.as_ref(), ans))
        });
        store.answer_question(
            &item.uid,
            AnswerQuestionRequest {
                actor,
                question_event,
                selections,
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

    let stale_policy = state
        .orca_bots
        .controller()
        .get_bot(&item.owner_bot_id)
        .await
        .ok()
        .and_then(|b| b.items_stale)
        .unwrap_or_default();
    match res {
        Ok((updated, event)) => Json(json!({
            "item": item_to_wire_json(&updated, stale_policy, None),
            "event": event,
        }))
        .into_response(),
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

    let resolved_until_ms = if let Some(ms) = req.until_ms {
        Some(ms)
    } else if let Some(ref val) = req.snoozed_until {
        if val.is_null() {
            None
        } else {
            match parse_rfc3339_or_ms(val) {
                Ok(ms) => Some(ms),
                Err(err) => return map_bots_err(err),
            }
        }
    } else {
        None
    };

    let res = if req.unsnooze || resolved_until_ms.is_none() {
        store.unsnooze_item(&item.uid, actor, now_ms)
    } else {
        store.snooze_item(&item.uid, actor, resolved_until_ms.unwrap_or(now_ms), now_ms)
    };

    let stale_policy = state
        .orca_bots
        .controller()
        .get_bot(&item.owner_bot_id)
        .await
        .ok()
        .and_then(|b| b.items_stale)
        .unwrap_or_default();

    match res {
        Ok((updated, event)) => Json(json!({
            "item": item_to_wire_json(&updated, stale_policy, None),
            "event": event,
        }))
        .into_response(),
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
    let decorate = |it: &BotItem| -> Value {
        let mut v = serde_json::to_value(it).unwrap_or_else(|_| json!({}));
        let is_app = it
            .source
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("approval"));
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "kind".into(),
                json!(if is_app { "approval" } else { "ask" }),
            );
            obj.insert(
                "priority".into(),
                json!(if is_app { "approval" } else { "normal" }),
            );
        }
        v
    };
    match state.orca_bots.items_store().needs_you_inbox_with_predicate(
        query.bot_id.as_deref(),
        query.focused_uid.as_deref(),
        query.cap,
        now_ms,
        |it| {
            it.source
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("approval"))
        },
    ) {
        Ok(fold) => {
            let visible_wire: Vec<Value> = fold.visible.iter().map(decorate).collect();
            let folded_wire: Vec<Value> = fold.folded.iter().map(decorate).collect();
            let mut items = visible_wire.clone();
            items.extend(folded_wire.clone());
            Json(json!({
                "items": items,
                "visible": visible_wire,
                "visible_items": visible_wire,
                "folded": folded_wire,
                "hidden_count": fold.hidden_count,
                "approval_count": fold.approval_count,
                "focused_preserved": fold.focused_preserved,
            }))
            .into_response()
        }
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
    let sched = &persisted.schedule;
    Json(json!({
        "on": sched.on,
        "every": sched.schedule.every,
        "hours": sched.schedule.every.hours(),
        "weekdaysOnly": sched.schedule.weekdays_only,
        "focus": sched.schedule.focus,
        "since": sched.schedule.since_ms.and_then(format_ms_rfc3339),
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

    let has_top_level_patch = req.on.is_some()
        || req.every.is_some()
        || req.weekdays_only.is_some()
        || req.focus.is_some()
        || req.since.is_some();

    let effective_patch_dto = match req.patch {
        Some(mut dto) => {
            if dto.on.is_none() {
                dto.on = req.on;
            }
            if dto.every.is_none() {
                dto.every = req.every;
            }
            if dto.weekdays_only.is_none() {
                dto.weekdays_only = req.weekdays_only;
            }
            if dto.focus.is_none() {
                dto.focus = req.focus;
            }
            if dto.since.is_none() {
                dto.since = req.since;
            }
            Some(dto)
        }
        None if has_top_level_patch => Some(gpui_telegram_proto::orca_contracts::OrcaLookAroundPatchDto {
            on: req.on.or(Some(true)),
            every: req.every,
            weekdays_only: req.weekdays_only,
            focus: req.focus,
            since: req.since,
        }),
        None => None,
    };

    let mut schedule_opt = None;
    if let Some(patch_dto) = effective_patch_dto {
        let every = match patch_dto.every.as_deref() {
            Some(raw) => match LookAroundEvery::parse(raw) {
                Some(ev) => Some(ev),
                None => {
                    return orca_err(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        format!("Unknown Look Around schedule preset '{raw}'"),
                    );
                }
            },
            None => None,
        };
        let since_ms = match patch_dto.since {
            Some(ref v) if !v.is_null() => match parse_rfc3339_or_ms(v) {
                Ok(ms) => Some(ms),
                Err(err) => return map_bots_err(err),
            },
            _ => None,
        };
        let patch = LookAroundPatch {
            on: patch_dto.on,
            every,
            weekdays_only: patch_dto.weekdays_only,
            focus: patch_dto.focus,
            since_ms,
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
        let sched_ref = schedule_opt.as_ref();
        return Json(json!({
            "on": sched_ref.map(|s| s.on),
            "every": sched_ref.map(|s| s.schedule.every),
            "hours": sched_ref.map(|s| s.schedule.every.hours()),
            "weekdaysOnly": sched_ref.map(|s| s.schedule.weekdays_only),
            "focus": sched_ref.and_then(|s| s.schedule.focus.clone()),
            "since": sched_ref.and_then(|s| s.schedule.since_ms).and_then(format_ms_rfc3339),
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
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let req: OrcaUpNextActionRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    if req.action.is_none() && (req.title.is_some() || req.why.is_some() || req.ask.is_some()) {
        return match state.orca_bots.up_next_store().submit_suggestion(
            &bot.id,
            bot.agent,
            req.title.unwrap_or_default(),
            req.why.unwrap_or_default(),
            req.ask.unwrap_or_default(),
            req.link,
            now_ms,
        ) {
            Ok(submitted) => (
                StatusCode::CREATED,
                Json(json!({
                    "suggestion": submitted.suggestion,
                    "deduped": submitted.deduped,
                    "credentialLinkStripped": submitted.credential_link_stripped,
                })),
            )
                .into_response(),
            Err(err) => map_bots_err(err),
        };
    }

    let action = req.action.as_deref().unwrap_or("").trim().to_ascii_lowercase();
    let Some(suggestion_id) = req
        .suggestion_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Missing 'suggestion_id' for up-next action",
        );
    };
    match action.as_str() {
        "start" => match state
            .orca_bots
            .up_next_store()
            .start_suggestion(&bot_id, suggestion_id)
        {
            Ok(started) => Json(json!({
                "suggestion": started.suggestion,
                "composer_ask": started.composer_ask,
                "ask": started.composer_ask,
            }))
            .into_response(),
            Err(err) => map_bots_err(err),
        },
        "dismiss" => match state
            .orca_bots
            .up_next_store()
            .dismiss_suggestion(&bot_id, suggestion_id, now_ms)
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
