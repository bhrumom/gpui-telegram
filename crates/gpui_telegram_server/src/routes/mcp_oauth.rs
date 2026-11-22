use crate::auth::verify_password;
use crate::db::mcp_oauth_store::{
    ct_str_eq, random_b64url, random_hex, sha256_hex, McpCodeExchangeOutcome,
    McpOAuthClientRecord, McpOAuthCodeRecord, McpRefreshOutcome, MCP_AUTH_CODE_TTL_MS,
};
use crate::routes::err_resp;
use crate::server::{AppState, AuthBearer};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Json, Response},
};
use chrono::Utc;
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::remote_control_contracts::{
    McpOAuthGrantSummaryDto, McpOAuthGrantsResponse,
};
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use url::{Host, Url};

pub const MCP_SUPPORTED_SCOPES: &[&str] = &[
    "devices:read",
    "devices:connect",
    "screen:view",
    "input:control",
    "terminal:exec",
    "files:read",
    "files:write",
    "assist:manage",
    "power:control",
    "messaging:read",
    "messaging:write",
    "chats:read",
    "miniapp:control",
    "tasks:manage",
];

const OAUTH_TX_COOKIE_NAME: &str = "fbmcp_oauth_sess";
const OAUTH_USER_COOKIE_NAME: &str = "fbmcp_user_sess";
const OAUTH_TX_TTL_MS: i64 = 15 * 60_000;
const OAUTH_USER_COOKIE_TTL_MS: i64 = 24 * 3_600_000;

#[derive(Debug, Clone)]
pub struct PendingOAuthTx {
    pub tx_id: String,
    pub cookie_secret_hash: String,
    pub csrf_token: String,
    pub client_id: String,
    pub client_name: String,
    pub redirect_uri: String,
    pub resource: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub requested_scopes: Vec<String>,
    pub state_param: Option<String>,
    pub authenticated_user_id: Option<u64>,
    pub qr_pair_token: Option<String>,
    pub expires_at_ms: i64,
}

#[derive(Debug, Default)]
pub struct McpOAuthRuntimeState {
    pending_txs: RwLock<HashMap<String, PendingOAuthTx>>,
    user_cookies: RwLock<HashMap<String, (u64, i64)>>,
}

impl McpOAuthRuntimeState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prune_expired(&self, now_ms: i64) {
        let mut txs = self.pending_txs.write();
        if txs.len() > 256 {
            txs.retain(|_, v| v.expires_at_ms > now_ms);
        }
        let mut cookies = self.user_cookies.write();
        if cookies.len() > 256 {
            cookies.retain(|_, (_, exp)| *exp > now_ms);
        }
    }
}

pub fn resolve_public_base_url(headers: &HeaderMap, state: &AppState) -> String {
    if let Ok(env_val) = std::env::var("MCP_PUBLIC_BASE_URL") {
        let trimmed = env_val.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| matches!(*s, "http" | "https"))
        .unwrap_or("http");
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.contains('/') && !s.chars().any(char::is_whitespace));
    if let Some(h) = host {
        format!("{proto}://{h}")
    } else {
        format!("http://{}:{}", state.config.host, state.config.port)
    }
}

pub fn is_supported_scope(scope: &str) -> bool {
    MCP_SUPPORTED_SCOPES.contains(&scope.trim())
}

pub fn parse_scopes(raw: Option<&str>) -> std::result::Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(s) = raw {
        for part in s.split_whitespace() {
            let clean = part.trim();
            if clean.is_empty() {
                continue;
            }
            if !is_supported_scope(clean) {
                return Err(format!("Unsupported OAuth scope: `{clean}`"));
            }
            if seen.insert(clean.to_string()) {
                out.push(clean.to_string());
            }
        }
    }
    Ok(out)
}

pub fn validate_redirect_uri(raw: &str) -> std::result::Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 || trimmed.chars().any(|c| c.is_control()) {
        return Err("Redirect URI is empty or contains invalid characters".to_string());
    }
    let parsed = Url::parse(trimmed).map_err(|e| format!("Invalid redirect URI: {e}"))?;
    if parsed.fragment().is_some() {
        return Err("Redirect URI must not contain a fragment".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Redirect URI must not contain userinfo".to_string());
    }

    match parsed.scheme() {
        "https" => {
            let Some(host) = parsed.host() else {
                return Err("HTTPS redirect URI requires a host".to_string());
            };
            if matches!(host, Host::Ipv4(ip) if ip == Ipv4Addr::UNSPECIFIED)
                || matches!(host, Host::Ipv6(ip) if ip == Ipv6Addr::UNSPECIFIED)
            {
                return Err("Unspecified IP host is not permitted".to_string());
            }
            Ok(trimmed.to_string())
        }
        "http" => {
            let Some(host) = parsed.host() else {
                return Err("HTTP redirect URI requires a loopback host".to_string());
            };
            let is_loopback = match host {
                Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
                Host::Ipv4(ip) => ip == Ipv4Addr::LOCALHOST,
                Host::Ipv6(ip) => ip == Ipv6Addr::LOCALHOST,
            };
            if !is_loopback {
                return Err(
                    "Non-loopback http:// redirect URIs are rejected; use https:// or loopback (127.0.0.1 / localhost / [::1])".to_string(),
                );
            }
            Ok(trimmed.to_string())
        }
        "cursor" | "vscode" | "antigravity" | "claude" => {
            if parsed.host_str().is_none() && parsed.path().is_empty() {
                return Err("IDE custom scheme redirect URI requires authority or path".to_string());
            }
            if parsed.path().contains("..") {
                return Err("Path traversal is not permitted in redirect URI".to_string());
            }
            Ok(trimmed.to_string())
        }
        other => Err(format!(
            "Unsupported redirect_uri scheme `{other}`"
        )),
    }
}

pub fn validate_target_resource(raw: Option<&str>, base_url: &str) -> std::result::Result<String, String> {
    let expected_mcp = format!("{}/mcp", base_url.trim_end_matches('/'));
    let Some(r) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(expected_mcp);
    };
    let r_norm = r.trim_end_matches('/');
    let base_norm = base_url.trim_end_matches('/');
    if r_norm == expected_mcp || r_norm == base_norm {
        return Ok(r_norm.to_string());
    }
    Err(format!(
        "Invalid resource audience `{r}`; expected `{expected_mcp}`"
    ))
}

