//! Loopback Local-Scripts Automation HTTP Server (`OB-F11b`).
//!
//! Pure-Rust 1:1 port of `src/main/automation-server.ts` built on `axum 0.8`:
//! - Binds strictly to `127.0.0.1:<port>` (`0` for ephemeral loopback port).
//! - Provisions `<data_dir>/automation` (`0o700`) and atomically writes `0o600`
//!   `token`, `headers`, and `url` files, registering the 256-bit `base64url` token
//!   with `openbot_storage::redact::register_secret_value(&token, false)` and
//!   cleaning up all three files on `AutomationServerHandle::shutdown`.
//! - Enforces exact security guard order on every request (every response carries
//!   `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`):
//!   1. `Host == 127.0.0.1:<port>` and absent `Origin` (`403 Forbidden`)
//!   2. Constant-time `Authorization: Bearer <token>` via `ct_eq` (`401 Unauthorized`)
//!   3. Request body cap `<= 32 KiB` (`413 Payload Too Large`)
//!   4. `Content-Type: application/json` (`415`) and valid JSON object (`400`)
//!   5. Fresh SQLite agent lookup (`404`) + `allow_automation` gate (`403`)
//!   6. Payload `<= 4,000` UTF-16 code units (`413`) & `waitId` validation (`400`)
//!   7. Per-agent rolling 1-hour (`3,600,000 ms`) sliding window capped at
//!      `automation_runs_per_hour` (default `30`, clamped `1..=10_000`) (`429` +
//!      `Retry-After`), reserving slots atomically and releasing on dispatch error.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path as AxumPath, Request, State};
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use openbot_contracts::agents::{
    AgentDefaults, AgentProviderId, AgentReasoningEffort, AgentSummary, DEFAULT_AGENT_DEFAULTS,
    MAX_AUTOMATION_RUNS_PER_HOUR, MIN_AUTOMATION_RUNS_PER_HOUR, UpdateAgentInput, is_agent_model,
    is_provider_account_id,
};
use openbot_contracts::conversation::ConversationMessageAuthor;
use openbot_contracts::limits::{INPUT_LIMITS, utf16_len};
use openbot_contracts::queue::{QueueDeliveryStatus, QueueSender};
use openbot_contracts::routines::{RoutineRunKind, RoutineRunSource};
use openbot_storage::json_preference_store::{
    JsonPreferenceStore, preference_files, resolve_preference_file,
};
use openbot_storage::mailbox_store::EnqueueInput;
use openbot_storage::redact::{redact_secrets, register_secret_value};
use openbot_storage::{OpenBotDatabase, StorageError, write_file_atomically};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::attention_registry::{ApprovalDecision, AttentionRegistry};
use crate::error::{OpenBotRuntimeError, RuntimeResult};
use crate::platform::routines::{RoutineEngine, RoutineRunRequest};
use crate::platform::{ct_eq, generate_base64url_token_32};

const HOUR_MS: u64 = 60 * 60 * 1_000;
const BODY_LIMIT_BYTES: usize = 32 * 1024;
const AUTOMATION_DIR_NAME: &str = "automation";
const AUTOMATION_TOKEN_FILE: &str = "token";
const AUTOMATION_HEADERS_FILE: &str = "headers";
const AUTOMATION_URL_FILE: &str = "url";
const AUTOMATION_CLIENT_MESSAGE_PREFIX: &str = "automation:";

#[derive(Debug, Clone)]
pub struct AutomationServerConfig {
    /// Loopback TCP port (`0` requests an ephemeral OS-assigned port on `127.0.0.1`).
    pub bind_port: u16,
    /// OpenBot runtime data directory; credential files live in `<data_dir>/automation`.
    pub data_dir: PathBuf,
}

pub struct AutomationServerHandle {
    addr: SocketAddr,
    token: String,
    automation_dir: PathBuf,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<JoinHandle<()>>,
}

impl AutomationServerHandle {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn automation_dir(&self) -> PathBuf {
        self.automation_dir.clone()
    }

    pub async fn shutdown(mut self) -> RuntimeResult<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
        cleanup_automation_files(&self.automation_dir);
        Ok(())
    }
}

impl Drop for AutomationServerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
            cleanup_automation_files(&self.automation_dir);
        }
    }
}

struct AutomationState {
    database: OpenBotDatabase,
    routines: Arc<RoutineEngine>,
    data_dir: PathBuf,
    expected_host: String,
    token_bytes: Vec<u8>,
    runs: Mutex<HashMap<String, Vec<(u64, u64)>>>,
    next_slot_id: AtomicU64,
}

pub struct AutomationHttpServer;

impl AutomationHttpServer {
    pub async fn start(
        database: OpenBotDatabase,
        routines: Arc<RoutineEngine>,
        config: AutomationServerConfig,
    ) -> RuntimeResult<AutomationServerHandle> {
        let bind_addr = SocketAddr::from(([127, 0, 0, 1], config.bind_port));
        let listener = TcpListener::bind(bind_addr).await?;
        let addr = listener.local_addr()?;
        let port = addr.port();

        let automation_dir = config.data_dir.join(AUTOMATION_DIR_NAME);
        let token = generate_base64url_token_32();
        register_secret_value(&token, false);

        if let Err(err) = write_automation_credentials(&automation_dir, &token, port) {
            cleanup_automation_files(&automation_dir);
            return Err(err);
        }

        let state = Arc::new(AutomationState {
            database,
            routines,
            data_dir: config.data_dir,
            expected_host: format!("127.0.0.1:{port}"),
            token_bytes: token.as_bytes().to_vec(),
            runs: Mutex::new(HashMap::new()),
            next_slot_id: AtomicU64::new(1),
        });

        let router = build_router(state);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let join_handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        Ok(AutomationServerHandle {
            addr,
            token,
            automation_dir,
            shutdown_tx: Some(shutdown_tx),
            join_handle: Some(join_handle),
        })
    }
}

