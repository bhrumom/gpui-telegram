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
    ExecutionHostRecord, GateStatus, HostSelectionCriteria, MailboxMessage, MailboxMessageType,
    MailboxPriority, MailboxRoute, MailboxTarget, OrchestrationTask, OrchestrationTaskStatus,
};
use orca_core::LOCAL_EXECUTION_HOST_ID;
use orca_ide::orchestration_engine::{
    parse_mailbox_target, CheckMailboxRequest, CompleteWorkerRequest, CreateGateRequest,
    CreateRunRequest, CreateTaskRequest, DecisionGatePayload, DecisionGateRecord,
    DispatchFederatedRequest, FederationRelayDirection, OrchestrationRunRecord,
    OrchestrationRunStatus, ResolveGateAction, SendMessageRequest, StartWorkerRequest,
    StartWorkerResult,
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

fn run_to_wire_json(run: &OrchestrationRunRecord) -> Value {
    let mut val = serde_json::to_value(run).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".into(), json!(run.run_id));
        obj.insert("runId".into(), json!(run.run_id));
        obj.insert("consumerGeneration".into(), json!(run.consumer_generation));
        obj.insert(
            "coordinatorSession".into(),
            json!(run.coordinator_orca_session_id),
        );
    }
    val
}

fn task_to_wire_json(task: &OrchestrationTask, parent_id: Option<&str>) -> Value {
    let status_wire = match task.status {
        OrchestrationTaskStatus::Succeeded => "completed",
        OrchestrationTaskStatus::Pending => "pending",
        OrchestrationTaskStatus::Ready => "ready",
        OrchestrationTaskStatus::Running => "running",
        OrchestrationTaskStatus::WaitingGate => "waiting_gate",
        OrchestrationTaskStatus::Failed => "failed",
        OrchestrationTaskStatus::Canceled => "canceled",
    };
    let mut val = serde_json::to_value(task).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".into(), json!(task.task_id));
        obj.insert("taskId".into(), json!(task.task_id));
        obj.insert("runId".into(), json!(task.run_id));
        obj.insert("deps".into(), json!(task.depends_on));
        obj.insert("parentId".into(), json!(parent_id));
        obj.insert("status".into(), json!(status_wire));
        obj.insert("taskStatus".into(), json!(task.status));
    }
    val
}

fn should_wake_coordinator(msg_type: MailboxMessageType) -> bool {
    matches!(
        msg_type,
        MailboxMessageType::Question
            | MailboxMessageType::DecisionGate
            | MailboxMessageType::Escalation
            | MailboxMessageType::WorkerDone
    )
}

fn mailbox_msg_to_wire_json(seq: Option<i64>, msg: &MailboxMessage) -> Value {
    let mut val = serde_json::to_value(msg).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".into(), json!(msg.message_id));
        obj.insert("messageId".into(), json!(msg.message_id));
        obj.insert("runId".into(), json!(msg.run_id));
        obj.insert("type".into(), json!(msg.message_type.as_str()));
        obj.insert("messageType".into(), json!(msg.message_type.as_str()));
        obj.insert(
            "wakeCoordinator".into(),
            json!(should_wake_coordinator(msg.message_type)),
        );
        if let Some(s) = seq {
            obj.insert("seq".into(), json!(s));
        }
    }
    val
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
        Ok(run) => (StatusCode::CREATED, Json(run_to_wire_json(&run))).into_response(),
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
        Ok(runs) => {
            let wire: Vec<Value> = runs.iter().map(run_to_wire_json).collect();
            Json(wire).into_response()
        }
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
        "run": run_to_wire_json(&run),
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
    let parent_id = req.parent_id.clone();
    match state.orca_ide.orchestration().create_task(req) {
        Ok(task) => (
            StatusCode::CREATED,
            Json(task_to_wire_json(&task, parent_id.as_deref())),
        )
            .into_response(),
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
            "task": task_to_wire_json(&task, None),
            "newly_ready": newly_ready,
        }))
        .into_response(),
        Err(err) => map_ide_err(err),
    }
}

