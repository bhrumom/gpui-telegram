use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use gpui_telegram_proto::computer_contracts::{
    ComputerNodeDto, ComputerRunPolicy, RegisterLocalComputerRequest, RemoteComputerCommandResult,
};
use gpui_telegram_proto::remote_control_contracts::PowerActionKind;
use gpui_telegram_server::{
    build_router_with_state, config::ServerConfig, db::ServerDatabase, server::AppState,
};
use gpui_telegram_webrtc::signaling::SignalingFrame;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tower::ServiceExt;
use url::Url;

const ONE_PIXEL_PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO7+X8sAAAAASUVORK5CYII=";

struct TestServerHarness {
    _temp_dir: TempDir,
    state: Arc<AppState>,
    router: Router,
}

impl TestServerHarness {
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("create temp dir");
        let db_path = temp_dir.path().join("test_server.db");
        let media_dir = temp_dir.path().join("media_cas");
        std::fs::create_dir_all(&media_dir).expect("create media dir");

        let mut cfg = ServerConfig::default();
        cfg.database_url = format!("sqlite://{}", db_path.display());
        cfg.media_storage_path = media_dir.to_string_lossy().to_string();
        cfg.host = "127.0.0.1".to_string();
        cfg.port = 8443;

        let state = Arc::new(AppState::try_new(cfg).expect("boot AppState"));
        let router = build_router_with_state(Arc::clone(&state));
        Self {
            _temp_dir: temp_dir,
            state,
            router,
        }
    }

    async fn req(&self, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let resp = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router oneshot");
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = to_bytes(resp.into_body(), 4 * 1024 * 1024)
            .await
            .expect("read response body");
        (status, headers, bytes.to_vec())
    }

    async fn json_req(
        &self,
        method: Method,
        uri: &str,
        bearer: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, axum::http::HeaderMap, Value) {
        self.json_req_headers(method, uri, bearer, &[], body).await
    }

    async fn json_req_headers(
        &self,
        method: Method,
        uri: &str,
        bearer: Option<&str>,
        extra_headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (StatusCode, axum::http::HeaderMap, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(tok) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {tok}"));
        }
        for (k, v) in extra_headers {
            builder = builder.header(*k, *v);
        }
        let body_bytes = if let Some(val) = body {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(val.to_string())
        } else {
            Body::empty()
        };
        let req = builder.body(body_bytes).expect("build json request");
        let (status, headers, raw) = self.req(req).await;
        let parsed = serde_json::from_slice(&raw).unwrap_or(Value::Null);
        (status, headers, parsed)
    }

    fn register_user(&self, username: &str, password: &str) -> (u64, String) {
        let (user, _sess, raw_tok) = self
            .state
            .session_manager
            .register(username, password, username, None, "IntegrationTest")
            .expect("register test user");
        (user.user_id.0, raw_tok)
    }

    async fn register_local_node(
        &self,
        user_token: &str,
        device_id: &str,
        display_name: &str,
        policy: ComputerRunPolicy,
    ) -> ComputerNodeDto {
        let (st, _, node_val) = self
            .json_req(
                Method::POST,
                "/api/v1/computers/local/register",
                Some(user_token),
                Some(serde_json::to_value(RegisterLocalComputerRequest {
                    device_id: device_id.to_string(),
                    display_name: display_name.to_string(),
                    ip_address: "100.64.0.10".to_string(),
                    os_name: "macOS 15".to_string(),
                    resolution: "2560x1440".to_string(),
                }).unwrap()),
            )
            .await;
        assert!(st.is_success(), "register_local_node failed: {st} {node_val}");
        let node: ComputerNodeDto = serde_json::from_value(node_val).expect("parse ComputerNodeDto");

        let (pst, _, pval) = self
            .json_req(
                Method::PATCH,
                &format!("/api/v1/computers/{}/policy", urlencoding_component(&node.node_id)),
                Some(user_token),
                Some(json!({ "run_policy": policy })),
            )
            .await;
        assert!(pst.is_success(), "update_policy failed: {pst} {pval}");
        serde_json::from_value(pval).expect("parse updated ComputerNodeDto")
    }
}

fn urlencoding_component(raw: &str) -> String {
    url::form_urlencoded::byte_serialize(raw.as_bytes()).collect()
}

fn extract_hidden_input(html: &str, name: &str) -> Option<String> {
    let pattern = format!(r#"name="{name}" value=""#);
    let idx = html.find(&pattern)?;
    let rest = &html[idx + pattern.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn extract_cookie_pair(headers: &axum::http::HeaderMap, cookie_name: &str) -> Option<String> {
    for val in headers.get_all(header::SET_COOKIE) {
        if let Ok(s) = val.to_str() {
            if let Some(rest) = s.strip_prefix(&format!("{cookie_name}=")) {
                let value = rest.split(';').next().unwrap_or("").trim();
                return Some(format!("{cookie_name}={value}"));
            }
        }
    }
    None
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

async fn run_full_oauth_pkce_flow(
    h: &TestServerHarness,
    username: &str,
    password: &str,
    scopes: &str,
    device_scope: &str,
    allowed_node_ids: &[&str],
) -> (String, String, String, String, String) {
    let redirect_uri = "http://127.0.0.1:43210/oauth/callback";
    let (reg_st, _, reg_val) = h
        .json_req(
            Method::POST,
            "/oauth/register",
            None,
            Some(json!({
                "client_name": "Claude Code Test",
                "redirect_uris": [redirect_uri],
                "token_endpoint_auth_method": "none"
            })),
        )
        .await;
    assert_eq!(reg_st, StatusCode::CREATED, "register failed: {reg_val}");
    let client_id = reg_val["client_id"].as_str().unwrap().to_string();

    let verifier = "test_pkce_verifier_string_with_sufficient_length_0123456789_abcdef";
    let challenge = pkce_challenge(verifier);
    let state_param = "state_xyz_99";
    let resource = "http://127.0.0.1:8443/mcp";

    let auth_url = format!(
        "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&scope={}&state={}&resource={}",
        urlencoding_component(&client_id),
        urlencoding_component(redirect_uri),
        urlencoding_component(&challenge),
        urlencoding_component(scopes),
        urlencoding_component(state_param),
        urlencoding_component(resource),
    );
    let (auth_st, auth_hdrs, auth_bytes) = h
        .req(Request::builder().method(Method::GET).uri(&auth_url).body(Body::empty()).unwrap())
        .await;
    assert_eq!(auth_st, StatusCode::OK);
    let html_login = String::from_utf8_lossy(&auth_bytes);
    let oauth_cookie = extract_cookie_pair(&auth_hdrs, "fbmcp_oauth_sess").expect("fbmcp_oauth_sess");
    let tx_id = extract_hidden_input(&html_login, "tx_id").expect("tx_id");
    let csrf_login = extract_hidden_input(&html_login, "csrf_token").expect("csrf_token");

    let login_form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("tx_id", &tx_id)
        .append_pair("csrf_token", &csrf_login)
        .append_pair("username", username)
        .append_pair("password", password)
        .finish();
    let (login_st, _login_hdrs, login_bytes) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/authorize/login")
                .header(header::COOKIE, &oauth_cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(login_form))
                .unwrap(),
        )
        .await;
    assert_eq!(login_st, StatusCode::OK);
    let html_consent = String::from_utf8_lossy(&login_bytes);
    let csrf_consent = extract_hidden_input(&html_consent, "csrf_token").expect("consent csrf");

    let mut form_ser = url::form_urlencoded::Serializer::new(String::new());
    form_ser.append_pair("tx_id", &tx_id);
    form_ser.append_pair("csrf_token", &csrf_consent);
    form_ser.append_pair("decision", "approve");
    form_ser.append_pair("device_scope", device_scope);
    for sc in scopes.split_whitespace() {
        form_ser.append_pair("scopes", sc);
    }
    for nid in allowed_node_ids {
        form_ser.append_pair("allowed_node_ids", nid);
    }
    let approve_body = form_ser.finish();

    let (app_st, app_hdrs, _) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/authorize/approve")
                .header(header::COOKIE, &oauth_cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(approve_body))
                .unwrap(),
        )
        .await;
    assert_eq!(app_st, StatusCode::FOUND, "expected 302 redirect from approve");
    let loc = app_hdrs.get(header::LOCATION).unwrap().to_str().unwrap();
    let parsed_loc = Url::parse(loc).expect("parse redirect location");
    let qparams: HashMap<String, String> = parsed_loc.query_pairs().into_owned().collect();
    assert_eq!(qparams.get("state").map(String::as_str), Some(state_param));
    let code = qparams.get("code").expect("auth code").clone();

    let tok_form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", &code)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("client_id", &client_id)
        .append_pair("code_verifier", verifier)
        .append_pair("resource", resource)
        .finish();
    let (tok_st, _, tok_raw) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(tok_form))
                .unwrap(),
        )
        .await;
    assert_eq!(tok_st, StatusCode::OK);
    let tok_val: Value = serde_json::from_slice(&tok_raw).unwrap();
    let access_token = tok_val["access_token"].as_str().unwrap().to_string();
    let refresh_token = tok_val["refresh_token"].as_str().unwrap().to_string();
    assert!(access_token.starts_with("fbmcp_at_"));
    assert!(refresh_token.starts_with("fbmcp_rt_"));

    (access_token, refresh_token, client_id, verifier.to_string(), code)
}