fn is_non_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()
                || ((v6.segments()[0] & 0xfe00) == 0xfc00)
                || ((v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

async fn resolve_oauth_client(
    state: &AppState,
    client_id: &str,
) -> std::result::Result<McpOAuthClientRecord, String> {
    let trimmed = client_id.trim();
    if trimmed.is_empty() {
        return Err("client_id is required".to_string());
    }
    if let Ok(Some(rec)) = state.db.get_mcp_oauth_client(trimmed) {
        return Ok(rec);
    }

    if trimmed.starts_with("https://") {
        let parsed = Url::parse(trimmed).map_err(|e| format!("Invalid CIMD client_id URL: {e}"))?;
        if parsed.fragment().is_some() || !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("CIMD URL must not contain fragments or userinfo".to_string());
        }
        match parsed.host() {
            Some(Host::Domain(d)) => {
                if d.eq_ignore_ascii_case("localhost") || d.ends_with(".local") || d.ends_with(".internal") {
                    return Err("CIMD URL cannot point to internal hostnames".to_string());
                }
            }
            Some(Host::Ipv4(ip)) if is_non_public_ip(IpAddr::V4(ip)) => {
                return Err("CIMD URL cannot point to private/loopback IP".to_string());
            }
            Some(Host::Ipv6(ip)) if is_non_public_ip(IpAddr::V6(ip)) => {
                return Err("CIMD URL cannot point to private/loopback IP".to_string());
            }
            None => return Err("CIMD URL missing host".to_string()),
            _ => {}
        }

        if cfg!(test) || std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
            return Err("Unknown CIMD client in offline mode".to_string());
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("HTTP client error: {e}"))?;
        let resp = client
            .get(trimmed)
            .send()
            .await
            .map_err(|e| format!("Failed fetching CIMD metadata: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("CIMD endpoint returned HTTP {}", resp.status()));
        }
        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("Failed reading CIMD body: {e}"))?;
        if body.len() > 64 * 1024 {
            return Err("CIMD metadata document exceeds 64KB limit".to_string());
        }
        let meta: Value = serde_json::from_slice(&body)
            .map_err(|e| format!("Malformed CIMD JSON document: {e}"))?;
        let doc_cid = meta.get("client_id").and_then(|v| v.as_str()).unwrap_or("");
        if doc_cid != trimmed {
            return Err("CIMD client_id field does not match document URL".to_string());
        }
        let client_name = meta
            .get("client_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("MCP Client")
            .to_string();
        let raw_uris = meta
            .get("redirect_uris")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "CIMD metadata missing redirect_uris array".to_string())?;
        let mut redirect_uris = Vec::with_capacity(raw_uris.len());
        for u in raw_uris {
            let s = u.as_str().ok_or_else(|| "redirect_uris must be strings".to_string())?;
            redirect_uris.push(validate_redirect_uri(s)?);
        }
        if redirect_uris.is_empty() {
            return Err("CIMD metadata must specify at least one redirect_uri".to_string());
        }
        let rec = McpOAuthClientRecord {
            client_id: trimmed.to_string(),
            client_name,
            redirect_uris,
            token_endpoint_auth_method: "none".to_string(),
            client_secret_hash: None,
            is_cimd: true,
            created_at_ms: Utc::now().timestamp_millis(),
        };
        let _ = state.db.upsert_mcp_oauth_client(&rec);
        return Ok(rec);
    }

    Err(format!("Unknown OAuth client_id `{trimmed}`"))
}

// ============================================================================
// Discovery Endpoints (RFC 9728, RFC 8414, OpenID Connect Discovery)
// ============================================================================

pub async fn oauth_protected_resource_metadata(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let base = resolve_public_base_url(&headers, &state);
    let body = json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "bearer_methods_supported": ["header"],
        "scopes_supported": MCP_SUPPORTED_SCOPES,
        "resource_name": "Fabushi Official MCP Server",
        "resource_documentation": format!("{base}/.well-known/oauth-authorization-server"),
    });
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(body),
    )
        .into_response()
}

pub async fn oauth_authorization_server_metadata(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let base = resolve_public_base_url(&headers, &state);
    let body = json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "revocation_endpoint": format!("{base}/oauth/revoke"),
        "scopes_supported": MCP_SUPPORTED_SCOPES,
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
        "code_challenge_methods_supported": ["S256"],
        "client_id_metadata_document_supported": true,
    });
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(body),
    )
        .into_response()
}

// ============================================================================
// RFC 7591 Dynamic Client Registration
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct DynamicRegisterRequest {
    #[serde(default)]
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
}

pub async fn oauth_register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DynamicRegisterRequest>,
) -> Response {
    if req.redirect_uris.is_empty() {
        return oauth_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "redirect_uris must be a non-empty array",
        );
    }

    let mut validated_uris = Vec::with_capacity(req.redirect_uris.len());
    for raw in &req.redirect_uris {
        match validate_redirect_uri(raw) {
            Ok(u) => validated_uris.push(u),
            Err(reason) => {
                return oauth_json_error(StatusCode::BAD_REQUEST, "invalid_redirect_uri", &reason);
            }
        }
    }

    let auth_method = req
        .token_endpoint_auth_method
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("none");
    if !matches!(auth_method, "none" | "client_secret_post") {
        return oauth_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "token_endpoint_auth_method must be 'none' or 'client_secret_post'",
        );
    }

    let client_name = req
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("MCP AI Client")
        .chars()
        .take(128)
        .collect::<String>();

    let now_ms = Utc::now().timestamp_millis();
    let client_id = format!("fbmcp_cli_{}", random_hex(12));
    let (plain_secret, secret_hash) = if auth_method == "client_secret_post" {
        let sec = format!("fbmcp_cs_{}", random_b64url(24));
        let h = sha256_hex(&sec);
        (Some(sec), Some(h))
    } else {
        (None, None)
    };

    let rec = McpOAuthClientRecord {
        client_id: client_id.clone(),
        client_name: client_name.clone(),
        redirect_uris: validated_uris.clone(),
        token_endpoint_auth_method: auth_method.to_string(),
        client_secret_hash: secret_hash,
        is_cimd: false,
        created_at_ms: now_ms,
    };

    if let Err(e) = state.db.upsert_mcp_oauth_client(&rec) {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "server_error", &e.to_string());
    }

    let mut resp_json = json!({
        "client_id": client_id,
        "client_name": client_name,
        "redirect_uris": validated_uris,
        "token_endpoint_auth_method": auth_method,
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "client_id_issued_at": now_ms / 1000,
    });
    if let Some(sec) = plain_secret {
        if let Some(obj) = resp_json.as_object_mut() {
            obj.insert("client_secret".into(), Value::String(sec));
            obj.insert("client_secret_expires_at".into(), json!(0));
        }
    }

    (
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(resp_json),
    )
        .into_response()
}

// ============================================================================
// Authorization UI, Login & Consent (GitHub MCP UX)
// ============================================================================

#[derive(Debug, Deserialize, Default)]
pub struct AuthorizeQuery {
    #[serde(default)]
    pub response_type: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub redirect_uri: String,
    #[serde(default)]
    pub code_challenge: String,
    #[serde(default)]
    pub code_challenge_method: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
}

fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw_cookies = headers.get_all(header::COOKIE);
    for hv in raw_cookies {
        let Ok(s) = hv.to_str() else { continue };
        for part in s.split(';') {
            let trimmed = part.trim();
            if let Some((k, v)) = trimmed.split_once('=') {
                if k.trim() == name {
                    let val = v.trim();
                    if !val.is_empty() {
                        return Some(val.to_string());
                    }
                }
            }
        }
    }
    None
}

fn build_set_cookie(name: &str, value: &str, max_age_secs: i64, is_https: bool) -> String {
    let secure_attr = if is_https { "; Secure" } else { "" };
    format!("{name}={value}; HttpOnly; SameSite=Lax; Path=/oauth; Max-Age={max_age_secs}{secure_attr}")
}

fn is_https_request(headers: &HeaderMap, state: &AppState) -> bool {
    resolve_public_base_url(headers, state).starts_with("https://")
}

fn redirect_with_oauth_error(
    redirect_uri: &str,
    error: &str,
    description: &str,
    state_param: Option<&str>,
) -> Response {
    let Ok(mut url) = Url::parse(redirect_uri) else {
        return oauth_json_error(StatusCode::BAD_REQUEST, error, description);
    };
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("error", error);
        qp.append_pair("error_description", description);
        if let Some(st) = state_param.filter(|s| !s.is_empty()) {
            qp.append_pair("state", st);
        }
    }
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, url.as_str().to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

fn redirect_with_code(redirect_uri: &str, code: &str, state_param: Option<&str>) -> Response {
    let Ok(mut url) = Url::parse(redirect_uri) else {
        return oauth_json_error(StatusCode::BAD_REQUEST, "invalid_redirect_uri", "Bad redirect_uri");
    };
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("code", code);
        if let Some(st) = state_param.filter(|s| !s.is_empty()) {
            qp.append_pair("state", st);
        }
    }
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, url.as_str().to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

pub async fn oauth_authorize_get(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuthorizeQuery>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    state.mcp_oauth_state.prune_expired(now_ms);
    let base_url = resolve_public_base_url(&headers, &state);
    let is_https = base_url.starts_with("https://");

    let client = match resolve_oauth_client(&state, &q.client_id).await {
        Ok(c) => c,
        Err(msg) => {
            return html_error_page(
                StatusCode::BAD_REQUEST,
                "无效的客户端 / Invalid OAuth Client",
                &msg,
            );
        }
    };

    let redirect_uri = q.redirect_uri.trim();
    if redirect_uri.is_empty() || !client.redirect_uris.iter().any(|u| u == redirect_uri) {
        return html_error_page(
            StatusCode::BAD_REQUEST,
            "回调地址未注册 / Unregistered Redirect URI",
            "The provided redirect_uri is not registered for this client_id.",
        );
    }

    if q.response_type.trim() != "code" {
        return redirect_with_oauth_error(
            redirect_uri,
            "unsupported_response_type",
            "Only response_type=code is supported",
            q.state.as_deref(),
        );
    }

    if q.code_challenge.trim().is_empty() || q.code_challenge_method.trim() != "S256" {
        return redirect_with_oauth_error(
            redirect_uri,
            "invalid_request",
            "PKCE with code_challenge_method=S256 is mandatory",
            q.state.as_deref(),
        );
    }

    let mut requested_scopes = match parse_scopes(q.scope.as_deref()) {
        Ok(s) => s,
        Err(reason) => {
            return redirect_with_oauth_error(
                redirect_uri,
                "invalid_scope",
                &reason,
                q.state.as_deref(),
            );
        }
    };
    if requested_scopes.is_empty() {
        requested_scopes = vec!["devices:read".to_string(), "devices:connect".to_string()];
    }

    let resource = match validate_target_resource(q.resource.as_deref(), &base_url) {
        Ok(r) => r,
        Err(reason) => {
            return redirect_with_oauth_error(
                redirect_uri,
                "invalid_target",
                &reason,
                q.state.as_deref(),
            );
        }
    };

    // Reuse existing authenticated OAuth browser cookie if valid
    let existing_user_id = extract_cookie(&headers, OAUTH_USER_COOKIE_NAME).and_then(|raw| {
        let hash = sha256_hex(&raw);
        let cookies = state.mcp_oauth_state.user_cookies.read();
        cookies
            .get(&hash)
            .and_then(|(uid, exp)| if *exp > now_ms { Some(*uid) } else { None })
    });

    let tx_id = format!("otx_{}", random_hex(16));
    let cookie_secret = random_b64url(24);
    let cookie_secret_hash = sha256_hex(&cookie_secret);
    let csrf_token = random_b64url(24);

    let qr_pair_token = if existing_user_id.is_none() {
        let pt = format!("pair_{}", random_hex(16));
        let _ = state.db.create_qr_session(&pt, now_ms + 300_000);
        Some(pt)
    } else {
        None
    };

    let tx = PendingOAuthTx {
        tx_id: tx_id.clone(),
        cookie_secret_hash,
        csrf_token,
        client_id: client.client_id.clone(),
        client_name: client.client_name.clone(),
        redirect_uri: redirect_uri.to_string(),
        resource,
        code_challenge: q.code_challenge.trim().to_string(),
        code_challenge_method: "S256".to_string(),
        requested_scopes,
        state_param: q.state.clone(),
        authenticated_user_id: existing_user_id,
        qr_pair_token,
        expires_at_ms: now_ms + OAUTH_TX_TTL_MS,
    };

    state
        .mcp_oauth_state
        .pending_txs
        .write()
        .insert(tx_id, tx.clone());

    let set_cookie = build_set_cookie(OAUTH_TX_COOKIE_NAME, &cookie_secret, OAUTH_TX_TTL_MS / 1000, is_https);
    let html = if let Some(uid) = existing_user_id {
        render_consent_html(&state, &tx, uid, None)
    } else {
        render_login_html(&tx, None)
    };

    let mut resp = (StatusCode::OK, Html(html)).into_response();
    if let Ok(hv) = HeaderValue::from_str(&set_cookie) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp.headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    resp
}

fn parse_form_or_json_map(headers: &HeaderMap, bytes: &Bytes) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.to_ascii_lowercase().contains("application/json"))
        || bytes.starts_with(b"{");

    if is_json {
        if let Ok(Value::Object(obj)) = serde_json::from_slice::<Value>(bytes) {
            for (k, v) in obj {
                match v {
                    Value::String(s) => map.entry(k).or_default().push(s),
                    Value::Bool(b) => map.entry(k).or_default().push(b.to_string()),
                    Value::Number(n) => map.entry(k).or_default().push(n.to_string()),
                    Value::Array(arr) => {
                        let entry = map.entry(k).or_default();
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                entry.push(s.to_string());
                            }
                        }
                    }
                    Value::Null => {}
                    other => map.entry(k).or_default().push(other.to_string()),
                }
            }
        }
    } else {
        for (k, v) in url::form_urlencoded::parse(bytes) {
            map.entry(k.into_owned()).or_default().push(v.into_owned());
        }
    }
    map
}

