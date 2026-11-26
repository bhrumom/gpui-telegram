//! Orca Webhooks (6 adapters, rolling-hour 200/300 caps, zero-leak CLI views,
//! external signature-verified ingest, content-free wake push) and Automations / `watch-pr`
//! REST routes (`F09`).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use gpui_telegram_proto::orca_contracts::{
    OrcaAutomationCreateRequest, OrcaAutomationPatchRequest, OrcaWatchPrRequest,
    OrcaWebhookCreateRequest, OrcaWebhookPatchRequest,
};
use orca_bots::{
    CreateAutomationRequest, CreateWebhookRequest, IngestWebhookCallRequest, WakeFrameKind,
};
use orca_core::triggers_webhooks::{
    validate_automation_event_triggers, AutomationBotResultsTo, AutomationBotRunsIn,
    AutomationBotTarget, WatchPrStepAction, WatchPrStopReason, WatchPrWakeOutcome,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::routes::orca_error::{map_bots_err, orca_err, parse_json_body};
use crate::server::{AppState, AuthBearer, OptionalAuthBearer};

fn now_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn now_epoch_u64() -> u64 {
    now_epoch_ms().max(0) as u64
}

// ---------------------------------------------------------------------------
// 1. Webhooks (ZERO-LEAK CLI Views + External 6-Adapter Ingest + Call History)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListWebhooksQuery {
    #[serde(default)]
    pub bot_id: Option<String>,
}

/// `GET /api/v1/orca/webhooks` — returns strictly secret-free `Vec<WebhookCliView>`.
/// NEVER exposes `ingest_url` or `secret_key`.
pub async fn list_webhooks(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListWebhooksQuery>,
) -> Response {
    let bot_id = query
        .bot_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match state.orca_bots.webhook_engine().list_cli_views(bot_id) {
        Ok(views) => Json(views).into_response(),
        Err(err) => map_bots_err(err),
    }
}

/// `POST /api/v1/orca/webhooks` — creates a host-managed webhook (`201 Created`).
///
/// SECURITY INVARIANT (ZERO-LEAK): default response returns ONLY `WebhookCliView`
/// (NEVER `ingest_url` or `secret_key`). Only includes `copy_card` when the caller
/// explicitly opts in via UI-only `include_copy_card: true`.
pub async fn create_webhook(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaWebhookCreateRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let create_req = CreateWebhookRequest {
        bot_id: req.bot_id,
        automation_id: req.automation_id,
        adapter: req.adapter.unwrap_or_default(),
        filters: req.filters.unwrap_or_default(),
        hourly_cap: req.hourly_cap,
        grace_minutes: req.grace_minutes,
        require_signature: req.require_signature.unwrap_or(false),
        now_ms: req.now_ms.unwrap_or_else(now_epoch_ms),
    };
    match state.orca_bots.webhook_engine().create_hook(create_req) {
        Ok((cli_view, copy_card)) => {
            if req.include_copy_card {
                (
                    StatusCode::CREATED,
                    Json(json!({
                        "webhook": cli_view,
                        "copy_card": copy_card,
                    })),
                )
                    .into_response()
            } else {
                (StatusCode::CREATED, Json(cli_view)).into_response()
            }
        }
        Err(err) => map_bots_err(err),
    }
}

/// `GET /api/v1/orca/webhooks/{hook_id}` — returns strictly secret-free `WebhookCliView`.
pub async fn show_webhook(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(hook_id): Path<String>,
) -> Response {
    match state.orca_bots.webhook_engine().show_cli_view(&hook_id) {
        Ok(view) => Json(view).into_response(),
        Err(err) => map_bots_err(err),
    }
}

