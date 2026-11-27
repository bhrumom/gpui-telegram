//! Team API HTTP + Real-Time SQLite SSE Server (`OB-F13`), `OpenBotServeHandle`,
//! and `JoinedServerClient` with WebRTC DataChannel v2 binary framing and `v1`..`v10`
//! 309-capability negotiation.
//!
//! Mirrors `src/main/{team-api-server,team-store,team-webrtc-file-transfer}.ts` and
//! `packages/contracts/src/team-protocol/*`.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{SecondsFormat, Utc};
use openbot_contracts::agents::{
    AgentProviderId, AgentReasoningEffort, CreateAgentInput, is_agent_model,
};
use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::channels::{
    Channel, ChannelBrowser, ChannelDraft, ChannelIcon, ChannelMember, ChannelMessage,
    ChannelMessageAuthor, ChannelMessageAuthorKind, ChannelSummary, SIGNED_OUT_CHANNEL_MEMBER_ID,
};
use openbot_contracts::conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus,
};
use openbot_contracts::limits::{INPUT_LIMITS, is_bounded_string, is_identifier, utf16_len};
use openbot_contracts::team_protocol::{
    TEAM_ABSENT_CAPABILITIES_HEADER, TEAM_APP_VERSION_HEADER, TEAM_CAPABILITY_REGISTRY,
    TEAM_LONG_CAPABILITY_LISTS_CAPABILITY, TEAM_PROTOCOL_V1, TEAM_PROTOCOL_V10,
    TEAM_PROTOCOL_V10_ABSENT_LIMIT, TEAM_PROTOCOL_V10_CAPABILITY_LIMIT,
    TEAM_PROTOCOL_V2_MAX_BINARY_FRAME_BYTES, TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET,
    TEAM_PROTOCOL_VERSION_HEADER, TeamCapabilitiesV10, TeamProtocolRange, TeamProtocolSupport,
    TeamProtocolUpdateDirection, TeamProtocolV2FileChunk, decode_team_protocol_support_expanded,
    decode_team_protocol_v2_file_chunk, encode_team_capabilities_v10,
    encode_team_protocol_v2_file_chunk, expand_team_capabilities_v10,
    highest_common_team_protocol, is_capability_slug, team_capabilities_for_protocol,
    team_capability_headers, team_current_capabilities, team_protocol_update_direction,
    to_current_agent_keys, to_wire_agent_keys,
};
use openbot_contracts::voice::VoiceLiveStart;
use openbot_storage::{
    AgentStore, ChannelChange, ChannelStore, LATEST_SCHEMA_VERSION, OpenBotDatabase,
    SendDirectMessageInput, StorageError, StoredOrchestrationEvent, TeamChatStore,
    VersionedJsonStore, preference_files,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::error::{OpenBotRuntimeError, RuntimeResult};
use crate::platform::automation_http::{
    AutomationHttpServer, AutomationServerConfig, AutomationServerHandle,
};
use crate::platform::voice::{VoiceService, parse_codex_live_voice};
use crate::platform::{PlatformServices, ct_eq};

const TEAM_APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const TEAM_API_VERSIONS: [&str; 10] = [
    "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v9", "v10",
];
const SSE_POLL_INTERVAL_MS: u64 = 50;
const SSE_BATCH_LIMIT: usize = 64;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamProtocolEvalRequest {
    pub client_version: Option<u16>,
    pub host_min: Option<u16>,
    pub host_max: Option<u16>,
    pub capabilities: Option<Vec<String>>,
    pub absent_capabilities: Option<Vec<String>>,
    pub wire_json: Option<Value>,
    pub webrtc_transfer_id: Option<String>,
    pub webrtc_offset: Option<u64>,
    pub webrtc_payload_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamProtocolEvalOutcome {
    pub negotiated_version: Option<u16>,
    pub update_direction: Option<String>,
    pub http_status: u16,
    pub error_code: Option<String>,
    pub total_capabilities: usize, // 309
    pub expanded_capabilities: Vec<String>,
    pub encoded_v10: TeamCapabilitiesV10,
    pub headers: BTreeMap<String, String>,
    pub wire_legacy: Option<Value>,
    pub wire_current: Option<Value>,
    pub webrtc_chunk_encoded_len: Option<usize>,
    pub webrtc_roundtrip_ok: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct TeamApiConfig {
    pub bind_port: u16,
    pub data_dir: PathBuf,
    pub require_auth_token: Option<String>,
}

pub struct TeamApiHandle {
    addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    shutdown_watch: watch::Sender<bool>,
    join_handle: Option<JoinHandle<()>>,
}

impl TeamApiHandle {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub async fn shutdown(mut self) -> RuntimeResult<()> {
        let _ = self.shutdown_watch.send(true);
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
        Ok(())
    }
}

impl Drop for TeamApiHandle {
    fn drop(&mut self) {
        let _ = self.shutdown_watch.send(true);
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpenBotServeConfig {
    pub data_dir: PathBuf,
    pub team_port: u16,
    pub automation_port: u16,
}

pub struct OpenBotServeHandle {
    pub team_api: TeamApiHandle,
    pub automation: AutomationServerHandle,
}

impl OpenBotServeHandle {
    pub async fn start(
        database: OpenBotDatabase,
        services: Arc<PlatformServices>,
        config: OpenBotServeConfig,
    ) -> RuntimeResult<Self> {
        let team_api = TeamApiServer::start(
            database.clone(),
            services.voice.clone(),
            TeamApiConfig {
                bind_port: config.team_port,
                data_dir: config.data_dir.clone(),
                require_auth_token: None,
            },
        )
        .await?;
        let automation = match AutomationHttpServer::start(
            database,
            services.routines.clone(),
            AutomationServerConfig {
                bind_port: config.automation_port,
                data_dir: config.data_dir,
            },
        )
        .await
        {
            Ok(handle) => handle,
            Err(err) => {
                let _ = team_api.shutdown().await;
                return Err(err);
            }
        };
        Ok(Self {
            team_api,
            automation,
        })
    }

    pub fn startup_json(&self) -> Value {
        json!({
            "team_api_url": self.team_api.url(),
            "automation_url": self.automation.url(),
            "automation_token": self.automation.token(),
        })
    }

    pub async fn shutdown(self) -> RuntimeResult<()> {
        let team_res = self.team_api.shutdown().await;
        let auto_res = self.automation.shutdown().await;
        team_res?;
        auto_res
    }
}

#[derive(Clone)]
struct TeamApiState {
    database: OpenBotDatabase,
    voice: Arc<VoiceService>,
    data_dir: PathBuf,
    require_auth_token: Option<String>,
    event_tx: broadcast::Sender<()>,
    shutdown_watch: watch::Sender<bool>,
    server_id: String,
    app_version: String,
}

pub struct TeamApiServer;

impl TeamApiServer {
    pub async fn start(
        database: OpenBotDatabase,
        voice: Arc<VoiceService>,
        config: TeamApiConfig,
    ) -> RuntimeResult<TeamApiHandle> {
        let listener = TcpListener::bind(("127.0.0.1", config.bind_port)).await?;
        let addr = listener.local_addr()?;
        let (event_tx, _) = broadcast::channel::<()>(256);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (shutdown_watch, _) = watch::channel(false);

        let state = TeamApiState {
            database,
            voice,
            data_dir: config.data_dir,
            require_auth_token: config.require_auth_token,
            event_tx,
            shutdown_watch: shutdown_watch.clone(),
            server_id: format!("srv-{}", Uuid::new_v4()),
            app_version: TEAM_APP_VERSION.to_string(),
        };

        let app = Router::new()
            .route("/v1/health", get(handle_health))
            .route("/v1/compatibility", get(handle_compatibility))
            .route("/v1/capabilities", get(handle_compatibility))
            .route("/v1/identity", get(handle_identity))
            .route("/v1/agents", get(handle_list_agents).post(handle_create_agent))
            .route("/v1/agents/{agent_id}", get(handle_get_agent))
            .route(
                "/v1/channels",
                get(handle_list_channels).post(handle_create_channel),
            )
            .route(
                "/v1/channels/{channel_id}/messages",
                post(handle_post_channel_message),
            )
            .route("/v1/events", get(handle_events_sse))
            .route("/v1/voice/live/start", post(handle_voice_live_start))
            .route("/v1/voice/live/stop", post(handle_voice_live_stop))
            .route("/v1/direct/threads", get(handle_list_direct_threads))
            .route("/v1/direct/messages", post(handle_send_direct_message))
            .layer(middleware::from_fn_with_state(state.clone(), team_api_guard))
            .with_state(state);

        let join_handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        Ok(TeamApiHandle {
            addr,
            shutdown_tx: Some(shutdown_tx),
            shutdown_watch,
            join_handle: Some(join_handle),
        })
    }

    pub fn eval_protocol(req: TeamProtocolEvalRequest) -> RuntimeResult<TeamProtocolEvalOutcome> {
        let host_min = req.host_min.unwrap_or(TEAM_PROTOCOL_V1);
        let host_max = req.host_max.unwrap_or(TEAM_PROTOCOL_V10);
        if host_min == 0 || host_min > host_max {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid host protocol range.".to_string(),
            ));
        }
        let host_range = TeamProtocolRange {
            minimum: host_min,
            maximum: host_max,
        };
        let client_version = req.client_version.unwrap_or(host_max);
        let client_range = TeamProtocolRange {
            minimum: client_version,
            maximum: client_version,
        };
        let negotiated_version = highest_common_team_protocol(client_range, host_range);
        let direction = team_protocol_update_direction(client_range, host_range);
        let (http_status, error_code, update_direction) = match (negotiated_version, direction) {
            (Some(_), _) => (200, None, None),
            (None, Some(TeamProtocolUpdateDirection::ClientUpdateRequired)) => (
                426,
                Some("client_update_required".to_string()),
                Some("client_update_required".to_string()),
            ),
            (None, Some(TeamProtocolUpdateDirection::HostUpdateRequired)) => (
                426,
                Some("host_update_required".to_string()),
                Some("host_update_required".to_string()),
            ),
            (None, None) => (
                426,
                Some("client_update_required".to_string()),
                Some("client_update_required".to_string()),
            ),
        };

        let effective_protocol = negotiated_version.unwrap_or(host_max);
        let (encoded_v10, expanded_capabilities) = match (req.capabilities, req.absent_capabilities)
        {
            (Some(caps), Some(absent)) => {
                validate_capability_list(&caps, TEAM_PROTOCOL_V10_CAPABILITY_LIMIT)?;
                validate_capability_list(&absent, TEAM_PROTOCOL_V10_ABSENT_LIMIT)?;
                let lists = TeamCapabilitiesV10 {
                    capabilities: caps,
                    absent,
                };
                let expanded = expand_team_capabilities_v10(&lists);
                let reencoded = encode_team_capabilities_v10(&expanded);
                (reencoded, expanded)
            }
            (None, Some(absent)) => {
                validate_capability_list(&absent, TEAM_PROTOCOL_V10_ABSENT_LIMIT)?;
                let absent_set: HashSet<&str> = absent.iter().map(String::as_str).collect();
                let filtered: Vec<String> = team_current_capabilities()
                    .into_iter()
                    .filter(|c| !absent_set.contains(*c))
                    .map(str::to_string)
                    .collect();
                let encoded = encode_team_capabilities_v10(&filtered);
                let expanded = expand_team_capabilities_v10(&encoded);
                (encoded, expanded)
            }
            (Some(caps), None) => {
                validate_capability_list(&caps, TEAM_CAPABILITY_REGISTRY.len())?;
                let encoded = encode_team_capabilities_v10(&caps);
                let expanded = expand_team_capabilities_v10(&encoded);
                (encoded, expanded)
            }
            (None, None) => {
                let caps: Vec<String> = team_capabilities_for_protocol(Some(effective_protocol))
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                let encoded = encode_team_capabilities_v10(&caps);
                let expanded = if effective_protocol >= TEAM_PROTOCOL_V10 {
                    expand_team_capabilities_v10(&encoded)
                } else {
                    caps
                };
                (encoded, expanded)
            }
        };

        let mut headers =
            team_capability_headers(Some(effective_protocol), &expanded_capabilities);
        headers.insert(
            TEAM_PROTOCOL_VERSION_HEADER.to_string(),
            effective_protocol.to_string(),
        );

        let (wire_legacy, wire_current) = match req.wire_json {
            Some(ref val) => {
                let legacy = to_wire_agent_keys(val);
                let current = to_current_agent_keys(&legacy);
                (Some(legacy), Some(current))
            }
            None => (None, None),
        };

        let (webrtc_chunk_encoded_len, webrtc_roundtrip_ok) = if req.webrtc_transfer_id.is_some()
            || req.webrtc_offset.is_some()
            || req.webrtc_payload_bytes.is_some()
        {
            let transfer_id = req.webrtc_transfer_id.as_deref().unwrap_or("xfer-1");
            let offset = req.webrtc_offset.unwrap_or(0);
            let payload_len = req.webrtc_payload_bytes.unwrap_or(16);
            let payload: Vec<u8> = (0..payload_len).map(|i| (i & 0xff) as u8).collect();
            let encoded =
                JoinedServerClient::encode_webrtc_file_chunk(transfer_id, offset, &payload)?;
            let (dec_id, dec_offset, dec_payload) =
                JoinedServerClient::decode_webrtc_file_chunk(&encoded)?;
            let ok = dec_id == transfer_id && dec_offset == offset && dec_payload == payload;
            (Some(encoded.len()), Some(ok))
        } else {
            (None, None)
        };

        Ok(TeamProtocolEvalOutcome {
            negotiated_version,
            update_direction,
            http_status,
            error_code,
            total_capabilities: TEAM_CAPABILITY_REGISTRY.len(),
            expanded_capabilities,
            encoded_v10,
            headers,
            wire_legacy,
            wire_current,
            webrtc_chunk_encoded_len,
            webrtc_roundtrip_ok,
        })
    }
}

fn attach_security_headers(mut resp: Response) -> Response {
    let headers = resp.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    resp
}

fn host_protocol_support(app_version: &str) -> TeamProtocolSupport {
    let all_caps: Vec<String> = team_current_capabilities()
        .into_iter()
        .map(str::to_string)
        .collect();
    let encoded = encode_team_capabilities_v10(&all_caps);
    TeamProtocolSupport {
        app_version: app_version.to_string(),
        protocol: TeamProtocolRange {
            minimum: TEAM_PROTOCOL_V1,
            maximum: TEAM_PROTOCOL_V10,
        },
        capabilities: encoded.capabilities,
        absent: Some(encoded.absent),
    }
}

async fn team_api_guard(
    State(state): State<TeamApiState>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    let is_health = path == "/v1/health";
    let is_bootstrap = matches!(path, "/v1/health" | "/v1/compatibility" | "/v1/capabilities");

    if !is_health {
        if let Some(raw_app) = req.headers().get("openbot-app-version") {
            let valid_app = raw_app
                .to_str()
                .ok()
                .is_some_and(|s| utf16_len(s) <= 64);
            if !valid_app {
                let host = host_protocol_support(&state.app_version);
                return attach_security_headers(
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": "Invalid Team API protocol headers.",
                            "code": "protocol_error",
                            "host": host,
                        })),
                    )
                        .into_response(),
                );
            }
        }

        if let Some(raw_ver) = req.headers().get("openbot-protocol-version") {
            let parsed = raw_ver
                .to_str()
                .ok()
                .map(str::trim)
                .filter(|s| {
                    if let Some(rest) = s.strip_prefix('-') {
                        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                    } else {
                        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
                    }
                })
                .and_then(|s| s.parse::<i64>().ok());
            let host = host_protocol_support(&state.app_version);
            match parsed {
                None => {
                    return attach_security_headers(
                        (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": "Invalid Team API protocol headers.",
                                "code": "protocol_error",
                                "host": host,
                            })),
                        )
                            .into_response(),
                    );
                }
                Some(v) if v < i64::from(TEAM_PROTOCOL_V1) => {
                    return attach_security_headers(
                        (
                            StatusCode::UPGRADE_REQUIRED,
                            Json(json!({
                                "error": "Client update required.",
                                "code": "client_update_required",
                                "host": host,
                            })),
                        )
                            .into_response(),
                    );
                }
                Some(v) if v > i64::from(TEAM_PROTOCOL_V10) => {
                    return attach_security_headers(
                        (
                            StatusCode::UPGRADE_REQUIRED,
                            Json(json!({
                                "error": "Host update required.",
                                "code": "host_update_required",
                                "host": host,
                            })),
                        )
                            .into_response(),
                    );
                }
                Some(_) => {}
            }
        }
    }

    if !is_bootstrap
        && let Some(ref expected) = state.require_auth_token
    {
        let authorized = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|tok| ct_eq(tok.as_bytes(), expected.as_bytes()));
        if !authorized {
            return attach_security_headers(error_json_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Unauthorized.",
            ));
        }
    }

    let response = next.run(req).await;
    attach_security_headers(response)
}