fn first_param<'a>(map: &'a HashMap<String, Vec<String>>, key: &str) -> Option<&'a str> {
    map.get(key)
        .and_then(|v| v.first())
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn verify_tx_cookie_and_csrf(
    headers: &HeaderMap,
    tx: &PendingOAuthTx,
    csrf_token: &str,
    now_ms: i64,
) -> std::result::Result<(), &'static str> {
    if now_ms > tx.expires_at_ms {
        return Err("Authorization session expired");
    }
    let Some(cookie_val) = extract_cookie(headers, OAUTH_TX_COOKIE_NAME) else {
        return Err("Missing OAuth session cookie");
    };
    let cookie_hash = sha256_hex(&cookie_val);
    if !ct_str_eq(&cookie_hash, &tx.cookie_secret_hash) {
        return Err("Invalid OAuth session cookie");
    }
    if !ct_str_eq(csrf_token.trim(), &tx.csrf_token) {
        return Err("Invalid CSRF token");
    }
    Ok(())
}

fn authenticate_user_for_oauth(
    state: &AppState,
    username: &str,
    password: &str,
    cloud_password: Option<&str>,
) -> std::result::Result<u64, String> {
    let uname = username.trim();
    if uname.is_empty() || password.is_empty() {
        return Err("用户名和密码不能为空 / Username and password are required".to_string());
    }

    if let Ok(Some(db_user)) = state.db.get_user_split_key_auth_by_identifier(uname) {
        let verified = db_user
            .password_hash
            .as_deref()
            .is_some_and(|h| verify_password(password, h));
        if verified {
            if let Ok(tfa) = state.db.get_two_factor_status(db_user.user_id) {
                if tfa.has_password {
                    let cpw = cloud_password.unwrap_or("");
                    let ok_2fa = state
                        .db
                        .get_two_factor_hash(db_user.user_id)
                        .ok()
                        .flatten()
                        .is_some_and(|h| verify_password(cpw, &h));
                    if !ok_2fa {
                        return Err("需要两步验证云密码 / 2FA cloud password required".to_string());
                    }
                }
            }
            return Ok(db_user.user_id.0);
        }
        return Err("用户名或密码错误 / Invalid username or password".to_string());
    }

    match state
        .session_manager
        .login(uname, password, "Fabushi OAuth Web")
    {
        Ok((u, sess, _tok)) => {
            state.session_manager.revoke_session(&sess.session_id);
            Ok(u.user_id.0)
        }
        Err(_) => Err("用户名或密码错误 / Invalid username or password".to_string()),
    }
}

pub async fn oauth_authorize_login(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    let params = parse_form_or_json_map(&headers, &body);
    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("application/json"))
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("application/json"));

    let tx_id = first_param(&params, "tx_id").unwrap_or("");
    let csrf = first_param(&params, "csrf_token").unwrap_or("");

    let tx_snap = {
        state.mcp_oauth_state.pending_txs.read().get(tx_id).cloned()
    };
    let Some(mut tx) = tx_snap else {
        return if wants_json {
            oauth_json_error(StatusCode::BAD_REQUEST, "invalid_request", "Unknown tx_id")
        } else {
            html_error_page(StatusCode::BAD_REQUEST, "会话无效 / Invalid Session", "Unknown or expired authorization transaction.")
        };
    };

    if let Err(reason) = verify_tx_cookie_and_csrf(&headers, &tx, csrf, now_ms) {
        return if wants_json {
            oauth_json_error(StatusCode::FORBIDDEN, "access_denied", reason)
        } else {
            html_error_page(StatusCode::FORBIDDEN, "安全校验失败 / Security Check Failed", reason)
        };
    }

    let user_id_res = if let Some(pair_tok) = first_param(&params, "pair_token") {
        match state.db.get_qr_session(pair_tok) {
            Ok(Some(qr)) if qr.status == "confirmed" => qr
                .session
                .map(|s| s.user_id)
                .ok_or_else(|| "QR pairing confirmed without session".to_string()),
            Ok(Some(qr)) if qr.status == "expired" => {
                Err("二维码已过期，请刷新页面 / QR code expired".to_string())
            }
            Ok(Some(_)) => Err("请先在 Fabushi 客户端扫描并确认登录 / Waiting for QR confirmation".to_string()),
            _ => Err("无效的配对二维码 / Invalid QR pairing token".to_string()),
        }
    } else {
        let username = first_param(&params, "username").unwrap_or("");
        let password = params
            .get("password")
            .and_then(|v| v.first())
            .map(String::as_str)
            .unwrap_or("");
        let cloud_pw = first_param(&params, "cloud_password");
        authenticate_user_for_oauth(&state, username, password, cloud_pw)
    };

    let user_id = match user_id_res {
        Ok(uid) => uid,
        Err(err_msg) => {
            if wants_json {
                return oauth_json_error(StatusCode::UNAUTHORIZED, "access_denied", &err_msg);
            }
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CACHE_CONTROL, "no-store")],
                Html(render_login_html(&tx, Some(&err_msg))),
            )
                .into_response();
        }
    };

    tx.authenticated_user_id = Some(user_id);
    // Rotate CSRF token for the consent step
    tx.csrf_token = random_b64url(24);
    state
        .mcp_oauth_state
        .pending_txs
        .write()
        .insert(tx.tx_id.clone(), tx.clone());

    let user_cookie = random_b64url(24);
    state
        .mcp_oauth_state
        .user_cookies
        .write()
        .insert(sha256_hex(&user_cookie), (user_id, now_ms + OAUTH_USER_COOKIE_TTL_MS));
    let is_https = is_https_request(&headers, &state);
    let set_user_cookie = build_set_cookie(
        OAUTH_USER_COOKIE_NAME,
        &user_cookie,
        OAUTH_USER_COOKIE_TTL_MS / 1000,
        is_https,
    );

    if wants_json {
        let nodes = state
            .db
            .list_computer_nodes(user_id, None, now_ms)
            .unwrap_or_default();
        let mut resp = Json(json!({
            "ok": true,
            "user_id": user_id,
            "tx_id": tx.tx_id,
            "csrf_token": tx.csrf_token,
            "client_name": tx.client_name,
            "requested_scopes": tx.requested_scopes,
            "devices": nodes,
        }))
        .into_response();
        if let Ok(hv) = HeaderValue::from_str(&set_user_cookie) {
            resp.headers_mut().append(header::SET_COOKIE, hv);
        }
        return resp;
    }

    let mut resp = (StatusCode::OK, Html(render_consent_html(&state, &tx, user_id, None))).into_response();
    if let Ok(hv) = HeaderValue::from_str(&set_user_cookie) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp.headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    resp
}