// ---------------------------------------------------------------------------
// 2. Generation-Fenced Workers, Terminal Archives & Resource Release
// ---------------------------------------------------------------------------

fn start_worker_response_json(
    eng: &orca_ide::orchestration_engine::OrchestrationEngine,
    res: &StartWorkerResult,
    replayed: bool,
) -> Value {
    let mut val = serde_json::to_value(res).unwrap_or_else(|_| json!({}));
    let is_remote = res.dispatch.host_id != LOCAL_EXECUTION_HOST_ID;
    let home_auto_release_at = if is_remote {
        Some(res.dispatch.created_at_ms.saturating_add(300_000))
    } else {
        None
    };
    let relay_items = if is_remote {
        let existing = eng
            .list_unacked_relay_items(&res.dispatch.dispatch_id, None)
            .unwrap_or_default();
        if existing.is_empty() {
            let _ = eng.dispatch_federated(DispatchFederatedRequest {
                dispatch_id: Some(res.dispatch.dispatch_id.clone()),
                run_id: res.context.run_id.clone(),
                task_id: res.context.task_id.clone(),
                home_host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
                selection: HostSelectionCriteria {
                    preferred_host_id: Some(res.dispatch.host_id.clone()),
                    required_capabilities: Vec::new(),
                    min_protocol_version: 1,
                    max_heartbeat_age_ms: 86_400_000,
                },
                depth: Some(res.dispatch.nesting_depth),
                home_auto_release_at_ms: home_auto_release_at,
                now_ms: Some(res.dispatch.created_at_ms),
            });
            eng.enqueue_relay_item(
                &res.dispatch.dispatch_id,
                FederationRelayDirection::ToWorker,
                json!({
                    "kind": "dispatch",
                    "runId": res.context.run_id,
                    "taskId": res.context.task_id,
                    "agentId": res.context.agent_id,
                    "targetServer": res.dispatch.host_id,
                }),
                Some(res.dispatch.created_at_ms),
            )
            .ok()
            .into_iter()
            .collect()
        } else {
            existing
        }
    } else {
        Vec::new()
    };
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".into(), json!(res.dispatch.dispatch_id));
        obj.insert("dispatchId".into(), json!(res.dispatch.dispatch_id));
        obj.insert("runId".into(), json!(res.context.run_id));
        obj.insert("taskId".into(), json!(res.context.task_id));
        obj.insert("agent".into(), json!(res.context.agent_id));
        obj.insert("worktree".into(), json!(res.context.worktree_id));
        obj.insert("keepOpen".into(), json!(res.context.keep_open));
        obj.insert("generation".into(), json!(res.dispatch.generation));
        obj.insert("replayed".into(), json!(replayed));
        if is_remote {
            obj.insert("targetServer".into(), json!(res.dispatch.host_id));
            obj.insert("homeAutoReleaseAt".into(), json!(home_auto_release_at));
            obj.insert("federationRelayItems".into(), json!(relay_items));
        }
    }
    val
}