fn write_automation_credentials(
    automation_dir: &Path,
    token: &str,
    port: u16,
) -> RuntimeResult<()> {
    fs::create_dir_all(automation_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(automation_dir, fs::Permissions::from_mode(0o700))?;
    }

    let files = [
        (AUTOMATION_TOKEN_FILE, token.to_string()),
        (
            AUTOMATION_HEADERS_FILE,
            format!("Authorization: Bearer {token}\n"),
        ),
        (AUTOMATION_URL_FILE, format!("http://127.0.0.1:{port}")),
    ];
    for (name, content) in files {
        let path = automation_dir.join(name);
        write_file_atomically(&path, content.as_bytes(), true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

fn cleanup_automation_files(automation_dir: &Path) {
    for name in [
        AUTOMATION_URL_FILE,
        AUTOMATION_TOKEN_FILE,
        AUTOMATION_HEADERS_FILE,
    ] {
        let _ = fs::remove_file(automation_dir.join(name));
    }
}

fn build_router(state: Arc<AutomationState>) -> Router {
    Router::new()
        .route("/v1/agents", get(list_agents_handler))
        .route("/run", post(run_handler))
        .route(
            "/v1/agents/{agent_id}/routines/{routine_id}/run",
            post(run_routine_handler),
        )
        .route("/v1/agents/{agent_id}/messages", post(post_message_handler))
        .route(
            "/v1/agents/{agent_id}/deliveries/{delivery_id}",
            get(get_delivery_handler),
        )
        .route("/v1/agents/{agent_id}/pending", get(get_pending_handler))
        .route(
            "/v1/agents/{agent_id}/context/clear",
            post(clear_context_handler),
        )
        .route(
            "/v1/agents/{agent_id}/settings",
            post(update_settings_handler),
        )
        .route(
            "/v1/agents/{agent_id}/prompts/{request_id}/answer",
            post(answer_prompt_handler),
        )
        .route(
            "/v1/agents/{agent_id}/approvals/{request_id}/respond",
            post(respond_approval_handler),
        )
        .fallback(fallback_not_found)
        .method_not_allowed_fallback(fallback_method_not_allowed)
        .layer(DefaultBodyLimit::disable())
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state,
            security_guard_middleware,
        ))
}

async fn security_guard_middleware(
    State(state): State<Arc<AutomationState>>,
    req: Request,
    next: Next,
) -> Response {
    let headers = req.headers();
    let host_ok = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|h| h == state.expected_host);
    if !host_ok || headers.contains_key(header::ORIGIN) {
        return json_response(StatusCode::FORBIDDEN, json!({ "error": "forbidden" }));
    }

    let auth_ok = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .is_some_and(|candidate| {
            !state.token_bytes.is_empty() && ct_eq(candidate.as_bytes(), &state.token_bytes)
        });
    if !auth_ok {
        return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
    }

    let mut resp = next.run(req).await;
    apply_security_headers(resp.headers_mut());
    resp
}

async fn fallback_not_found() -> Response {
    json_response(StatusCode::NOT_FOUND, json!({ "error": "not found" }))
}

async fn fallback_method_not_allowed() -> Response {
    json_response(
        StatusCode::METHOD_NOT_ALLOWED,
        json!({ "error": "method not allowed" }),
    )
}

async fn read_body_limited(headers: &HeaderMap, body: Body) -> Result<Vec<u8>, Response> {
    if let Some(len_hdr) = headers.get(header::CONTENT_LENGTH)
        && let Ok(len_str) = len_hdr.to_str()
        && let Ok(declared) = len_str.trim().parse::<usize>()
        && declared > BODY_LIMIT_BYTES
    {
        return Err(body_too_large_response());
    }

    let bytes = axum::body::to_bytes(body, BODY_LIMIT_BYTES)
        .await
        .map_err(|_| body_too_large_response())?;
    if bytes.len() > BODY_LIMIT_BYTES {
        return Err(body_too_large_response());
    }
    Ok(bytes.to_vec())
}

fn body_too_large_response() -> Response {
    json_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        json!({ "error": format!("Request body exceeds {BODY_LIMIT_BYTES} bytes.") }),
    )
}

async fn read_json_object(
    headers: &HeaderMap,
    body: Body,
) -> Result<Map<String, Value>, Response> {
    let bytes = read_body_limited(headers, body).await?;
    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.split(';').next())
        .map(|s| s.trim().to_ascii_lowercase());
    if media_type.as_deref() != Some("application/json") {
        return Err(json_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            json!({ "error": "Content-Type must be application/json." }),
        ));
    }
    if bytes.is_empty() {
        return Ok(Map::new());
    }
    let parsed: Value = serde_json::from_slice(&bytes).map_err(|_| {
        json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "The request body is not valid JSON." }),
        )
    })?;
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "The request body must be a JSON object." }),
        )),
    }
}