#[derive(Debug, Deserialize)]
pub struct QrPollQuery {
    pub tx_id: String,
    pub pair_token: String,
}

pub async fn oauth_authorize_qr_poll(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Query(q): Query<QrPollQuery>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    let tx_opt = state
        .mcp_oauth_state
        .pending_txs
        .read()
        .get(q.tx_id.trim())
        .cloned();
    let Some(mut tx) = tx_opt else {
        return oauth_json_error(StatusCode::NOT_FOUND, "invalid_request", "Unknown tx_id");
    };
    if now_ms > tx.expires_at_ms {
        return oauth_json_error(StatusCode::BAD_REQUEST, "expired", "Authorization session expired");
    }
    let Some(cookie_val) = extract_cookie(&headers, OAUTH_TX_COOKIE_NAME) else {
        return oauth_json_error(StatusCode::FORBIDDEN, "forbidden", "Missing OAuth session cookie");
    };
    if !ct_str_eq(&sha256_hex(&cookie_val), &tx.cookie_secret_hash) {
        return oauth_json_error(StatusCode::FORBIDDEN, "forbidden", "Invalid OAuth session cookie");
    }
    if tx.qr_pair_token.as_deref() != Some(q.pair_token.trim()) {
        return oauth_json_error(StatusCode::FORBIDDEN, "forbidden", "pair_token mismatch");
    }

    match state.db.get_qr_session(q.pair_token.trim()) {
        Ok(Some(qr)) if qr.status == "confirmed" => {
            let Some(sess) = qr.session else {
                return Json(json!({ "status": "pending" })).into_response();
            };
            tx.authenticated_user_id = Some(sess.user_id);
            state
                .mcp_oauth_state
                .pending_txs
                .write()
                .insert(tx.tx_id.clone(), tx);

            let user_cookie = random_b64url(24);
            state.mcp_oauth_state.user_cookies.write().insert(
                sha256_hex(&user_cookie),
                (sess.user_id, now_ms + OAUTH_USER_COOKIE_TTL_MS),
            );
            let set_user_cookie = build_set_cookie(
                OAUTH_USER_COOKIE_NAME,
                &user_cookie,
                OAUTH_USER_COOKIE_TTL_MS / 1000,
                is_https_request(&headers, &state),
            );
            let mut resp = Json(json!({
                "status": "confirmed",
                "user_id": sess.user_id,
            }))
            .into_response();
            if let Ok(hv) = HeaderValue::from_str(&set_user_cookie) {
                resp.headers_mut().append(header::SET_COOKIE, hv);
            }
            resp
        }
        Ok(Some(qr)) => Json(json!({ "status": qr.status })).into_response(),
        Ok(None) => oauth_json_error(StatusCode::NOT_FOUND, "not_found", "QR pair session not found"),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", &e.to_string()),
    }
}