#[tokio::test]
async fn mcp_oauth_discovery_and_401_rfc9728_challenge() {
    let h = TestServerHarness::new();

    // 1. Unauthenticated /mcp returns 401 + RFC 9728 WWW-Authenticate header
    let (st, hdrs, _) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            None,
            &[
                ("x-forwarded-proto", "https"),
                ("x-forwarded-host", "mcp.ombhrum.com"),
            ],
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" })),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let www = hdrs
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate header present")
        .to_str()
        .unwrap();
    assert!(
        www.contains(r#"resource_metadata="https://mcp.ombhrum.com/.well-known/oauth-protected-resource""#),
        "unexpected WWW-Authenticate: {www}"
    );
    assert!(www.contains(r#"scope="devices:read devices:connect""#), "www={www}");

    // 2. RFC 9728 Protected Resource Metadata (both root and /mcp suffix)
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
    ] {
        let (st_prm, _, prm) = h
            .json_req_headers(
                Method::GET,
                path,
                None,
                &[
                    ("x-forwarded-proto", "https"),
                    ("x-forwarded-host", "mcp.ombhrum.com"),
                ],
                None,
            )
            .await;
        assert_eq!(st_prm, StatusCode::OK);
        assert_eq!(prm["resource"], "https://mcp.ombhrum.com/mcp");
        assert_eq!(prm["authorization_servers"][0], "https://mcp.ombhrum.com");
        assert_eq!(prm["bearer_methods_supported"][0], "header");
        let scopes = prm["scopes_supported"].as_array().unwrap();
        for required in [
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
        ] {
            assert!(scopes.iter().any(|v| v.as_str() == Some(required)), "missing scope {required}");
        }
    }

    // 3. RFC 8414 Authorization Server Metadata & OIDC Discovery
    for path in [
        "/.well-known/oauth-authorization-server",
        "/.well-known/openid-configuration",
    ] {
        let (st_as, _, meta) = h
            .json_req_headers(
                Method::GET,
                path,
                None,
                &[("host", "127.0.0.1:8443")],
                None,
            )
            .await;
        assert_eq!(st_as, StatusCode::OK);
        assert_eq!(meta["authorization_endpoint"], "http://127.0.0.1:8443/oauth/authorize");
        assert_eq!(meta["token_endpoint"], "http://127.0.0.1:8443/oauth/token");
        assert_eq!(meta["registration_endpoint"], "http://127.0.0.1:8443/oauth/register");
        assert_eq!(meta["revocation_endpoint"], "http://127.0.0.1:8443/oauth/revoke");
        assert_eq!(meta["code_challenge_methods_supported"][0], "S256");
        assert_eq!(meta["client_id_metadata_document_supported"], true);
    }
}

#[tokio::test]
async fn mcp_oauth_dynamic_registration_ssrf_and_redirect_allowlist() {
    let h = TestServerHarness::new();

    for valid_uri in [
        "http://127.0.0.1:54321/cb",
        "http://localhost:8080/callback",
        "http://[::1]:4000/cb",
        "https://claude.ai/api/mcp/auth_callback",
        "cursor://anysphere.cursor-retrieval/oauth/callback",
        "vscode://vscode.mcp/callback",
        "antigravity://oauth/callback",
        "claude://oauth/callback",
    ] {
        let (st, _, val) = h
            .json_req(
                Method::POST,
                "/oauth/register",
                None,
                Some(json!({
                    "client_name": "Allowed Client",
                    "redirect_uris": [valid_uri]
                })),
            )
            .await;
        assert_eq!(st, StatusCode::CREATED, "expected valid URI {valid_uri}, got {val}");
        assert!(val["client_id"].as_str().unwrap().starts_with("fbmcp_cli_"));
    }

    for invalid_uri in [
        "javascript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "http://evil.example.com/callback",
        "http://192.168.1.50/callback",
        "https://example.com/callback#fragment",
        "https://user:pass@example.com/callback",
        "file:///etc/passwd",
    ] {
        let (st, _, val) = h
            .json_req(
                Method::POST,
                "/oauth/register",
                None,
                Some(json!({
                    "client_name": "Evil Client",
                    "redirect_uris": [invalid_uri]
                })),
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "expected rejection for {invalid_uri}, got {val}");
    }
}

#[tokio::test]
async fn mcp_oauth_pkce_s256_password_qr_login_and_replay_protection() {
    let h = TestServerHarness::new();
    let (uid, user_tok) = h.register_user("alice_mcp", "SecretPass123!");
    let _node = h
        .register_local_node(&user_tok, "mac-alice", "Alice MacBook", ComputerRunPolicy::AlwaysAllowAuthorized)
        .await;

    // 1. Register client and test open redirect mismatch rejection
    let (reg_st, _, reg_val) = h
        .json_req(
            Method::POST,
            "/oauth/register",
            None,
            Some(json!({
                "client_name": "Cursor MCP",
                "redirect_uris": ["http://127.0.0.1:49999/cb"]
            })),
        )
        .await;
    assert_eq!(reg_st, StatusCode::CREATED);
    let cid = reg_val["client_id"].as_str().unwrap();

    let verifier = "pkce_verifier_0123456789_abcdefghijklmnopqrstuvwxyz_012345";
    let challenge = pkce_challenge(verifier);

    let (mismatch_st, _, _) = h
        .req(
            Request::builder()
                .method(Method::GET)
                .uri(&format!(
                    "/oauth/authorize?response_type=code&client_id={cid}&redirect_uri={}&code_challenge={challenge}&code_challenge_method=S256",
                    urlencoding_component("https://attacker.example/steal")
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(mismatch_st, StatusCode::BAD_REQUEST, "unregistered redirect_uri must fail");

    // 2. Full valid PKCE S256 login + consent flow + capture issued code
    let (at, rt, reg_cid, used_verifier, used_code) = run_full_oauth_pkce_flow(
        &h,
        "alice_mcp",
        "SecretPass123!",
        "devices:read terminal:exec",
        "all",
        &[],
    )
    .await;
    assert!(!at.is_empty());
    assert!(!rt.is_empty());

    // Grant is active before replay
    let (pre_g_st, _, pre_g_val) = h
        .json_req(Method::GET, "/api/v1/mcp/grants", Some(&user_tok), None)
        .await;
    assert_eq!(pre_g_st, StatusCode::OK);
    assert_eq!(pre_g_val["grants"].as_array().unwrap().len(), 1);

    // 3. Single-use authorization code replay MUST fail AND revoke previously issued family tokens
    let replay_form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", &used_code)
        .append_pair("redirect_uri", "http://127.0.0.1:43210/oauth/callback")
        .append_pair("client_id", &reg_cid)
        .append_pair("code_verifier", &used_verifier)
        .append_pair("resource", "http://127.0.0.1:8443/mcp")
        .finish();
    let (rep_st, _, rep_raw) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(replay_form))
                .unwrap(),
        )
        .await;
    assert_eq!(rep_st, StatusCode::BAD_REQUEST);
    let rep_val: Value = serde_json::from_slice(&rep_raw).unwrap();
    assert_eq!(rep_val["error"], "invalid_grant");

    let (post_rep_mcp, _, _) = h
        .json_req(
            Method::POST,
            "/mcp",
            Some(&at),
            Some(json!({ "jsonrpc": "2.0", "id": 99, "method": "ping" })),
        )
        .await;
    assert_eq!(post_rep_mcp, StatusCode::UNAUTHORIZED, "code replay must revoke issued access token");

    // 4. Wrong PKCE code_verifier & wrong RFC 8707 resource rejection
    let (auth_st, auth_hdrs, auth_bytes) = h
        .req(
            Request::builder()
                .method(Method::GET)
                .uri(&format!(
                    "/oauth/authorize?response_type=code&client_id={cid}&redirect_uri={}&code_challenge={challenge}&code_challenge_method=S256&scope=devices:read&resource={}",
                    urlencoding_component("http://127.0.0.1:49999/cb"),
                    urlencoding_component("http://127.0.0.1:8443/mcp")
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(auth_st, StatusCode::OK);
    let login_html = String::from_utf8_lossy(&auth_bytes);
    let cookie = extract_cookie_pair(&auth_hdrs, "fbmcp_oauth_sess").unwrap();
    let tx_id = extract_hidden_input(&login_html, "tx_id").unwrap();
    let csrf = extract_hidden_input(&login_html, "csrf_token").unwrap();
    let pair_token = extract_hidden_input(&login_html, "pair_token").expect("QR pair_token");

    // Wrong CSRF check
    let bad_csrf = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("tx_id", &tx_id)
        .append_pair("csrf_token", "forged_csrf_token")
        .append_pair("username", "alice_mcp")
        .append_pair("password", "SecretPass123!")
        .finish();
    let (csrf_st, _, _) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/authorize/login")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(bad_csrf))
                .unwrap(),
        )
        .await;
    assert_eq!(csrf_st, StatusCode::FORBIDDEN);

    // 5. QR pairing bridge login via /api/v1/auth/pair/accept -> /oauth/authorize/qr-poll
    let (acc_st, _, acc_val) = h
        .json_req(
            Method::POST,
            "/api/v1/auth/pair/accept",
            Some(&user_tok),
            Some(json!({ "pair_token": pair_token })),
        )
        .await;
    assert_eq!(acc_st, StatusCode::OK, "pair accept failed: {acc_val}");

    let (poll_st, _, poll_raw) = h
        .req(
            Request::builder()
                .method(Method::GET)
                .uri(&format!("/oauth/authorize/qr-poll?tx_id={tx_id}&pair_token={pair_token}"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(poll_st, StatusCode::OK);
    let poll_val: Value = serde_json::from_slice(&poll_raw).unwrap();
    assert_eq!(poll_val["status"], "confirmed");

    // QR login -> consent HTML -> approve -> wrong verifier check
    let qr_login_form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("tx_id", &tx_id)
        .append_pair("csrf_token", &csrf)
        .append_pair("pair_token", &pair_token)
        .finish();
    let (ql_st, _, ql_bytes) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/authorize/login")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(qr_login_form))
                .unwrap(),
        )
        .await;
    assert_eq!(ql_st, StatusCode::OK);
    let consent_html = String::from_utf8_lossy(&ql_bytes);
    let csrf2 = extract_hidden_input(&consent_html, "csrf_token").unwrap();

    let app_form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("tx_id", &tx_id)
        .append_pair("csrf_token", &csrf2)
        .append_pair("decision", "approve")
        .append_pair("device_scope", "all")
        .append_pair("scopes", "devices:read")
        .finish();
    let (app_st, app_hdrs, _) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/authorize/approve")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(app_form))
                .unwrap(),
        )
        .await;
    assert_eq!(app_st, StatusCode::FOUND);
    let loc = app_hdrs.get(header::LOCATION).unwrap().to_str().unwrap();
    let qr_code = Url::parse(loc)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();

    // Wrong RFC 8707 resource MUST fail before code consumption
    let wrong_res = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", &qr_code)
        .append_pair("redirect_uri", "http://127.0.0.1:49999/cb")
        .append_pair("client_id", cid)
        .append_pair("code_verifier", verifier)
        .append_pair("resource", "https://evil.example.com/mcp")
        .finish();
    let (wr_st, _, wr_raw) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(wrong_res))
                .unwrap(),
        )
        .await;
    assert_eq!(wr_st, StatusCode::BAD_REQUEST);
    let wr_json: Value = serde_json::from_slice(&wr_raw).unwrap();
    assert_eq!(wr_json["error"], "invalid_target");

    // Valid QR authorization code exchange with true verifier & resource succeeds
    let ok_qr = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", &qr_code)
        .append_pair("redirect_uri", "http://127.0.0.1:49999/cb")
        .append_pair("client_id", cid)
        .append_pair("code_verifier", verifier)
        .append_pair("resource", "http://127.0.0.1:8443/mcp")
        .finish();
    let (qr_tok_st, _, _) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(ok_qr))
                .unwrap(),
        )
        .await;
    assert_eq!(qr_tok_st, StatusCode::OK);

    // Issue a fresh code to test wrong PKCE code_verifier rejection
    let (auth_st3, auth_hdrs3, auth_bytes3) = h
        .req(
            Request::builder()
                .method(Method::GET)
                .uri(&format!(
                    "/oauth/authorize?response_type=code&client_id={cid}&redirect_uri={}&code_challenge={challenge}&code_challenge_method=S256&scope=devices:read&resource={}",
                    urlencoding_component("http://127.0.0.1:49999/cb"),
                    urlencoding_component("http://127.0.0.1:8443/mcp")
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(auth_st3, StatusCode::OK);
    let html3 = String::from_utf8_lossy(&auth_bytes3);
    let c3 = extract_cookie_pair(&auth_hdrs3, "fbmcp_oauth_sess").unwrap();
    let tx3 = extract_hidden_input(&html3, "tx_id").unwrap();
    let csrf3 = extract_hidden_input(&html3, "csrf_token").unwrap();
    let lform3 = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("tx_id", &tx3)
        .append_pair("csrf_token", &csrf3)
        .append_pair("username", "alice_mcp")
        .append_pair("password", "SecretPass123!")
        .finish();
    let (l3_st, _, l3_bytes) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/authorize/login")
                .header(header::COOKIE, &c3)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(lform3))
                .unwrap(),
        )
        .await;
    assert_eq!(l3_st, StatusCode::OK);
    let ch3 = String::from_utf8_lossy(&l3_bytes);
    let csrf3_c = extract_hidden_input(&ch3, "csrf_token").unwrap();
    let aform3 = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("tx_id", &tx3)
        .append_pair("csrf_token", &csrf3_c)
        .append_pair("decision", "approve")
        .append_pair("device_scope", "all")
        .append_pair("scopes", "devices:read")
        .finish();
    let (a3_st, a3_hdrs, _) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/authorize/approve")
                .header(header::COOKIE, &c3)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(aform3))
                .unwrap(),
        )
        .await;
    assert_eq!(a3_st, StatusCode::FOUND);
    let code3 = Url::parse(a3_hdrs.get(header::LOCATION).unwrap().to_str().unwrap())
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();

    // Wrong verifier MUST fail with invalid_grant
    let wrong_ver = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", &code3)
        .append_pair("redirect_uri", "http://127.0.0.1:49999/cb")
        .append_pair("client_id", cid)
        .append_pair("code_verifier", "wrong_verifier_0123456789_0123456789_0123456789_abc")
        .append_pair("resource", "http://127.0.0.1:8443/mcp")
        .finish();
    let (wv_st, _, wv_raw) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(wrong_ver))
                .unwrap(),
        )
        .await;
    assert_eq!(wv_st, StatusCode::BAD_REQUEST);
    let wv_json: Value = serde_json::from_slice(&wv_raw).unwrap();
    assert_eq!(wv_json["error"], "invalid_grant");

    // Verify grants endpoint now reflects the QR-authorized grant
    let (g_st, _, g_val) = h
        .json_req(Method::GET, "/api/v1/mcp/grants", Some(&user_tok), None)
        .await;
    assert_eq!(g_st, StatusCode::OK);
    assert_eq!(g_val["grants"].as_array().unwrap().len(), 1);
    let _ = uid;
}