async fn handle_health() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "schema_version": LATEST_SCHEMA_VERSION,
            "protocol_min": TEAM_PROTOCOL_V1,
            "protocol_max": TEAM_PROTOCOL_V10,
        })),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
struct CompatibilityResponse {
    #[serde(rename = "appVersion")]
    app_version: String,
    protocol: TeamProtocolRange,
    api_versions: [&'static str; 10],
    providers: Vec<&'static str>,
    capabilities: Vec<String>,
    absent: Vec<String>,
}

fn compatibility_capabilities() -> Vec<String> {
    let mut caps = Vec::with_capacity(TEAM_PROTOCOL_V10_CAPABILITY_LIMIT);
    let mut seen = HashSet::with_capacity(TEAM_PROTOCOL_V10_CAPABILITY_LIMIT);

    for entry in &TEAM_CAPABILITY_REGISTRY {
        if entry.since >= TEAM_PROTOCOL_V10
            && caps.len() < TEAM_PROTOCOL_V10_CAPABILITY_LIMIT
            && seen.insert(entry.capability)
        {
            caps.push(entry.capability.to_string());
        }
    }

    let baseline_target = TEAM_PROTOCOL_V10_CAPABILITY_LIMIT.saturating_sub(caps.len());
    let mut added_baseline = 0usize;
    for entry in &TEAM_CAPABILITY_REGISTRY {
        if added_baseline >= baseline_target {
            break;
        }
        if entry.since < TEAM_PROTOCOL_V10 && seen.insert(entry.capability) {
            caps.push(entry.capability.to_string());
            added_baseline += 1;
        }
    }

    if !seen.contains(TEAM_LONG_CAPABILITY_LISTS_CAPABILITY) {
        if caps.len() >= TEAM_PROTOCOL_V10_CAPABILITY_LIMIT {
            caps.pop();
        }
        caps.push(TEAM_LONG_CAPABILITY_LISTS_CAPABILITY.to_string());
    }

    caps
}

async fn handle_compatibility(State(state): State<TeamApiState>) -> Response {
    let body = CompatibilityResponse {
        app_version: state.app_version.clone(),
        protocol: TeamProtocolRange {
            minimum: TEAM_PROTOCOL_V1,
            maximum: TEAM_PROTOCOL_V10,
        },
        api_versions: TEAM_API_VERSIONS,
        providers: AgentProviderId::ALL
            .iter()
            .map(|provider| provider.as_str())
            .collect(),
        capabilities: compatibility_capabilities(),
        absent: Vec::new(),
    };
    let all_caps: Vec<String> = team_current_capabilities()
        .into_iter()
        .map(str::to_string)
        .collect();
    let cap_headers = team_capability_headers(Some(TEAM_PROTOCOL_V10), &all_caps);
    let mut response = (StatusCode::OK, Json(body)).into_response();
    let headers = response.headers_mut();
    if let Ok(name) =
        HeaderName::from_bytes(TEAM_PROTOCOL_VERSION_HEADER.to_ascii_lowercase().as_bytes())
        && let Ok(val) = HeaderValue::from_str(&TEAM_PROTOCOL_V10.to_string())
    {
        headers.insert(name, val);
    }
    if let Ok(name) =
        HeaderName::from_bytes(TEAM_APP_VERSION_HEADER.to_ascii_lowercase().as_bytes())
        && let Ok(val) = HeaderValue::from_str(&state.app_version)
    {
        headers.insert(name, val);
    }
    for (k, v) in cap_headers {
        if let Ok(name) = HeaderName::from_bytes(k.to_ascii_lowercase().as_bytes())
            && let Ok(val) = HeaderValue::from_str(&v)
        {
            headers.insert(name, val);
        }
    }
    if let Ok(name) = HeaderName::from_bytes(
        TEAM_ABSENT_CAPABILITIES_HEADER
            .to_ascii_lowercase()
            .as_bytes(),
    ) && !headers.contains_key(&name)
    {
        headers.insert(name, HeaderValue::from_static(""));
    }
    response
}

async fn handle_identity(State(state): State<TeamApiState>) -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "serverId": state.server_id,
            "serverName": "OpenBot Local Host",
            "appVersion": state.app_version,
            "protocol": {
                "minimum": TEAM_PROTOCOL_V1,
                "maximum": TEAM_PROTOCOL_V10,
            }
        })),
    )
        .into_response()
}