pub async fn oauth_authorize_approve(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    let params = parse_form_or_json_map(&headers, &body);
    let tx_id = first_param(&params, "tx_id").unwrap_or("");
    let csrf = first_param(&params, "csrf_token").unwrap_or("");

    let tx_opt = state
        .mcp_oauth_state
        .pending_txs
        .read()
        .get(tx_id)
        .cloned();
    let Some(tx) = tx_opt else {
        return html_error_page(
            StatusCode::BAD_REQUEST,
            "授权请求已失效 / Authorization Request Expired",
            "Unknown or already completed OAuth transaction.",
        );
    };

    if let Err(reason) = verify_tx_cookie_and_csrf(&headers, &tx, csrf, now_ms) {
        return html_error_page(
            StatusCode::FORBIDDEN,
            "安全校验失败 / Security Check Failed",
            reason,
        );
    }

    let Some(user_id) = tx.authenticated_user_id else {
        return (
            StatusCode::UNAUTHORIZED,
            Html(render_login_html(&tx, Some("请先登录 / Please sign in first"))),
        )
            .into_response();
    };

    // Consume pending transaction so approval is single-use
    state.mcp_oauth_state.pending_txs.write().remove(&tx.tx_id);

    let decision = first_param(&params, "decision").unwrap_or("approve");
    if decision.eq_ignore_ascii_case("deny") || decision.eq_ignore_ascii_case("cancel") {
        return redirect_with_oauth_error(
            &tx.redirect_uri,
            "access_denied",
            "User denied the authorization request",
            tx.state_param.as_deref(),
        );
    }

    // Collect granted scopes
    let mut granted_scopes = Vec::new();
    let mut seen_scopes = BTreeSet::new();
    if let Some(raw_scopes) = params.get("scopes").or_else(|| params.get("scope")) {
        for raw in raw_scopes {
            for part in raw.split(|c: char| c.is_whitespace() || c == ',') {
                let s = part.trim();
                if !s.is_empty() && is_supported_scope(s) && seen_scopes.insert(s.to_string()) {
                    granted_scopes.push(s.to_string());
                }
            }
        }
    }
    if granted_scopes.is_empty() {
        granted_scopes = tx.requested_scopes.clone();
    }

    // Collect allowed_node_ids (all vs selected)
    let device_scope = first_param(&params, "device_scope").unwrap_or("");
    let submitted_nodes: Vec<String> = params
        .get("allowed_node_ids")
        .map(|vals| {
            vals.iter()
                .flat_map(|v| v.split(','))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();

    let allowed_node_ids = if device_scope.eq_ignore_ascii_case("all")
        || submitted_nodes.iter().any(|n| n == "*")
        || (device_scope.is_empty() && submitted_nodes.is_empty())
    {
        vec!["*".to_string()]
    } else {
        let mut owned = Vec::new();
        let mut seen = BTreeSet::new();
        for nid in submitted_nodes {
            let exists = state
                .db
                .get_computer_node(user_id, &nid, None, now_ms)
                .ok()
                .flatten()
                .is_some();
            if exists && seen.insert(nid.clone()) {
                owned.push(nid);
            }
        }
        if owned.is_empty() {
            owned.push("__none__".to_string());
        }
        owned
    };

    let raw_code = format!("fbmcp_code_{}", random_b64url(24));
    let code_rec = McpOAuthCodeRecord {
        code_hash: sha256_hex(&raw_code),
        client_id: tx.client_id.clone(),
        user_id,
        redirect_uri: tx.redirect_uri.clone(),
        resource: tx.resource.clone(),
        code_challenge: tx.code_challenge.clone(),
        code_challenge_method: tx.code_challenge_method.clone(),
        scopes: granted_scopes,
        allowed_node_ids,
        issued_family_id: None,
        expires_at_ms: now_ms + MCP_AUTH_CODE_TTL_MS,
        consumed_at_ms: None,
    };

    if let Err(e) = state.db.create_mcp_oauth_code(&code_rec) {
        return redirect_with_oauth_error(
            &tx.redirect_uri,
            "server_error",
            &format!("Failed to persist authorization code: {e}"),
            tx.state_param.as_deref(),
        );
    }

    redirect_with_code(&tx.redirect_uri, &raw_code, tx.state_param.as_deref())
}

// ============================================================================
// Token Exchange, Refresh Rotation & Revocation (RFC 6749, 7009, 7636, 8707)
// ============================================================================

pub async fn oauth_token(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    let base_url = resolve_public_base_url(&headers, &state);
    let params = parse_form_or_json_map(&headers, &body);

    let grant_type = first_param(&params, "grant_type").unwrap_or("");
    let client_id = first_param(&params, "client_id").unwrap_or("");
    let client_secret = first_param(&params, "client_secret");

    match grant_type {
        "authorization_code" => {
            if client_id.is_empty() {
                return oauth_json_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "client_id is required",
                );
            }
            let client = match resolve_oauth_client(&state, client_id).await {
                Ok(c) => c,
                Err(msg) => {
                    return oauth_json_error(StatusCode::UNAUTHORIZED, "invalid_client", &msg);
                }
            };
            if client.token_endpoint_auth_method == "client_secret_post" {
                let expected = client.client_secret_hash.as_deref().unwrap_or("");
                let provided_hash = client_secret.map(sha256_hex).unwrap_or_default();
                if expected.is_empty() || !ct_str_eq(&provided_hash, expected) {
                    return oauth_json_error(
                        StatusCode::UNAUTHORIZED,
                        "invalid_client",
                        "Invalid client_secret",
                    );
                }
            }

            let code = match first_param(&params, "code") {
                Some(c) => c,
                None => {
                    return oauth_json_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request",
                        "code is required",
                    );
                }
            };
            let redirect_uri = match first_param(&params, "redirect_uri") {
                Some(u) => u,
                None => {
                    return oauth_json_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request",
                        "redirect_uri is required",
                    );
                }
            };
            let code_verifier = match first_param(&params, "code_verifier") {
                Some(v) => v,
                None => {
                    return oauth_json_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request",
                        "code_verifier (PKCE S256) is mandatory",
                    );
                }
            };

            let req_resource = match first_param(&params, "resource") {
                Some(r) => match validate_target_resource(Some(r), &base_url) {
                    Ok(norm) => Some(norm),
                    Err(msg) => {
                        return oauth_json_error(StatusCode::BAD_REQUEST, "invalid_target", &msg);
                    }
                },
                None => None,
            };

            match state.db.exchange_mcp_oauth_code(
                code,
                &client.client_id,
                redirect_uri,
                code_verifier,
                req_resource.as_deref(),
                now_ms,
            ) {
                Ok(McpCodeExchangeOutcome::Issued(pair)) => oauth_token_success_response(&pair),
                Ok(McpCodeExchangeOutcome::InvalidGrant(msg)) => {
                    oauth_json_error(StatusCode::BAD_REQUEST, "invalid_grant", &msg)
                }
                Ok(McpCodeExchangeOutcome::InvalidClient(msg)) => {
                    oauth_json_error(StatusCode::UNAUTHORIZED, "invalid_client", &msg)
                }
                Ok(McpCodeExchangeOutcome::InvalidTarget(msg)) => {
                    oauth_json_error(StatusCode::BAD_REQUEST, "invalid_target", &msg)
                }
                Err(e) => oauth_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    &e.to_string(),
                ),
            }
        }
        "refresh_token" => {
            let refresh_token = match first_param(&params, "refresh_token") {
                Some(rt) => rt,
                None => {
                    return oauth_json_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request",
                        "refresh_token is required",
                    );
                }
            };
            if !client_id.is_empty() {
                if let Ok(client) = resolve_oauth_client(&state, client_id).await {
                    if client.token_endpoint_auth_method == "client_secret_post" {
                        let expected = client.client_secret_hash.as_deref().unwrap_or("");
                        let provided_hash = client_secret.map(sha256_hex).unwrap_or_default();
                        if expected.is_empty() || !ct_str_eq(&provided_hash, expected) {
                            return oauth_json_error(
                                StatusCode::UNAUTHORIZED,
                                "invalid_client",
                                "Invalid client_secret",
                            );
                        }
                    }
                }
            }

            let req_scopes = match first_param(&params, "scope") {
                Some(s) => match parse_scopes(Some(s)) {
                    Ok(list) => Some(list),
                    Err(msg) => {
                        return oauth_json_error(StatusCode::BAD_REQUEST, "invalid_scope", &msg);
                    }
                },
                None => None,
            };
            let req_resource = match first_param(&params, "resource") {
                Some(r) => match validate_target_resource(Some(r), &base_url) {
                    Ok(norm) => Some(norm),
                    Err(msg) => {
                        return oauth_json_error(StatusCode::BAD_REQUEST, "invalid_target", &msg);
                    }
                },
                None => None,
            };

            match state.db.rotate_mcp_refresh_token(
                refresh_token,
                if client_id.is_empty() { None } else { Some(client_id) },
                req_scopes.as_deref(),
                req_resource.as_deref(),
                now_ms,
            ) {
                Ok(McpRefreshOutcome::Rotated(pair)) => oauth_token_success_response(&pair),
                Ok(McpRefreshOutcome::InvalidGrant(msg)) => {
                    oauth_json_error(StatusCode::BAD_REQUEST, "invalid_grant", &msg)
                }
                Ok(McpRefreshOutcome::InvalidClient(msg)) => {
                    oauth_json_error(StatusCode::UNAUTHORIZED, "invalid_client", &msg)
                }
                Ok(McpRefreshOutcome::InvalidScope(msg)) => {
                    oauth_json_error(StatusCode::BAD_REQUEST, "invalid_scope", &msg)
                }
                Ok(McpRefreshOutcome::InvalidTarget(msg)) => {
                    oauth_json_error(StatusCode::BAD_REQUEST, "invalid_target", &msg)
                }
                Err(e) => oauth_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    &e.to_string(),
                ),
            }
        }
        _ => oauth_json_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "Only authorization_code and refresh_token grant types are supported",
        ),
    }
}

fn oauth_token_success_response(pair: &crate::db::mcp_oauth_store::McpIssuedTokenPair) -> Response {
    let scope_str = pair.scopes.join(" ");
    (
        StatusCode::OK,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        Json(json!({
            "access_token": pair.access_token,
            "token_type": "Bearer",
            "expires_in": pair.expires_in_secs,
            "refresh_token": pair.refresh_token,
            "scope": scope_str,
        })),
    )
        .into_response()
}

pub async fn oauth_revoke(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let params = parse_form_or_json_map(&headers, &body);
    let now_ms = Utc::now().timestamp_millis();
    if let Some(tok) = first_param(&params, "token") {
        let _ = state.db.revoke_mcp_token(tok, now_ms);
    }
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "revoked": true })),
    )
        .into_response()
}