fn load_agent_defaults(state: &AutomationState) -> AgentDefaults {
    let base = state
        .database
        .agent_store()
        .map(|store| store.agent_defaults().clone())
        .unwrap_or(DEFAULT_AGENT_DEFAULTS);
    let pref_path = resolve_preference_file(&state.data_dir, preference_files::AGENT_DEFAULTS_V1);
    if !pref_path.exists() {
        return base;
    }
    let store = JsonPreferenceStore::new(pref_path, base.clone());
    store.load().unwrap_or(base)
}

fn load_agent_fresh(
    state: &AutomationState,
    agent_id: &str,
) -> Result<(AgentSummary, AgentDefaults), Response> {
    if !is_valid_path_id(agent_id) {
        return Err(json_response(
            StatusCode::NOT_FOUND,
            json!({ "error": "not found" }),
        ));
    }
    let agents = state
        .database
        .agent_roster()
        .list_agents()
        .map_err(map_storage_error)?;
    let defaults = load_agent_defaults(state);
    let Some(agent) = agents.into_iter().find(|candidate| candidate.id == agent_id) else {
        return Err(json_response(
            StatusCode::NOT_FOUND,
            json!({ "error": "not found" }),
        ));
    };
    if !agent.allow_automation.unwrap_or(defaults.allow_automation) {
        return Err(json_response(
            StatusCode::FORBIDDEN,
            json!({ "error": "Automation is turned off for this agent." }),
        ));
    }
    Ok((agent, defaults))
}

fn take_rate_limit_slot(
    state: &AutomationState,
    agent_id: &str,
    limit_per_hour: u32,
) -> Result<u64, Response> {
    let limit = limit_per_hour.clamp(MIN_AUTOMATION_RUNS_PER_HOUR, MAX_AUTOMATION_RUNS_PER_HOUR);
    let now_ms = current_unix_ms();
    let mut runs = state.runs.lock();
    let entry = runs.entry(agent_id.to_string()).or_default();
    entry.retain(|(ts, _)| now_ms.saturating_sub(*ts) < HOUR_MS);
    if entry.len() >= limit as usize {
        let oldest_ts = entry.iter().map(|(ts, _)| *ts).min().unwrap_or(now_ms);
        let retry_after_ms = HOUR_MS.saturating_sub(now_ms.saturating_sub(oldest_ts));
        let retry_after_secs = retry_after_ms.div_ceil(1_000).max(1);
        return Err(json_response_with_headers(
            StatusCode::TOO_MANY_REQUESTS,
            json!({
                "error": {
                    "code": "rate_limit_exceeded",
                    "message": "Automation rate limit exceeded.",
                    "limit_per_hour": limit,
                },
                "limit_per_hour": limit,
            }),
            &[(header::RETRY_AFTER, retry_after_secs.to_string())],
        ));
    }
    let slot_id = state.next_slot_id.fetch_add(1, Ordering::Relaxed);
    entry.push((now_ms, slot_id));
    Ok(slot_id)
}

fn release_rate_limit_slot(state: &AutomationState, agent_id: &str, slot_id: u64) {
    let mut runs = state.runs.lock();
    if let Some(entry) = runs.get_mut(agent_id) {
        if let Some(pos) = entry.iter().position(|(_, id)| *id == slot_id) {
            entry.remove(pos);
        }
        if entry.is_empty() {
            runs.remove(agent_id);
        }
    }
}

async fn list_agents_handler(State(state): State<Arc<AutomationState>>) -> Response {
    let agents = match state.database.agent_roster().list_agents() {
        Ok(list) => list,
        Err(err) => return map_storage_error(err),
    };
    let defaults = load_agent_defaults(&state);
    let routine_store = state.database.agent_routine_store();
    let mut rows = Vec::new();
    for agent in agents {
        if !agent.allow_automation.unwrap_or(defaults.allow_automation) {
            continue;
        }
        let routines_json: Vec<Value> = match routine_store.list_records(&agent.id) {
            Ok(records) => records
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "name": r.name,
                        "active": r.active,
                    })
                })
                .collect(),
            Err(_) => match state.routines.list_routines(&agent.id, false) {
                Ok(list) => list
                    .into_iter()
                    .map(|r| {
                        json!({
                            "id": r.id,
                            "name": r.name,
                            "active": r.active,
                        })
                    })
                    .collect(),
                Err(err) => return map_runtime_error(err),
            },
        };
        rows.push(json!({
            "id": agent.id,
            "name": agent.name,
            "routines": routines_json,
        }));
    }
    json_response(StatusCode::OK, Value::Array(rows))
}