async fn handle_list_agents(State(state): State<TeamApiState>) -> Response {
    match state.database.agent_roster().list_agents() {
        Ok(agents) => (StatusCode::OK, Json(agents)).into_response(),
        Err(err) => runtime_error_to_response(err.into()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAgentHttpRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, alias = "avatar_seed")]
    avatar_seed: Option<String>,
    #[serde(default, alias = "avatar_hue")]
    avatar_hue: Option<AvatarHue>,
    #[serde(default, alias = "avatar_color")]
    avatar_color: Option<Option<String>>,
    #[serde(default, alias = "initial_message")]
    initial_message: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, alias = "reasoning_effort")]
    reasoning_effort: Option<AgentReasoningEffort>,
    #[serde(default, alias = "provider_account_id")]
    provider_account_id: Option<String>,
    #[serde(default, alias = "operation_id")]
    operation_id: Option<String>,
}

async fn handle_create_agent(
    State(state): State<TeamApiState>,
    Json(body): Json<CreateAgentHttpRequest>,
) -> Response {
    let provider = match body.provider.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => match AgentProviderId::parse(raw) {
            Some(p) => Some(p),
            None => {
                return error_json_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    &format!("Unknown agent provider: {raw}"),
                );
            }
        },
        None => None,
    };
    if let Some(ref model) = body.model
        && !is_agent_model(model)
    {
        return error_json_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "Invalid agent model.",
        );
    }
    let input = CreateAgentInput {
        name: body.name,
        description: body.description.unwrap_or_default(),
        avatar_seed: body
            .avatar_seed
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("agent-{}", Uuid::new_v4())),
        avatar_hue: body.avatar_hue,
        avatar_color: body.avatar_color,
        initial_message: body
            .initial_message
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Hello!".to_string()),
        provider,
        model: body.model,
        reasoning_effort: body.reasoning_effort,
        provider_account_id: body.provider_account_id,
        operation_id: body.operation_id,
    };
    let store = match AgentStore::open(state.database.clone(), &state.data_dir) {
        Ok(s) => s,
        Err(err) => return runtime_error_to_response(err.into()),
    };
    match store.create_agent(&input) {
        Ok(agent) => {
            let _ = state.event_tx.send(());
            (StatusCode::CREATED, Json(agent)).into_response()
        }
        Err(err) => runtime_error_to_response(err.into()),
    }
}

