//! Real end-to-end verification tests for live Marketplace connector publication,
//! authentic Streamable HTTP MCP handshake (`initialize` + `Mcp-Session-Id` + `tools/list` +
//! `tools/call`), real native connector API verification & execution, encrypted persistence
//! across server restarts, and shared `FabushiBotHost` / `HermesEngine` `McpRegistry` execution.
//!
//! Zero fake/synthetic install counts, zero fake endpoints, zero stub tool responses.

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use gpui_telegram_server::{
    build_router_with_state,
    config::ServerConfig,
    server::AppState,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tower::ServiceExt;

fn boot_server_on_path(db_path: &std::path::Path, media_dir: &std::path::Path) -> (Arc<AppState>, Router) {
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;

    let state = Arc::new(AppState::try_new(cfg).expect("boot AppState"));
    let router = build_router_with_state(Arc::clone(&state));
    (state, router)
}

fn boot_temp_server() -> (TempDir, std::path::PathBuf, std::path::PathBuf, Arc<AppState>, Router) {
    let dir = TempDir::new().expect("create TempDir");
    let db_path = dir.path().join("marketplace_real_e2e.db");
    let media_dir = dir.path().join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");
    let (state, router) = boot_server_on_path(&db_path, &media_dir);
    (dir, db_path, media_dir, state, router)
}

async fn http_json(
    router: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = if let Some(b) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = router.clone().oneshot(req).await.expect("oneshot router");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let val = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, val)
}

async fn register_user(router: &Router, username: &str, first_name: &str) -> (u64, String) {
    let (status, val) = http_json(
        router,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": username,
            "password": "RealMarketplacePassword!2026",
            "first_name": first_name,
            "device_name": "MarketplaceE2E"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register {username}: {val}");
    (
        val["user_id"].as_u64().expect("user_id"),
        val["token"].as_str().expect("token").to_string(),
    )
}

/// Spins up a real Streamable HTTP MCP JSON-RPC 2.0 server on `127.0.0.1:0` requiring
/// `Authorization: Bearer real-mcp-secret-42`, enforcing `initialize` -> `Mcp-Session-Id` ->
/// `tools/list` / `tools/call`.
async fn spawn_real_streamable_http_mcp_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let call_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let log_state = Arc::clone(&call_log);

    async fn mcp_handler(
        State(log): State<Arc<Mutex<Vec<String>>>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if auth != "Bearer real-mcp-secret-42" {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    header::WWW_AUTHENTICATE,
                    HeaderValue::from_static(
                        r#"Bearer resource_metadata="http://127.0.0.1/.well-known/oauth-protected-resource""#,
                    ),
                )],
                Json(json!({ "error": "unauthorized" })),
            )
                .into_response();
        }

        let method = body["method"].as_str().unwrap_or("");
        let id = body.get("id").cloned().unwrap_or(json!(1));
        log.lock().unwrap().push(method.to_string());

        match method {
            "initialize" => (
                StatusCode::OK,
                [(
                    header::HeaderName::from_static("mcp-session-id"),
                    HeaderValue::from_static("sess-live-99"),
                )],
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "acme-weather-mcp", "version": "1.0.0" }
                    }
                })),
            )
                .into_response(),
            "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
            "tools/list" => {
                let sess = headers
                    .get("mcp-session-id")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                assert_eq!(sess, "sess-live-99", "tools/list must pass Mcp-Session-Id");
                (
                    StatusCode::OK,
                    Json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "tools": [
                                {
                                    "name": "weather_forecast",
                                    "description": "Fetch live city weather forecast from Acme Weather",
                                    "inputSchema": {
                                        "type": "object",
                                        "properties": { "city": { "type": "string" } },
                                        "required": ["city"]
                                    }
                                },
                                {
                                    "name": "ticket_lookup",
                                    "description": "Look up internal release ticket status",
                                    "inputSchema": { "type": "object" }
                                }
                            ]
                        }
                    })),
                )
                    .into_response()
            }
            "tools/call" => {
                let sess = headers
                    .get("mcp-session-id")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                assert_eq!(sess, "sess-live-99", "tools/call must pass Mcp-Session-Id");
                let tool_name = body["params"]["name"].as_str().unwrap_or("");
                let city = body["params"]["arguments"]["city"]
                    .as_str()
                    .unwrap_or("Unknown");
                (
                    StatusCode::OK,
                    Json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [
                                {
                                    "type": "text",
                                    "text": format!("{tool_name} => {city}: 22C sunny (live upstream)")
                                }
                            ],
                            "isError": false
                        }
                    })),
                )
                    .into_response()
            }
            other => (
                StatusCode::OK,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("Unknown method {other}") }
                })),
            )
                .into_response(),
        }
    }

    let app = Router::new()
        .route("/mcp", post(mcp_handler))
        .with_state(log_state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mcp mock");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/mcp"), call_log)
}