/// `PATCH /api/v1/orca/webhooks/{hook_id}` — updates JSON-path filters and/or paused state.
pub async fn patch_webhook(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(hook_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaWebhookPatchRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let engine = state.orca_bots.webhook_engine();
    let mut latest = None;
    if let Some(filters) = req.filters {
        match engine.update_hook_filters(&hook_id, filters) {
            Ok(view) => latest = Some(view),
            Err(err) => return map_bots_err(err),
        }
    }
    if let Some(paused) = req.paused {
        match engine.set_hook_paused(&hook_id, paused) {
            Ok(view) => latest = Some(view),
            Err(err) => return map_bots_err(err),
        }
    }
    match latest {
        Some(view) => Json(view).into_response(),
        None => match engine.show_cli_view(&hook_id) {
            Ok(view) => Json(view).into_response(),
            Err(err) => map_bots_err(err),
        },
    }
}

/// `POST /api/v1/orca/webhooks/{hook_id}/rotate-secret` — UI-only secret rotation
/// advancing `epoch` to fence stale deliveries.
pub async fn rotate_webhook_secret(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(hook_id): Path<String>,
) -> Response {
    match state
        .orca_bots
        .webhook_engine()
        .rotate_hook_secret(&hook_id)
    {
        Ok(copy_card) => Json(copy_card).into_response(),
        Err(err) => map_bots_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct IngestWebhookQuery {
    #[serde(default)]
    pub delivery_epoch: Option<u64>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub now_ms: Option<u64>,
}

fn header_map_to_btree(headers: &HeaderMap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, val) in headers.iter() {
        if let Ok(s) = val.to_str() {
            out.insert(name.as_str().to_ascii_lowercase(), s.to_string());
        }
    }
    out
}

/// `POST /api/v1/orca/webhooks/{hook_id}/ingest` — External 6-adapter webhook ingress.
///
/// Uses `OptionalAuthBearer` (NO Bearer token required; authenticated via adapter HMAC/key).
/// Enforces 200/hook & 300/account rolling-hour caps, local payload spooling, per-hook call
/// tracking in `state.orca_webhook_calls`, content-free `WakeFrameKind::WebhookCall` push,
/// and returns a strictly zero-leak receipt (NEVER echoes raw body, headers, or secret).
pub async fn ingest_webhook(
    _opt_auth: OptionalAuthBearer,
    State(state): State<Arc<AppState>>,
    Path(hook_id): Path<String>,
    Query(query): Query<IngestWebhookQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header_btree = header_map_to_btree(&headers);

    let delivery_epoch = query.delivery_epoch.or_else(|| {
        header_btree
            .get("x-orca-delivery-epoch")
            .or_else(|| header_btree.get("x-delivery-epoch"))
            .and_then(|v| v.trim().parse::<u64>().ok())
    });

    let idempotency_key = query
        .idempotency_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            header_btree
                .get("x-idempotency-key")
                .or_else(|| header_btree.get("x-github-delivery"))
                .or_else(|| header_btree.get("x-orca-idempotency-key"))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });

    let now_ms = query.now_ms.unwrap_or_else(now_epoch_u64);
    let engine = state.orca_bots.webhook_engine();

    let outcome = match engine.ingest_call(IngestWebhookCallRequest {
        hook_id: hook_id.clone(),
        headers: header_btree,
        raw_body: body.to_vec(),
        delivery_epoch,
        idempotency_key,
        now_ms,
    }) {
        Ok(o) => o,
        Err(err) => return map_bots_err(err),
    };

    if let Ok(call_rec) = engine.get_call(&outcome.call_id) {
        let mut calls_map = state.orca_webhook_calls.write();
        let entries = calls_map.entry(hook_id.clone()).or_default();
        if let Some(existing) = entries.iter_mut().find(|c| c.call_id == call_rec.call_id) {
            *existing = call_rec;
        } else {
            entries.push(call_rec);
        }
    }

    state
        .orca_wake_hub
        .notify_host("*", WakeFrameKind::WebhookCall);

    // ZERO-LEAK: never echo raw body, headers, spool_path, ingest_url, or secret_key.
    (
        StatusCode::OK,
        Json(json!({
            "call_id": outcome.call_id,
            "receipt_status": outcome.receipt_status,
            "state": outcome.state,
            "start_answer": outcome.start_answer,
            "trigger_event": outcome.trigger_event,
            "notice": outcome.notice,
        })),
    )
        .into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct WebhookCallsQuery {
    #[serde(default)]
    pub call_id: Option<String>,
}

/// `GET /api/v1/orca/webhooks/{hook_id}/calls` — lists tracked calls for `hook_id`
/// (or a single call when `?call_id=` is specified). NEVER exposes URL/secret/raw body.
pub async fn list_webhook_calls(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(hook_id): Path<String>,
    Query(query): Query<WebhookCallsQuery>,
) -> Response {
    let engine = state.orca_bots.webhook_engine();
    if let Err(err) = engine.show_cli_view(&hook_id) {
        return map_bots_err(err);
    }

    if let Some(cid) = query
        .call_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return match engine.get_call(cid) {
            Ok(rec) if rec.hook_id == hook_id => Json(rec).into_response(),
            Ok(_) => orca_err(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("Webhook call '{cid}' does not belong to hook '{hook_id}'."),
            ),
            Err(err) => map_bots_err(err),
        };
    }

    let tracked = state
        .orca_webhook_calls
        .read()
        .get(&hook_id)
        .cloned()
        .unwrap_or_default();
    Json(tracked).into_response()
}