#[tokio::test]
async fn mcp_oauth_refresh_rotation_reuse_family_revocation_and_grants_api() {
    let h = TestServerHarness::new();
    let (_uid, user_tok) = h.register_user("bob_rotate", "RotatePass123!");
    let (at1, rt1, cid, _, _) = run_full_oauth_pkce_flow(
        &h,
        "bob_rotate",
        "RotatePass123!",
        "devices:read",
        "all",
        &[],
    )
    .await;

    // 1. Rotate rt1 -> (at2, rt2)
    let rot_form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", &rt1)
        .append_pair("client_id", &cid)
        .finish();
    let (rot_st, _, rot_raw) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(rot_form.clone()))
                .unwrap(),
        )
        .await;
    assert_eq!(rot_st, StatusCode::OK);
    let rot_val: Value = serde_json::from_slice(&rot_raw).unwrap();
    let at2 = rot_val["access_token"].as_str().unwrap().to_string();
    let rt2 = rot_val["refresh_token"].as_str().unwrap().to_string();
    assert_ne!(at1, at2);
    assert_ne!(rt1, rt2);

    // at2 works on /mcp
    let (ok_st, _, _) = h
        .json_req(
            Method::POST,
            "/mcp",
            Some(&at2),
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })),
        )
        .await;
    assert_eq!(ok_st, StatusCode::OK);

    // 2. Reuse old rt1 -> MUST fail AND revoke entire token family (including at2 & rt2!)
    let (reuse_st, _, reuse_raw) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(rot_form))
                .unwrap(),
        )
        .await;
    assert_eq!(reuse_st, StatusCode::BAD_REQUEST);
    let reuse_val: Value = serde_json::from_slice(&reuse_raw).unwrap();
    assert_eq!(reuse_val["error"], "invalid_grant");

    let (rev_st, _, _) = h
        .json_req(
            Method::POST,
            "/mcp",
            Some(&at2),
            Some(json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" })),
        )
        .await;
    assert_eq!(rev_st, StatusCode::UNAUTHORIZED, "family revocation must kill active rotated access_token");

    let rot2_form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", &rt2)
        .append_pair("client_id", &cid)
        .finish();
    let (rt2_st, _, _) = h
        .req(
            Request::builder()
                .method(Method::POST)
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(rot2_form))
                .unwrap(),
        )
        .await;
    assert_eq!(rt2_st, StatusCode::BAD_REQUEST);

    // 3. New grant -> list & delete via /api/v1/mcp/grants/{family_id} -> /mcp rejects immediately
    let (at3, _, _, _, _) = run_full_oauth_pkce_flow(
        &h,
        "bob_rotate",
        "RotatePass123!",
        "devices:read",
        "all",
        &[],
    )
    .await;
    let (gl_st, _, gl_val) = h
        .json_req(Method::GET, "/api/v1/mcp/grants", Some(&user_tok), None)
        .await;
    assert_eq!(gl_st, StatusCode::OK);
    let grants = gl_val["grants"].as_array().unwrap();
    assert_eq!(grants.len(), 1);
    let family_id = grants[0]["family_id"].as_str().unwrap().to_string();

    let (del_st, _, _) = h
        .json_req(
            Method::DELETE,
            &format!("/api/v1/mcp/grants/{family_id}"),
            Some(&user_tok),
            None,
        )
        .await;
    assert_eq!(del_st, StatusCode::OK);

    let (after_del_st, _, _) = h
        .json_req(
            Method::POST,
            "/mcp",
            Some(&at3),
            Some(json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" })),
        )
        .await;
    assert_eq!(after_del_st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn official_mcp_protocol_tools_image_scope_device_allowlist_and_audit() {
    let h = TestServerHarness::new();
    let (uid, user_tok) = h.register_user("carol_mcp", "CarolPass123!");
    let node_allowed = h
        .register_local_node(
            &user_tok,
            "dev-allowed",
            "Allowed Workstation",
            ComputerRunPolicy::AlwaysAllowAuthorized,
        )
        .await;
    let node_forbidden = h
        .register_local_node(
            &user_tok,
            "dev-secret",
            "Secret Workstation",
            ComputerRunPolicy::AlwaysAllowAuthorized,
        )
        .await;

    // Create chat + bot for full-app tools
    let chat = h
        .state
        .db
        .create_chat_with_creator(
            gpui_telegram_core::types::UserId(uid),
            "MCP Workspace Chat",
            &[],
            chrono::Utc::now().timestamp_millis(),
        )
        .expect("create chat");

    // Token A: broad scopes but restricted ONLY to `node_allowed`
    let (at_full, _, client_id, _, _) = run_full_oauth_pkce_flow(
        &h,
        "carol_mcp",
        "CarolPass123!",
        "devices:read devices:connect screen:view input:control terminal:exec files:read files:write assist:manage messaging:read messaging:write chats:read miniapp:control tasks:manage",
        "selected",
        &[&node_allowed.node_id],
    )
    .await;

    // Also authorize caller_device_id = format!("mcp:{client_id}") on node_allowed (and node_forbidden)
    // Wait: let's see if ComputerControlService accepts our command when authorized!
    let _ = h
        .json_req(
            Method::PATCH,
            &format!("/api/v1/computers/{}/policy", urlencoding_component(&node_allowed.node_id)),
            Some(&user_tok),
            Some(json!({
                "add_controller": {
                    "controller_id": format!("mcp:{client_id}"),
                    "kind": "device",
                    "display_name": "Claude Code Test",
                    "added_at_ms": 0
                }
            })),
        )
        .await;

    // Spawn mock device worker answering commands for `dev-allowed`
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let router_bg = h.router.clone();
    let tok_bg = user_tok.clone();
    let worker = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(15)) => {
                    let poll_req = Request::builder()
                        .method(Method::GET)
                        .uri("/api/v1/computers/local/poll")
                        .header(header::AUTHORIZATION, format!("Bearer {tok_bg}"))
                        .header("X-Device-Id", "dev-allowed")
                        .body(Body::empty())
                        .unwrap();
                    let resp = router_bg.clone().oneshot(poll_req).await.unwrap();
                    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
                    let v: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
                    if let Some(cmds) = v.as_array().or_else(|| v["commands"].as_array()) {
                        for cmd in cmds {
                            let cmd_id = cmd["command_id"].as_str().unwrap_or("");
                            let op_type = cmd["operation"]["op"].as_str().unwrap_or("");
                            let mut result = RemoteComputerCommandResult {
                                command_id: cmd_id.to_string(),
                                node_id: cmd["node_id"].as_str().unwrap_or("").to_string(),
                                ok: true,
                                exit_code: 0,
                                stdout: format!("executed:{op_type}"),
                                stderr: String::new(),
                                entries: vec![],
                                frame_data_url: None,
                                executed_by: "mock-dev-allowed".to_string(),
                                duration_ms: 5,
                            };
                            if op_type == "capture_screen" {
                                result.frame_data_url = Some(ONE_PIXEL_PNG_DATA_URL.to_string());
                            }
                            let comp_req = Request::builder()
                                .method(Method::POST)
                                .uri(format!("/api/v1/computers/commands/{}/complete", urlencoding_component(cmd_id)))
                                .header(header::AUTHORIZATION, format!("Bearer {tok_bg}"))
                                .header(header::CONTENT_TYPE, "application/json")
                                .body(Body::from(serde_json::to_vec(&result).unwrap()))
                                .unwrap();
                            let _ = router_bg.clone().oneshot(comp_req).await;
                        }
                    }
                }
            }
        }
    });

    // 1. Test MCP protocol versions 2025-03-26, 2025-06-18, 2025-11-25
    let mut last_sid = String::new();
    for ver in ["2025-03-26", "2025-06-18", "2025-11-25"] {
        let (st, hdrs, init_val) = h
            .json_req(
                Method::POST,
                "/mcp",
                Some(&at_full),
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": ver,
                        "clientInfo": { "name": "test-client", "version": "1.0" }
                    }
                })),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(init_val["result"]["protocolVersion"], ver);
        assert_eq!(hdrs.get("MCP-Protocol-Version").unwrap().to_str().unwrap(), ver);
        last_sid = hdrs.get("Mcp-Session-Id").unwrap().to_str().unwrap().to_string();
    }

    // notifications/initialized & ping & tools/list
    let (ni_st, _, _) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid), ("MCP-Protocol-Version", "2025-11-25")],
            Some(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })),
        )
        .await;
    assert_eq!(ni_st, StatusCode::ACCEPTED);

    let (tl_st, _, tl_val) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            Some(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })),
        )
        .await;
    assert_eq!(tl_st, StatusCode::OK);
    let tools = tl_val["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 31, "expected all 31 MCP tools");

    // 2. list_devices respects allowed_node_ids allowlist (only node_allowed returned!)
    let (ld_st, _, ld_val) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            Some(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "list_devices", "arguments": {} }
            })),
        )
        .await;
    assert_eq!(ld_st, StatusCode::OK);
    let devs = ld_val["result"]["structuredContent"]["devices"].as_array().unwrap();
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0]["node_id"], node_allowed.node_id);

    // 3. Calling terminal_exec on forbidden node MUST fail with 403
    let (fb_st, _, fb_val) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            Some(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "terminal_exec",
                    "arguments": { "node_id": node_forbidden.node_id, "command": "whoami" }
                }
            })),
        )
        .await;
    assert_eq!(fb_st, StatusCode::FORBIDDEN, "expected 403 for disallowed device: {fb_val}");

    // 4. capture_screen returns MCP ImageContent
    let (cs_st, _, cs_val) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            Some(json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "capture_screen",
                    "arguments": { "node_id": node_allowed.node_id }
                }
            })),
        )
        .await;
    assert_eq!(cs_st, StatusCode::OK, "capture_screen failed: {cs_val}");
    let contents = cs_val["result"]["content"].as_array().unwrap();
    assert_eq!(contents[0]["type"], "image");
    assert_eq!(contents[0]["mimeType"], "image/png");
    assert!(!contents[0]["data"].as_str().unwrap().is_empty());

    // 5. terminal_exec, mouse_action, file_list, clipboard_set execute cleanly
    for (rpc_id, name, args) in [
        (6, "terminal_exec", json!({ "node_id": node_allowed.node_id, "command": "pwd" })),
        (7, "mouse_action", json!({ "node_id": node_allowed.node_id, "action": "click", "x": 100, "y": 200 })),
        (8, "file_list", json!({ "node_id": node_allowed.node_id, "path": "/tmp" })),
        (9, "clipboard_set", json!({ "node_id": node_allowed.node_id, "text": "hello-clip" })),
    ] {
        let (tst, _, tval) = h
            .json_req_headers(
                Method::POST,
                "/mcp",
                Some(&at_full),
                &[("Mcp-Session-Id", &last_sid)],
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": rpc_id,
                    "method": "tools/call",
                    "params": { "name": name, "arguments": args }
                })),
            )
            .await;
        assert_eq!(tst, StatusCode::OK, "tool {name} failed: {tval}");
        assert_eq!(tval["result"]["structuredContent"]["ok"], true, "tool {name} result={tval}");
    }

    // 6. Full-app tools: send_message, list_dialogs, get_chat_history, search_messages, create_task, list_tasks, list_bots, miniapp_list, assist_get_info
    let (sm_st, _, sm_val) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            Some(json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "tools/call",
                "params": {
                    "name": "send_message",
                    "arguments": { "chat_id": chat.chat_id.0, "text": "Hello via Official MCP server!" }
                }
            })),
        )
        .await;
    assert_eq!(sm_st, StatusCode::OK, "send_message failed: {sm_val}");

    let (ch_st, _, ch_val) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            Some(json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "tools/call",
                "params": {
                    "name": "get_chat_history",
                    "arguments": { "chat_id": chat.chat_id.0 }
                }
            })),
        )
        .await;
    assert_eq!(ch_st, StatusCode::OK);
    assert!(!ch_val["result"]["structuredContent"]["messages"].as_array().unwrap().is_empty());

    let (sr_st, _, sr_val) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            Some(json!({
                "jsonrpc": "2.0",
                "id": 12,
                "method": "tools/call",
                "params": { "name": "search_messages", "arguments": { "query": "Official MCP" } }
            })),
        )
        .await;
    assert_eq!(sr_st, StatusCode::OK);
    assert!(!sr_val["result"]["structuredContent"]["hits"].as_array().unwrap().is_empty());

    let (ct_st, _, ct_val) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            Some(json!({
                "jsonrpc": "2.0",
                "id": 13,
                "method": "tools/call",
                "params": { "name": "create_task", "arguments": { "title": "Inspect logs" } }
            })),
        )
        .await;
    assert_eq!(ct_st, StatusCode::OK);
    assert!(ct_val["result"]["structuredContent"]["task_id"].as_str().is_some());

    for (id, tname) in [(14, "list_dialogs"), (15, "list_tasks"), (16, "list_bots"), (17, "miniapp_list"), (18, "assist_get_info")] {
        let (st, _, val) = h
            .json_req_headers(
                Method::POST,
                "/mcp",
                Some(&at_full),
                &[("Mcp-Session-Id", &last_sid)],
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "tools/call",
                    "params": { "name": tname, "arguments": {} }
                })),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "tool {tname} failed: {val}");
    }

    // 7. Token B with ONLY `devices:read` -> 403 insufficient_scope on `terminal_exec`
    let (at_readonly, _, _, _, _) = run_full_oauth_pkce_flow(
        &h,
        "carol_mcp",
        "CarolPass123!",
        "devices:read",
        "all",
        &[],
    )
    .await;
    let (sc_st, sc_hdrs, _) = h
        .json_req(
            Method::POST,
            "/mcp",
            Some(&at_readonly),
            Some(json!({
                "jsonrpc": "2.0",
                "id": 19,
                "method": "tools/call",
                "params": {
                    "name": "terminal_exec",
                    "arguments": { "node_id": node_allowed.node_id, "command": "ls" }
                }
            })),
        )
        .await;
    assert_eq!(sc_st, StatusCode::FORBIDDEN);
    let sc_www = sc_hdrs.get(header::WWW_AUTHENTICATE).unwrap().to_str().unwrap();
    assert!(sc_www.contains(r#"error="insufficient_scope""#), "www={sc_www}");
    assert!(sc_www.contains(r#"scope="terminal:exec""#), "www={sc_www}");

    // 8. resources/list & resources/read & GET /mcp SSE & audit log & active remote session
    let (rl_st, _, rl_val) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            Some(json!({ "jsonrpc": "2.0", "id": 20, "method": "resources/list" })),
        )
        .await;
    assert_eq!(rl_st, StatusCode::OK);
    assert_eq!(rl_val["result"]["resources"].as_array().unwrap().len(), 3);

    let (rr_st, _, rr_val) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            Some(json!({
                "jsonrpc": "2.0",
                "id": 21,
                "method": "resources/read",
                "params": { "uri": "fabushi://devices" }
            })),
        )
        .await;
    assert_eq!(rr_st, StatusCode::OK);
    assert_eq!(rr_val["result"]["contents"][0]["uri"], "fabushi://devices");

    let sse_resp = h
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {at_full}"))
                .header("Mcp-Session-Id", &last_sid)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("GET /mcp SSE");
    assert_eq!(sse_resp.status(), StatusCode::OK);
    assert!(
        sse_resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.contains("text/event-stream"))
    );
    drop(sse_resp);

    let (rs_st, _, rs_val) = h
        .json_req(Method::GET, "/api/v1/remote/sessions", Some(&user_tok), None)
        .await;
    assert_eq!(rs_st, StatusCode::OK);
    assert!(!rs_val["sessions"].as_array().unwrap().is_empty());

    let (aud_st, _, aud_val) = h
        .json_req(Method::GET, "/api/v1/mcp/audit", Some(&user_tok), None)
        .await;
    assert_eq!(aud_st, StatusCode::OK);
    assert!(aud_val["entries"].as_array().unwrap().len() >= 10);

    // 9. DELETE /mcp closes explicit session cleanly
    let (del_st, _, _) = h
        .json_req_headers(
            Method::DELETE,
            "/mcp",
            Some(&at_full),
            &[("Mcp-Session-Id", &last_sid)],
            None,
        )
        .await;
    assert_eq!(del_st, StatusCode::OK);

    let _ = stop_tx.send(());
    let _ = worker.await;
}