async fn handle_get_agent(
    State(state): State<TeamApiState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    match state.database.agent_roster().get_agent(&agent_id) {
        Ok(Some(agent)) => (StatusCode::OK, Json(agent)).into_response(),
        Ok(None) => error_json_response(
            StatusCode::NOT_FOUND,
            "not_found",
            &format!("Agent not found: {agent_id}"),
        ),
        Err(err) => runtime_error_to_response(err.into()),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelListItem {
    #[serde(flatten)]
    summary: ChannelSummary,
    channel: Channel,
}

async fn handle_list_channels(State(state): State<TeamApiState>) -> Response {
    let store = ChannelStore::new(state.database.clone());
    match store.list(SIGNED_OUT_CHANNEL_MEMBER_ID, true) {
        Ok(summaries) => {
            let items: Vec<ChannelListItem> = summaries
                .into_iter()
                .map(|summary| {
                    let channel = summary.channel.clone();
                    ChannelListItem { summary, channel }
                })
                .collect();
            (StatusCode::OK, Json(items)).into_response()
        }
        Err(err) => runtime_error_to_response(err.into()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateChannelHttpRequest {
    name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    members: Vec<Value>,
    #[serde(default, alias = "lead_agent_id")]
    lead_agent_id: Option<String>,
    #[serde(default)]
    isolated: Option<bool>,
    #[serde(default)]
    browser: Option<ChannelBrowser>,
    #[serde(default)]
    icon: Option<Option<ChannelIcon>>,
}

async fn handle_create_channel(
    State(state): State<TeamApiState>,
    Json(body): Json<CreateChannelHttpRequest>,
) -> Response {
    let members = match parse_channel_members(&body.members) {
        Ok(m) => m,
        Err(err) => return runtime_error_to_response(err),
    };
    if members.is_empty() {
        return error_json_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "At least one channel member is required (members must not be empty).",
        );
    }
    let draft = ChannelDraft {
        name: body.name,
        title: body.title.unwrap_or_default(),
        instructions: body.instructions.unwrap_or_default(),
        members,
        lead_agent_id: body.lead_agent_id,
        isolated: body.isolated,
        browser: body.browser,
        icon: body.icon,
    };
    if let Err(err) = draft.validate_strict(None) {
        return runtime_error_to_response(err.into());
    }
    let known_agents = match state.database.agent_roster().list_agents() {
        Ok(agents) => agents,
        Err(err) => return runtime_error_to_response(err.into()),
    };
    let known_ids: HashSet<&str> = known_agents.iter().map(|a| a.id.as_str()).collect();
    for member in &draft.members {
        if !known_ids.contains(member.agent_id.as_str()) {
            return error_json_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                &format!("Channel member '{}' is unavailable.", member.agent_id),
            );
        }
    }
    let store = ChannelStore::new(state.database.clone());
    let existing_ids = match store.ids() {
        Ok(ids) => ids,
        Err(err) => return runtime_error_to_response(err.into()),
    };
    if existing_ids.len() >= INPUT_LIMITS.agents {
        return runtime_error_to_response(OpenBotRuntimeError::LimitExceeded(format!(
            "A host can have at most {} channels.",
            INPUT_LIMITS.agents
        )));
    }
    let channel_id = format!("channel-{}", Uuid::new_v4());
    let channel = store.create(&channel_id, draft);
    let saved = match store.commit(
        &format!("create:{channel_id}"),
        ChannelChange {
            channel,
            messages: Vec::new(),
            tasks: Vec::new(),
            assignments: Vec::new(),
        },
    ) {
        Ok(saved) => saved,
        Err(err) => return runtime_error_to_response(err.into()),
    };
    for member in &saved.draft.members {
        if let Err(err) = store.context(&saved.id, &member.agent_id) {
            return runtime_error_to_response(err.into());
        }
    }
    let _ = state.event_tx.send(());
    (StatusCode::CREATED, Json(saved)).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostChannelMessageHttpRequest {
    text: String,
    #[serde(default, alias = "message_id", alias = "clientMessageId", alias = "client_message_id")]
    message_id: Option<String>,
    #[serde(default)]
    author: Option<ChannelMessageAuthor>,
    #[serde(default, alias = "author_id")]
    author_id: Option<String>,
    #[serde(default, alias = "author_name")]
    author_name: Option<String>,
    #[serde(default, alias = "author_kind")]
    author_kind: Option<ChannelMessageAuthorKind>,
}

async fn handle_post_channel_message(
    State(state): State<TeamApiState>,
    AxumPath(channel_id): AxumPath<String>,
    Json(body): Json<PostChannelMessageHttpRequest>,
) -> Response {
    let text = body.text.trim();
    if text.is_empty() || !is_bounded_string(text, INPUT_LIMITS.message_text) {
        return error_json_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "Channel message text must be non-empty and within length limits.",
        );
    }
    let msg_id = match body.message_id {
        Some(id) => {
            if let Err(err) = validate_strict_identifier(&id, "messageId") {
                return runtime_error_to_response(err);
            }
            id
        }
        None => format!("msg-{}", Uuid::new_v4()),
    };
    if let Some(ref id) = body.author_id
        && let Err(err) = validate_strict_identifier(id, "authorId")
    {
        return runtime_error_to_response(err);
    }
    if let Some(ref name) = body.author_name
        && let Err(err) = validate_author_name(name, "authorName")
    {
        return runtime_error_to_response(err);
    }
    let author = match body.author {
        Some(author) => {
            if let Err(err) = validate_strict_identifier(&author.id, "author.id") {
                return runtime_error_to_response(err);
            }
            if let Err(err) = validate_author_name(&author.name, "author.name") {
                return runtime_error_to_response(err);
            }
            author
        }
        None => {
            let author_id = body
                .author_id
                .unwrap_or_else(|| SIGNED_OUT_CHANNEL_MEMBER_ID.to_string());
            let author_name = body.author_name.unwrap_or_else(|| "User".to_string());
            ChannelMessageAuthor {
                kind: body.author_kind.unwrap_or(ChannelMessageAuthorKind::Member),
                id: author_id,
                name: author_name,
            }
        }
    };
    let store = ChannelStore::new(state.database.clone());
    let channel = match store.get(&channel_id) {
        Ok(c) => c,
        Err(err) => return runtime_error_to_response(err.into()),
    };
    if channel.archived {
        return error_json_response(
            StatusCode::CONFLICT,
            "channel_archived",
            crate::channel_service::CHANNEL_ARCHIVED_ERROR,
        );
    }
    let next_seq = match state.database.with_conn(|conn| {
        let max_seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM projection_channel_messages WHERE channel_id = ?1",
            rusqlite::params![&channel_id],
            |r| r.get(0),
        )?;
        Ok((max_seq.max(0) as u64) + 1)
    }) {
        Ok(s) => s,
        Err(err) => return runtime_error_to_response(err.into()),
    };
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let conv_author = match author.kind {
        ChannelMessageAuthorKind::Member => ConversationMessageAuthor::User,
        ChannelMessageAuthorKind::Agent => ConversationMessageAuthor::Agent,
        ChannelMessageAuthorKind::Coordinator => ConversationMessageAuthor::System,
    };
    let channel_msg = ChannelMessage {
        id: msg_id.clone(),
        channel_id: channel.id.clone(),
        sequence: next_seq,
        author,
        task_id: None,
        superseded: false,
        message: ConversationMessage::new_simple(
            msg_id,
            conv_author,
            text.to_string(),
            now,
            ConversationMessageStatus::Completed,
        ),
        deleted: None,
        sender_task_id: None,
    };
    match store.capture_message(channel, channel_msg.clone()) {
        Ok(_) => {
            let _ = state.event_tx.send(());
            (StatusCode::CREATED, Json(channel_msg)).into_response()
        }
        Err(err) => runtime_error_to_response(err.into()),
    }
}

#[derive(Debug, Default, Deserialize)]
struct EventsQuery {
    #[serde(rename = "lastEventId", alias = "last_event_id")]
    last_event_id: Option<String>,
}

struct SseStreamState {
    database: OpenBotDatabase,
    rx: broadcast::Receiver<()>,
    shutdown: watch::Receiver<bool>,
    cursor: i64,
    pending: VecDeque<Event>,
    sent_connected: bool,
}

async fn handle_events_sse(
    State(state): State<TeamApiState>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> Response {
    let raw_last = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            query
                .last_event_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });

    let rx = state.event_tx.subscribe();
    let shutdown = state.shutdown_watch.subscribe();
    let initial_cursor = match raw_last {
        Some(raw) => match raw.parse::<i64>() {
            Ok(id) if id >= 0 => id,
            _ => {
                return error_json_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    "Invalid Last-Event-ID cursor.",
                );
            }
        },
        None => match state.database.with_conn(|conn| {
            let max_seq: i64 = conn.query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM orchestration_events",
                [],
                |r| r.get(0),
            )?;
            Ok(max_seq)
        }) {
            Ok(seq) => seq,
            Err(err) => return runtime_error_to_response(err.into()),
        },
    };

    let init = SseStreamState {
        database: state.database.clone(),
        rx,
        shutdown,
        cursor: initial_cursor,
        pending: VecDeque::new(),
        sent_connected: false,
    };

    let stream = futures::stream::unfold(init, |mut st| async move {
        if !st.sent_connected {
            st.sent_connected = true;
            let ready_payload = json!({ "ok": true, "cursor": st.cursor }).to_string();
            return Some((
                Ok::<Event, Infallible>(Event::default().event("ready").data(ready_payload)),
                st,
            ));
        }
        loop {
            if let Some(event) = st.pending.pop_front() {
                return Some((Ok::<Event, Infallible>(event), st));
            }
            if *st.shutdown.borrow() {
                return None;
            }
            if let Ok(batch) = st.database.read_events_since(st.cursor, SSE_BATCH_LIMIT)
                && !batch.is_empty()
            {
                if let Some(last) = batch.last() {
                    st.cursor = last.sequence;
                }
                for ev in &batch {
                    orchestration_event_to_sse_frames(ev, &mut st.pending);
                }
                continue;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(SSE_POLL_INTERVAL_MS)) => {}
                recv = st.rx.recv() => {
                    if matches!(recv, Err(broadcast::error::RecvError::Closed)) {
                        return None;
                    }
                }
                changed = st.shutdown.changed() => {
                    if changed.is_err() || *st.shutdown.borrow() {
                        return None;
                    }
                }
            }
        }
    });

    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoiceLiveStartHttpRequest {
    #[serde(alias = "agent_id")]
    agent_id: String,
    sdp: String,
    #[serde(default)]
    voice: Option<String>,
}