/// `POST /api/v1/orca/orchestration/workers/start` (`201 Created`).
pub async fn start_worker(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let raw_val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let req: StartWorkerRequest = match serde_json::from_value(raw_val.clone()) {
        Ok(r) => r,
        Err(err) => {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Invalid StartWorkerRequest: {err}"),
            );
        }
    };
    let req_id = raw_val
        .get("requestId")
        .or_else(|| raw_val.get("request_id"))
        .or_else(|| raw_val.get("operationId"))
        .or_else(|| raw_val.get("operation_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let caller_fp = raw_val
        .get("callerFingerprint")
        .or_else(|| raw_val.get("caller_fingerprint"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("default-caller");

    let eng = state.orca_ide.orchestration();
    if let Some(idemp_key) = req_id {
        let now_ms = req.now_ms;
        match eng.execute_idempotent(caller_fp, idemp_key, &raw_val, now_ms, |e| {
            e.start_worker(req)
        }) {
            Ok((res, replayed)) => (
                StatusCode::CREATED,
                Json(start_worker_response_json(eng, &res, replayed)),
            )
                .into_response(),
            Err(err) => map_ide_err(err),
        }
    } else {
        match eng.start_worker(req) {
            Ok(res) => (
                StatusCode::CREATED,
                Json(start_worker_response_json(eng, &res, false)),
            )
                .into_response(),
            Err(err) => map_ide_err(err),
        }
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
    let eng = state.orca_ide.orchestration();
    let archives = match eng.read_worker_archive(&dispatch_id) {
        Ok(a) => a,
        Err(err) => return map_ide_err(err),
    };
    let recipient = format!("dispatch:{dispatch_id}");
    let inbox = eng
        .check_mailbox(CheckMailboxRequest {
            recipient_id: recipient,
            dispatch_id: None,
            expected_generation: None,
            now_ms: None,
        })
        .unwrap_or_default();
    if inbox.is_empty() {
        Json(archives).into_response()
    } else {
        Json(json!({
            "dispatchId": dispatch_id,
            "archives": archives,
            "messages": inbox,
        }))
        .into_response()
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

fn default_from_coordinator() -> String {
    "coordinator".to_string()
}

fn default_status_msg_type() -> MailboxMessageType {
    MailboxMessageType::Status
}

#[derive(Debug, Deserialize)]
pub struct SendOrchMessageBody {
    #[serde(alias = "runId")]
    pub run_id: String,
    #[serde(default = "default_from_coordinator", alias = "fromId", alias = "from")]
    pub from_id: String,
    #[serde(alias = "to")]
    pub target: Value,
    #[serde(
        default = "default_status_msg_type",
        alias = "messageType",
        alias = "type"
    )]
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
/// OR a shorthand string (`"@all"`, `"@idle"`, `"@worktree:<id>"`, `"@agent:<name>"`, `"@<agent>"`, `"dispatch:<id>"`).
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
            let mut val = serde_json::to_value(&outcome).unwrap_or_else(|_| json!({}));
            if let Some(obj) = val.as_object_mut() {
                obj.insert("message".into(), mailbox_msg_to_wire_json(None, &outcome.message));
                obj.insert(
                    "wakeCoordinator".into(),
                    json!(should_wake_coordinator(outcome.message.message_type)),
                );
                if let Some(ref fu) = outcome.follow_up {
                    obj.insert("followUpOfDispatchId".into(), json!(fu.original_dispatch_id));
                    obj.insert("followUpDispatchId".into(), json!(fu.follow_up_dispatch_id));
                    obj.insert("followUpTaskId".into(), json!(fu.follow_up_task_id));
                }
            }
            (StatusCode::CREATED, Json(val)).into_response()
        }
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct CheckMailboxQuery {
    #[serde(default, alias = "recipientId")]
    pub recipient_id: Option<String>,
    #[serde(default, alias = "runId")]
    pub run_id: Option<String>,
    #[serde(default, alias = "dispatchId")]
    pub dispatch_id: Option<String>,
    #[serde(default, alias = "expectedGeneration")]
    pub expected_generation: Option<u64>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// `GET /api/v1/orca/orchestration/mailbox` (`200 OK`, `409 stale_dispatch`).
/// Supports `?recipient_id=...`, `?dispatchId=...`, or `?runId=...`.
pub async fn check_mailbox(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<CheckMailboxQuery>,
) -> Response {
    let eng = state.orca_ide.orchestration();
    let explicit_recipient = query
        .recipient_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            query
                .dispatch_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|did| {
                    if did.starts_with("dispatch:") {
                        did.to_string()
                    } else {
                        format!("dispatch:{did}")
                    }
                })
        });

    if let Some(recipient_id) = explicit_recipient {
        let req = CheckMailboxRequest {
            recipient_id,
            dispatch_id: if query.expected_generation.is_some() {
                query.dispatch_id.filter(|s| !s.trim().is_empty())
            } else {
                None
            },
            expected_generation: query.expected_generation,
            now_ms: query.now_ms,
        };
        return match eng.check_mailbox(req) {
            Ok(deliveries) => {
                let wire: Vec<Value> = deliveries
                    .into_iter()
                    .enumerate()
                    .map(|(i, d)| {
                        let msg_wire = mailbox_msg_to_wire_json(Some((i + 1) as i64), &d.message);
                        let mut v = msg_wire;
                        if let Some(obj) = v.as_object_mut() {
                            obj.insert("delivery_id".into(), json!(d.delivery_id));
                            obj.insert("recipient_id".into(), json!(d.recipient_id));
                            obj.insert("state".into(), json!(d.state));
                            obj.insert("message".into(), json!(d.message));
                        }
                        v
                    })
                    .collect();
                Json(wire).into_response()
            }
            Err(err) => map_ide_err(err),
        };
    }

    if let Some(run_id) = query
        .run_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return match eng.list_run_mailbox(run_id) {
            Ok(msgs) => {
                let wire: Vec<Value> = msgs
                    .iter()
                    .map(|(seq, m)| mailbox_msg_to_wire_json(Some(*seq), m))
                    .collect();
                Json(wire).into_response()
            }
            Err(err) => map_ide_err(err),
        };
    }

    orca_err(
        StatusCode::BAD_REQUEST,
        "invalid_argument",
        "Missing required query parameter 'recipient_id', 'dispatchId', or 'runId'.",
    )
}

#[derive(Debug, Deserialize)]
pub struct AskQuestionBody {
    #[serde(default, alias = "runId")]
    pub run_id: Option<String>,
    #[serde(alias = "dispatchId", alias = "dispatch")]
    pub dispatch_id: String,
    #[serde(alias = "question")]
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
    let eng = state.orca_ide.orchestration();
    let run_id = match req.run_id.filter(|s| !s.trim().is_empty()) {
        Some(r) => r,
        None => match eng.lookup_dispatch_run_task(&req.dispatch_id) {
            Ok((r, _)) => r,
            Err(err) => return map_ide_err(err),
        },
    };
    match eng.ask_question(&run_id, &req.dispatch_id, &req.prompt, req.now_ms) {
        Ok(q) => {
            let mut val = serde_json::to_value(&q).unwrap_or_else(|_| json!({}));
            if let Some(obj) = val.as_object_mut() {
                obj.insert("id".into(), json!(q.question_id));
                obj.insert("questionId".into(), json!(q.question_id));
                obj.insert("status".into(), json!("pending"));
            }
            (StatusCode::CREATED, Json(val)).into_response()
        }
        Err(err) => map_ide_err(err),
    }
}

fn gate_to_wire_json(gate: &DecisionGateRecord, decision_label: Option<&str>) -> Value {
    let is_resolved = matches!(gate.status, GateStatus::Approved | GateStatus::Rejected);
    let mut val = serde_json::to_value(gate).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".into(), json!(gate.gate_id));
        obj.insert("gateId".into(), json!(gate.gate_id));
        obj.insert("runId".into(), json!(gate.run_id));
        obj.insert("taskId".into(), json!(gate.task_id));
        obj.insert("gateStatus".into(), json!(gate.status));
        if is_resolved {
            obj.insert("status".into(), json!("resolved"));
            let dec = decision_label.unwrap_or(match gate.status {
                GateStatus::Approved => "approve",
                GateStatus::Rejected => "reject",
                _ => "pending",
            });
            obj.insert("decision".into(), json!(dec));
        } else {
            obj.insert("wakeQueued".into(), json!(true));
            obj.insert("wakeCoordinator".into(), json!(true));
        }
    }
    val
}