// ============================================================================
// Authenticated MCP Grant & Audit Management API
// ============================================================================

pub async fn mcp_grants_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state.db.list_mcp_oauth_grants(auth.user_id.0, now_ms) {
        Ok(list) => {
            let grants: Vec<McpOAuthGrantSummaryDto> = list
                .into_iter()
                .map(|g| McpOAuthGrantSummaryDto {
                    family_id: g.family_id,
                    client_id: g.client_id,
                    client_name: g.client_name,
                    scopes: g.scopes,
                    allowed_node_ids: g.allowed_node_ids,
                    created_at_ms: g.created_at_ms,
                    last_used_ms: g.last_used_ms,
                    expires_at_ms: g.expires_at_ms,
                })
                .collect();
            (
                StatusCode::OK,
                Json(McpOAuthGrantsResponse { grants }),
            )
                .into_response()
        }
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", &e.to_string()),
    }
}

pub async fn mcp_grant_revoke(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(family_id): Path<String>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .revoke_mcp_grant_family(auth.user_id.0, &family_id, now_ms)
    {
        Ok(true) => {
            state
                .remote_sessions
                .disconnect_by_family_id(&state, auth.user_id.0, &family_id, "oauth_grant_revoked");
            (
                StatusCode::OK,
                Json(json!({ "revoked": true, "family_id": family_id })),
            )
                .into_response()
        }
        Ok(false) => err_resp(StatusCode::NOT_FOUND, "not_found", "OAuth grant not found"),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", &e.to_string()),
    }
}

pub async fn mcp_audit_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let limit = q
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    match state.db.list_mcp_tool_audit(auth.user_id.0, limit) {
        Ok(entries) => (
            StatusCode::OK,
            Json(json!({ "entries": entries, "total": entries.len() })),
        )
            .into_response(),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", &e.to_string()),
    }
}

// ============================================================================
// Bilingual (ZH/EN) HTML Templates
// ============================================================================

fn oauth_json_error(status: StatusCode, error: &str, description: &str) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        Json(json!({
            "error": error,
            "error_description": description,
        })),
    )
        .into_response()
}

fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

fn scope_label(scope: &str) -> (&'static str, &'static str) {
    match scope {
        "devices:read" => ("查看设备列表与状态", "View device inventory and status"),
        "devices:connect" => ("连接控制设备与发起接管请求", "Connect devices and request human takeover"),
        "screen:view" => ("查看屏幕截图与无障碍元素树", "Capture screen and read accessibility tree"),
        "input:control" => ("模拟鼠标、键盘与语义界面操作", "Control mouse, keyboard, and semantic UI"),
        "terminal:exec" => ("打开与执行远程终端命令", "Open and execute remote terminal sessions"),
        "files:read" => ("读取远程目录、文件与剪贴板", "Read remote directories, files, and clipboard"),
        "files:write" => ("写入远程文件与设置剪贴板", "Write remote files and update clipboard"),
        "assist:manage" => ("管理远程协助识别码与开关", "Manage Remote Assist ID, code, and toggle"),
        "power:control" => ("锁屏、睡眠、重启、关机与远程唤醒", "Lock, sleep, reboot, shutdown, and Wake-on-LAN"),
        "messaging:read" => ("读取聊天记录与搜索消息", "Read chat history and search messages"),
        "messaging:write" => ("发送聊天消息", "Send messages to authorized chats"),
        "chats:read" => ("查看会话与联系人列表", "List chat dialogs"),
        "miniapp:control" => ("查看与管理 Bot 及小程序", "Inspect bots and mini apps"),
        "tasks:manage" => ("查看与创建后台任务", "List and create background tasks"),
        _ => ("访问受保护资源", "Access protected resource"),
    }
}

