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
    BotAutomationRecord, CreateAutomationRequest, CreateWebhookRequest, IngestWebhookCallRequest,
    WakeFrameKind, WebhookCallRecord, WebhookCliView,
};
use orca_core::triggers_webhooks::{
    validate_automation_event_triggers, AutomationBotResultsTo, AutomationBotRunsIn,
    AutomationBotTarget, AutomationConnectorEventTrigger, AutomationEventKind,
    AutomationEventSource, AutomationEventTrigger, AutomationGoogleTrigger,
    AutomationWebhookTrigger, GitHubEventKind, OneShotWait, WatchPrStepAction, WatchPrStopReason,
    WatchPrWakeOutcome, WebhookFilters, WebhookWhereClause, WebhookWhereOp,
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

/// Strictly secret-free wire view of a webhook (`WebhookCliView` + `"id"`).
/// STRUCTURAL INVARIANT: NEVER includes `url`, `ingestUrl`, `secret`, or `signingSecret`.
fn webhook_cli_view_to_json(view: &WebhookCliView) -> Value {
    let mut val = serde_json::to_value(view).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".into(), json!(view.hook_id));
    }
    val
}

fn normalize_webhook_filter_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("body.")
        || trimmed.starts_with("headers.")
        || trimmed == "body"
        || trimmed == "headers"
        || trimmed.contains("__proto__")
        || trimmed.contains("prototype")
        || trimmed.contains("constructor")
    {
        trimmed.to_string()
    } else {
        format!("body.{trimmed}")
    }
}

fn value_to_filter_strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Bool(b) => vec![b.to_string()],
        Value::Number(n) => vec![n.to_string()],
        Value::Array(arr) => arr
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.clone()),
                Value::Bool(b) => Some(b.to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_flexible_webhook_filters(val: &Value) -> Result<WebhookFilters, Response> {
    if val.is_null() {
        return Ok(WebhookFilters::default());
    }
    let Some(obj) = val.as_object() else {
        return Err(orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Webhook filters/rules must be a JSON object.",
        ));
    };
    let mut where_clauses = Vec::new();
    if let Some(arr) = obj.get("where").and_then(Value::as_array) {
        for item in arr {
            let Some(cobj) = item.as_object() else {
                return Err(orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "Each webhook 'where' clause must be an object.",
                ));
            };
            let raw_path = cobj
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("");
            let path = normalize_webhook_filter_path(raw_path);
            let raw_op = cobj
                .get("op")
                .and_then(Value::as_str)
                .unwrap_or("");
            let op = match raw_op.trim() {
                "=" | "==" | "eq" => WebhookWhereOp::Eq,
                "!=" | "ne" | "neq" => WebhookWhereOp::NotEq,
                "in" => WebhookWhereOp::In,
                "contains" => WebhookWhereOp::Contains,
                "exists" => WebhookWhereOp::Exists,
                "!exists" | "not_exists" => WebhookWhereOp::NotExists,
                other => {
                    return Err(orca_err(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        format!("Unknown webhook filter operator '{other}'."),
                    ));
                }
            };
            let values = match op {
                WebhookWhereOp::Exists | WebhookWhereOp::NotExists => Vec::new(),
                _ => {
                    if let Some(vs) = cobj.get("values") {
                        value_to_filter_strings(vs)
                    } else if let Some(v) = cobj.get("value") {
                        value_to_filter_strings(v)
                    } else {
                        Vec::new()
                    }
                }
            };
            where_clauses.push(WebhookWhereClause { path, op, values });
        }
    }
    let pick = obj
        .get("pick")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(normalize_webhook_filter_path)
                .collect()
        })
        .unwrap_or_default();
    let text_contains = obj
        .get("textContains")
        .or_else(|| obj.get("text_contains"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let subject_from = obj
        .get("subjectFrom")
        .or_else(|| obj.get("subject_from"))
        .and_then(Value::as_str)
        .map(normalize_webhook_filter_path);
    let subject = obj
        .get("subject")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Ok(WebhookFilters {
        where_clauses,
        text_contains,
        subject_from,
        subject,
        pick,
    })
}

/// `GET /api/v1/orca/webhooks` — returns strictly secret-free `Vec<WebhookCliView>`.
/// NEVER exposes `url`, `ingestUrl`, `secret`, or `signingSecret`.
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
        Ok(views) => {
            let wire: Vec<Value> = views.iter().map(webhook_cli_view_to_json).collect();
            Json(wire).into_response()
        }
        Err(err) => map_bots_err(err),
    }
}