async fn handle_voice_live_start(
    State(state): State<TeamApiState>,
    Json(body): Json<VoiceLiveStartHttpRequest>,
) -> Response {
    match state.database.agent_roster().get_agent(&body.agent_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_json_response(
                StatusCode::NOT_FOUND,
                "not_found",
                &format!("Agent not found: {}", body.agent_id),
            );
        }
        Err(err) => return runtime_error_to_response(err.into()),
    }
    let voice = match body.voice.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => match parse_codex_live_voice(raw) {
            Some(v) => Some(v),
            None => {
                return error_json_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    &format!("Unknown Codex live voice: {raw}"),
                );
            }
        },
        None => None,
    };
    match state.voice.start_live_session(
        VoiceLiveStart {
            agent_id: body.agent_id,
            sdp: body.sdp,
        },
        voice,
    ) {
        Ok(session) => (StatusCode::OK, Json(session)).into_response(),
        Err(err) => runtime_error_to_response(err),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoiceLiveStopHttpRequest {
    #[serde(alias = "session_id")]
    session_id: String,
}

async fn handle_voice_live_stop(
    State(state): State<TeamApiState>,
    Json(body): Json<VoiceLiveStopHttpRequest>,
) -> Response {
    match state.voice.stop_live_session(&body.session_id) {
        Ok(stopped) => (StatusCode::OK, Json(json!({ "stopped": stopped }))).into_response(),
        Err(err) => runtime_error_to_response(err),
    }
}

#[derive(Debug, Default, Deserialize)]
struct DirectThreadsQuery {
    #[serde(rename = "memberId", alias = "member_id")]
    member_id: Option<String>,
}

async fn handle_list_direct_threads(
    State(state): State<TeamApiState>,
    Query(query): Query<DirectThreadsQuery>,
) -> Response {
    let member_id = match query.member_id {
        Some(raw) => {
            if let Err(err) = validate_strict_identifier(&raw, "memberId") {
                return runtime_error_to_response(err);
            }
            raw
        }
        None => SIGNED_OUT_CHANNEL_MEMBER_ID.to_string(),
    };
    let store = TeamChatStore::new(state.database.clone());
    match store.list_threads(&member_id) {
        Ok(threads) => (StatusCode::OK, Json(threads)).into_response(),
        Err(err) => runtime_error_to_response(err.into()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendDirectMessageHttpRequest {
    #[serde(default, alias = "client_message_id")]
    client_message_id: Option<String>,
    #[serde(default, alias = "sender_member_id")]
    sender_member_id: Option<String>,
    #[serde(alias = "recipient_member_id")]
    recipient_member_id: String,
    text: String,
}

async fn handle_send_direct_message(
    State(state): State<TeamApiState>,
    Json(body): Json<SendDirectMessageHttpRequest>,
) -> Response {
    if let Err(err) = validate_strict_identifier(&body.recipient_member_id, "recipientMemberId") {
        return runtime_error_to_response(err);
    }
    let sender_member_id = match body.sender_member_id {
        Some(s) => {
            if let Err(err) = validate_strict_identifier(&s, "senderMemberId") {
                return runtime_error_to_response(err);
            }
            s
        }
        None => SIGNED_OUT_CHANNEL_MEMBER_ID.to_string(),
    };
    let client_message_id = match body.client_message_id {
        Some(s) => {
            if let Err(err) = validate_strict_identifier(&s, "clientMessageId") {
                return runtime_error_to_response(err);
            }
            s
        }
        None => format!("dm-msg-{}", Uuid::new_v4()),
    };
    let input = SendDirectMessageInput {
        client_message_id,
        sender_member_id,
        recipient_member_id: body.recipient_member_id,
        text: body.text,
        created_at: None,
    };
    let store = TeamChatStore::new(state.database.clone());
    match store.send_message(&input) {
        Ok(msg) => {
            let _ = state.event_tx.send(());
            (StatusCode::CREATED, Json(msg)).into_response()
        }
        Err(err) => runtime_error_to_response(err.into()),
    }
}

fn orchestration_event_to_sse_frames(ev: &StoredOrchestrationEvent, out: &mut VecDeque<Event>) {
    let parsed_payload: Value = serde_json::from_str(&ev.payload_json)
        .unwrap_or_else(|_| Value::String(ev.payload_json.clone()));
    let envelope = json!({
        "sequence": ev.sequence,
        "eventId": ev.event_id,
        "commandId": ev.command_id,
        "aggregateType": ev.aggregate_type,
        "aggregateId": ev.aggregate_id,
        "eventType": ev.event_type,
        "occurredAt": ev.occurred_at,
        "payload": parsed_payload,
    });
    let id = ev.sequence.to_string();
    let data = envelope.to_string();

    out.push_back(
        Event::default()
            .id(id.clone())
            .event(ev.event_type.clone())
            .data(data.clone()),
    );

    if ev.event_type == "channel.changed" {
        let is_created = ev.command_id.starts_with("channels:create:")
            || parsed_payload
                .get("channel")
                .and_then(|c| c.get("revision"))
                .and_then(Value::as_u64)
                == Some(1);
        let lifecycle_event = if is_created {
            "channel.created"
        } else {
            "channel.updated"
        };
        out.push_back(
            Event::default()
                .id(id.clone())
                .event(lifecycle_event)
                .data(data.clone()),
        );

        let has_posted_messages = !ev.command_id.starts_with("channels:message-deleted:")
            && parsed_payload
                .get("messages")
                .and_then(Value::as_array)
                .is_some_and(|msgs| {
                    msgs.iter()
                        .any(|m| m.get("deleted").and_then(Value::as_bool) != Some(true))
                });
        if has_posted_messages {
            out.push_back(
                Event::default()
                    .id(id)
                    .event("message.posted")
                    .data(data),
            );
        }
    }
}

fn validate_strict_identifier(value: &str, field: &str) -> RuntimeResult<()> {
    if value.trim().is_empty() || value.trim() != value || !is_identifier(value) {
        return Err(OpenBotRuntimeError::Validation(format!(
            "{field} must be a non-empty identifier without leading/trailing whitespace and at most {} characters.",
            INPUT_LIMITS.identifier
        )));
    }
    Ok(())
}

fn validate_author_name(value: &str, field: &str) -> RuntimeResult<()> {
    if value.trim().is_empty()
        || value.trim() != value
        || !is_bounded_string(value, INPUT_LIMITS.account_name)
    {
        return Err(OpenBotRuntimeError::Validation(format!(
            "{field} must be non-empty without leading/trailing whitespace and at most {} characters.",
            INPUT_LIMITS.account_name
        )));
    }
    Ok(())
}

fn parse_channel_members(raw: &[Value]) -> RuntimeResult<Vec<ChannelMember>> {
    let mut members = Vec::with_capacity(raw.len());
    for item in raw {
        if let Some(s) = item.as_str() {
            validate_strict_identifier(s, "Channel member agentId")?;
            members.push(ChannelMember {
                agent_id: s.to_string(),
            });
        } else if let Some(obj) = item.as_object() {
            let aid = obj
                .get("agentId")
                .or_else(|| obj.get("agent_id"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    OpenBotRuntimeError::Validation(
                        "Channel member object must contain string agentId.".to_string(),
                    )
                })?;
            validate_strict_identifier(aid, "Channel member agentId")?;
            members.push(ChannelMember {
                agent_id: aid.to_string(),
            });
        } else {
            return Err(OpenBotRuntimeError::Validation(
                "Channel member must be a string or object with agentId.".to_string(),
            ));
        }
    }
    Ok(members)
}

fn validate_capability_list(list: &[String], limit: usize) -> RuntimeResult<()> {
    if list.len() > limit {
        return Err(OpenBotRuntimeError::LimitExceeded(format!(
            "Capability list length {} exceeds limit of {limit}.",
            list.len()
        )));
    }
    for slug in list {
        if !is_capability_slug(slug) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Invalid capability slug: {slug}"
            )));
        }
    }
    Ok(())
}