async fn run_handler(
    State(state): State<Arc<AutomationState>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let obj = match read_json_object(&headers, body).await {
        Ok(map) => map,
        Err(resp) => return resp,
    };
    let agent_id = match extract_required_id_alias(&obj, "agent_id", "agentId") {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let routine_id = match extract_optional_id_alias(&obj, "routine_id", "routineId") {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let client_message_id =
        match extract_optional_id_alias(&obj, "client_message_id", "clientMessageId") {
            Ok(id) => id,
            Err(resp) => return resp,
        };

    let (agent, defaults) = match load_agent_fresh(&state, &agent_id) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let payload = match extract_payload(&obj) {
        Ok(text) => text,
        Err(resp) => return resp,
    };
    let wait_id = match extract_wait_id(&obj) {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    if let Some(routine_id) = routine_id {
        return execute_routine_run(&state, &agent, &defaults, routine_id, payload, wait_id);
    }

    let idempotency_key = match client_message_id {
        Some(cid) => match prefix_client_message_id(&cid) {
            Ok(prefixed) => Some(prefixed),
            Err(resp) => return resp,
        },
        None => None,
    };
    let slot_id = match take_rate_limit_slot(&state, &agent.id, defaults.automation_runs_per_hour) {
        Ok(slot) => slot,
        Err(resp) => return resp,
    };

    let mailbox = match state.database.mailbox_store() {
        Ok(store) => store,
        Err(err) => {
            release_rate_limit_slot(&state, &agent.id, slot_id);
            return map_storage_error(err);
        }
    };
    let message_text = if payload.trim().is_empty() {
        "[automation run]".to_string()
    } else {
        payload
    };
    let receipt = match mailbox.enqueue(EnqueueInput {
        recipient_agent_ids: vec![agent.id.clone()],
        text: message_text,
        sender: Some(QueueSender::User),
        idempotency_key,
        ..Default::default()
    }) {
        Ok(receipt) => receipt,
        Err(err) => {
            release_rate_limit_slot(&state, &agent.id, slot_id);
            return map_storage_error(err);
        }
    };

    let delivery_id = receipt
        .deliveries
        .first()
        .map(|d| d.id.clone())
        .unwrap_or_else(|| receipt.message_id.clone());
    let run_id = receipt.message_id;
    json_response(
        StatusCode::ACCEPTED,
        json!({
            "run_id": run_id,
            "runId": run_id,
            "delivery_id": delivery_id,
            "deliveryId": delivery_id,
            "agent_id": agent.id,
            "agentId": agent.id,
        }),
    )
}

async fn run_routine_handler(
    State(state): State<Arc<AutomationState>>,
    AxumPath((agent_id, routine_id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let obj = match read_json_object(&headers, body).await {
        Ok(map) => map,
        Err(resp) => return resp,
    };
    if !is_valid_path_id(&routine_id) {
        return json_response(StatusCode::NOT_FOUND, json!({ "error": "not found" }));
    }
    let (agent, defaults) = match load_agent_fresh(&state, &agent_id) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let payload = match extract_payload(&obj) {
        Ok(text) => text,
        Err(resp) => return resp,
    };
    let wait_id = match extract_wait_id(&obj) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    execute_routine_run(&state, &agent, &defaults, routine_id, payload, wait_id)
}

fn execute_routine_run(
    state: &AutomationState,
    agent: &AgentSummary,
    defaults: &AgentDefaults,
    routine_id: String,
    payload: String,
    wait_id: Option<String>,
) -> Response {
    let routine_exists = state
        .routines
        .list_routines(&agent.id, false)
        .map(|list| list.iter().any(|r| r.id == routine_id))
        .unwrap_or(false)
        || state
            .database
            .agent_routine_store()
            .get_record(&agent.id, &routine_id)
            .ok()
            .flatten()
            .is_some();
    if !routine_exists {
        return json_response(StatusCode::NOT_FOUND, json!({ "error": "routine not found" }));
    }

    let slot_id = match take_rate_limit_slot(state, &agent.id, defaults.automation_runs_per_hour) {
        Ok(slot) => slot,
        Err(resp) => return resp,
    };

    let req = RoutineRunRequest {
        owner_id: agent.id.clone(),
        routine_id,
        is_channel: false,
        kind: RoutineRunKind::Manual,
        source: Some(RoutineRunSource::Script),
        payload: Some(payload),
        wait_id,
        quiet_no_update: false,
        report_text: None,
        await_events: Vec::new(),
    };
    match state.routines.run_routine(req) {
        Ok(outcome) => json_response(
            StatusCode::ACCEPTED,
            json!({
                "runId": outcome.run_id,
                "run_id": outcome.run_id,
                "deliveryId": outcome.delivery_id,
                "delivery_id": outcome.delivery_id,
                "agentId": agent.id,
                "agent_id": agent.id,
                "routineId": outcome.routine_id,
                "routine_id": outcome.routine_id,
                "status": outcome.status,
            }),
        ),
        Err(err) => {
            release_rate_limit_slot(state, &agent.id, slot_id);
            map_runtime_error(err)
        }
    }
}

async fn post_message_handler(
    State(state): State<Arc<AutomationState>>,
    AxumPath(agent_id): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let obj = match read_json_object(&headers, body).await {
        Ok(map) => map,
        Err(resp) => return resp,
    };
    let (agent, defaults) = match load_agent_fresh(&state, &agent_id) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let text = match required_non_empty_string(&obj, "text", INPUT_LIMITS.message_text) {
        Ok(val) => val,
        Err(resp) => return resp,
    };
    let raw_client_id =
        match extract_required_id_alias(&obj, "client_message_id", "clientMessageId") {
            Ok(id) => id,
            Err(resp) => return resp,
        };
    let prefixed_id = match prefix_client_message_id(&raw_client_id) {
        Ok(val) => val,
        Err(resp) => return resp,
    };

    let slot_id = match take_rate_limit_slot(&state, &agent.id, defaults.automation_runs_per_hour) {
        Ok(slot) => slot,
        Err(resp) => return resp,
    };
    let mailbox = match state.database.mailbox_store() {
        Ok(store) => store,
        Err(err) => {
            release_rate_limit_slot(&state, &agent.id, slot_id);
            return map_storage_error(err);
        }
    };
    let receipt = match mailbox.enqueue(EnqueueInput {
        recipient_agent_ids: vec![agent.id.clone()],
        text,
        sender: Some(QueueSender::User),
        idempotency_key: Some(prefixed_id),
        ..Default::default()
    }) {
        Ok(receipt) => receipt,
        Err(err) => {
            release_rate_limit_slot(&state, &agent.id, slot_id);
            return map_storage_error(err);
        }
    };
    match serde_json::to_value(&receipt) {
        Ok(val) => json_response(StatusCode::ACCEPTED, val),
        Err(err) => internal_error(&err.to_string()),
    }
}

async fn get_delivery_handler(
    State(state): State<Arc<AutomationState>>,
    AxumPath((agent_id, delivery_id)): AxumPath<(String, String)>,
) -> Response {
    let (agent, _) = match load_agent_fresh(&state, &agent_id) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    if !is_valid_path_id(&delivery_id) {
        return json_response(StatusCode::NOT_FOUND, json!({ "error": "not found" }));
    }
    let mailbox = match state.database.mailbox_store() {
        Ok(store) => store,
        Err(err) => return map_storage_error(err),
    };
    let Some(ctx) = mailbox.get_delivery(&delivery_id) else {
        return json_response(StatusCode::NOT_FOUND, json!({ "error": "not found" }));
    };
    if ctx.delivery.recipient_agent_id != agent.id {
        return json_response(StatusCode::NOT_FOUND, json!({ "error": "not found" }));
    }
    let ended = matches!(
        ctx.delivery.status,
        QueueDeliveryStatus::Completed
            | QueueDeliveryStatus::Failed
            | QueueDeliveryStatus::Interrupted
            | QueueDeliveryStatus::Cancelled
    );
    let answer = if ended {
        ctx.delivery
            .turn_id
            .as_deref()
            .and_then(|tid| extract_turn_answer(&state.database, &agent, tid))
    } else {
        None
    };
    let redacted_error = ctx.delivery.error.as_deref().map(redact_secrets);
    json_response(
        StatusCode::OK,
        json!({
            "id": delivery_id,
            "status": ctx.delivery.status,
            "turnId": ctx.delivery.turn_id,
            "error": redacted_error,
            "answer": answer,
        }),
    )
}

async fn get_pending_handler(
    State(state): State<Arc<AutomationState>>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    let (agent, _) = match load_agent_fresh(&state, &agent_id) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let waits = match state.routines.list_waits(&agent.id) {
        Ok(list) => list,
        Err(err) => return map_runtime_error(err),
    };
    let registry = match AttentionRegistry::open(state.database.clone(), &state.data_dir) {
        Ok(reg) => reg,
        Err(err) => return map_runtime_error(err),
    };
    let pending = registry.list_active(Some(&agent.id));
    let prompts: Vec<Value> = pending
        .questions
        .iter()
        .map(|record| {
            let requires_openbot = record.prompt.questions.iter().any(|q| q.is_secret);
            let questions: Vec<Value> = if requires_openbot {
                Vec::new()
            } else {
                record
                    .prompt
                    .questions
                    .iter()
                    .map(|q| {
                        json!({
                            "id": q.id,
                            "header": q.header,
                            "question": q.question,
                            "options": q.options,
                        })
                    })
                    .collect()
            };
            json!({
                "requestId": record.request_id,
                "agentId": record.agent_id,
                "threadId": record.thread_id,
                "turnId": record.turn_id,
                "requiresOpenBot": requires_openbot,
                "questions": questions,
                "createdAt": record.created_at,
                "expiresAtMs": record.expires_at_ms,
            })
        })
        .collect();

    json_response(
        StatusCode::OK,
        json!({
            "agentId": agent.id,
            "agent_id": agent.id,
            "waits": waits,
            "approvals": pending.approvals,
            "prompts": prompts,
            "uiBlocks": pending.ui_blocks,
            "takeovers": pending.takeovers,
        }),
    )
}

async fn clear_context_handler(
    State(state): State<Arc<AutomationState>>,
    AxumPath(agent_id): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if let Err(resp) = read_body_limited(&headers, body).await {
        return resp;
    }
    let (agent, _) = match load_agent_fresh(&state, &agent_id) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    if let Some(thread_id) = agent.thread_id.as_deref() {
        let threads = state.database.thread_store();
        match threads.read_active_turn_id(&agent.id, Some(thread_id)) {
            Ok(Some(_)) => {
                return json_response(
                    StatusCode::CONFLICT,
                    json!({ "error": "Cannot clear context while a turn is running." }),
                );
            }
            Ok(None) => {}
            Err(err) => return map_storage_error(err),
        }
        if let Err(err) = threads
            .provider_sessions()
            .deactivate_provider_sessions(thread_id)
        {
            return map_storage_error(err);
        }
        match threads.delete_thread_history(&agent.id, thread_id) {
            Ok(_) | Err(StorageError::NotFound(_)) => {}
            Err(err) => return map_storage_error(err),
        }
    }
    if let Ok(store) = state.database.agent_store() {
        let _ = store.clear_preview(&agent.id);
    }
    json_response(StatusCode::OK, json!({ "ok": true }))
}

async fn update_settings_handler(
    State(state): State<Arc<AutomationState>>,
    AxumPath(agent_id): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let obj = match read_json_object(&headers, body).await {
        Ok(map) => map,
        Err(resp) => return resp,
    };
    let (agent, _) = match load_agent_fresh(&state, &agent_id) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let input = match parse_settings_update(&agent.id, &obj) {
        Ok(update) => update,
        Err(resp) => return resp,
    };
    let store = match state.database.agent_store() {
        Ok(s) => s,
        Err(err) => return map_storage_error(err),
    };
    let updated = match store.update_agent(&input, None) {
        Ok(u) => u,
        Err(err) => return map_storage_error(err),
    };
    json_response(
        StatusCode::OK,
        json!({
            "id": updated.id,
            "name": updated.name,
            "provider": updated.provider,
            "model": updated.model,
            "reasoningEffort": updated.reasoning_effort,
            "providerAccountId": updated.provider_account_id,
        }),
    )
}

async fn answer_prompt_handler(
    State(state): State<Arc<AutomationState>>,
    AxumPath((agent_id, request_id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let obj = match read_json_object(&headers, body).await {
        Ok(map) => map,
        Err(resp) => return resp,
    };
    let (agent, _) = match load_agent_fresh(&state, &agent_id) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    if !is_valid_path_id(&request_id) {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "requestId is invalid." }),
        );
    }
    let cancelled = match obj.get("cancelled") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "cancelled must be a boolean." }),
            );
        }
    };
    let answers = match parse_prompt_answers(&obj, cancelled) {
        Ok(map) => map,
        Err(resp) => return resp,
    };

    let registry = match AttentionRegistry::open(state.database.clone(), &state.data_dir) {
        Ok(reg) => reg,
        Err(err) => return map_runtime_error(err),
    };
    if registry
        .list_active(None)
        .questions
        .iter()
        .any(|q| q.request_id == request_id && q.agent_id != agent.id)
    {
        return json_response(StatusCode::NOT_FOUND, json!({ "error": "not found" }));
    }
    match registry.respond_question(&request_id, answers, cancelled) {
        Ok(_) | Err(OpenBotRuntimeError::NotFound(_)) => {
            json_response(StatusCode::OK, json!({ "ok": true }))
        }
        Err(err) => map_runtime_error(err),
    }
}