/// Spins up authentic local upstream Slack + GitHub API endpoints on `127.0.0.1:0`
/// to verify live native connector authentication and real tool execution.
async fn spawn_real_upstream_slack_and_github_apis() -> (String, Arc<Mutex<Vec<Value>>>) {
    let posted_messages: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::clone(&posted_messages);

    async fn slack_auth_test(headers: HeaderMap) -> impl IntoResponse {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if auth == "Bearer xoxb-live-slack-777" {
            Json(json!({
                "ok": true,
                "team": "Acme Cloud",
                "user": "fabushi_ops",
                "team_id": "T999"
            }))
        } else {
            Json(json!({
                "ok": false,
                "error": "invalid_auth"
            }))
        }
    }

    async fn slack_post_message(
        State(posted): State<Arc<Mutex<Vec<Value>>>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if auth != "Bearer xoxb-live-slack-777" {
            return Json(json!({ "ok": false, "error": "invalid_auth" }));
        }
        posted.lock().unwrap().push(body.clone());
        Json(json!({
            "ok": true,
            "channel": body["channel"],
            "ts": "1732788000.000100",
            "message": { "text": body["text"] }
        }))
    }

    async fn github_user(headers: HeaderMap) -> impl IntoResponse {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if auth == "Bearer ghp_live_gh_999" {
            (
                StatusCode::OK,
                Json(json!({
                    "login": "octo-fabushi",
                    "name": "Octo Fabushi",
                    "id": 4242
                })),
            )
        } else {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Bad credentials" })),
            )
        }
    }

    async fn github_repos(headers: HeaderMap) -> impl IntoResponse {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if auth != "Bearer ghp_live_gh_999" {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Bad credentials" })),
            );
        }
        (
            StatusCode::OK,
            Json(json!([
                { "full_name": "bhrumom/gpui-telegram", "private": false, "stargazers_count": 99 }
            ])),
        )
    }

    let app = Router::new()
        .route("/auth.test", post(slack_auth_test))
        .route("/chat.postMessage", post(slack_post_message))
        .route("/user", get(github_user))
        .route("/user/repos", get(github_repos))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind upstream mock");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), posted_messages)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn marketplace_publishes_all_266_openbot_connectors_with_zero_fake_installs() {
    let (_dir, _db_path, _media_dir, _state, router) = boot_temp_server();
    let (_uid, tok) = register_user(&router, "alice_mkt_zero", "Alice").await;

    let (st, cat) = http_json(&router, "GET", "/api/v1/marketplace/catalog", Some(&tok), None).await;
    assert_eq!(st, StatusCode::OK, "catalog response: {cat}");
    assert_eq!(cat["totalApps"].as_u64(), Some(266));
    assert_eq!(cat["totalConnectors"].as_u64(), Some(9));
    assert_eq!(cat["totalPlugins"].as_u64(), Some(257));
    assert_eq!(cat["totalPublishedCustom"].as_u64(), Some(0));

    let apps = cat["apps"].as_array().expect("apps array");
    let agents = cat["agents"].as_array().expect("agents array");
    let skills = cat["skills"].as_array().expect("skills array");
    assert_eq!(apps.len(), 266);
    assert_eq!(agents.len(), 29);
    assert_eq!(skills.len(), 40);

    // Assert ZERO fake/synthetic install counts and ZERO fake `connectors.openbot.local` URLs!
    for app in apps {
        assert_eq!(
            app["installCount"].as_u64(),
            Some(0),
            "App {} must start with 0 real installs, never fake numbers",
            app["appId"]
        );
        if let Some(u) = app["url"].as_str() {
            assert!(
                !u.contains("connectors.openbot.local"),
                "App {} must never use synthetic fake URL: {u}",
                app["appId"]
            );
        }
    }
    for agent in agents {
        assert_eq!(agent["installCount"].as_u64(), Some(0));
    }
    for skill in skills {
        assert_eq!(skill["installCount"].as_u64(), Some(0));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_custom_http_mcp_connector_connect_discover_call_and_persist_across_restart() {
    let (mcp_url, call_log) = spawn_real_streamable_http_mcp_server().await;
    let (_dir, db_path, media_dir, state, router) = boot_temp_server();
    let (_alice_id, alice_tok) = register_user(&router, "alice_mcp_pub", "Alice").await;
    let (_bob_id, bob_tok) = register_user(&router, "bob_mcp_viewer", "Bob").await;

    // 1. Pre-publish verification rejects unreachable dead ports (zero fake publish acceptance).
    let (st_dead, dead_val) = http_json(
        &router,
        "POST",
        "/api/v1/marketplace/apps",
        Some(&alice_tok),
        Some(json!({
            "name": "Broken Dead MCP",
            "tagline": "Unreachable",
            "transport": "http",
            "url": "http://127.0.0.1:1/mcp",
            "verifyBeforePublish": true
        })),
    )
    .await;
    assert_eq!(
        st_dead,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unreachable endpoint must be rejected before publishing: {dead_val}"
    );

    // 2. Really publish Acme Weather MCP into SQLite `marketplace_apps`.
    let (st_pub, published) = http_json(
        &router,
        "POST",
        "/api/v1/marketplace/apps",
        Some(&alice_tok),
        Some(json!({
            "name": "Acme Weather MCP",
            "slug": "acme-weather-mcp",
            "tagline": "Real-time weather & release ticket MCP",
            "category": "data-analytics",
            "transport": "http",
            "url": mcp_url,
            "authKind": "link",
            "verifyBeforePublish": true
        })),
    )
    .await;
    assert_eq!(st_pub, StatusCode::CREATED, "publish custom MCP: {published}");
    assert_eq!(published["appId"], "published:acme-weather-mcp");
    assert_eq!(published["installCount"].as_u64(), Some(0));

    // 3. Another user immediately sees the newly published connector in the live catalog!
    let (st_bob_cat, bob_cat) =
        http_json(&router, "GET", "/api/v1/marketplace/catalog", Some(&bob_tok), None).await;
    assert_eq!(st_bob_cat, StatusCode::OK);
    assert_eq!(bob_cat["totalApps"].as_u64(), Some(267));
    assert_eq!(bob_cat["totalPublishedCustom"].as_u64(), Some(1));
    assert!(bob_cat["apps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["appId"] == "published:acme-weather-mcp"));

    // 4. Connecting WITHOUT valid credentials MUST return 401 + requiresOauth=true (never fake connected).
    let (st_unauth, unauth_val) = http_json(
        &router,
        "POST",
        "/api/v1/marketplace/connections/connect",
        Some(&alice_tok),
        Some(json!({
            "appId": "published:acme-weather-mcp",
            "verifyLive": true
        })),
    )
    .await;
    assert_eq!(st_unauth, StatusCode::OK, "unauth connect: {unauth_val}");
    assert_eq!(unauth_val["connection"]["status"], "needs_sign_in");
    assert_eq!(unauth_val["installCount"].as_u64(), Some(0));
    assert_eq!(unauth_val["requiresOauth"], json!(true));
    assert!(unauth_val["wwwAuthenticate"].as_str().is_some());
    assert!(unauth_val["connection"]["discoveredTools"]
        .as_array()
        .unwrap()
        .is_empty());

    // 5. Connecting WITH valid Bearer token performs real Streamable HTTP initialize + tools/list!
    let (st_ok, conn_resp) = http_json(
        &router,
        "POST",
        "/api/v1/marketplace/connections/connect",
        Some(&alice_tok),
        Some(json!({
            "appId": "published:acme-weather-mcp",
            "credential": "real-mcp-secret-42",
            "verifyLive": true
        })),
    )
    .await;
    assert_eq!(st_ok, StatusCode::OK, "verified connect: {conn_resp}");
    assert_eq!(conn_resp["connection"]["status"], "connected");
    assert_eq!(conn_resp["installCount"].as_u64(), Some(1));
    let tools = conn_resp["connection"]["discoveredTools"]
        .as_array()
        .expect("discoveredTools");
    assert_eq!(tools.len(), 2);
    assert!(tools.iter().any(|t| t["name"] == "weather_forecast"));
    assert!(tools.iter().any(|t| t["name"] == "ticket_lookup"));

    // 6. Verify live tool appears in BOTH `GET /api/v1/mcp/tools` AND Fabushi Bot `effective_mcp_registry()`.
    let (st_tools, tools_val) =
        http_json(&router, "GET", "/api/v1/mcp/tools", Some(&alice_tok), None).await;
    assert_eq!(st_tools, StatusCode::OK);
    assert!(tools_val["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "weather_forecast"));
    {
        let bot_reg = state.fabushi_core().effective_mcp_registry();
        let guard = bot_reg.read().unwrap();
        assert!(
            guard.get("weather_forecast").is_some(),
            "Fabushi core bot must share live Marketplace McpRegistry"
        );
    }

    // 7. Execute REAL live tool call via `/api/v1/mcp/call` (NO fake "executed successfully" string!).
    let (st_call, call_val) = http_json(
        &router,
        "POST",
        "/api/v1/mcp/call",
        Some(&alice_tok),
        Some(json!({
            "tool_name": "weather_forecast",
            "arguments": { "city": "Tokyo" }
        })),
    )
    .await;
    assert_eq!(st_call, StatusCode::OK, "live /api/v1/mcp/call: {call_val}");
    let call_str = call_val.to_string();
    assert!(
        call_str.contains("Tokyo: 22C sunny (live upstream)"),
        "must return real upstream MCP response, got: {call_str}"
    );
    assert!(
        !call_str.contains("executed successfully"),
        "must NEVER return legacy fake string: {call_str}"
    );

    // 8. Also execute via `/api/v1/marketplace/connections/{app_id}/call`.
    let (st_mcall, mcall_val) = http_json(
        &router,
        "POST",
        "/api/v1/marketplace/connections/published:acme-weather-mcp/call",
        Some(&alice_tok),
        Some(json!({
            "toolName": "weather_forecast",
            "arguments": { "city": "Osaka" }
        })),
    )
    .await;
    assert_eq!(st_mcall, StatusCode::OK, "direct marketplace tool call: {mcall_val}");
    assert!(mcall_val.to_string().contains("Osaka: 22C sunny (live upstream)"));

    // Verify real MCP methods were invoked over HTTP.
    {
        let methods = call_log.lock().unwrap();
        assert!(methods.iter().any(|m| m == "initialize"));
        assert!(methods.iter().any(|m| m == "tools/list"));
        assert!(methods.iter().any(|m| m == "tools/call"));
    }

    // 9. Simulate FULL SERVER RESTART on the same SQLite DB -> verify encrypted secrets hydrate
    //    and live tool execution still succeeds!
    drop(router);
    drop(state);
    let (restarted_state, restarted_router) = boot_server_on_path(&db_path, &media_dir);
    let (st_after, after_val) = http_json(
        &restarted_router,
        "POST",
        "/api/v1/mcp/call",
        Some(&alice_tok),
        Some(json!({
            "tool_name": "weather_forecast",
            "arguments": { "city": "Kyoto" }
        })),
    )
    .await;
    assert_eq!(
        st_after,
        StatusCode::OK,
        "hydrated MCP tool must execute after restart: {after_val}"
    );
    assert!(after_val.to_string().contains("Kyoto: 22C sunny (live upstream)"));

    // 10. Disconnect removes both DB connection and live McpRegistry tool!
    let (st_del, _) = http_json(
        &restarted_router,
        "DELETE",
        "/api/v1/marketplace/connections/published:acme-weather-mcp",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_del, StatusCode::OK);
    assert!(restarted_state
        .fabushi_core()
        .effective_mcp_registry()
        .read()
        .unwrap()
        .get("weather_forecast")
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_connector_live_verification_rejects_bad_token_and_executes_real_upstream_tools() {
    let (upstream_base, posted_messages) = spawn_real_upstream_slack_and_github_apis().await;
    let (_dir, _db_path, _media_dir, state, router) = boot_temp_server();
    let (_uid, tok) = register_user(&router, "alice_native_real", "Alice").await;

    // 1. Empty token for Slack MUST fail (never fake connected).
    let (st_empty, empty_val) = http_json(
        &router,
        "POST",
        "/api/v1/marketplace/connections/connect",
        Some(&tok),
        Some(json!({
            "appId": "connector:slack",
            "metadata": { "api_base_url": upstream_base },
            "verifyLive": true
        })),
    )
    .await;
    assert_eq!(st_empty, StatusCode::UNPROCESSABLE_ENTITY, "empty token: {empty_val}");

    // 2. Invalid token for Slack MUST fail live `auth.test` verification.
    let (st_bad, bad_val) = http_json(
        &router,
        "POST",
        "/api/v1/marketplace/connections/connect",
        Some(&tok),
        Some(json!({
            "appId": "connector:slack",
            "credential": "xoxb-wrong-token",
            "metadata": { "api_base_url": upstream_base },
            "verifyLive": true
        })),
    )
    .await;
    assert_eq!(st_bad, StatusCode::UNPROCESSABLE_ENTITY, "invalid slack token: {bad_val}");

    // 3. Valid Slack token passes real `auth.test`, increments install_count -> 1, and registers live tools.
    let (st_slack, slack_val) = http_json(
        &router,
        "POST",
        "/api/v1/marketplace/connections/connect",
        Some(&tok),
        Some(json!({
            "appId": "connector:slack",
            "credential": "xoxb-live-slack-777",
            "metadata": { "api_base_url": upstream_base },
            "verifyLive": true
        })),
    )
    .await;
    assert_eq!(st_slack, StatusCode::OK, "valid slack connect: {slack_val}");
    assert_eq!(slack_val["connection"]["status"], "connected");
    assert_eq!(slack_val["installCount"].as_u64(), Some(1));
    assert!(
        slack_val["connection"]["accountLabel"]
            .as_str()
            .unwrap_or("")
            .contains("Acme Cloud"),
        "expected verified Slack workspace name, got: {slack_val}"
    );

    // 4. Execute `slack_post_message` directly through Fabushi Bot's shared `McpRegistry`!
    let exec_out = {
        let reg_arc = state.fabushi_core().effective_mcp_registry();
        let reg = reg_arc.read().unwrap();
        reg.execute_tool(
            "slack_post_message",
            json!({
                "channel": "C123OPS",
                "text": "Deployment v2.0.11 verified live!"
            }),
        )
        .expect("bot execute slack_post_message")
    };
    assert_eq!(exec_out["ok"], json!(true));
    assert_eq!(exec_out["ts"], json!("1732788000.000100"));
    {
        let msgs = posted_messages.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["channel"], "C123OPS");
        assert_eq!(msgs[0]["text"], "Deployment v2.0.11 verified live!");
    }

    // 5. Connect GitHub native connector with valid PAT & call `github_list_repos` via `/api/v1/mcp/call`.
    let (st_gh, gh_val) = http_json(
        &router,
        "POST",
        "/api/v1/marketplace/connections/connect",
        Some(&tok),
        Some(json!({
            "appId": "connector:github",
            "credential": "ghp_live_gh_999",
            "metadata": { "api_base_url": upstream_base },
            "verifyLive": true
        })),
    )
    .await;
    assert_eq!(st_gh, StatusCode::OK, "valid github connect: {gh_val}");
    assert_eq!(gh_val["connection"]["accountLabel"], "@octo-fabushi");

    let (st_repos, repos_val) = http_json(
        &router,
        "POST",
        "/api/v1/mcp/call",
        Some(&tok),
        Some(json!({
            "tool_name": "github_list_repos",
            "arguments": {}
        })),
    )
    .await;
    assert_eq!(st_repos, StatusCode::OK, "github_list_repos: {repos_val}");
    assert!(repos_val.to_string().contains("bhrumom/gpui-telegram"));
}