/// `POST /api/v1/orca/webhooks` — creates a host-managed webhook (`201 Created`).
///
/// Returns the one-time `url`/`ingestUrl` and `secret`/`signingSecret` ONLY on creation;
/// subsequent `GET /webhooks`, `GET /webhooks/{id}`, `GET /webhooks/{id}/calls`, and ALL
/// `fabushi webhooks` CLI commands strictly omit them.
pub async fn create_webhook(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaWebhookCreateRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let filters = match req.filters.as_ref() {
        Some(v) => match parse_flexible_webhook_filters(v) {
            Ok(f) => f,
            Err(resp) => return resp,
        },
        None => WebhookFilters::default(),
    };
    let automation_id = req
        .automation_id
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("auto_{}", uuid::Uuid::new_v4().simple()));
    let create_req = CreateWebhookRequest {
        bot_id: req.bot_id,
        automation_id,
        adapter: req.adapter.unwrap_or_default(),
        filters,
        hourly_cap: req.hourly_cap,
        grace_minutes: req.grace_minutes,
        require_signature: req.require_signature.unwrap_or(false),
        now_ms: req.now_ms.unwrap_or_else(now_epoch_ms),
    };
    match state
        .orca_bots
        .webhook_engine()
        .create_hook_named(create_req, req.name)
    {
        Ok((cli_view, copy_card)) => {
            let mut created = webhook_cli_view_to_json(&cli_view);
            if let Some(obj) = created.as_object_mut() {
                obj.insert("url".into(), json!(copy_card.ingest_url));
                obj.insert("ingestUrl".into(), json!(copy_card.ingest_url));
                obj.insert("secret".into(), json!(copy_card.secret_key));
                obj.insert("signingSecret".into(), json!(copy_card.secret_key));
                if req.include_copy_card {
                    obj.insert("webhook".into(), webhook_cli_view_to_json(&cli_view));
                    obj.insert("copy_card".into(), json!(copy_card));
                }
            }
            (StatusCode::CREATED, Json(created)).into_response()
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
        Ok(view) => Json(webhook_cli_view_to_json(&view)).into_response(),
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
    if let Some(ref fval) = req.filters {
        let filters = match parse_flexible_webhook_filters(fval) {
            Ok(f) => f,
            Err(resp) => return resp,
        };
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
        Some(view) => Json(webhook_cli_view_to_json(&view)).into_response(),
        None => match engine.show_cli_view(&hook_id) {
            Ok(view) => Json(webhook_cli_view_to_json(&view)).into_response(),
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

fn webhook_call_to_wire_json(rec: &WebhookCallRecord) -> Value {
    let state_str = if rec.filter_rejected {
        "filtered"
    } else {
        rec.state.as_str()
    };
    json!({
        "id": rec.call_id,
        "callId": rec.call_id,
        "hookId": rec.hook_id,
        "botId": rec.bot_id,
        "adapter": rec.adapter,
        "epoch": rec.epoch,
        "state": state_str,
        "filterRejected": rec.filter_rejected,
        "receivedAtMs": rec.received_at_ms,
        "updatedAtMs": rec.updated_at_ms,
    })
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
            Ok(rec) if rec.hook_id == hook_id => Json(webhook_call_to_wire_json(&rec)).into_response(),
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
    let wire: Vec<Value> = tracked.iter().map(webhook_call_to_wire_json).collect();
    Json(wire).into_response()
}

// ---------------------------------------------------------------------------
// 2. Automations & `watch-pr`
// ---------------------------------------------------------------------------

fn automation_to_wire_json(rec: &BotAutomationRecord) -> Value {
    let local_only = rec.has_local_only_triggers();
    let mut val = serde_json::to_value(rec).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("localOnly".into(), json!(local_only));
        obj.insert(
            "executionScope".into(),
            json!(if local_only { "local" } else { "cloud" }),
        );
    }
    val
}

fn parse_flexible_automation_trigger(
    val: &Value,
    idx: usize,
) -> Result<AutomationEventTrigger, Response> {
    if let Ok(t) = serde_json::from_value::<AutomationEventTrigger>(val.clone()) {
        return Ok(t);
    }
    let Some(obj) = val.as_object() else {
        return Err(orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Automation trigger must be a JSON object.",
        ));
    };
    let src_str = obj
        .get("source")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let kind_str = obj
        .get("kind")
        .or_else(|| obj.get("event"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let source = AutomationEventSource::parse(src_str).ok_or_else(|| {
        orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("Unknown automation trigger source '{src_str}'."),
        )
    })?;
    let trigger_id = obj
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("trg-{}-{}", source.as_str(), idx + 1));

    match source {
        AutomationEventSource::Slack
        | AutomationEventSource::Github
        | AutomationEventSource::Linear => {
            let kind = AutomationEventKind::parse(src_str, kind_str).ok_or_else(|| {
                orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unknown event kind '{kind_str}' for source '{src_str}'."),
                )
            })?;
            let str_vec = |k: &str| -> Vec<String> {
                obj.get(k)
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(ToOwned::to_owned)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let is_workflow_finished = matches!(
                kind,
                AutomationEventKind::Github(GitHubEventKind::WorkflowFinished)
            );
            let mut in_places = str_vec("in");
            if is_workflow_finished && in_places.is_empty() && obj.get("in").is_none() {
                in_places.push("github:1:1".to_string());
            }
            let run = match obj.get("run").and_then(Value::as_str) {
                Some(r) => Some(r.to_owned()),
                None if is_workflow_finished && obj.get("run").is_none() => Some("1".to_owned()),
                None => None,
            };
            Ok(AutomationEventTrigger::Connector(
                AutomationConnectorEventTrigger {
                    id: trigger_id,
                    kind,
                    in_places,
                    from: str_vec("from"),
                    text_contains: obj
                        .get("textContains")
                        .or_else(|| obj.get("text_contains"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    subject: match obj.get("subject").and_then(Value::as_str) {
                        Some(s) => Some(s.to_owned()),
                        None if kind.is_pr_state_event() => Some("*".to_owned()),
                        None => None,
                    },
                    label: obj
                        .get("label")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    run,
                    on_owner_message: obj
                        .get("onOwnerMessage")
                        .or_else(|| obj.get("on_owner_message"))
                        .and_then(Value::as_bool),
                    from_owner: obj
                        .get("fromOwner")
                        .or_else(|| obj.get("from_owner"))
                        .and_then(Value::as_bool),
                    names: Default::default(),
                },
            ))
        }
        AutomationEventSource::Gmail => {
            if !kind_str.is_empty() && AutomationEventKind::parse(src_str, kind_str).is_none() {
                return Err(orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unknown event kind '{kind_str}' for source 'gmail'."),
                ));
            }
            Ok(AutomationEventTrigger::Google(AutomationGoogleTrigger::Gmail {
                id: trigger_id,
                account: obj
                    .get("account")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                from: Vec::new(),
                to: Vec::new(),
                subject_contains: obj
                    .get("subjectContains")
                    .or_else(|| obj.get("subject_contains"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                label: obj
                    .get("label")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            }))
        }
        AutomationEventSource::Calendar => {
            if !kind_str.is_empty() && AutomationEventKind::parse(src_str, kind_str).is_none() {
                return Err(orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unknown event kind '{kind_str}' for source 'calendar'."),
                ));
            }
            Ok(AutomationEventTrigger::Google(
                AutomationGoogleTrigger::Calendar {
                    id: trigger_id,
                    account: obj
                        .get("account")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    calendar: obj
                        .get("calendar")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    minutes_before: obj
                        .get("minutesBefore")
                        .or_else(|| obj.get("minutes_before"))
                        .and_then(Value::as_u64)
                        .map(|v| v as u32)
                        .unwrap_or(15),
                },
            ))
        }
        AutomationEventSource::Webhook => {
            if !kind_str.is_empty()
                && !matches!(kind_str, "call" | "webhook" | "received")
                && AutomationEventKind::parse(src_str, kind_str).is_none()
            {
                return Err(orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unknown event kind '{kind_str}' for source 'webhook'."),
                ));
            }
            let hook_rule = if trigger_id.starts_with("wh-wh_") {
                trigger_id
            } else {
                format!("wh-wh_{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
            };
            let filters = match obj.get("filters").or_else(|| obj.get("rules")) {
                Some(fv) => parse_flexible_webhook_filters(fv)?,
                None => WebhookFilters::default(),
            };
            Ok(AutomationEventTrigger::Webhook(AutomationWebhookTrigger {
                id: hook_rule,
                rev: 1,
                filters,
            }))
        }
    }
}

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
                let wire: Vec<Value> = list.iter().map(automation_to_wire_json).collect();
                Json(wire).into_response()
            }
            Err(err) => map_bots_err(err),
        }
    } else {
        match engine.list_automations(bot_id) {
            Ok(list) => {
                let wire: Vec<Value> = list.iter().map(automation_to_wire_json).collect();
                Json(wire).into_response()
            }
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
    let mut triggers = Vec::new();
    if let Some(ref single) = req.trigger {
        match parse_flexible_automation_trigger(single, 0) {
            Ok(t) => triggers.push(t),
            Err(resp) => return resp,
        }
    }
    for (idx, raw_t) in req.triggers.iter().enumerate() {
        match parse_flexible_automation_trigger(raw_t, triggers.len() + idx) {
            Ok(t) => triggers.push(t),
            Err(resp) => return resp,
        }
    }

    let resolved_runs_in = if let Some(ref ri) = req.runs_in {
        if let Some(s) = ri.as_str() {
            match s.trim().to_ascii_lowercase().as_str() {
                "new" => AutomationBotRunsIn::New,
                "chat" => AutomationBotRunsIn::Chat {
                    session_id: req
                        .chat_session_id
                        .clone()
                        .filter(|x| !x.trim().is_empty())
                        .unwrap_or_else(|| "home".to_string()),
                    title: None,
                    home_if_gone: None,
                    to_conversation: None,
                },
                _ => AutomationBotRunsIn::New,
            }
        } else {
            serde_json::from_value(ri.clone()).unwrap_or(AutomationBotRunsIn::New)
        }
    } else if let Some(sid) = req
        .chat_session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        AutomationBotRunsIn::Chat {
            session_id: sid.to_string(),
            title: None,
            home_if_gone: None,
            to_conversation: None,
        }
    } else {
        AutomationBotRunsIn::New
    };

    let resolved_results_to = if let Some(ref rt) = req.results_to {
        if let Some(s) = rt.as_str() {
            match s.trim().to_ascii_lowercase().as_str() {
                "chat" => AutomationBotResultsTo::Chat,
                _ => AutomationBotResultsTo::Chat,
            }
        } else {
            serde_json::from_value(rt.clone()).unwrap_or(AutomationBotResultsTo::Chat)
        }
    } else {
        AutomationBotResultsTo::Chat
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
            runs_in: resolved_runs_in,
            results_to: resolved_results_to,
            agent_setup: None,
            let_bot_react: None,
            precheck: None,
            setup_trust_hash: None,
            missed_run_grace_minutes: None,
        },
    };

    let dtstart_ms = if req.dtstart_ms.is_some() {
        req.dtstart_ms
    } else if let Some(ref sched) = req.schedule {
        match sched.get("dtstart").and_then(Value::as_str) {
            Some(s) if s.starts_with("9999-01-01") => None,
            Some(s) => chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.timestamp_millis()),
            None => sched.get("dtstartMs").and_then(Value::as_i64),
        }
    } else {
        None
    };

    let rrule = req.rrule.or_else(|| {
        req.schedule
            .as_ref()
            .and_then(|s| s.get("rrule"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    });

    let prompt = req
        .prompt
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("Handle automation {}", req.name));

    let now_ms = req.now_ms.map(|v| v.max(0) as u64).unwrap_or_else(now_epoch_u64);
    let has_workflow_finished = triggers.iter().any(|t| {
        matches!(
            t,
            AutomationEventTrigger::Connector(c)
                if matches!(c.kind, AutomationEventKind::Github(GitHubEventKind::WorkflowFinished))
        )
    });
    let once = req.once.or_else(|| {
        has_workflow_finished.then_some(OneShotWait {
            by: Some(now_ms.saturating_add(86_400_000)),
        })
    });
    let create_req = CreateAutomationRequest {
        name: req.name,
        prompt,
        enabled: req.enabled.unwrap_or(true),
        bot_target,
        dtstart_ms,
        rrule,
        triggers,
        once,
        now_ms,
    };
    match state
        .orca_bots
        .automation_engine()
        .register_automation(create_req)
    {
        Ok(record) => (StatusCode::CREATED, Json(automation_to_wire_json(&record))).into_response(),
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
    let pr_target = if let Some(pr) = req.pr.filter(|s| !s.trim().is_empty()) {
        pr
    } else if let (Some(repo), Some(num)) = (
        req.repo.filter(|s| !s.trim().is_empty()),
        req.pr_number,
    ) {
        format!("{repo}#{num}")
    } else {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Provide 'pr' (owner/repo#n) or both 'repo' and 'prNumber' for watch-pr.",
        );
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
        &pr_target,
        now_ms,
        req.duration.as_deref(),
        req.ci_finished_supported.unwrap_or(true),
    ) {
        Ok(watch) => {
            let mut val = serde_json::to_value(&watch).unwrap_or_else(|_| json!({}));
            if let Some(obj) = val.as_object_mut() {
                obj.insert("id".into(), json!(watch.watch_id));
            }
            (StatusCode::CREATED, Json(val)).into_response()
        }
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