async fn respond_approval_handler(
    State(state): State<Arc<AutomationState>>,
    AxumPath((agent_id, request_id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let obj = match read_json_object(&headers, body).await {
        Ok(map) => map,
        Err(resp) => return resp,
    };
    let (agent, _) = match load_agent_fresh(&state, &agent_id) {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    if !is_valid_path_id(&request_id) {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "requestId is invalid." }),
        );
    }
    let decision = match parse_approval_decision(&obj) {
        Ok(val) => val,
        Err(resp) => return resp,
    };
    let registry = match AttentionRegistry::open(state.database.clone(), &state.data_dir) {
        Ok(reg) => reg,
        Err(err) => return map_runtime_error(err),
    };
    if let Some(existing) = registry.get_approval(&request_id)
        && existing.agent_id != agent.id
    {
        return json_response(StatusCode::NOT_FOUND, json!({ "error": "not found" }));
    }
    match registry.respond_approval(&request_id, decision, None, true) {
        Ok(_) | Err(OpenBotRuntimeError::NotFound(_)) => {
            json_response(StatusCode::OK, json!({ "ok": true }))
        }
        Err(err) => map_runtime_error(err),
    }
}

fn extract_required_id_alias(
    obj: &Map<String, Value>,
    snake: &str,
    camel: &str,
) -> Result<String, Response> {
    match extract_optional_id_alias(obj, snake, camel)? {
        Some(id) => Ok(id),
        None => Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": format!("{snake} is required.") }),
        )),
    }
}

