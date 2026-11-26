//! Durable Multi-Agent Orchestration REST routes (`F11`):
//! Runs, Task DAG transitions, Generation-Fenced Workers & Archives,
//! 9-Type Typed Mailbox (structured or `@all`/`@idle`/`@agent`/`@worktree` shorthand),
//! Questions, Decision/Review/Merge Gates, and Host Federation.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, patch, post},
    Router,
};
use orca_bots::WakeFrameKind;
use orca_core::orchestration_protocol::{
    ExecutionHostRecord, HostSelectionCriteria, MailboxMessageType, MailboxPriority, MailboxRoute,
    MailboxTarget, OrchestrationTaskStatus,
};
use orca_ide::orchestration_engine::{
    parse_mailbox_target, CheckMailboxRequest, CompleteWorkerRequest, CreateGateRequest,
    CreateRunRequest, CreateTaskRequest, OrchestrationRunStatus, ResolveGateAction,
    SendMessageRequest, StartWorkerRequest,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::routes::orca_error::{map_ide_err, orca_err, parse_json_body, parse_optional_json_body};
use crate::server::{AppState, AuthBearer};

// ---------------------------------------------------------------------------
// 1. Runs & Tasks DAG
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListRunsQuery {
    #[serde(default)]
    pub status: Option<String>,
}

/// `POST /api/v1/orca/orchestration/runs` (`201 Created`).
pub async fn create_run(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: CreateRunRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state.orca_ide.orchestration().create_run(req) {
        Ok(run) => (StatusCode::CREATED, Json(run)).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `GET /api/v1/orca/orchestration/runs` (`200 OK`, optional `?status=`).
pub async fn list_runs(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListRunsQuery>,
) -> Response {
    let status = match query
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => match OrchestrationRunStatus::parse(raw) {
            Ok(st) => Some(st),
            Err(err) => return map_ide_err(err),
        },
        None => None,
    };
    match state.orca_ide.orchestration().list_runs(status) {
        Ok(runs) => Json(runs).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `GET /api/v1/orca/orchestration/runs/{run_id}` (`200 OK` -> `{run, ready_tasks}`).
pub async fn get_run(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Response {
    let eng = state.orca_ide.orchestration();
    let run = match eng.get_run(&run_id) {
        Ok(r) => r,
        Err(err) => return map_ide_err(err),
    };
    let ready_tasks = match eng.ready_tasks(&run_id) {
        Ok(tasks) => tasks,
        Err(err) => return map_ide_err(err),
    };
    Json(json!({
        "run": run,
        "ready_tasks": ready_tasks,
    }))
    .into_response()
}

/// `POST /api/v1/orca/orchestration/tasks` (`201 Created`).
pub async fn create_task(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: CreateTaskRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state.orca_ide.orchestration().create_task(req) {
        Ok(task) => (StatusCode::CREATED, Json(task)).into_response(),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Deserialize)]
pub struct TransitionTaskBody {
    #[serde(alias = "runId")]
    pub run_id: String,
    #[serde(alias = "next")]
    pub status: OrchestrationTaskStatus,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// `PATCH /api/v1/orca/orchestration/tasks/{task_id}` (`200 OK` -> `{task, newly_ready}`).
pub async fn transition_task(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: TransitionTaskBody = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state
        .orca_ide
        .orchestration()
        .transition_task(&req.run_id, &task_id, req.status, req.now_ms)
    {
        Ok((task, newly_ready)) => Json(json!({
            "task": task,
            "newly_ready": newly_ready,
        }))
        .into_response(),
        Err(err) => map_ide_err(err),
    }
}

// ---------------------------------------------------------------------------
// 2. Generation-Fenced Workers, Terminal Archives & Resource Release
// ---------------------------------------------------------------------------

/// `POST /api/v1/orca/orchestration/workers/start` (`201 Created`).
pub async fn start_worker(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: StartWorkerRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state.orca_ide.orchestration().start_worker(req) {
        Ok(res) => (StatusCode::CREATED, Json(res)).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `POST /api/v1/orca/orchestration/workers/done` (`200 OK`, `409 stale_dispatch`).
pub async fn complete_worker(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: CompleteWorkerRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state.orca_ide.orchestration().complete_worker(req) {
        Ok(outcome) => Json(outcome).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `GET /api/v1/orca/orchestration/workers/{dispatch_id}/archive` (`200 OK`).
pub async fn read_worker_archive(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(dispatch_id): Path<String>,
) -> Response {
    match state
        .orca_ide
        .orchestration()
        .read_worker_archive(&dispatch_id)
    {
        Ok(archives) => Json(archives).into_response(),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ReleaseWorkerBody {
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// `POST /api/v1/orca/orchestration/workers/{dispatch_id}/release` (`200 OK`).
pub async fn release_worker(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(dispatch_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: ReleaseWorkerBody = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state
        .orca_ide
        .orchestration()
        .release_worker_resources(&dispatch_id, req.now_ms)
    {
        Ok(released) => Json(json!({
            "dispatch_id": dispatch_id,
            "released": released,
        }))
        .into_response(),
        Err(err) => map_ide_err(err),
    }
}

// ---------------------------------------------------------------------------
// 3. 9-Type Typed Mailbox, Questions & Decision/Review/Merge Gates
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SendOrchMessageBody {
    #[serde(alias = "runId")]
    pub run_id: String,
    #[serde(alias = "fromId")]
    pub from_id: String,
    #[serde(alias = "to")]
    pub target: Value,
    #[serde(alias = "messageType")]
    pub message_type: MailboxMessageType,
    #[serde(default)]
    pub priority: MailboxPriority,
    #[serde(default = "default_local_route")]
    pub route: MailboxRoute,
    pub body: String,
    #[serde(default, alias = "dispatchGeneration")]
    pub dispatch_generation: Option<u64>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

fn default_local_route() -> MailboxRoute {
    MailboxRoute::LocalOnly
}

/// `POST /api/v1/orca/orchestration/messages` (`201 Created`).
/// Accepts `target` (or `to`) as either a structured `MailboxTarget` JSON object
/// OR a shorthand string (`"@all"`, `"@idle"`, `"@worktree:<id>"`, `"@<agent>"`, `"dispatch:<id>"`).
/// Pushes a content-free `WakeFrameKind::Delivery` frame to `state.orca_wake_hub`.
pub async fn send_message(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: SendOrchMessageBody = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let target: MailboxTarget = if let Some(raw) = req.target.as_str() {
        if raw.trim().is_empty() {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "Mailbox target string cannot be empty.",
            );
        }
        parse_mailbox_target(raw)
    } else {
        match serde_json::from_value(req.target) {
            Ok(t) => t,
            Err(err) => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Invalid mailbox target: {err}"),
                );
            }
        }
    };

    let send_req = SendMessageRequest {
        run_id: req.run_id,
        from_id: req.from_id,
        target,
        message_type: req.message_type,
        priority: req.priority,
        route: req.route,
        body: req.body,
        dispatch_generation: req.dispatch_generation,
        now_ms: req.now_ms,
    };
    match state.orca_ide.orchestration().send_message(send_req) {
        Ok(outcome) => {
            state
                .orca_wake_hub
                .notify_host("*", WakeFrameKind::Delivery);
            (StatusCode::CREATED, Json(outcome)).into_response()
        }
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct CheckMailboxQuery {
    #[serde(default)]
    pub recipient_id: Option<String>,
    #[serde(default)]
    pub dispatch_id: Option<String>,
    #[serde(default)]
    pub expected_generation: Option<u64>,
    #[serde(default)]
    pub now_ms: Option<i64>,
}

/// `GET /api/v1/orca/orchestration/mailbox` (`200 OK`, `409 stale_dispatch`).
pub async fn check_mailbox(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<CheckMailboxQuery>,
) -> Response {
    let Some(recipient_id) = query
        .recipient_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
    else {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Missing required query parameter 'recipient_id'.",
        );
    };
    let req = CheckMailboxRequest {
        recipient_id,
        dispatch_id: query
            .dispatch_id
            .filter(|s| !s.trim().is_empty()),
        expected_generation: query.expected_generation,
        now_ms: query.now_ms,
    };
    match state.orca_ide.orchestration().check_mailbox(req) {
        Ok(deliveries) => Json(deliveries).into_response(),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Deserialize)]
pub struct AskQuestionBody {
    #[serde(alias = "runId")]
    pub run_id: String,
    #[serde(alias = "dispatchId")]
    pub dispatch_id: String,
    pub prompt: String,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// `POST /api/v1/orca/orchestration/questions` (`201 Created`).
pub async fn ask_question(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: AskQuestionBody = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state.orca_ide.orchestration().ask_question(
        &req.run_id,
        &req.dispatch_id,
        &req.prompt,
        req.now_ms,
    ) {
        Ok(q) => (StatusCode::CREATED, Json(q)).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `POST /api/v1/orca/orchestration/gates` (`201 Created`).
pub async fn create_gate(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: CreateGateRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state.orca_ide.orchestration().create_gate(req) {
        Ok(gate) => (StatusCode::CREATED, Json(gate)).into_response(),
        Err(err) => map_ide_err(err),
    }
}

fn parse_resolve_gate_action(val: &Value) -> Result<ResolveGateAction, Response> {
    let target = match val.get("action") {
        Some(inner) if inner.is_object() => inner.clone(),
        _ => val.clone(),
    };
    if let Ok(action) = serde_json::from_value::<ResolveGateAction>(target.clone()) {
        return Ok(action);
    }
    if let Some(obj) = target.as_object()
        && let Some(act_str) = obj.get("action").and_then(Value::as_str)
    {
        let actor = obj
            .get("actor")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("owner")
            .to_string();
        return match act_str.trim().to_ascii_lowercase().as_str() {
            "approve" => Ok(ResolveGateAction::Approve {
                actor,
                note: obj.get("note").and_then(Value::as_str).map(ToOwned::to_owned),
            }),
            "reject" | "deny" => Ok(ResolveGateAction::Reject {
                actor,
                reason: obj
                    .get("reason")
                    .or_else(|| obj.get("note"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            }),
            "evaluate_only" | "evaluate" => Ok(ResolveGateAction::EvaluateOnly),
            other => Err(orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Unsupported gate resolve action '{other}'."),
            )),
        };
    }
    Err(orca_err(
        StatusCode::BAD_REQUEST,
        "invalid_argument",
        "Malformed ResolveGateAction payload.",
    ))
}

/// `POST /api/v1/orca/orchestration/gates/{id}/resolve` (`200 OK`, `408 wait_timeout`).
pub async fn resolve_gate(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let action = match parse_resolve_gate_action(&val) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let now_ms = val
        .get("now_ms")
        .or_else(|| val.get("nowMs"))
        .and_then(Value::as_i64);
    match state
        .orca_ide
        .orchestration()
        .resolve_gate(&id, action, now_ms)
    {
        Ok(gate) => Json(gate).into_response(),
        Err(err) => map_ide_err(err),
    }
}

// ---------------------------------------------------------------------------
// 4. Cross-Host Federation Hosts & Router
// ---------------------------------------------------------------------------

/// `GET /api/v1/orca/orchestration/hosts` (`200 OK`).
pub async fn list_hosts(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state.orca_ide.orchestration().list_hosts() {
        Ok(hosts) => Json(hosts).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `POST /api/v1/orca/orchestration/hosts` (`201 Created`).
pub async fn register_host(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let host_val = val.get("host").cloned().unwrap_or_else(|| val.clone());
    let host: ExecutionHostRecord = match serde_json::from_value(host_val) {
        Ok(h) => h,
        Err(err) => {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Invalid ExecutionHostRecord: {err}"),
            );
        }
    };
    let supports_mail = val
        .get("supports_mail")
        .or_else(|| val.get("supportsMail"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    match state
        .orca_ide
        .orchestration()
        .register_host(host, supports_mail)
    {
        Ok(saved) => (StatusCode::CREATED, Json(saved)).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `POST /api/v1/orca/orchestration/hosts/select` (`200 OK`).
pub async fn select_host(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let criteria_val = val.get("criteria").cloned().unwrap_or_else(|| val.clone());
    let criteria: HostSelectionCriteria = match serde_json::from_value(criteria_val) {
        Ok(c) => c,
        Err(err) => {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Invalid HostSelectionCriteria: {err}"),
            );
        }
    };
    let now_ms = val
        .get("now_ms")
        .or_else(|| val.get("nowMs"))
        .and_then(Value::as_i64);
    match state
        .orca_ide
        .orchestration()
        .select_host(&criteria, now_ms)
    {
        Ok(selected) => Json(selected).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// Router exposing all Orca Multi-Agent Orchestration REST routes.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/orca/orchestration/runs",
            get(list_runs).post(create_run),
        )
        .route("/api/v1/orca/orchestration/runs/{run_id}", get(get_run))
        .route("/api/v1/orca/orchestration/tasks", post(create_task))
        .route(
            "/api/v1/orca/orchestration/tasks/{task_id}",
            patch(transition_task),
        )
        .route(
            "/api/v1/orca/orchestration/workers/start",
            post(start_worker),
        )
        .route(
            "/api/v1/orca/orchestration/workers/done",
            post(complete_worker),
        )
        .route(
            "/api/v1/orca/orchestration/workers/{dispatch_id}/archive",
            get(read_worker_archive),
        )
        .route(
            "/api/v1/orca/orchestration/workers/{dispatch_id}/release",
            post(release_worker),
        )
        .route("/api/v1/orca/orchestration/messages", post(send_message))
        .route("/api/v1/orca/orchestration/mailbox", get(check_mailbox))
        .route("/api/v1/orca/orchestration/questions", post(ask_question))
        .route("/api/v1/orca/orchestration/gates", post(create_gate))
        .route(
            "/api/v1/orca/orchestration/gates/{id}/resolve",
            post(resolve_gate),
        )
        .route(
            "/api/v1/orca/orchestration/hosts",
            get(list_hosts).post(register_host),
        )
        .route(
            "/api/v1/orca/orchestration/hosts/select",
            post(select_host),
        )
}