// ---------------------------------------------------------------------------
// 2. Automations & `watch-pr`
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListAutomationsQuery {
    #[serde(default)]
    pub bot_id: Option<String>,
    #[serde(default)]
    pub local_only: bool,
}

/// `GET /api/v1/orca/automations` — lists automations, optionally filtered by `bot_id` / `local_only`.
pub async fn list_automations(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListAutomationsQuery>,
) -> Response {
    let bot_id = query
        .bot_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let engine = state.orca_bots.automation_engine();
    if query.local_only {
        match engine.list_local_host_automations() {
            Ok(mut list) => {
                if let Some(bid) = bot_id {
                    list.retain(|a| a.bot_target.bot_id == bid);
                }
                Json(list).into_response()
            }
            Err(err) => map_bots_err(err),
        }
    } else {
        match engine.list_automations(bot_id) {
            Ok(list) => Json(list).into_response(),
            Err(err) => map_bots_err(err),
        }
    }
}

/// `POST /api/v1/orca/automations` — registers a validated bot automation (`201 Created`).
pub async fn create_automation(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaAutomationCreateRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot_target = match req.bot_target {
        Some(mut bt) => {
            if bt.bot_id.trim().is_empty() {
                bt.bot_id = req.bot_id.clone();
            }
            bt
        }
        None => AutomationBotTarget {
            bot_id: req.bot_id,
            runs_in: match req
                .chat_session_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(sid) => AutomationBotRunsIn::Chat {
                    session_id: sid.to_string(),
                    title: None,
                    home_if_gone: None,
                    to_conversation: None,
                },
                None => AutomationBotRunsIn::New,
            },
            results_to: AutomationBotResultsTo::Chat,
            agent_setup: None,
            let_bot_react: None,
            precheck: None,
            setup_trust_hash: None,
            missed_run_grace_minutes: None,
        },
    };
    let now_ms = req.now_ms.map(|v| v.max(0) as u64).unwrap_or_else(now_epoch_u64);
    let create_req = CreateAutomationRequest {
        name: req.name,
        prompt: req.prompt,
        enabled: req.enabled.unwrap_or(true),
        bot_target,
        dtstart_ms: req.dtstart_ms,
        rrule: req.rrule,
        triggers: req.triggers,
        once: req.once,
        now_ms,
    };
    match state
        .orca_bots
        .automation_engine()
        .register_automation(create_req)
    {
        Ok(record) => (StatusCode::CREATED, Json(record)).into_response(),
        Err(err) => map_bots_err(err),
    }
}

/// `PATCH /api/v1/orca/automations/{id}` — enables/disables or edits an existing automation.
pub async fn patch_automation(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaAutomationPatchRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = req.now_ms.map(|v| v.max(0) as u64).unwrap_or_else(now_epoch_u64);
    let engine = state.orca_bots.automation_engine();

    let has_field_edits = req.name.is_some()
        || req.prompt.is_some()
        || req.rrule.is_some()
        || req.triggers.is_some();

    if !has_field_edits {
        if let Some(enabled) = req.enabled {
            return match engine.set_automation_enabled(&id, enabled, now_ms) {
                Ok(rec) => Json(rec).into_response(),
                Err(err) => map_bots_err(err),
            };
        }
        return match engine.list_automations(None) {
            Ok(list) => match list.into_iter().find(|a| a.id == id) {
                Some(rec) => Json(rec).into_response(),
                None => orca_err(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    format!("Automation '{id}' not found."),
                ),
            },
            Err(err) => map_bots_err(err),
        };
    }

    let existing = match engine.list_automations(None) {
        Ok(list) => match list.into_iter().find(|a| a.id == id) {
            Some(rec) => rec,
            None => {
                return orca_err(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    format!("Automation '{id}' not found."),
                );
            }
        },
        Err(err) => return map_bots_err(err),
    };

    let is_event_only = existing.is_event_only();
    let existing_rrule = existing.rrule;
    let next_name = req.name.unwrap_or(existing.name);
    let next_prompt = req.prompt.unwrap_or(existing.prompt);
    if next_name.trim().is_empty() || next_prompt.trim().is_empty() {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Automation name and prompt must be non-empty.",
        );
    }
    let next_triggers = req.triggers.unwrap_or(existing.triggers);
    let next_dtstart = if is_event_only {
        None
    } else {
        Some(existing.dtstart_ms)
    };
    let next_rrule = req.rrule.or_else(|| {
        if is_event_only {
            None
        } else {
            Some(existing_rrule)
        }
    });
    let validate_dtstart = next_dtstart.unwrap_or(orca_bots::EVENT_ONLY_SCHEDULE.dtstart_ms);
    if let Err(err) = validate_automation_event_triggers(
        &next_triggers,
        Some(&existing.bot_target),
        validate_dtstart,
        existing.once.as_ref(),
    ) {
        return map_bots_err(orca_bots::OrcaBotsError::Bot(err));
    }

    let _ = engine.remove_automation(&id);
    match engine.register_automation(CreateAutomationRequest {
        name: next_name,
        prompt: next_prompt,
        enabled: req.enabled.unwrap_or(existing.enabled),
        bot_target: existing.bot_target,
        dtstart_ms: next_dtstart,
        rrule: next_rrule,
        triggers: next_triggers,
        once: existing.once,
        now_ms,
    }) {
        Ok(rec) => Json(rec).into_response(),
        Err(err) => map_bots_err(err),
    }
}