/// `POST /api/v1/orca/orchestration/gates` (`201 Created`).
pub async fn create_gate(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let eng = state.orca_ide.orchestration();
    let req: CreateGateRequest = if let Ok(parsed) = serde_json::from_value::<CreateGateRequest>(val.clone()) {
        parsed
    } else {
        let dispatch_id = val
            .get("dispatchId")
            .or_else(|| val.get("dispatch_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let lookup = dispatch_id.and_then(|d| eng.lookup_dispatch_run_task(d).ok());
        let run_id = val
            .get("runId")
            .or_else(|| val.get("run_id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| lookup.as_ref().map(|(r, _)| r.clone()))
            .unwrap_or_default();
        let task_id = val
            .get("taskId")
            .or_else(|| val.get("task_id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| lookup.as_ref().map(|(_, t)| t.clone()))
            .unwrap_or_default();
        if run_id.trim().is_empty() || task_id.trim().is_empty() {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "Missing 'runId'/'taskId' (or resolvable 'dispatchId') for gate creation.",
            );
        }
        let payload = if let Some(p) = val.get("payload") {
            match serde_json::from_value(p.clone()) {
                Ok(dp) => dp,
                Err(err) => {
                    return orca_err(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        format!("Invalid gate payload: {err}"),
                    );
                }
            }
        } else {
            let prompt = val
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or("Approve decision gate?")
                .to_string();
            DecisionGatePayload::Decision {
                prompt,
                resolution_note: None,
            }
        };
        CreateGateRequest {
            run_id,
            task_id,
            gate_id: val
                .get("gateId")
                .or_else(|| val.get("gate_id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            payload,
            deadline_ms: val
                .get("deadlineMs")
                .or_else(|| val.get("deadline_ms"))
                .and_then(Value::as_i64),
            now_ms: val
                .get("nowMs")
                .or_else(|| val.get("now_ms"))
                .and_then(Value::as_i64),
        }
    };
    match eng.create_gate(req) {
        Ok(gate) => (StatusCode::CREATED, Json(gate_to_wire_json(&gate, None))).into_response(),
        Err(err) => map_ide_err(err),
    }
}

fn parse_resolve_gate_action(val: &Value) -> Result<(ResolveGateAction, Option<String>), Response> {
    let target = match val.get("action") {
        Some(inner) if inner.is_object() => inner.clone(),
        _ => val.clone(),
    };
    if let Ok(action) = serde_json::from_value::<ResolveGateAction>(target.clone()) {
        return Ok((action, None));
    }
    if let Some(obj) = target.as_object() {
        let act_or_dec = obj
            .get("action")
            .or_else(|| obj.get("decision"))
            .and_then(Value::as_str);
        if let Some(act_str) = act_or_dec {
            let actor = obj
                .get("actor")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("owner")
                .to_string();
            let note = obj
                .get("note")
                .or_else(|| obj.get("rationale"))
                .or_else(|| obj.get("reason"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let raw_dec = act_str.trim().to_string();
            return match raw_dec.to_ascii_lowercase().as_str() {
                "approve" | "approved" | "yes" | "accept" | "allow" => Ok((
                    ResolveGateAction::Approve { actor, note },
                    Some(raw_dec),
                )),
                "reject" | "rejected" | "deny" | "denied" | "no" | "defer" | "decline" => Ok((
                    ResolveGateAction::Reject {
                        actor,
                        reason: note,
                    },
                    Some(raw_dec),
                )),
                "evaluate_only" | "evaluate" => Ok((ResolveGateAction::EvaluateOnly, Some(raw_dec))),
                other => Err(orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unsupported gate resolve action/decision '{other}'."),
                )),
            };
        }
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
    let (action, raw_decision) = match parse_resolve_gate_action(&val) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let now_ms = val
        .get("now_ms")
        .or_else(|| val.get("nowMs"))
        .and_then(Value::as_i64);
    let eng = state.orca_ide.orchestration();
    match eng.resolve_gate(&id, action, now_ms) {
        Ok(gate) => {
            let dec_str = raw_decision.as_deref().unwrap_or(match gate.status {
                GateStatus::Approved => "approve",
                GateStatus::Rejected => "reject",
                _ => "pending",
            });
            if let Ok(Some(disp_id)) = eng.lookup_dispatch_for_task(&gate.task_id) {
                let note = format!("gate {} resolved: {}", gate.gate_id, dec_str);
                let _ = eng.record_worker_archive(&disp_id, &note, now_ms);
            }
            Json(gate_to_wire_json(&gate, Some(dec_str))).into_response()
        }
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