fn extract_optional_id_alias(
    obj: &Map<String, Value>,
    snake: &str,
    camel: &str,
) -> Result<Option<String>, Response> {
    let parse_one = |field: &str| -> Result<Option<String>, Response> {
        match obj.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => {
                let trimmed = s.trim();
                if trimmed.is_empty() || utf16_len(trimmed) > INPUT_LIMITS.identifier {
                    return Err(json_response(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": format!("{field} is invalid.") }),
                    ));
                }
                Ok(Some(trimmed.to_string()))
            }
            Some(_) => Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("{field} must be a string.") }),
            )),
        }
    };

    let first = parse_one(snake)?;
    let second = parse_one(camel)?;
    match (first, second) {
        (Some(a), Some(b)) if a != b => Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": format!("Conflicting {snake} and {camel} values.") }),
        )),
        (Some(a), _) => Ok(Some(a)),
        (None, Some(b)) => Ok(Some(b)),
        (None, None) => Ok(None),
    }
}

fn extract_wait_id(obj: &Map<String, Value>) -> Result<Option<String>, Response> {
    let parse_strict = |field: &str| -> Result<Option<String>, Response> {
        match obj.get(field) {
            None => Ok(None),
            Some(Value::String(s)) => {
                if s.is_empty() || s.trim().is_empty() || utf16_len(s) > INPUT_LIMITS.identifier {
                    return Err(json_response(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "waitId must be a valid wait id." }),
                    ));
                }
                Ok(Some(s.clone()))
            }
            Some(_) => Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "waitId must be a valid wait id." }),
            )),
        }
    };

    let first = parse_strict("wait_id")?;
    let second = parse_strict("waitId")?;
    match (first, second) {
        (Some(a), Some(b)) if a != b => Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Conflicting wait_id and waitId values." }),
        )),
        (Some(a), _) => Ok(Some(a)),
        (None, Some(b)) => Ok(Some(b)),
        (None, None) => Ok(None),
    }
}

fn extract_payload(obj: &Map<String, Value>) -> Result<String, Response> {
    let payload = match obj.get("payload") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => {
            return Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "payload must be a string." }),
            ));
        }
    };
    if utf16_len(&payload) > INPUT_LIMITS.automation_payload {
        return Err(json_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({
                "error": format!("Payload exceeds {} characters.", INPUT_LIMITS.automation_payload),
            }),
        ));
    }
    Ok(payload)
}