/// `DELETE /api/v1/orca/automations/{id}` — removes an automation (`404 not_found` if absent).
pub async fn delete_automation(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match state.orca_bots.automation_engine().remove_automation(&id) {
        Ok(true) => Json(json!({ "removed": true, "automation_id": id })).into_response(),
        Ok(false) => orca_err(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("Automation '{id}' not found."),
        ),
        Err(err) => map_bots_err(err),
    }
}

/// `POST /api/v1/orca/automations/watch-pr` — starts a 2-attempt bounded PR watch (`201 Created`).
pub async fn start_watch_pr(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaWatchPrRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = req.now_ms.map(|v| v.max(0) as u64).unwrap_or_else(now_epoch_u64);
    let session_id = req
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("home");
    match state.orca_bots.automation_engine().watch_pr().start_watch(
        req.bot_id,
        session_id,
        &req.pr,
        now_ms,
        req.duration.as_deref(),
        req.ci_finished_supported.unwrap_or(true),
    ) {
        Ok(watch) => (StatusCode::CREATED, Json(watch)).into_response(),
        Err(err) => map_bots_err(err),
    }
}

fn parse_watch_pr_wake_outcome(val: &Value) -> Result<WatchPrWakeOutcome, Response> {
    let outcome_field = val.get("outcome").unwrap_or(val);
    if let Some(s) = outcome_field.as_str() {
        return match s.trim().to_ascii_lowercase().as_str() {
            "merged_or_closed" | "merged" | "closed" => Ok(WatchPrWakeOutcome::MergedOrClosed),
            "ready_to_merge" | "ready" => Ok(WatchPrWakeOutcome::ReadyToMerge),
            "needs_change" | "needs_fix" => {
                let authorized = val
                    .get("authorized")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Ok(WatchPrWakeOutcome::NeedsChange { authorized })
            }
            "still_running_or_question" | "still_running" | "question" => {
                Ok(WatchPrWakeOutcome::StillRunningOrQuestion)
            }
            other => Err(orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Unknown watch-pr wake outcome '{other}'."),
            )),
        };
    }
    if let Some(obj) = outcome_field.as_object() {
        if let Some(nc) = obj.get("needs_change") {
            let authorized = nc
                .get("authorized")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            return Ok(WatchPrWakeOutcome::NeedsChange { authorized });
        }
        if let Some(kind) = obj.get("kind").and_then(Value::as_str) {
            return match kind.trim().to_ascii_lowercase().as_str() {
                "merged_or_closed" => Ok(WatchPrWakeOutcome::MergedOrClosed),
                "ready_to_merge" => Ok(WatchPrWakeOutcome::ReadyToMerge),
                "needs_change" => {
                    let authorized = obj
                        .get("authorized")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Ok(WatchPrWakeOutcome::NeedsChange { authorized })
                }
                "still_running_or_question" => Ok(WatchPrWakeOutcome::StillRunningOrQuestion),
                other => Err(orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unknown watch-pr wake outcome kind '{other}'."),
                )),
            };
        }
    }
    Err(orca_err(
        StatusCode::BAD_REQUEST,
        "invalid_argument",
        "Missing or invalid 'outcome' for watch-pr wake.",
    ))
}