#[tokio::test]
async fn official_mcp_disconnect_all_kill_switch_aborts_inflight_and_subsequent_calls() {
    let h = Arc::new(TestServerHarness::new());
    let (_uid, user_tok) = h.register_user("dave_kill", "KillPass123!");
    let node_a = h
        .register_local_node(
            &user_tok,
            "dev-kill",
            "Kill Switch Mac",
            ComputerRunPolicy::AskEveryTime,
        )
        .await;

    let (at, _, _, _, _) = run_full_oauth_pkce_flow(
        &h,
        "dave_kill",
        "KillPass123!",
        "devices:read terminal:exec",
        "all",
        &[],
    )
    .await;

    // Initialize MCP session
    let (init_st, init_hdrs, _) = h
        .json_req(
            Method::POST,
            "/mcp",
            Some(&at),
            Some(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" }
            })),
        )
        .await;
    assert_eq!(init_st, StatusCode::OK);
    let sid = init_hdrs.get("Mcp-Session-Id").unwrap().to_str().unwrap().to_string();

    // Launch in-flight `terminal_exec` that waits because AskEveryTime has no responder
    let h_clone = Arc::clone(&h);
    let at_clone = at.clone();
    let sid_clone = sid.clone();
    let nid_clone = node_a.node_id.clone();
    let inflight = tokio::spawn(async move {
        h_clone
            .json_req_headers(
                Method::POST,
                "/mcp",
                Some(&at_clone),
                &[("Mcp-Session-Id", &sid_clone)],
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "terminal_exec",
                        "arguments": { "node_id": nid_clone, "command": "sleep 60" }
                    }
                })),
            )
            .await
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;

    // Host hits global kill-switch!
    let (ks_st, _, ks_val) = h
        .json_req(
            Method::POST,
            "/api/v1/remote/sessions/disconnect-all",
            Some(&user_tok),
            None,
        )
        .await;
    assert_eq!(ks_st, StatusCode::OK, "disconnect-all failed: {ks_val}");
    assert_eq!(ks_val["kill_switch"], true);

    // In-flight call MUST abort immediately (well under 2s, not waiting for 60s timeout)
    let (aborted_st, _, aborted_val) =
        tokio::time::timeout(tokio::time::Duration::from_secs(2), inflight)
            .await
            .expect("in-flight MCP call did not abort immediately")
            .expect("join inflight");
    assert_eq!(aborted_st, StatusCode::GONE, "expected 410 Gone on abort: {aborted_val}");

    // Subsequent call with SAME Mcp-Session-Id MUST be rejected immediately
    let (sub_st, _, _) = h
        .json_req_headers(
            Method::POST,
            "/mcp",
            Some(&at),
            &[("Mcp-Session-Id", &sid)],
            Some(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "list_devices", "arguments": {} }
            })),
        )
        .await;
    assert_eq!(sub_st, StatusCode::GONE);

    // Subsequent call WITHOUT Mcp-Session-Id using the same access token MUST also be rejected
    let (hdrless_st, _, _) = h
        .json_req(
            Method::POST,
            "/mcp",
            Some(&at),
            Some(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": { "name": "list_devices", "arguments": {} }
            })),
        )
        .await;
    assert_eq!(hdrless_st, StatusCode::GONE);

    let (rs_st, _, rs_val) = h
        .json_req(Method::GET, "/api/v1/remote/sessions", Some(&user_tok), None)
        .await;
    assert_eq!(rs_st, StatusCode::OK);
    assert!(rs_val["sessions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn remote_assist_codes_validation_rate_limit_and_same_account_webrtc() {
    let h = TestServerHarness::new();
    let (host_uid, host_tok) = h.register_user("eve_assist", "EvePass123!");
    let (_helper_uid, helper_tok) = h.register_user("frank_helper", "FrankPass123!");
    let _host_node = h
        .register_local_node(&host_tok, "mac-eve", "Eve Studio", ComputerRunPolicy::AskEveryTime)
        .await;

    // 1. Allocate permanent 9-digit assist_id + temporary code
    let (info_st, _, info_val) = h
        .json_req_headers(
            Method::GET,
            "/api/v1/remote/assist/info",
            Some(&host_tok),
            &[("X-Device-Id", "mac-eve")],
            None,
        )
        .await;
    assert_eq!(info_st, StatusCode::OK);
    let assist_id = info_val["assist_id"].as_str().unwrap().to_string();
    assert_eq!(assist_id.len(), 9);
    assert!(assist_id.chars().all(|c| c.is_ascii_digit()));
    let temp_code_1 = info_val["current_code"].as_str().unwrap().to_string();
    assert_eq!(info_val["allow_assist"], true);

    // 2. reset-code rotates temp code while preserving permanent assist_id
    let (rst_st, _, rst_val) = h
        .json_req_headers(
            Method::POST,
            "/api/v1/remote/assist/reset-code",
            Some(&host_tok),
            &[("X-Device-Id", "mac-eve")],
            None,
        )
        .await;
    assert_eq!(rst_st, StatusCode::OK);
    assert_eq!(rst_val["assist_id"], assist_id);
    let temp_code_2 = rst_val["current_code"].as_str().unwrap().to_string();
    assert_ne!(temp_code_1, temp_code_2);

    // 3. set-code validation (8-16 alphanumeric w/ letters & digits)
    for bad_code in ["abc1", "abcdefgh", "12345678", "Abc1234!", "TooLongPassword12345"] {
        let (bad_st, _, _) = h
            .json_req_headers(
                Method::POST,
                "/api/v1/remote/assist/set-code",
                Some(&host_tok),
                &[("X-Device-Id", "mac-eve")],
                Some(json!({ "code": bad_code })),
            )
            .await;
        assert_eq!(bad_st, StatusCode::BAD_REQUEST, "expected rejection for `{bad_code}`");
    }

    let custom_code = "Fabu2026Go";
    let (set_st, _, set_val) = h
        .json_req_headers(
            Method::POST,
            "/api/v1/remote/assist/set-code",
            Some(&host_tok),
            &[("X-Device-Id", "mac-eve")],
            Some(json!({ "code": custom_code })),
        )
        .await;
    assert_eq!(set_st, StatusCode::OK);
    assert_eq!(set_val["code_mode"], "custom");
    assert!(set_val["current_code"].is_null());

    // 4. allow=false disables assist -> start returns 403
    let (dis_st, _, _) = h
        .json_req_headers(
            Method::POST,
            "/api/v1/remote/assist/allow",
            Some(&host_tok),
            &[("X-Device-Id", "mac-eve")],
            Some(json!({ "allow_assist": false })),
        )
        .await;
    assert_eq!(dis_st, StatusCode::OK);

    let (blocked_st, _, _) = h
        .json_req(
            Method::POST,
            "/api/v1/remote/assist/start",
            Some(&helper_tok),
            Some(json!({
                "assist_id": assist_id,
                "access_code": custom_code,
                "requester_device_id": "win-frank",
                "requester_name": "Frank",
                "requester_platform": "windows",
                "mode": "desktop"
            })),
        )
        .await;
    assert_eq!(blocked_st, StatusCode::FORBIDDEN);

    // Re-enable -> start succeeds + returns temp_relay_token + shows in /remote/sessions
    let (en_st, _, _) = h
        .json_req_headers(
            Method::POST,
            "/api/v1/remote/assist/allow",
            Some(&host_tok),
            &[("X-Device-Id", "mac-eve")],
            Some(json!({ "allow_assist": true })),
        )
        .await;
    assert_eq!(en_st, StatusCode::OK);

    let (start_st, _, start_val) = h
        .json_req(
            Method::POST,
            "/api/v1/remote/assist/start",
            Some(&helper_tok),
            Some(json!({
                "assist_id": assist_id,
                "access_code": custom_code,
                "requester_device_id": "win-frank",
                "requester_name": "Frank",
                "requester_platform": "windows",
                "mode": "desktop"
            })),
        )
        .await;
    assert_eq!(start_st, StatusCode::OK, "assist start failed: {start_val}");
    let relay_tok = start_val["temp_relay_token"].as_str().unwrap().to_string();
    let assist_sid = start_val["session_id"].as_str().unwrap().to_string();
    assert!(relay_tok.starts_with("fbrelay_"));

    let (rs_st, _, rs_val) = h
        .json_req(Method::GET, "/api/v1/remote/sessions", Some(&host_tok), None)
        .await;
    assert_eq!(rs_st, StatusCode::OK);
    assert!(
        rs_val["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["session_id"] == assist_sid)
    );

    // 5. Rate limit: 5 failed code attempts lock out further attempts (HTTP 429 even with valid code)
    for attempt in 1..=5 {
        let (f_st, _, f_val) = h
            .json_req(
                Method::POST,
                "/api/v1/remote/assist/start",
                Some(&helper_tok),
                Some(json!({
                    "assist_id": assist_id,
                    "access_code": "WrongCode9",
                    "mode": "desktop"
                })),
            )
            .await;
        if attempt < 5 {
            assert_eq!(f_st, StatusCode::UNAUTHORIZED, "attempt {attempt}: {f_val}");
        } else {
            assert_eq!(f_st, StatusCode::TOO_MANY_REQUESTS, "attempt 5 should lock: {f_val}");
        }
    }
    let (locked_st, _, _) = h
        .json_req(
            Method::POST,
            "/api/v1/remote/assist/start",
            Some(&helper_tok),
            Some(json!({
                "assist_id": assist_id,
                "access_code": custom_code,
                "mode": "desktop"
            })),
        )
        .await;
    assert_eq!(locked_st, StatusCode::TOO_MANY_REQUESTS);

    // 6. Same-account multi-device WebRTC signaling (and verify legacy user dispatch_frame invariant stays intact)
    let uid = gpui_telegram_core::types::UserId(host_uid);
    let legacy_res = h
        .state
        .signaling_hub
        .dispatch_frame(
            uid,
            SignalingFrame::Invite {
                call_id: "legacy-self-call".to_string(),
                from_user: uid,
                to_user: uid,
                is_video: true,
            },
        )
        .await;
    assert!(legacy_res.is_err(), "legacy dispatch_frame must still forbid same-user calls");

    let (tx_mac, mut rx_mac) = mpsc::channel(16);
    let (tx_phone, mut rx_phone) = mpsc::channel(16);
    h.state
        .signaling_hub
        .register_device_client(uid, "macbook-pro", tx_mac);
    h.state
        .signaling_hub
        .register_device_client(uid, "iphone-16", tx_phone);

    let call_id = "dev-call-1".to_string();
    let (sig1_st, _, sig1_val) = h
        .json_req(
            Method::POST,
            "/api/v1/remote/webrtc/signal",
            Some(&host_tok),
            Some(json!({
                "sender_device_id": "macbook-pro",
                "target_device_id": "iphone-16",
                "frame": SignalingFrame::Invite {
                    call_id: call_id.clone(),
                    from_user: uid,
                    to_user: uid,
                    is_video: true,
                }
            })),
        )
        .await;
    assert_eq!(sig1_st, StatusCode::OK, "device invite failed: {sig1_val}");
    let recv_phone = rx_phone.recv().await.expect("iphone-16 received Invite");
    assert!(matches!(recv_phone, SignalingFrame::Invite { .. }));

    // Ringing -> Accept -> Offer -> Answer -> IceCandidate -> Hangup
    for frame in [
        SignalingFrame::Ringing {
            call_id: call_id.clone(),
        },
        SignalingFrame::Accept {
            call_id: call_id.clone(),
        },
        SignalingFrame::Offer {
            call_id: call_id.clone(),
            sdp: "v=0\r\nm=video".to_string(),
        },
        SignalingFrame::Answer {
            call_id: call_id.clone(),
            sdp: "v=0\r\nm=video answer".to_string(),
        },
        SignalingFrame::IceCandidate {
            call_id: call_id.clone(),
            candidate: "candidate:1 1 UDP 2122252543 10.0.0.2 54400 typ host".to_string(),
            sdp_mid: Some("0".to_string()),
            sdp_mline_index: Some(0),
        },
        SignalingFrame::Hangup {
            call_id: call_id.clone(),
            reason: "completed".to_string(),
        },
    ] {
        let sender_dev = match &frame {
            SignalingFrame::Offer { .. } => "macbook-pro",
            _ => "iphone-16",
        };
        let (sst, _, sval) = h
            .json_req(
                Method::POST,
                "/api/v1/remote/webrtc/signal",
                Some(&host_tok),
                Some(json!({
                    "sender_device_id": sender_dev,
                    "frame": frame
                })),
            )
            .await;
        assert_eq!(sst, StatusCode::OK, "device signal failed: {sval}");
    }
    assert!(matches!(rx_mac.recv().await, Some(SignalingFrame::Ringing { .. })));
    assert!(matches!(rx_mac.recv().await, Some(SignalingFrame::Accept { .. })));
    assert!(matches!(rx_phone.recv().await, Some(SignalingFrame::Offer { .. })));
    assert!(matches!(rx_mac.recv().await, Some(SignalingFrame::Answer { .. })));
    assert!(matches!(rx_mac.recv().await, Some(SignalingFrame::IceCandidate { .. })));
    assert!(matches!(rx_mac.recv().await, Some(SignalingFrame::Hangup { .. })));

    let _ = PowerActionKind::Lock;
    let _ = ServerDatabase::open_in_memory();
}