fn error_json_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": message,
            "code": code,
        })),
    )
        .into_response()
}

fn runtime_error_to_response(err: OpenBotRuntimeError) -> Response {
    let status = match &err {
        OpenBotRuntimeError::NotFound(_)
        | OpenBotRuntimeError::Storage(StorageError::NotFound(_)) => StatusCode::NOT_FOUND,
        OpenBotRuntimeError::Conflict(_)
        | OpenBotRuntimeError::Storage(StorageError::Conflict(_)) => StatusCode::CONFLICT,
        OpenBotRuntimeError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
        OpenBotRuntimeError::Forbidden(_)
        | OpenBotRuntimeError::ApprovalRequired { .. }
        | OpenBotRuntimeError::Storage(StorageError::Forbidden(_)) => StatusCode::FORBIDDEN,
        OpenBotRuntimeError::LimitExceeded(_)
        | OpenBotRuntimeError::Storage(StorageError::LimitExceeded(_)) => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        OpenBotRuntimeError::Validation(_)
        | OpenBotRuntimeError::Contract(_)
        | OpenBotRuntimeError::Json(_)
        | OpenBotRuntimeError::Storage(StorageError::Validation(_)) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_json_response(status, err.error_code(), &err.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinedServerEntry {
    pub server_id: String,
    pub server_name: String,
    pub base_url: String,
    pub negotiated_protocol: u16,
    pub joined_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredJoinedServers {
    #[serde(default)]
    active_server_id: Option<String>,
    #[serde(default)]
    servers: Vec<JoinedServerEntry>,
}

pub struct JoinedServerClient {
    data_dir: PathBuf,
    store: VersionedJsonStore<StoredJoinedServers>,
}

impl JoinedServerClient {
    pub fn open(data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let store = VersionedJsonStore::new(
            data_dir.join(preference_files::TEAM_SERVER_V2),
            2,
            StoredJoinedServers::default(),
        );
        let _ = store.load()?;
        Ok(Arc::new(Self { data_dir, store }))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn list_servers(&self) -> Vec<JoinedServerEntry> {
        self.store.get().servers
    }

    pub fn save_server(&self, entry: JoinedServerEntry) -> RuntimeResult<Vec<JoinedServerEntry>> {
        if entry.server_id.trim().is_empty()
            || !is_bounded_string(&entry.server_id, INPUT_LIMITS.identifier)
        {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid joined serverId.".to_string(),
            ));
        }
        let updated = self.store.update(|mut state| {
            if let Some(existing) = state
                .servers
                .iter_mut()
                .find(|s| s.server_id == entry.server_id)
            {
                *existing = entry.clone();
            } else {
                state.servers.push(entry.clone());
            }
            if state.active_server_id.is_none() {
                state.active_server_id = Some(entry.server_id.clone());
            }
            state
        })?;
        Ok(updated.servers)
    }

    pub fn remove_server(&self, server_id: &str) -> RuntimeResult<bool> {
        let mut removed = false;
        let _ = self.store.update(|mut state| {
            let before = state.servers.len();
            state.servers.retain(|s| s.server_id != server_id);
            removed = state.servers.len() < before;
            if state.active_server_id.as_deref() == Some(server_id) {
                state.active_server_id = state.servers.first().map(|s| s.server_id.clone());
            }
            state
        })?;
        Ok(removed)
    }

    pub fn negotiate_support(&self, remote: &TeamProtocolSupport) -> RuntimeResult<u16> {
        let expanded = decode_team_protocol_support_expanded(remote)?;
        let local = TeamProtocolRange {
            minimum: TEAM_PROTOCOL_V1,
            maximum: TEAM_PROTOCOL_V10,
        };
        if let Some(version) = highest_common_team_protocol(local, expanded.protocol) {
            return Ok(version);
        }
        match team_protocol_update_direction(local, expanded.protocol) {
            Some(TeamProtocolUpdateDirection::ClientUpdateRequired) => Err(
                OpenBotRuntimeError::Validation(
                    "client_update_required: remote host requires a newer OpenBot client protocol."
                        .to_string(),
                ),
            ),
            Some(TeamProtocolUpdateDirection::HostUpdateRequired) => Err(
                OpenBotRuntimeError::Validation(
                    "host_update_required: remote host protocol is older than supported minimum."
                        .to_string(),
                ),
            ),
            None => Err(OpenBotRuntimeError::Validation(
                "protocol_mismatch: no common Team API protocol version.".to_string(),
            )),
        }
    }

    pub fn encode_webrtc_file_chunk(
        transfer_id: &str,
        offset: u64,
        payload: &[u8],
    ) -> RuntimeResult<Vec<u8>> {
        if payload.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "WebRTC v2 file chunk payload must not be empty.".to_string(),
            ));
        }
        if offset > TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "WebRTC v2 file chunk offset {offset} exceeds maximum safe offset {TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET}."
            )));
        }
        let frame_size = 10usize
            .saturating_add(transfer_id.len())
            .saturating_add(payload.len());
        if frame_size > TEAM_PROTOCOL_V2_MAX_BINARY_FRAME_BYTES {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "WebRTC v2 file chunk frame size {frame_size} exceeds maximum of {TEAM_PROTOCOL_V2_MAX_BINARY_FRAME_BYTES} bytes."
            )));
        }
        let chunk = TeamProtocolV2FileChunk {
            transfer_id: transfer_id.to_string(),
            offset,
            bytes: payload.to_vec(),
        };
        Ok(encode_team_protocol_v2_file_chunk(
            &chunk,
            Some(TEAM_PROTOCOL_V2_MAX_BINARY_FRAME_BYTES),
        )?)
    }

    pub fn decode_webrtc_file_chunk(frame: &[u8]) -> RuntimeResult<(String, u64, Vec<u8>)> {
        if frame.len() > TEAM_PROTOCOL_V2_MAX_BINARY_FRAME_BYTES {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "WebRTC v2 file chunk frame length {} exceeds maximum of {TEAM_PROTOCOL_V2_MAX_BINARY_FRAME_BYTES} bytes.",
                frame.len()
            )));
        }
        let chunk = decode_team_protocol_v2_file_chunk(frame)?;
        Ok((chunk.transfer_id, chunk.offset, chunk.bytes))
    }
}