fn watch_pr_action_to_json(action: WatchPrStepAction) -> Value {
    match action {
        WatchPrStepAction::AnnounceAndStop { reason } => json!({
            "action": "announce_and_stop",
            "reason": match reason {
                WatchPrStopReason::MergedOrClosed => "merged_or_closed",
                WatchPrStopReason::ReadyToMerge => "ready_to_merge",
            },
        }),
        WatchPrStepAction::NotifyOwnerAndRearmSameAttempt { attempt } => json!({
            "action": "notify_owner_and_rearm_same_attempt",
            "attempt": attempt,
        }),
        WatchPrStepAction::DispatchWorkerFix {
            attempt,
            next_attempt,
            rearm_after_worker_push,
        } => json!({
            "action": "dispatch_worker_fix",
            "attempt": attempt,
            "next_attempt": next_attempt,
            "rearm_after_worker_push": rearm_after_worker_push,
        }),
        WatchPrStepAction::EscalateToOwnerAndStop { attempt } => json!({
            "action": "escalate_to_owner_and_stop",
            "attempt": attempt,
        }),
        WatchPrStepAction::ForwardQuestionAndRearmSameAttempt { attempt } => json!({
            "action": "forward_question_and_rearm_same_attempt",
            "attempt": attempt,
        }),
    }
}

/// `GET /api/v1/orca/automations/watch-pr/{watch_id}` — inspects an active or stopped PR watch.
pub async fn get_watch_pr(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(watch_id): Path<String>,
) -> Response {
    let coordinator = state.orca_bots.automation_engine().watch_pr();
    match coordinator.get_watch(&watch_id) {
        Ok(watch) => {
            let events = coordinator.required_github_events(&watch_id).unwrap_or_default();
            Json(json!({
                "watch": watch,
                "required_github_events": events,
            }))
            .into_response()
        }
        Err(err) => map_bots_err(err),
    }
}

/// `POST /api/v1/orca/automations/watch-pr/{watch_id}/wake` — evaluates a PR wake step,
/// enforcing the hard 2-attempt worker-fix cap and deadline expiry.
pub async fn wake_watch_pr(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(watch_id): Path<String>,
    body: Bytes,
) -> Response {
    let val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let outcome = match parse_watch_pr_wake_outcome(&val) {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    let now_ms = val
        .get("now_ms")
        .or_else(|| val.get("nowMs"))
        .and_then(Value::as_u64)
        .unwrap_or_else(now_epoch_u64);

    let coordinator = state.orca_bots.automation_engine().watch_pr();
    let step = match coordinator.handle_wake(&watch_id, outcome, now_ms) {
        Ok(s) => s,
        Err(err) => return map_bots_err(err),
    };
    let watch = match coordinator.get_watch(&watch_id) {
        Ok(w) => w,
        Err(err) => return map_bots_err(err),
    };
    Json(json!({
        "watch_id": watch_id,
        "step": watch_pr_action_to_json(step),
        "watch": watch,
    }))
    .into_response()
}

/// Router exposing all Orca Webhooks and Automations / `watch-pr` routes.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/orca/webhooks", get(list_webhooks).post(create_webhook))
        .route(
            "/api/v1/orca/webhooks/{hook_id}",
            get(show_webhook).patch(patch_webhook),
        )
        .route(
            "/api/v1/orca/webhooks/{hook_id}/rotate-secret",
            post(rotate_webhook_secret),
        )
        .route("/api/v1/orca/webhooks/{hook_id}/ingest", post(ingest_webhook))
        .route("/api/v1/orca/webhooks/{hook_id}/calls", get(list_webhook_calls))
        .route(
            "/api/v1/orca/automations",
            get(list_automations).post(create_automation),
        )
        .route(
            "/api/v1/orca/automations/{id}",
            axum::routing::patch(patch_automation).delete(delete_automation),
        )
        .route("/api/v1/orca/automations/watch-pr", post(start_watch_pr))
        .route(
            "/api/v1/orca/automations/watch-pr/{watch_id}",
            get(get_watch_pr),
        )
        .route(
            "/api/v1/orca/automations/watch-pr/{watch_id}/wake",
            post(wake_watch_pr),
        )
}