fn prefix_client_message_id(raw: &str) -> Result<String, Response> {
    let prefixed = format!("{AUTOMATION_CLIENT_MESSAGE_PREFIX}{raw}");
    if utf16_len(&prefixed) > INPUT_LIMITS.identifier {
        return Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "clientMessageId is too long." }),
        ));
    }
    Ok(prefixed)
}

fn required_non_empty_string(
    obj: &Map<String, Value>,
    field: &str,
    limit: usize,
) -> Result<String, Response> {
    match obj.get(field) {
        Some(Value::String(s)) if !s.trim().is_empty() && utf16_len(s) <= limit => Ok(s.clone()),
        _ => Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": format!("{field} is invalid.") }),
        )),
    }
}

fn parse_settings_update(
    agent_id: &str,
    obj: &Map<String, Value>,
) -> Result<UpdateAgentInput, Response> {
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "provider"
                | "model"
                | "reasoningEffort"
                | "reasoning_effort"
                | "providerAccountId"
                | "provider_account_id"
        ) {
            return Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("{key} cannot be changed here.") }),
            ));
        }
    }

    let provider = match obj.get("provider") {
        None => None,
        Some(Value::String(s)) => match AgentProviderId::parse(s) {
            Some(p) => Some(p),
            None => {
                return Err(json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "provider is invalid." }),
                ));
            }
        },
        Some(_) => {
            return Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "provider is invalid." }),
            ));
        }
    };

    let model = match obj.get("model") {
        None => None,
        Some(Value::String(s)) if is_agent_model(s) => Some(s.clone()),
        Some(_) => {
            return Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "model is invalid." }),
            ));
        }
    };

    let effort_raw = obj
        .get("reasoningEffort")
        .or_else(|| obj.get("reasoning_effort"));
    if let (Some(a), Some(b)) = (obj.get("reasoningEffort"), obj.get("reasoning_effort"))
        && a != b
    {
        return Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Conflicting reasoningEffort values." }),
        ));
    }
    let reasoning_effort = match effort_raw {
        None => None,
        Some(Value::String(s)) => match AgentReasoningEffort::parse(s) {
            Some(e) => Some(e),
            None => {
                return Err(json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "reasoningEffort is invalid." }),
                ));
            }
        },
        Some(_) => {
            return Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "reasoningEffort is invalid." }),
            ));
        }
    };

    let account_raw = obj
        .get("providerAccountId")
        .or_else(|| obj.get("provider_account_id"));
    if let (Some(a), Some(b)) = (obj.get("providerAccountId"), obj.get("provider_account_id"))
        && a != b
    {
        return Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Conflicting providerAccountId values." }),
        ));
    }
    let provider_account_id = match account_raw {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) if is_provider_account_id(s) => Some(Some(s.clone())),
        Some(_) => {
            return Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "providerAccountId is invalid." }),
            ));
        }
    };

    if provider.is_none()
        && model.is_none()
        && reasoning_effort.is_none()
        && provider_account_id.is_none()
    {
        return Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Send provider, model, reasoningEffort or providerAccountId." }),
        ));
    }

    Ok(UpdateAgentInput {
        agent_id: agent_id.to_string(),
        provider,
        model,
        reasoning_effort,
        provider_account_id,
        ..Default::default()
    })
}

fn parse_prompt_answers(
    obj: &Map<String, Value>,
    cancelled: bool,
) -> Result<BTreeMap<String, Vec<String>>, Response> {
    if cancelled {
        return Ok(BTreeMap::new());
    }
    let Some(raw_answers) = obj.get("answers") else {
        return Ok(BTreeMap::new());
    };
    let Some(map) = raw_answers.as_object() else {
        return Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "answers must be an object." }),
        ));
    };
    if map.len() > INPUT_LIMITS.prompt_questions {
        return Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Too many prompt answers." }),
        ));
    }

    let mut answers = BTreeMap::new();
    let mut total_units = 0usize;
    for (question_id, entry) in map {
        if question_id.is_empty() || utf16_len(question_id) > INPUT_LIMITS.identifier {
            return Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "A prompt answer is invalid." }),
            ));
        }
        let values: Vec<String> = match entry {
            Value::String(single) => vec![single.clone()],
            Value::Array(items) => {
                if items.len() > INPUT_LIMITS.prompt_answers_per_question {
                    return Err(json_response(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "A prompt answer is invalid." }),
                    ));
                }
                let mut collected = Vec::with_capacity(items.len());
                for item in items {
                    let Some(text) = item.as_str() else {
                        return Err(json_response(
                            StatusCode::BAD_REQUEST,
                            json!({ "error": "A prompt answer is invalid." }),
                        ));
                    };
                    collected.push(text.to_string());
                }
                collected
            }
            _ => {
                return Err(json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "A prompt answer is invalid." }),
                ));
            }
        };

        for text in &values {
            let len = utf16_len(text);
            if len > INPUT_LIMITS.prompt_answer_text {
                return Err(json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "A prompt answer is invalid." }),
                ));
            }
            total_units = total_units.saturating_add(len);
            if total_units > INPUT_LIMITS.prompt_answers_total_text {
                return Err(json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "Prompt answers exceed maximum total length." }),
                ));
            }
        }
        answers.insert(question_id.clone(), values);
    }
    Ok(answers)
}