const BASE_CSS: &str = r#"
<style>
  :root { color-scheme: dark light; --bg:#0d1117; --card:#161b22; --border:#30363d; --fg:#e6edf3; --muted:#8b949e; --accent:#1e6bff; --danger:#f85149; --ok:#2ea043; }
  body { margin:0; font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"PingFang SC","Microsoft YaHei",sans-serif; background:var(--bg); color:var(--fg); display:flex; min-height:100vh; align-items:center; justify-content:center; padding:20px; }
  .card { width:100%; max-width:520px; background:var(--card); border:1px solid var(--border); border-radius:14px; padding:26px; box-shadow:0 16px 40px rgba(0,0,0,.35); }
  .badge { display:inline-block; background:rgba(30,107,255,.16); color:#79b8ff; border:1px solid rgba(30,107,255,.35); padding:4px 10px; border-radius:999px; font-size:12px; font-weight:600; }
  h1 { font-size:20px; margin:12px 0 6px; }
  .sub { color:var(--muted); font-size:13px; margin:0 0 18px; line-height:1.5; }
  .err { background:rgba(248,81,73,.14); border:1px solid var(--danger); color:#ffa198; border-radius:8px; padding:10px 12px; font-size:13px; margin-bottom:14px; }
  label { display:block; font-size:13px; font-weight:600; margin:10px 0 6px; }
  input[type=text], input[type=password] { width:100%; box-sizing:border-box; padding:10px 12px; border-radius:8px; border:1px solid var(--border); background:#0d1117; color:var(--fg); font-size:14px; }
  .btn { width:100%; margin-top:16px; padding:11px 16px; border:none; border-radius:8px; background:var(--accent); color:#fff; font-size:14px; font-weight:600; cursor:pointer; }
  .btn-row { display:flex; gap:10px; margin-top:18px; }
  .btn-sec { flex:1; padding:11px 16px; border:1px solid var(--border); border-radius:8px; background:transparent; color:var(--fg); font-size:14px; font-weight:600; cursor:pointer; }
  .btn-pri { flex:2; padding:11px 16px; border:none; border-radius:8px; background:var(--accent); color:#fff; font-size:14px; font-weight:600; cursor:pointer; }
  .box { border:1px solid var(--border); border-radius:10px; padding:12px; margin-top:14px; background:rgba(255,255,255,.02); }
  .chk { display:flex; align-items:flex-start; gap:10px; padding:7px 0; border-bottom:1px solid rgba(255,255,255,.06); font-size:13px; }
  .chk:last-child { border-bottom:none; }
  .mono { font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:12px; color:#79b8ff; word-break:break-all; }
</style>
"#;

fn render_login_html(tx: &PendingOAuthTx, error_msg: Option<&str>) -> String {
    let client_name = html_escape(&tx.client_name);
    let client_id = html_escape(&tx.client_id);
    let tx_id = html_escape(&tx.tx_id);
    let csrf = html_escape(&tx.csrf_token);
    let err_block = error_msg
        .map(|m| format!(r#"<div class="err">{}</div>"#, html_escape(m)))
        .unwrap_or_default();

    let qr_block = if let Some(ref pt) = tx.qr_pair_token {
        let pt_esc = html_escape(pt);
        let qr_uri = html_escape(&format!("tg://login?token={pt}"));
        format!(
            r#"
            <div class="box">
              <div style="font-size:13px;font-weight:600;">扫码登录 / QR Pairing Login</div>
              <div class="sub" style="margin:4px 0 8px;">在已登录的 Fabushi 桌面/手机端确认此配对码：<br/>Confirm this pairing token in an authenticated Fabushi app:</div>
              <div class="mono">{qr_uri}</div>
              <form method="post" action="/oauth/authorize/login" style="margin-top:10px;">
                <input type="hidden" name="tx_id" value="{tx_id}" />
                <input type="hidden" name="csrf_token" value="{csrf}" />
                <input type="hidden" name="pair_token" value="{pt_esc}" />
                <button type="submit" class="btn-sec" style="width:100%;">我已在 Fabushi 确认 / Continue with QR</button>
              </form>
            </div>
            <script>
            (function(){{
              const t=setInterval(async()=>{{
                try{{
                  const r=await fetch('/oauth/authorize/qr-poll?tx_id={tx_id}&pair_token={pt_esc}');
                  if(!r.ok)return;
                  const j=await r.json();
                  if(j.status==='confirmed'){{ clearInterval(t); window.location.reload(); }}
                }}catch(_){{}}
              }}, 2000);
            }})();
            </script>
            "#
        )
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Fabushi OAuth 登录 / Sign In</title>{BASE_CSS}</head>
<body>
  <div class="card">
    <span class="badge">Fabushi Official MCP · OAuth 2.1</span>
    <h1>登录以授权 {client_name}</h1>
    <p class="sub">Sign in to your Fabushi account to authorize <strong>{client_name}</strong> (<span class="mono">{client_id}</span>).</p>
    {err_block}
    <form method="post" action="/oauth/authorize/login">
      <input type="hidden" name="tx_id" value="{tx_id}" />
      <input type="hidden" name="csrf_token" value="{csrf}" />
      <label>用户名或邮箱 / Username or Email</label>
      <input type="text" name="username" required autocomplete="username" />
      <label>密码 / Password</label>
      <input type="password" name="password" required autocomplete="current-password" />
      <label>两步验证密码（如已开启）/ 2FA Password (Optional)</label>
      <input type="password" name="cloud_password" autocomplete="off" />
      <button type="submit" class="btn">登录并继续 / Sign In &amp; Continue</button>
    </form>
    {qr_block}
  </div>
</body></html>"#
    )
}

fn render_consent_html(
    state: &AppState,
    tx: &PendingOAuthTx,
    user_id: u64,
    error_msg: Option<&str>,
) -> String {
    let now_ms = Utc::now().timestamp_millis();
    let user_label = state
        .db
        .get_user(UserId(user_id))
        .ok()
        .flatten()
        .and_then(|u| u.username.or(Some(u.first_name)))
        .unwrap_or_else(|| format!("User {user_id}"));
    let nodes = state
        .db
        .list_computer_nodes(user_id, None, now_ms)
        .unwrap_or_default();

    let client_name = html_escape(&tx.client_name);
    let redirect_uri = html_escape(&tx.redirect_uri);
    let resource = html_escape(&tx.resource);
    let tx_id = html_escape(&tx.tx_id);
    let csrf = html_escape(&tx.csrf_token);
    let err_block = error_msg
        .map(|m| format!(r#"<div class="err">{}</div>"#, html_escape(m)))
        .unwrap_or_default();

    let mut scope_items = String::new();
    for s in &tx.requested_scopes {
        let s_esc = html_escape(s);
        let (zh, en) = scope_label(s);
        scope_items.push_str(&format!(
            r#"<label class="chk" style="margin:0;font-weight:400;"><input type="checkbox" name="scopes" value="{s_esc}" checked /><div><div><strong>{s_esc}</strong> · {zh}</div><div style="color:#8b949e;font-size:12px;">{en}</div></div></label>"#
        ));
    }

    let mut device_items = String::new();
    for n in &nodes {
        let nid = html_escape(&n.node_id);
        let dname = html_escape(&n.display_name);
        let status_dot = if n.online { "🟢 在线 Online" } else { "⚪ 离线 Offline" };
        device_items.push_str(&format!(
            r#"<label class="chk" style="margin:0;font-weight:400;"><input type="checkbox" name="allowed_node_ids" value="{nid}" /><div><div>{dname} <span style="font-size:12px;color:#8b949e;">({status_dot})</span></div><div class="mono">{nid}</div></div></label>"#
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>授权 AI 客户端 / Authorize MCP Client</title>{BASE_CSS}</head>
<body>
  <div class="card">
    <span class="badge">Fabushi Official MCP · 权限确认 / Consent</span>
    <h1>授权 {client_name} 访问您的账号？</h1>
    <p class="sub">当前账号 Current User: <strong>{}</strong><br/>目标资源 Resource: <span class="mono">{resource}</span><br/>回调地址 Redirect: <span class="mono">{redirect_uri}</span></p>
    {err_block}
    <form method="post" action="/oauth/authorize/approve">
      <input type="hidden" name="tx_id" value="{tx_id}" />
      <input type="hidden" name="csrf_token" value="{csrf}" />
      <div class="box">
        <div style="font-size:13px;font-weight:600;margin-bottom:6px;">申请的权限范围 / Requested Permissions</div>
        {scope_items}
      </div>
      <div class="box">
        <div style="font-size:13px;font-weight:600;margin-bottom:6px;">允许控制的设备 / Allowed Devices</div>
        <label class="chk" style="margin:0;font-weight:400;"><input type="radio" name="device_scope" value="all" checked /><div><strong>所有当前及新增设备 / All Devices (*)</strong></div></label>
        <label class="chk" style="margin:0;font-weight:400;"><input type="radio" name="device_scope" value="selected" /><div><strong>仅限勾选设备 / Selected Devices Only</strong></div></label>
        {device_items}
      </div>
      <div class="btn-row">
        <button type="submit" name="decision" value="deny" class="btn-sec">拒绝 / Deny</button>
        <button type="submit" name="decision" value="approve" class="btn-pri">同意授权 / Authorize</button>
      </div>
    </form>
  </div>
</body></html>"#,
        html_escape(&user_label)
    )
}

fn html_error_page(status: StatusCode, title: &str, message: &str) -> Response {
    let html = format!(
        r#"<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"/><title>{}</title>{BASE_CSS}</head>
<body><div class="card"><span class="badge">Fabushi OAuth Error</span><h1>{}</h1><div class="err">{}</div></div></body></html>"#,
        html_escape(title),
        html_escape(title),
        html_escape(message)
    );
    (status, [(header::CACHE_CONTROL, "no-store")], Html(html)).into_response()
}