fn parse_approval_decision(obj: &Map<String, Value>) -> Result<ApprovalDecision, Response> {
    if let Some(val) = obj.get("decision") {
        let Some(raw) = val.as_str() else {
            return Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "approval decision is invalid." }),
            ));
        };
        return match raw {
            "accept" | "approve" => Ok(ApprovalDecision::Accept),
            "accept-for-session" | "acceptForSession" => Ok(ApprovalDecision::AcceptForSession),
            "decline" | "reject" => Ok(ApprovalDecision::Decline),
            "cancel" => Ok(ApprovalDecision::Cancel),
            _ => Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "approval decision is invalid." }),
            )),
        };
    }
    if let Some(val) = obj.get("approved") {
        return match val.as_bool() {
            Some(true) => Ok(ApprovalDecision::Accept),
            Some(false) => Ok(ApprovalDecision::Decline),
            None => Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "approved must be a boolean." }),
            )),
        };
    }
    Ok(ApprovalDecision::Accept)
}

fn extract_turn_answer(
    database: &OpenBotDatabase,
    agent: &AgentSummary,
    turn_id: &str,
) -> Option<String> {
    let thread_id = agent.thread_id.as_deref()?;
    let snapshot = database
        .thread_store()
        .read_turn_conversation(&agent.id, thread_id, turn_id)
        .ok()?;
    let preferred = snapshot
        .messages
        .iter()
        .rev()
        .find(|m| {
            m.author == ConversationMessageAuthor::Assistant
                && m.item_type.is_none()
                && !m.text.trim().is_empty()
        })
        .or_else(|| {
            snapshot.messages.iter().rev().find(|m| {
                m.author == ConversationMessageAuthor::Assistant && !m.text.trim().is_empty()
            })
        })?;
    Some(redact_secrets(&preferred.text))
}

fn is_valid_path_id(value: &str) -> bool {
    !value.is_empty() && utf16_len(value) <= INPUT_LIMITS.identifier
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn apply_security_headers(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
}

fn json_response(status: StatusCode, body: Value) -> Response {
    json_response_with_headers(status, body, &[])
}

fn json_response_with_headers(
    status: StatusCode,
    body: Value,
    extra_headers: &[(HeaderName, String)],
) -> Response {
    let mut resp = (status, axum::Json(body)).into_response();
    let headers = resp.headers_mut();
    apply_security_headers(headers);
    for (name, value) in extra_headers {
        if let Ok(hv) = HeaderValue::from_str(value) {
            headers.insert(name.clone(), hv);
        }
    }
    resp
}

fn internal_error(message: &str) -> Response {
    let safe = redact_secrets(message);
    json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({ "error": if safe.is_empty() { "internal error".to_string() } else { safe } }),
    )
}

fn map_storage_error(err: StorageError) -> Response {
    match err {
        StorageError::Validation(msg) => {
            json_response(StatusCode::BAD_REQUEST, json!({ "error": redact_secrets(&msg) }))
        }
        StorageError::NotFound(msg) => {
            json_response(StatusCode::NOT_FOUND, json!({ "error": redact_secrets(&msg) }))
        }
        StorageError::Conflict(msg) => {
            json_response(StatusCode::CONFLICT, json!({ "error": redact_secrets(&msg) }))
        }
        StorageError::Forbidden(msg) => {
            json_response(StatusCode::FORBIDDEN, json!({ "error": redact_secrets(&msg) }))
        }
        StorageError::LimitExceeded(msg) => json_response(
            StatusCode::TOO_MANY_REQUESTS,
            json!({ "error": redact_secrets(&msg) }),
        ),
        other => internal_error(&other.to_string()),
    }
}

fn map_runtime_error(err: OpenBotRuntimeError) -> Response {
    match err {
        OpenBotRuntimeError::Validation(msg) => {
            json_response(StatusCode::BAD_REQUEST, json!({ "error": redact_secrets(&msg) }))
        }
        OpenBotRuntimeError::Contract(inner) => json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": redact_secrets(&inner.to_string()) }),
        ),
        OpenBotRuntimeError::Json(inner) => json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": redact_secrets(&inner.to_string()) }),
        ),
        OpenBotRuntimeError::NotFound(msg) => {
            json_response(StatusCode::NOT_FOUND, json!({ "error": redact_secrets(&msg) }))
        }
        OpenBotRuntimeError::Conflict(msg) => {
            json_response(StatusCode::CONFLICT, json!({ "error": redact_secrets(&msg) }))
        }
        OpenBotRuntimeError::Unauthorized(msg) => json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": redact_secrets(&msg) }),
        ),
        OpenBotRuntimeError::Forbidden(msg) => {
            json_response(StatusCode::FORBIDDEN, json!({ "error": redact_secrets(&msg) }))
        }
        OpenBotRuntimeError::ApprovalRequired { message, .. } => json_response(
            StatusCode::FORBIDDEN,
            json!({ "error": redact_secrets(&message) }),
        ),
        OpenBotRuntimeError::LimitExceeded(msg) => json_response(
            StatusCode::TOO_MANY_REQUESTS,
            json!({ "error": redact_secrets(&msg) }),
        ),
        OpenBotRuntimeError::RateLimited { message, .. } => json_response(
            StatusCode::TOO_MANY_REQUESTS,
            json!({ "error": redact_secrets(&message) }),
        ),
        OpenBotRuntimeError::Storage(inner) => map_storage_error(inner),
        other => internal_error(&other.to_string()),
    }
}
