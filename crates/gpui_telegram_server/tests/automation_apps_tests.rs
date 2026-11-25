use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use bot_browser::script_app::{
    AutomationScriptApp, ScriptParameter, ScriptStep, SemanticLocator, SCRIPT_APP_SCHEMA_VERSION,
};
use gpui_telegram_client::{
    ClientConfig, HttpClientError, LoginRequest, PublishAutomationAppRequest,
    RecordAutomationRunRequest, RegisterRequest, TelegramClient,
};
use gpui_telegram_proto::automation_apps::{
    compute_automation_manifest_sha256, AutomationAppDetail, InstallAutomationAppResponse,
    ListAutomationAppsResponse, ListInstalledAutomationAppsResponse, RecordAutomationRunResponse,
    UninstallAutomationAppResponse,
};
use gpui_telegram_server::{build_router_with_state, config::ServerConfig, server::AppState};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tower::ServiceExt;

fn boot_server_in_dir(dir: &TempDir) -> (ServerConfig, Arc<AppState>, Router) {
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", dir.path().join("server.db").display());
    cfg.media_storage_path = dir.path().join("cas").to_string_lossy().into_owned();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;
    let state = Arc::new(AppState::try_new(cfg.clone()).expect("boot AppState"));
    let router = build_router_with_state(state.clone());
    (cfg, state, router)
}

async fn api(
    router: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let res = router.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, val)
}

async fn register_user(router: &Router, username: &str) -> (u64, String) {
    let (status, val) = api(
        router,
        Method::POST,
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": username,
            "password": "StrongPassword!123",
            "first_name": username,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register failed: {val}");
    (
        val["user_id"].as_u64().unwrap(),
        val["token"].as_str().unwrap().to_string(),
    )
}

fn build_clean_script_app(
    app_id: &str,
    version: &str,
    category: &str,
    domains: Vec<&str>,
    author_id: Option<String>,
) -> AutomationScriptApp {
    AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: app_id.to_string(),
        slug: app_id.to_ascii_lowercase().replace('_', "-"),
        name: format!("App {app_id}"),
        version: version.to_string(),
        description: "Basic automation workflow for clean testing".to_string(),
        category: category.to_string(),
        target_domains: domains.into_iter().map(String::from).collect(),
        parameters: vec![
            ScriptParameter {
                name: "query".to_string(),
                description: "Search term".to_string(),
                required: true,
                default_value: Some("rust".to_string()),
                is_secret: false,
            },
            ScriptParameter {
                name: "auth_token".to_string(),
                description: "User vault token".to_string(),
                required: true,
                default_value: None,
                is_secret: true,
            },
        ],
        steps: vec![
            ScriptStep::Navigate {
                url_template: "https://x.com/explore".to_string(),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator {
                    test_id: Some("searchBox".to_string()),
                    ..Default::default()
                },
                timeout_ms: 3000,
            },
            ScriptStep::Fill {
                locator: SemanticLocator {
                    test_id: Some("searchBox".to_string()),
                    ..Default::default()
                },
                value_template: "{{query}}".to_string(),
                secret_vault_key: None,
            },
            ScriptStep::Fill {
                locator: SemanticLocator {
                    name_attr: Some("password".to_string()),
                    ..Default::default()
                },
                value_template: String::new(),
                secret_vault_key: Some("vault://x.com/default".to_string()),
            },
            ScriptStep::Scroll {
                delta_x: 0.0,
                delta_y: 250.5,
            },
        ],
        required_capabilities: vec!["browser".to_string()],
        author_id,
        source_trace_summary: Some("Recorded explore workflow".to_string()),
    }
}

#[tokio::test]
async fn test_v41_schema_and_auth_guards() {
    let dir = TempDir::new().unwrap();
    let (_cfg, state, router) = boot_server_in_dir(&dir);

    // 1. Verify migration v41 & all 4 tables exist in SQLite
    state
        .db
        .pool()
        .with_reader(|conn| {
            let has_v41: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM _schema_migrations WHERE version = 41)",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(has_v41, "migration v41 must be recorded");
            for tbl in [
                "automation_apps",
                "automation_app_versions",
                "user_automation_apps",
                "automation_app_runs",
            ] {
                let exists: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                        [tbl],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert!(exists, "table {tbl} must exist");
            }
            Ok(())
        })
        .unwrap();

    // 2. Auth guards (401 on protected endpoints)
    let clean_app = build_clean_script_app("x-search-clean", "1.0.0", "social", vec!["x.com"], None);
    let publish_body = json!({
        "manifest": serde_json::to_value(&clean_app).unwrap(),
        "is_published": true
    });
    for (method, uri, body) in [
        (Method::POST, "/api/v1/automation-apps", Some(publish_body.clone())),
        (Method::GET, "/api/v1/automation-apps/installed", None),
        (Method::POST, "/api/v1/automation-apps/x-search-clean/install", None),
        (Method::DELETE, "/api/v1/automation-apps/x-search-clean/install", None),
        (
            Method::POST,
            "/api/v1/automation-apps/x-search-clean/runs",
            Some(json!({ "success": true, "steps_executed": 1, "total_duration_ms": 10 })),
        ),
    ] {
        let (st, val) = api(&router, method, uri, None, body).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "expected 401 for {uri}: {val}");
    }

    // 3. Anonymous catalog GET works (200) for published apps, and 404 for unpublished apps
    let (_uid, tok) = register_user(&router, "alice_guard").await;
    let (st_pub, _) = api(&router, Method::POST, "/api/v1/automation-apps", Some(&tok), Some(publish_body)).await;
    assert_eq!(st_pub, StatusCode::CREATED);

    let priv_app = build_clean_script_app("private-draft", "1.0.0", "general", vec!["example.com"], None);
    let (st_priv, _) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&tok),
        Some(json!({
            "manifest": serde_json::to_value(&priv_app).unwrap(),
            "is_published": false
        })),
    )
    .await;
    assert_eq!(st_priv, StatusCode::CREATED);

    let (st_list, list_val) = api(&router, Method::GET, "/api/v1/automation-apps", None, None).await;
    assert_eq!(st_list, StatusCode::OK);
    let list: ListAutomationAppsResponse = serde_json::from_value(list_val).unwrap();
    assert_eq!(list.total, 1);
    assert_eq!(list.apps[0].app_id, "x-search-clean");

    let (st_get_pub, _) = api(&router, Method::GET, "/api/v1/automation-apps/x-search-clean", None, None).await;
    assert_eq!(st_get_pub, StatusCode::OK);

    // Unpublished app hidden from anonymous and other users, visible to author
    let (st_get_priv_anon, _) = api(&router, Method::GET, "/api/v1/automation-apps/private-draft", None, None).await;
    assert_eq!(st_get_priv_anon, StatusCode::NOT_FOUND);
    let (_uid2, tok2) = register_user(&router, "bob_guard").await;
    let (st_get_priv_bob, _) = api(&router, Method::GET, "/api/v1/automation-apps/private-draft", Some(&tok2), None).await;
    assert_eq!(st_get_priv_bob, StatusCode::NOT_FOUND);
    let (st_get_priv_alice, _) = api(&router, Method::GET, "/api/v1/automation-apps/private-draft", Some(&tok), None).await;
    assert_eq!(st_get_priv_alice, StatusCode::OK);
}

#[tokio::test]
async fn test_secret_leak_rejections_and_app_id_validation() {
    let dir = TempDir::new().unwrap();
    let (_cfg, state, router) = boot_server_in_dir(&dir);
    let (_uid, tok) = register_user(&router, "sec_tester").await;

    let base_manifest = serde_json::to_value(build_clean_script_app(
        "leak-check",
        "1.0.0",
        "social",
        vec!["x.com"],
        None,
    ))
    .unwrap();

    // Case 1: auth_token=... in description
    let mut m1 = base_manifest.clone();
    m1["description"] = json!("leaked auth_token=super_secret_cookie_123");
    let (st1, v1) = api(&router, Method::POST, "/api/v1/automation-apps", Some(&tok), Some(json!({ "manifest": m1 }))).await;
    assert_eq!(st1, StatusCode::UNPROCESSABLE_ENTITY, "auth_token leak: {v1}");
    assert_eq!(v1["error"], "secret_leak_detected");

    // Case 2: ct0=... in url_template
    let mut m2 = base_manifest.clone();
    m2["steps"][0]["url_template"] = json!("https://x.com/home?ct0=deadbeef123456");
    let (st2, v2) = api(&router, Method::POST, "/api/v1/automation-apps", Some(&tok), Some(json!({ "manifest": m2 }))).await;
    assert_eq!(st2, StatusCode::UNPROCESSABLE_ENTITY, "ct0 leak: {v2}");

    // Case 3: Authorization: Bearer ...
    let mut m3 = base_manifest.clone();
    m3["source_trace_summary"] = json!("Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abc123def");
    let (st3, v3) = api(&router, Method::POST, "/api/v1/automation-apps", Some(&tok), Some(json!({ "manifest": m3 }))).await;
    assert_eq!(st3, StatusCode::UNPROCESSABLE_ENTITY, "Bearer leak: {v3}");

    // Case 4: Standalone Basic <b64> credential (not preceded by Authorization:)
    let mut m4 = base_manifest.clone();
    m4["description"] = json!("Use header Basic dXNlcjpwYXNz to log in");
    let (st4, v4) = api(&router, Method::POST, "/api/v1/automation-apps", Some(&tok), Some(json!({ "manifest": m4 }))).await;
    assert_eq!(st4, StatusCode::UNPROCESSABLE_ENTITY, "Standalone Basic leak: {v4}");

    // Case 5: Raw password=... in Fill
    let mut m5 = base_manifest.clone();
    m5["steps"][2]["value_template"] = json!("password=MyRawPassword99!");
    let (st5, v5) = api(&router, Method::POST, "/api/v1/automation-apps", Some(&tok), Some(json!({ "manifest": m5 }))).await;
    assert_eq!(st5, StatusCode::UNPROCESSABLE_ENTITY, "password= leak: {v5}");

    // Case 6: Secret leak inside changelog
    let (st6, v6) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&tok),
        Some(json!({
            "manifest": base_manifest.clone(),
            "changelog": "Tested with sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz"
        })),
    )
    .await;
    assert_eq!(st6, StatusCode::UNPROCESSABLE_ENTITY, "changelog leak: {v6}");

    // Case 7: Secret leak inside unknown extra field in manifest
    let mut m7 = base_manifest.clone();
    m7["extra_debug_headers"] = json!({ "cookie": "auth_token=rawcookievalue" });
    let (st7, v7) = api(&router, Method::POST, "/api/v1/automation-apps", Some(&tok), Some(json!({ "manifest": m7 }))).await;
    assert_eq!(st7, StatusCode::UNPROCESSABLE_ENTITY, "unknown field leak: {v7}");

    // Case 8: is_secret=true parameter carrying default_value
    let mut m8 = base_manifest.clone();
    m8["parameters"][1]["default_value"] = json!("hardcoded_token");
    let (st8, v8) = api(&router, Method::POST, "/api/v1/automation-apps", Some(&tok), Some(json!({ "manifest": m8 }))).await;
    assert_eq!(st8, StatusCode::UNPROCESSABLE_ENTITY, "secret param default_value: {v8}");

    // Case 9: Sensitive Fill without secret_vault_key or protected placeholder
    let mut m9 = base_manifest.clone();
    m9["steps"][3] = json!({
        "kind": "fill",
        "locator": { "name_attr": "password" },
        "value_template": "raw_password_literal"
    });
    let (st9, v9) = api(&router, Method::POST, "/api/v1/automation-apps", Some(&tok), Some(json!({ "manifest": m9 }))).await;
    assert_eq!(st9, StatusCode::UNPROCESSABLE_ENTITY, "sensitive Fill leak: {v9}");

    // Case 10: Reserved app_id == "installed" or invalid chars
    let mut m10 = base_manifest.clone();
    m10["app_id"] = json!("installed");
    let (st10, v10) = api(&router, Method::POST, "/api/v1/automation-apps", Some(&tok), Some(json!({ "manifest": m10 }))).await;
    assert_eq!(st10, StatusCode::BAD_REQUEST, "app_id == installed: {v10}");
    assert_eq!(v10["error"], "invalid_app_id");

    // Verify ZERO rows persisted after all rejections
    let count: i64 = state
        .db
        .pool()
        .with_reader(|conn| {
            conn.query_row("SELECT COUNT(*) FROM automation_apps", [], |r| r.get(0))
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert_eq!(count, 0, "no rejected apps should be persisted");
}

#[tokio::test]
async fn test_ownership_version_conflict_search_install_and_run_telemetry() {
    let dir = TempDir::new().unwrap();
    let (_cfg, state, router) = boot_server_in_dir(&dir);
    let (alice_id, alice_tok) = register_user(&router, "alice_store").await;
    let (bob_id, bob_tok) = register_user(&router, "bob_store").await;

    // 1. Author spoofing rejected (403)
    let spoofed = build_clean_script_app(
        "alice-x-bot",
        "1.0.0",
        "social",
        vec!["x.com"],
        Some(bob_id.to_string()),
    );
    let (st_spoof, _) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": serde_json::to_value(&spoofed).unwrap() })),
    )
    .await;
    assert_eq!(st_spoof, StatusCode::FORBIDDEN);

    // 2. Alice publishes v1.0.0 (201) with omitted author_id -> server stamps Alice's user_id
    let app_v1 = build_clean_script_app("alice-x-bot", "1.0.0", "social", vec!["x.com", "api.x.com"], None);
    let (st_v1, val_v1) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": serde_json::to_value(&app_v1).unwrap() })),
    )
    .await;
    assert_eq!(st_v1, StatusCode::CREATED);
    let detail_v1: AutomationAppDetail = serde_json::from_value(val_v1).unwrap();
    assert_eq!(detail_v1.author_user_id, alice_id);
    assert_eq!(detail_v1.author_username.as_deref(), Some("alice_store"));
    assert_eq!(
        detail_v1.manifest["author_id"].as_str(),
        Some(alice_id.to_string().as_str())
    );
    let expected_sha = compute_automation_manifest_sha256(
        &serde_json::to_string(&detail_v1.manifest).unwrap(),
    );
    assert_eq!(detail_v1.content_sha256, expected_sha);

    // 3. Duplicate version -> 409 Conflict
    let (st_dup, v_dup) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": serde_json::to_value(&app_v1).unwrap() })),
    )
    .await;
    assert_eq!(st_dup, StatusCode::CONFLICT, "dup version: {v_dup}");

    // 4. Bob tries to publish v1.1.0 under Alice's app_id -> 403 Forbidden
    let bob_hijack = build_clean_script_app("alice-x-bot", "1.1.0", "social", vec!["x.com"], None);
    let (st_hijack, _) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&bob_tok),
        Some(json!({ "manifest": serde_json::to_value(&bob_hijack).unwrap() })),
    )
    .await;
    assert_eq!(st_hijack, StatusCode::FORBIDDEN);

    // 5. Alice publishes v1.1.0 -> 200 OK
    let app_v1_1 = build_clean_script_app("alice-x-bot", "1.1.0", "social", vec!["api.x.com"], None);
    let (st_v1_1, val_v1_1) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": serde_json::to_value(&app_v1_1).unwrap() })),
    )
    .await;
    assert_eq!(st_v1_1, StatusCode::OK);
    let detail_v1_1: AutomationAppDetail = serde_json::from_value(val_v1_1).unwrap();
    assert_eq!(detail_v1_1.latest_version, "1.1.0");
    assert_eq!(detail_v1_1.versions, vec!["1.1.0", "1.0.0"]);

    // Publish another app on myx.com to verify exact/subdomain domain matching
    let myx_app = build_clean_script_app("myx-helper", "1.0.0", "productivity", vec!["myx.com"], None);
    let (st_myx, _) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&bob_tok),
        Some(json!({ "manifest": serde_json::to_value(&myx_app).unwrap() })),
    )
    .await;
    assert_eq!(st_myx, StatusCode::CREATED);

    // 6. Search, domain matching (`x.com` matches `api.x.com` but NOT `myx.com`), SQLi safety & clamped pagination
    let (st_dom, val_dom) = api(&router, Method::GET, "/api/v1/automation-apps?domain=x.com&limit=999", Some(&bob_tok), None).await;
    assert_eq!(st_dom, StatusCode::OK);
    let res_dom: ListAutomationAppsResponse = serde_json::from_value(val_dom).unwrap();
    assert_eq!(res_dom.limit, 100); // clamped from 999 -> 100
    assert_eq!(res_dom.total, 1);
    assert_eq!(res_dom.apps[0].app_id, "alice-x-bot");

    let (st_lim0, val_lim0) = api(&router, Method::GET, "/api/v1/automation-apps?limit=0", None, None).await;
    assert_eq!(st_lim0, StatusCode::OK);
    let res_lim0: ListAutomationAppsResponse = serde_json::from_value(val_lim0).unwrap();
    assert_eq!(res_lim0.limit, 1); // clamped from 0 -> 1
    assert_eq!(res_lim0.apps.len(), 1);
    assert_eq!(res_lim0.total, 2);

    let (st_sqli, val_sqli) = api(
        &router,
        Method::GET,
        "/api/v1/automation-apps?q=%27%20OR%201%3D1%20--",
        None,
        None,
    )
    .await;
    assert_eq!(st_sqli, StatusCode::OK);
    let res_sqli: ListAutomationAppsResponse = serde_json::from_value(val_sqli).unwrap();
    assert_eq!(res_sqli.total, 0);

    // 7. Missing app 404s
    for (method, uri, body) in [
        (Method::GET, "/api/v1/automation-apps/does-not-exist", None),
        (Method::POST, "/api/v1/automation-apps/does-not-exist/install", None),
        (Method::DELETE, "/api/v1/automation-apps/does-not-exist/install", None),
        (
            Method::POST,
            "/api/v1/automation-apps/does-not-exist/runs",
            Some(json!({ "success": true, "steps_executed": 1, "total_duration_ms": 5 })),
        ),
    ] {
        let (st, _) = api(&router, method, uri, Some(&bob_tok), body).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "expected 404 on {uri}");
    }

    // 8. Atomic install / idempotent re-install / uninstall
    let (st_i1, v_i1) = api(&router, Method::POST, "/api/v1/automation-apps/alice-x-bot/install", Some(&bob_tok), None).await;
    assert_eq!(st_i1, StatusCode::OK);
    let i1: InstallAutomationAppResponse = serde_json::from_value(v_i1).unwrap();
    assert!(i1.newly_installed);
    assert_eq!(i1.install_count, 1);

    let (st_i2, v_i2) = api(&router, Method::POST, "/api/v1/automation-apps/alice-x-bot/install", Some(&bob_tok), None).await;
    assert_eq!(st_i2, StatusCode::OK);
    let i2: InstallAutomationAppResponse = serde_json::from_value(v_i2).unwrap();
    assert!(!i2.newly_installed);
    assert_eq!(i2.install_count, 1); // no double count!

    let (st_i3, v_i3) = api(&router, Method::POST, "/api/v1/automation-apps/alice-x-bot/install", Some(&alice_tok), None).await;
    assert_eq!(st_i3, StatusCode::OK);
    let i3: InstallAutomationAppResponse = serde_json::from_value(v_i3).unwrap();
    assert!(i3.newly_installed);
    assert_eq!(i3.install_count, 2);

    // 9. Run telemetry -> increments run_count, updates last_run_at_ms, persists automation_app_runs row
    let (st_run, v_run) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps/alice-x-bot/runs",
        Some(&bob_tok),
        Some(json!({
            "version": "1.1.0",
            "success": true,
            "steps_executed": 5,
            "total_duration_ms": 640,
            "used_llm": false
        })),
    )
    .await;
    assert_eq!(st_run, StatusCode::OK);
    let run_resp: RecordAutomationRunResponse = serde_json::from_value(v_run).unwrap();
    assert_eq!(run_resp.run_count, 1);
    assert!(run_resp.last_run_at_ms > 0);

    let runs = state.db.list_automation_app_runs("alice-x-bot", 10).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].user_id, bob_id as i64);
    assert_eq!(runs[0].version, "1.1.0");
    assert!(runs[0].success);
    assert_eq!(runs[0].steps_executed, 5);
    assert_eq!(runs[0].total_duration_ms, 640);
    assert!(!runs[0].used_llm);

    let (st_inst_list, v_inst_list) = api(&router, Method::GET, "/api/v1/automation-apps/installed", Some(&bob_tok), None).await;
    assert_eq!(st_inst_list, StatusCode::OK);
    let inst_list: ListInstalledAutomationAppsResponse = serde_json::from_value(v_inst_list).unwrap();
    assert_eq!(inst_list.total, 1);
    assert_eq!(inst_list.apps[0].last_run_at_ms, Some(run_resp.last_run_at_ms));
    assert_eq!(inst_list.apps[0].local_run_count, 1);

    // Bob uninstalls -> install_count drops from 2 to 1 and removed from Bob's /installed
    let (st_un, v_un) = api(&router, Method::DELETE, "/api/v1/automation-apps/alice-x-bot/install", Some(&bob_tok), None).await;
    assert_eq!(st_un, StatusCode::OK);
    let un: UninstallAutomationAppResponse = serde_json::from_value(v_un).unwrap();
    assert!(un.removed);

    let (_, v_det_after) = api(&router, Method::GET, "/api/v1/automation-apps/alice-x-bot", Some(&alice_tok), None).await;
    let det_after: AutomationAppDetail = serde_json::from_value(v_det_after).unwrap();
    assert_eq!(det_after.install_count, 1);
}

#[tokio::test]
async fn test_real_tcp_server_restart_and_cross_client_sync_e2e() {
    let server_dir = TempDir::new().unwrap();
    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();

    // 1. Boot real TCP server #1
    let (cfg, state1, router1) = boot_server_in_dir(&server_dir);
    let listener1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr1 = listener1.local_addr().unwrap();
    let (shutdown_tx1, shutdown_rx1) = tokio::sync::oneshot::channel::<()>();
    let mut srv_handle1 = tokio::spawn(async move {
        let _ = axum::serve(listener1, router1)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx1.await;
            })
            .await;
    });

    let client_a = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: format!("http://{addr1}"),
            ..Default::default()
        },
        alice_dir.path(),
    )
    .unwrap();
    let alice_auth = client_a
        .register(&RegisterRequest {
            username: "e2e_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .unwrap();
    client_a.set_auth_session(alice_auth.clone());

    let app = build_clean_script_app("e2e-x-poster", "1.0.0", "social", vec!["x.com"], None);
    let published = client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&app).unwrap(),
            is_published: true,
            changelog: Some("Initial E2E release".into()),
        })
        .await
        .unwrap();
    assert_eq!(published.app_id, "e2e-x-poster");

    // Verify secret rejection via real TelegramClient
    let mut bad_app = app.clone();
    bad_app.description = "Basic dXNlcjpwYXNz".to_string();
    let leak_err = client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&bad_app).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(leak_err, HttpClientError::Api { status: 422, .. }));

    // Register Bob before restart so Bob has a persistent user row
    let client_b_reg = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: format!("http://{addr1}"),
            ..Default::default()
        },
        bob_dir.path(),
    )
    .unwrap();
    let bob_auth = client_b_reg
        .register(&RegisterRequest {
            username: "e2e_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        })
        .await
        .unwrap();
    client_b_reg.clear_session();
    client_a.clear_session();
    tokio::task::spawn_blocking(move || drop((client_a, client_b_reg)))
        .await
        .unwrap();
    let _ = shutdown_tx1.send(());
    if tokio::time::timeout(std::time::Duration::from_secs(2), &mut srv_handle1)
        .await
        .is_err()
    {
        srv_handle1.abort();
        let _ = srv_handle1.await;
    }
    drop(state1);

    // 2. Restart server on the exact same TempDir SQLite database!
    let state2 = Arc::new(AppState::try_new(cfg).expect("reboot AppState"));
    let router2 = build_router_with_state(state2.clone());
    let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let srv_handle2 = tokio::spawn(async move {
        let _ = axum::serve(listener2, router2).await;
    });

    let cfg_b2 = ClientConfig {
        server_url: format!("http://{addr2}"),
        ..Default::default()
    };
    let client_b = TelegramClient::open_with_dir(cfg_b2.clone(), bob_dir.path()).unwrap();
    let bob_login = client_b
        .login(&LoginRequest {
            username: bob_auth.username.clone(),
            password: "StrongPassword!123".into(),
            device_name: "mac-b".into(),
        })
        .await
        .unwrap();
    client_b.set_auth_session(bob_login.clone());

    // Client B searches and installs the app persisted across server restart
    let search = client_b
        .search_automation_apps(Some("e2e-x-poster"), Some("social"), Some("x.com"), None, None)
        .await
        .unwrap();
    assert_eq!(search.total, 1);
    assert_eq!(search.apps[0].author_user_id, alice_auth.user_id);

    let inst = client_b.install_automation_app("e2e-x-poster").await.unwrap();
    assert_eq!(inst.install_count, 1);
    assert!(inst.newly_installed);

    let roundtripped: AutomationScriptApp =
        serde_json::from_value(inst.app.manifest.clone()).unwrap();
    assert_eq!(roundtripped.steps, app.steps);
    assert_eq!(roundtripped.parameters, app.parameters);

    // Drop Client B and re-open offline to verify LocalStore persistence
    client_b.clear_session();
    tokio::task::spawn_blocking(move || drop(client_b))
        .await
        .unwrap();

    let client_b_reopened = TelegramClient::open_with_dir(cfg_b2, bob_dir.path()).unwrap();
    client_b_reopened.set_auth_session(bob_login);
    let cached_installed = client_b_reopened.list_installed_automation_apps_cached();
    assert_eq!(cached_installed.len(), 1);
    assert_eq!(cached_installed[0].app_id, "e2e-x-poster");
    let offline_script: AutomationScriptApp =
        serde_json::from_value(cached_installed[0].manifest.clone()).unwrap();
    assert_eq!(offline_script.steps.len(), app.steps.len());

    // Record run + sync installed list
    let run_resp = client_b_reopened
        .record_automation_app_run(
            "e2e-x-poster",
            &RecordAutomationRunRequest {
                version: Some("1.0.0".into()),
                success: true,
                steps_executed: 5,
                total_duration_ms: 410,
                used_llm: false,
                error: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(run_resp.run_count, 1);

    let synced = client_b_reopened
        .list_installed_automation_apps()
        .await
        .unwrap();
    assert_eq!(synced.total, 1);
    assert_eq!(synced.apps[0].local_run_count, 1);

    client_b_reopened.clear_session();
    tokio::task::spawn_blocking(move || drop(client_b_reopened))
        .await
        .unwrap();
    srv_handle2.abort();
    drop(state2);
}

#[tokio::test]
async fn test_m2_remediation_secret_bypasses_false_positives_and_edge_cases() {
    let dir = TempDir::new().unwrap();
    let (_cfg, state, router) = boot_server_in_dir(&dir);
    let (_alice_id, alice_tok) = register_user(&router, "rem_alice").await;
    let (_bob_id, bob_tok) = register_user(&router, "rem_bob").await;

    let base_manifest = serde_json::to_value(build_clean_script_app(
        "rem-check",
        "1.0.0",
        "social",
        vec!["x.com"],
        None,
    ))
    .unwrap();

    // Fix 1a: literal \n + \u0063t0=... in changelog -> 422 secret_leak_detected
    let (st_f1a, v_f1a) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({
            "manifest": base_manifest.clone(),
            "changelog": "line1\n\\u0063t0=deadbeef_secret_99"
        })),
    )
    .await;
    assert_eq!(st_f1a, StatusCode::UNPROCESSABLE_ENTITY, "changelog \\n+\\u0063t0: {v_f1a}");
    assert_eq!(v_f1a["error"], "secret_leak_detected");

    // Fix 1b: literal \n + \u0063t0=... in unknown manifest field -> 422 secret_leak_detected
    let mut m_f1b = base_manifest.clone();
    m_f1b["extra_note"] = json!("line1\n\\u0063t0=deadbeef_secret_99");
    let (st_f1b, v_f1b) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": m_f1b })),
    )
    .await;
    assert_eq!(st_f1b, StatusCode::UNPROCESSABLE_ENTITY, "unknown field \\n+\\u0063t0: {v_f1b}");
    assert_eq!(v_f1b["error"], "secret_leak_detected");

    // Fix 1c: unescaped quote + \u003d in changelog -> 422 secret_leak_detected
    let (st_f1c, v_f1c) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({
            "manifest": base_manifest.clone(),
            "changelog": "foo \"bar\" auth_token\\u003dsecret_val"
        })),
    )
    .await;
    assert_eq!(st_f1c, StatusCode::UNPROCESSABLE_ENTITY, "quote+\\u003d: {v_f1c}");
    assert_eq!(v_f1c["error"], "secret_leak_detected");

    // Fix 2a: prefixed sensitive JSON keys in unknown objects -> 422 secret_leak_detected
    let mut m_f2a = base_manifest.clone();
    m_f2a["extra_headers"] = json!({ "twitter_auth_token": "real_secret_value_xyz" });
    let (st_f2a, v_f2a) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": m_f2a })),
    )
    .await;
    assert_eq!(st_f2a, StatusCode::UNPROCESSABLE_ENTITY, "twitter_auth_token: {v_f2a}");
    assert_eq!(v_f2a["error"], "secret_leak_detected");

    // Fix 2b: {"extra": {"user_password": "raw_pw"}} -> 422 secret_leak_detected
    let mut m_f2b = base_manifest.clone();
    m_f2b["extra"] = json!({ "user_password": "raw_pw" });
    let (st_f2b, v_f2b) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": m_f2b })),
    )
    .await;
    assert_eq!(st_f2b, StatusCode::UNPROCESSABLE_ENTITY, "user_password: {v_f2b}");
    assert_eq!(v_f2b["error"], "secret_leak_detected");

    // Fix 3: English prose with standalone Basic / Bearer succeeds (201), while real leaks still 422
    for (idx, ok_desc) in [
        "Basic JavaScript helper for scraping",
        "Use Basic JavaScript.",
        "Basic authentication: guide",
        "Basic authentication: helper text",
        "Bearer authentication helper",
        "Uses Bearer authentication.",
        "Bearer credential helper",
        "Bearer auth helper",
        "Bearer ({{auth_token}})",
        "Authorization: Bearer ({{auth_token}})",
        "Use Basic ({{auth_token}})",
    ]
    .iter()
    .enumerate()
    {
        let mut m_ok = base_manifest.clone();
        let id = format!("prose-ok-{idx}");
        m_ok["app_id"] = json!(id);
        m_ok["slug"] = json!(id);
        m_ok["description"] = json!(ok_desc);
        let (st_ok, v_ok) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": m_ok })),
        )
        .await;
        assert_eq!(st_ok, StatusCode::CREATED, "prose `{ok_desc}` should pass: {v_ok}");
    }

    for bad_desc in [
        "Basic dXNlcjpwYXNz",
        "Use Basic dXNlcjpwYXNz.",
        "Use Basic dXNlcjpwYXNz!",
        "Use Basic (dXNlcjpwYXNz)",
        "Use Basic [dXNlcjpwYXNz].",
        "Basic authentication: dXNlcjpwYXNz",
        "Basic auth: dXNlcjpwYXNz",
        "Basic authentication:dXNlcjpwYXNz",
        "Bearer abcdefghijklmnop",
        "Bearer {{query}}",
        "Bearer ({{query}})",
        "Authorization: Bearer authentication",
        "Bearer authentication: secret_1234567890",
        "Bearer authentication:secret_1234567890",
        "Bearer credential: tok_live_9876543210",
        "Bearer authentication = secret_1234567890",
        "Bearer (secret_1234567890)",
        "Bearer [secret_1234567890]",
        "Bearer auth: secret_1234567890",
        "Bearer cred: tok_live_9876543210",
        "Bearer bearer_tok: secret_1234567890",
    ] {
        let mut m_bad = base_manifest.clone();
        m_bad["app_id"] = json!("rejected-desc-check");
        m_bad["slug"] = json!("rejected-desc-check");
        m_bad["description"] = json!(bad_desc);
        let (st_bad, v_bad) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": m_bad })),
        )
        .await;
        assert_eq!(st_bad, StatusCode::UNPROCESSABLE_ENTITY, "should reject `{bad_desc}` in description: {v_bad}");
        assert_eq!(v_bad["error"], "secret_leak_detected");

        // Also check changelog for the same bypass vectors
        let (st_chg, v_chg) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": base_manifest.clone(), "changelog": bad_desc })),
        )
        .await;
        assert_eq!(st_chg, StatusCode::UNPROCESSABLE_ENTITY, "should reject `{bad_desc}` in changelog: {v_chg}");
        assert_eq!(v_chg["error"], "secret_leak_detected");
    }

    // Iteration 3 Fix 3 + Iteration 4 Challenge 3: Suffixed, infixed, camelCase sensitive JSON keys
    // and non-step secret_vault_key in unknown objects -> 422 secret_leak_detected
    for (key_name, key_val) in [
        ("auth_token_prod", "prod_secret_token_xyz"),
        ("ct0_backup", "deadbeef_ct0_backup"),
        ("my_ct0_backup", "deadbeef_ct0_nested"),
        ("my-auth-token-prod", "prod_secret_token_nested"),
        ("authToken", "camel_case_secret_12345"),
        ("apiKey", "camel_case_apikey_12345"),
        ("accessToken", "camel_case_access_12345"),
        ("refreshToken", "camel_case_refresh_12345"),
        ("privateKey", "camel_case_privkey_12345"),
        ("sessionToken", "camel_case_session_12345"),
        ("bearerToken", "camel_case_bearer_12345"),
        ("secret_vault_key", "raw_plaintext_secret_12345"),
    ] {
        let mut m_key = base_manifest.clone();
        m_key["extra_headers"] = json!({ key_name: key_val });
        let (st_key, v_key) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": m_key })),
        )
        .await;
        assert_eq!(
            st_key,
            StatusCode::UNPROCESSABLE_ENTITY,
            "sensitive JSON key `{key_name}` should fail with 422: {v_key}"
        );
        assert_eq!(v_key["error"], "secret_leak_detected");
    }

    // Fix 7 #1: DELETE /install on unpublished app by non-author returns 404 (no existence oracle)
    let mut m_priv = base_manifest.clone();
    m_priv["app_id"] = json!("priv-oracle-check");
    m_priv["slug"] = json!("priv-oracle-check");
    let (st_priv, _) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": m_priv, "is_published": false })),
    )
    .await;
    assert_eq!(st_priv, StatusCode::CREATED);
    let (st_del_priv, _) = api(
        &router,
        Method::DELETE,
        "/api/v1/automation-apps/priv-oracle-check/install",
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(st_del_priv, StatusCode::NOT_FOUND);

    // Fix 7 #2: Trimmed metadata sync into manifest_val + consistent content_sha256
    let mut m_trim = base_manifest.clone();
    m_trim["app_id"] = json!("  trimmed-meta-app  ");
    m_trim["slug"] = json!("trimmed-meta-app");
    m_trim["name"] = json!("  Trimmed Meta App  ");
    m_trim["version"] = json!("  1.0.0  ");
    m_trim["category"] = json!("  social  ");
    let (st_trim, v_trim) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": m_trim })),
    )
    .await;
    assert_eq!(st_trim, StatusCode::CREATED, "trimmed publish: {v_trim}");
    let det_trim: AutomationAppDetail = serde_json::from_value(v_trim).unwrap();
    assert_eq!(det_trim.app_id, "trimmed-meta-app");
    assert_eq!(det_trim.manifest["app_id"], "trimmed-meta-app");
    assert_eq!(det_trim.manifest["name"], "Trimmed Meta App");
    assert_eq!(det_trim.manifest["version"], "1.0.0");
    assert_eq!(det_trim.manifest["category"], "social");
    assert_eq!(
        compute_automation_manifest_sha256(&serde_json::to_string(&det_trim.manifest).unwrap()),
        det_trim.content_sha256
    );

    // Fix 7 #3: ?domain=sub.x.com matches app targeting ["x.com"], while myx.com never matches
    let mut m_myx = base_manifest.clone();
    m_myx["app_id"] = json!("myx-only-app");
    m_myx["slug"] = json!("myx-only-app");
    m_myx["target_domains"] = json!(["myx.com"]);
    let (st_myx, _) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": m_myx })),
    )
    .await;
    assert_eq!(st_myx, StatusCode::CREATED);

    let (st_sub, v_sub) = api(
        &router,
        Method::GET,
        "/api/v1/automation-apps?domain=sub.x.com",
        None,
        None,
    )
    .await;
    assert_eq!(st_sub, StatusCode::OK);
    let list_sub: ListAutomationAppsResponse = serde_json::from_value(v_sub).unwrap();
    assert!(
        list_sub.apps.iter().any(|a| a.app_id == "prose-ok-0"),
        "sub.x.com should match app targeting x.com"
    );
    assert!(
        !list_sub.apps.iter().any(|a| a.app_id == "myx-only-app"),
        "sub.x.com must NOT match myx.com"
    );

    // Fix 7 #4: ?offset=18446744073709551615 clamps to 10000 with 200 OK
    let (st_off, v_off) = api(
        &router,
        Method::GET,
        "/api/v1/automation-apps?offset=18446744073709551615",
        None,
        None,
    )
    .await;
    assert_eq!(st_off, StatusCode::OK, "u64::MAX offset should clamp: {v_off}");
    let list_off: ListAutomationAppsResponse = serde_json::from_value(v_off).unwrap();
    assert_eq!(list_off.offset, 10_000);
    assert!(list_off.apps.is_empty());

    // Fix 4 & Iteration 3 Fix 5.1: Installed v1.0.0 vs Latest v2.0.0 returns v1.0.0 manifest + matching top-level metadata + SHA-256
    let v1_app = build_clean_script_app("ver-pinned-app", "1.0.0", "social", vec!["x.com"], None);
    let (st_p1, val_p1) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": serde_json::to_value(&v1_app).unwrap() })),
    )
    .await;
    assert_eq!(st_p1, StatusCode::CREATED);
    let det_p1: AutomationAppDetail = serde_json::from_value(val_p1).unwrap();

    let (st_inst, _) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps/ver-pinned-app/install",
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(st_inst, StatusCode::OK);

    let mut v2_app = v1_app.clone();
    v2_app.version = "2.0.0".to_string();
    v2_app.slug = "ver-pinned-app-v2-slug".to_string();
    v2_app.name = "Version 2 Name".to_string();
    v2_app.description = "Version 2 Description".to_string();
    v2_app.category = "productivity".to_string();
    v2_app.target_domains = vec!["github.com".to_string()];
    v2_app.steps.push(ScriptStep::Scroll {
        delta_x: 0.0,
        delta_y: 999.0,
    });
    let (st_p2, val_p2) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": serde_json::to_value(&v2_app).unwrap() })),
    )
    .await;
    assert_eq!(st_p2, StatusCode::OK);
    let det_p2: AutomationAppDetail = serde_json::from_value(val_p2).unwrap();
    assert_ne!(det_p1.content_sha256, det_p2.content_sha256);

    let (st_bob_inst, v_bob_inst) = api(
        &router,
        Method::GET,
        "/api/v1/automation-apps/installed",
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(st_bob_inst, StatusCode::OK);
    let inst_resp: ListInstalledAutomationAppsResponse = serde_json::from_value(v_bob_inst).unwrap();
    assert_eq!(inst_resp.total, 1);
    let pinned = &inst_resp.apps[0];
    assert_eq!(pinned.app_id, "ver-pinned-app");
    assert_eq!(pinned.slug, v1_app.slug);
    assert_eq!(pinned.installed_version, "1.0.0");
    assert_eq!(pinned.latest_version, "2.0.0");
    assert_eq!(pinned.name, v1_app.name);
    assert_eq!(pinned.description, v1_app.description);
    assert_eq!(pinned.category, v1_app.category);
    assert_eq!(pinned.target_domains, v1_app.target_domains);
    assert_eq!(pinned.manifest["version"], "1.0.0");
    assert_eq!(
        pinned.manifest["steps"].as_array().map(|a| a.len()),
        Some(v1_app.steps.len())
    );
    assert_eq!(pinned.content_sha256, det_p1.content_sha256);
    assert_eq!(
        compute_automation_manifest_sha256(&serde_json::to_string(&pinned.manifest).unwrap()),
        pinned.content_sha256
    );
    // Double check stored DB v1.0.0 canonical JSON matches
    let stored_v1_sha = state
        .db
        .list_automation_app_versions("ver-pinned-app")
        .unwrap()
        .into_iter()
        .find(|v| v.version == "1.0.0")
        .unwrap()
        .content_sha256;
    assert_eq!(pinned.content_sha256, stored_v1_sha);

    // Iteration 3 Fix 4: POST /runs validates version length/emptiness and existence in automation_app_versions
    let oversized_ver = "x".repeat(4096);
    for bad_ver in [oversized_ver.as_str(), "", "   ", "99.99.99-nonexistent"] {
        let (st_bad_run, v_bad_run) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps/ver-pinned-app/runs",
            Some(&bob_tok),
            Some(json!({
                "version": bad_ver,
                "success": true,
                "steps_executed": 1,
                "total_duration_ms": 10
            })),
        )
        .await;
        assert_eq!(
            st_bad_run,
            StatusCode::BAD_REQUEST,
            "invalid/nonexistent version `{bad_ver}` should fail with 400: {v_bad_run}"
        );
        assert_eq!(v_bad_run["error"], "validation_error");
    }

    // Valid pinned version "1.0.0" and omitted version (None -> latest_version) both succeed (200)
    let (st_ok_v1, _) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps/ver-pinned-app/runs",
        Some(&bob_tok),
        Some(json!({
            "version": "1.0.0",
            "success": true,
            "steps_executed": 5,
            "total_duration_ms": 120
        })),
    )
    .await;
    assert_eq!(st_ok_v1, StatusCode::OK);

    let (st_ok_none, v_ok_none) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps/ver-pinned-app/runs",
        Some(&bob_tok),
        Some(json!({
            "success": true,
            "steps_executed": 6,
            "total_duration_ms": 150
        })),
    )
    .await;
    assert_eq!(st_ok_none, StatusCode::OK);
    let run_none_resp: RecordAutomationRunResponse = serde_json::from_value(v_ok_none).unwrap();
    assert_eq!(run_none_resp.run_count, 2);
}

#[tokio::test]
async fn test_bug3_query_placeholders_and_trailing_bypass_vectors() {
    let dir = TempDir::new().unwrap();
    let (_cfg, _state, router) = boot_server_in_dir(&dir);
    let (_alice_id, alice_tok) = register_user(&router, "bug3_alice").await;

    let mut base_app = build_clean_script_app(
        "bug3-base",
        "1.0.0",
        "general",
        vec!["example.com"],
        None,
    );
    base_app.parameters.push(ScriptParameter {
        name: "api_token".to_string(),
        description: "API secret token".to_string(),
        required: true,
        default_value: None,
        is_secret: true,
    });

    // Valid sanitized query placeholders & protected expressions -> 201 Created
    for (idx, ok_url) in [
        "https://example.com/auth?token={{auth_token}}",
        "https://example.com/auth?token={{auth_token}}&next=/home",
        "https://example.com/auth?token={{auth_token}}#frag",
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-ok-url-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.steps = vec![ScriptStep::Navigate {
            url_template: (*ok_url).to_string(),
        }];
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "valid placeholder URL `{ok_url}` should return 201: {val}"
        );
    }

    for (idx, ok_desc) in [
        "(token={{auth_token}})",
        "{token: {{auth_token}}}",
        "Bearer {{api_token}}",
        "Authorization: Bearer {{api_token}}",
        "Basic {{api_token}}",
        "Basic ({{auth_token}})",
        "Basic auth guide",
        r#"{"token":""}"#,
        r#"{"token": ""}"#,
        r#"{"url_template":"https://example.com/auth?token={{auth_token}}"}"#,
        r#"{"description":"Bearer {{api_token}}"}"#,
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-ok-desc-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "valid protected placeholder `{ok_desc}` should return 201: {val}"
        );
    }

    // Valid manifest round-trip with both url_template={{auth_token}} and description="Bearer {{api_token}}"
    let mut rt_app = base_app.clone();
    rt_app.app_id = "bug3-ok-roundtrip".into();
    rt_app.slug = "bug3-ok-roundtrip".into();
    rt_app.description = "Bearer {{api_token}}".into();
    rt_app.steps = vec![ScriptStep::Navigate {
        url_template: "https://example.com/auth?token={{auth_token}}".into(),
    }];
    let (st_rt, val_rt) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": serde_json::to_value(&rt_app).unwrap() })),
    )
    .await;
    assert_eq!(st_rt, StatusCode::CREATED, "roundtrip manifest failed: {val_rt}");

    // Bypass & leak vectors -> 422 secret_leak_detected
    for bad_text in [
        "https://example.com/auth?token={{auth_token}}&secret=raw_secret_123456",
        "https://example.com/auth?token=real_secret{{auth_token}}",
        "https://example.com/auth?token={{raw_secret_with_spaces}}",
        "https://example.com/auth?token={{auth_token}}real_secret",
        "https://example.com/auth?token={{auth_token}}{{api_token}}",
        "https://example.com/auth?token=\"raw_secret",
        "https://example.com/auth?token='raw_secret",
        "https://example.com/auth?token=\"\"raw_secret_123456",
        "https://example.com/auth?token=''raw_secret_123456",
        "https://example.com/auth?token={{auth_token}}\"raw_secret_123456",
        "https://example.com/auth?token={{auth_token}}'raw_secret_123456",
        "https://example.com/auth?token=\"{{auth_token}}\"raw_secret_123456",
        "https://example.com/auth?token='{{auth_token}}'raw_secret_123456",
        "https://example.com/auth?token={{auth_token}}\\\"raw_secret_123456",
        "https://example.com/auth?token=%7B%7Bauth_token%7D%7D",
        "Bearer {{api_token}}raw_secret_123",
        "Authorization: Bearer {{api_token}}raw_secret_123",
        "Authorization: Bearer {{api_token}}\"raw_secret_123456",
        "Authorization: Bearer {{api_token}}\\\"raw_secret_123456",
        "Authorization: Bearer {{api_token}}#raw_secret_123456",
        "Bearer {{api_token}}#raw_secret_123456",
        "Bearer \"{{api_token}}\"raw_secret_123456",
        "Bearer \"raw_secret\"",
        "Bearer 'raw_secret'",
        "Basic {{api_token}}dXNlcjpwYXNz",
        "Basic dXNlcjpwYXNz",
        r#"{"token":"raw_secret"}"#,
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-bad".into();
        app.slug = "bug3-bad".into();
        app.description = bad_text.to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "bypass/leak vector `{bad_text}` must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
    }

    // Section 4.2 unified positive controls (UTF-8/emoji/balanced-enclosure/JSON containers) -> 201
    for (idx, ok_desc) in [
        r#"{"url_template":"https://example.com/auth?token={{auth_token}}"}"#,
        r#"{"token":""}"#,
        r#"{"description":"Bearer {{api_token}}"}"#,
        "(token={{auth_token}})",
        "{token: {{auth_token}}}",
        "Bearer ({{api_token}})",
        "Basic ({{auth_token}})",
        "Basic \"你好\"",
        "Bearer 你好",
        "Bearer 认证授权流程的详细文档说明",
        "使用 Basic 认证与 Bearer {{api_token}} 流程说明 🚀",
        "https://example.com/auth?token={{auth_token}}&label=你好世界🚀",
        "Basic auth guide",
        "Bearer auth helper",
        "Uses Bearer authentication.",
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-s42-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "Section 4.2 positive control `{ok_desc}` should return 201: {val}"
        );
    }

    // Section 4.2 unified Challenger & unbalanced-delimiter bypass vectors -> 422 secret_leak_detected
    for bad_text in [
        "Basic \"dXNlcjpwYXNz\"",
        "Basic 'dXNlcjpwYXNz'",
        "Basic: \"dXNlcjpwYXNz\"",
        "Basic \"x\"dXNlcjpwYXNz",
        "https://example.com/auth?token={{auth_token}}\":raw_secret_123456",
        "https://example.com/auth?token=\"\":raw_secret_123456",
        "https://example.com/auth?token='':raw_secret_123456",
        "Authorization: Bearer {{api_token}}\":raw_secret_123456",
        "Bearer {{api_token}}\",raw_secret_123456",
        "Basic {{api_token}}\":dXNlcjpwYXNz",
        "Bearer \"x\"raw_secret_123456",
        "Bearer x\"raw_secret_123456",
        "Bearer abc$raw_secret_123456",
        "Bearer \"s$cr3t!key123\"",
        "Bearer 你好secret_123456",
        "Bearer \"你好\"raw_secret_123456",
        "Bearer 🚀{{api_token}}",
        "Bearer \"my secret token 123\"",
        "Basic 你好dXNlcjpwYXNz",
        "Bearer ({{api_token}})raw_secret_123456",
        "Basic ({{api_token}})raw_secret_123456",
        "https://example.com/auth?token={{auth_token}}}raw_secret_123456",
        "(token={{auth_token}})raw_secret_123456",
        r#"{"token":"raw_secret"}"#,
        r#"Debug payload: {"authToken":"raw_secret"}"#,
        "https://example.com/auth?token=%7B%7Bauth_token%7D%7D",
        r#"{\\\"auth_token\\\": \\\"deadbeef12345\\\"}"#,
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-s42-bad".into();
        app.slug = "bug3-s42-bad".into();
        app.description = bad_text.to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "Section 4.2 bypass vector `{bad_text}` must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
    }

    // Excessive JSON nesting depth in unknown manifest field -> fail closed (422)
    let mut deep_manifest = serde_json::to_value(&base_app).unwrap();
    deep_manifest["app_id"] = json!("bug3-s42-deep");
    deep_manifest["slug"] = json!("bug3-s42-deep");
    let mut deep_val = json!("leaf");
    for _ in 0..20 {
        deep_val = json!({ "nested": deep_val });
    }
    deep_manifest["extra_meta"] = deep_val;
    let (st_deep, val_deep) = api(
        &router,
        Method::POST,
        "/api/v1/automation-apps",
        Some(&alice_tok),
        Some(json!({ "manifest": deep_manifest })),
    )
    .await;
    assert_eq!(
        st_deep, StatusCode::UNPROCESSABLE_ENTITY,
        "excessive JSON depth must fail closed with 422: {val_deep}"
    );
    assert_eq!(val_deep["error"], "secret_leak_detected");

    // Phase 1 It4 Challenger Classes 1-6 + Reviewer p1_5 Minor Finding (manifest.*, extra_headers, changelog) + positive controls
    for (idx, ok_desc) in [
        r#"{"Basic":"auth guide"}"#,
        r#"{"Bearer":"{{api_token}}"}"#,
        "Bearer ( {{api_token}} )",
        "Basic auth: value: {{api_token}}",
        "https://example.com/auth?authToken={{auth_token}}",
        "Bearer auth:",
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p14-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_4 positive control `{ok_desc}` should return 201: {val}"
        );
    }

    for bad_text in [
        r#"{"Bearer":"secret_1234567890"}"#,
        r#"{"Basic":"dXNlcjpwYXNz"}"#,
        "Bearer ( raw_secret_123456)",
        "Bearer [ raw_secret_123456]",
        "Basic ( dXNlcjpwYXNz)",
        "Basic (user dXNlcjpwYXNz)",
        "https://example.com/auth?authToken=raw_secret_123456",
        "https://example.com/auth?auth_token_1=raw_secret_123456",
        r#"Debug: {"authToken": "raw_secret_123456""#,
        r#"{"Authorization": "Bearer {{api_token}} raw_secret_123456"}"#,
        r#"{"Authorization": "Basic {{api_token}} dXNlcjpwYXNz"}"#,
        "Authorization: Bearer {{api_token}} raw_secret_123456",
        "Basic auth: value: dXNlcjpwYXNz",
        "`sk-ant-api03-1234567890abcdef1234`",
        "<sk-proj-1234567890abcdef1234>",
        "`eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U`",
        "Bearer raw_secret_123456:",
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p14-bad".into();
        app.slug = "bug3-p14-bad".into();
        app.description = bad_text.to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_4 bypass vector `{bad_text}` in description must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");

        let (st_chg, val_chg) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&base_app).unwrap(), "changelog": bad_text })),
        )
        .await;
        assert_eq!(
            st_chg, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_4 bypass vector `{bad_text}` in changelog must return 422: {val_chg}"
        );
        assert_eq!(val_chg["error"], "secret_leak_detected");
    }

    // Extra unknown top-level / nested keys: extra_headers {"Bearer": ...}, {"Basic": ...}, {"Authorization": "... {{api_token}} raw_secret"}
    for extra_headers in [
        json!({ "Bearer": "secret_1234567890" }),
        json!({ "Basic": "dXNlcjpwYXNz" }),
        json!({ "Authorization": "Bearer {{api_token}} raw_secret_123456" }),
        json!({ "basicAuth": "dXNlcjpwYXNz" }),
        json!({ "bearerAuth": "raw_secret_123456" }),
        json!({ "Bearer": "key sec_12345" }),
    ] {
        let (st_ex, val_ex) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({
                "manifest": serde_json::to_value(&base_app).unwrap(),
                "extra_headers": extra_headers,
            })),
        )
        .await;
        assert_eq!(
            st_ex, StatusCode::UNPROCESSABLE_ENTITY,
            "extra_headers `{extra_headers}` must return 422: {val_ex}"
        );
        assert_eq!(val_ex["error"], "secret_leak_detected");
    }

    // Phase 1 It5 Challenger p1_8 Classes A-F + Reviewer p1_7 Minor Notes 1-2 (manifest.*, changelog) + positive controls
    for (idx, ok_desc) in [
        r#"{"Bearer": "Use {{api_token}}"}"#,
        r#"{"basicAuth": "auth guide"}"#,
        r#"{"bearerAuth": "{{api_token}}"}"#,
        "Bearer (\n {{api_token}} \n)",
        "Bearer { {{api_token}} }",
        "Bearer < {{api_token}} >",
        r#"Authorization: "Bearer {{api_token}}""#,
        "https://example.com/auth?bearerAuth={{api_token}}",
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p15-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_5 positive control `{ok_desc}` should return 201: {val}"
        );
    }

    for bad_text in [
        r#"{"basicAuth": "dXNlcjpwYXNz"}"#,
        r#"{"bearerAuth": "raw_secret_123456"}"#,
        r#"{"bearer_auth": "raw_secret_123456"}"#,
        r#"{"basic_auth": "dXNlcjpwYXNz"}"#,
        r#"{"myBearer": "raw_secret_123456"}"#,
        r#"{"basic_1": "dXNlcjpwYXNz"}"#,
        r#"{"bearer_1": "raw_secret_123456"}"#,
        "https://example.com/auth?bearerAuth=raw_secret_123456",
        "https://example.com/auth?basicAuth=dXNlcjpwYXNz",
        "bearerAuth: raw_secret_123456",
        "Bearer (\nraw_secret_123456\n)",
        "Bearer [\nraw_secret_123456\n]",
        "Basic (\ndXNlcjpwYXNz\n)",
        "Basic [\ndXNlcjpwYXNz\n]",
        "Bearer (\r\nraw_secret_123456\r\n)",
        "Basic (\r\ndXNlcjpwYXNz\r\n)",
        "Bearer ( raw_secret_123456",
        "Bearer [ raw_secret_123456",
        "Basic ( dXNlcjpwYXNz",
        "Basic [ dXNlcjpwYXNz",
        "Bearer ( a raw_secret_123456",
        "Bearer { raw_secret_123456 }",
        "Bearer < raw_secret_123456 >",
        "Basic { dXNlcjpwYXNz }",
        "Basic < dXNlcjpwYXNz >",
        r#"Authorization: "Bearer {{api_token}} " raw_secret_123456"#,
        "Authorization: 'Basic {{api_token}} ' dXNlcjpwYXNz",
        r#"X-Authorization: "Bearer {{api_token}} " raw_secret_123456"#,
        r#"Debug: {"Authorization": "Bearer {{api_token}} " raw_secret_123456"#,
        r#"{"Bearer": "key sec_12345"}"#,
        r#"{"Bearer": "my s3cr3t!"}"#,
        "{\"Bearer\": \"(\nsec_12345\n)\"}",
        "authToken你好=raw_secret_123456",
        r#"Debug: {"你好authToken": "raw_secret_123456""#,
        "Bearer sec_123:",
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p15-bad".into();
        app.slug = "bug3-p15-bad".into();
        app.description = bad_text.to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_5 bypass vector `{bad_text}` in description must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");

        let (st_chg, val_chg) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&base_app).unwrap(), "changelog": bad_text })),
        )
        .await;
        assert_eq!(
            st_chg, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_5 bypass vector `{bad_text}` in changelog must return 422: {val_chg}"
        );
        assert_eq!(val_chg["error"], "secret_leak_detected");
    }

    // Phase 1 It6 Challenger p1_10 Classes 1-5 + Reviewer p1_10 Findings 1-3 + positive controls
    for (idx, ok_desc) in [
        "bearerAuth: auth guide",
        "basicAuth: auth guide",
        r#"{"bearerAuth": "Use {{api_token}}"}"#,
        r#"{"Bearer": "Use <{{api_token}}>"}"#,
        r#"Use header {"Authorization": "Bearer {{api_token}}", "Accept": "application/json"}"#,
        "https://example.com/auth?bearerAuth={{api_token}}&tab=1",
        r#"Use "Bearer {{api_token}}"."#,
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p16-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_6 positive control `{ok_desc}` should return 201: {val}"
        );
    }

    let padded_bearer_sp = format!("Bearer (\n{}\nraw_secret_123456\n)", " ".repeat(1030));
    let padded_basic_sp = format!("Basic (\n{}\ndXNlcjpwYXNz\n)", " ".repeat(1030));
    let padded_bracket_nl = format!("Bearer [\n{}\nraw_secret_123456\n]", "\n".repeat(1030));

    let p16_bad_texts: Vec<&str> = vec![
        "bearerAuth: key sec_12345",
        "bearerAuth: my raw_secret_123456",
        "bearerAuth: auth: raw_secret_123456",
        "basicAuth: auth: dXNlcjpwYXNz",
        r#"bearerAuth: "{{api_token}}" raw_secret_123456"#,
        r#"basicAuth: "{{api_token}}" dXNlcjpwYXNz"#,
        "basicAuth: user dXNlcjpwYXNz",
        "bearerAuth={{api_token}} raw_secret_123456",
        r#"Debug: {"bearerAuth": "auth" raw_secret_123456"#,
        r#"Debug: {"basicAuth": "auth" dXNlcjpwYXNz"#,
        "bearerAuth: ( raw_secret_123456 )",
        "bearerAuth: (\nraw_secret_123456\n)",
        "basicAuth: ( dXNlcjpwYXNz )",
        "basicAuth: (\ndXNlcjpwYXNz\n)",
        "Debug: {\"bearerAuth\": \"(\nraw_secret_123456\n)\"",
        "Debug: {\"basicAuth\": \"(\ndXNlcjpwYXNz\n)\"",
        "Bearer ({{api_token}}) raw_secret_123456",
        r#"Bearer "{{api_token}}" raw_secret_123456"#,
        "Bearer (auth) raw_secret_123456",
        "Bearer {auth} raw_secret_123456",
        "Bearer <auth> raw_secret_123456",
        "Bearer (\n{{api_token}}\n) raw_secret_123456",
        "Basic ({{api_token}}) dXNlcjpwYXNz",
        r#"Basic "{{api_token}}" dXNlcjpwYXNz"#,
        "Basic (auth) dXNlcjpwYXNz",
        "Basic <auth> dXNlcjpwYXNz",
        &padded_bearer_sp,
        &padded_basic_sp,
        &padded_bracket_nl,
        "Bearer ( [ ) raw_secret_123456",
        "Basic ( [ ) dXNlcjpwYXNz",
        "Bearer ( a\nraw_secret_123456",
        "Bearer [\n a\nraw_secret_123456",
        "Bearer { a\nraw_secret_123456",
        "Bearer < a\nraw_secret_123456",
        "Basic ( a\ndXNlcjpwYXNz",
        "Basic [ a\ndXNlcjpwYXNz",
        "Basic (\n a\ndXNlcjpwYXNz",
        r#"{"basic1": "dXNlcjpwYXNz"}"#,
        r#"{"bearer1": "raw_secret_123456"}"#,
        r#"{"authToken1": "raw_secret_123456"}"#,
        r#"{"password1": "raw_secret_123456"}"#,
        r#"{"apiKey2": "raw_secret_123456"}"#,
        "https://example.com/auth?bearer1=raw_secret_123456",
        "https://example.com/auth?authToken1=raw_secret_123456",
        "https://example.com/auth?ct0=raw_secret_123456",
    ];

    for bad_text in p16_bad_texts {
        let mut app = base_app.clone();
        app.app_id = "bug3-p16-bad".into();
        app.slug = "bug3-p16-bad".into();
        app.description = bad_text.to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_6 bypass vector `{bad_text}` in description must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");

        let (st_chg, val_chg) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&base_app).unwrap(), "changelog": bad_text })),
        )
        .await;
        assert_eq!(
            st_chg, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_6 bypass vector `{bad_text}` in changelog must return 422: {val_chg}"
        );
        assert_eq!(val_chg["error"], "secret_leak_detected");
    }

    for extra_headers in [
        json!({ "basic1": "dXNlcjpwYXNz" }),
        json!({ "authToken1": "raw_secret_123456" }),
        json!({ "ct0": "raw_secret_123456" }),
    ] {
        let (st_ex, val_ex) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({
                "manifest": serde_json::to_value(&base_app).unwrap(),
                "extra_headers": extra_headers,
            })),
        )
        .await;
        assert_eq!(
            st_ex, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_6 extra_headers `{extra_headers}` must return 422: {val_ex}"
        );
        assert_eq!(val_ex["error"], "secret_leak_detected");
    }

    for (pname, default_val) in [
        ("authToken", "raw_secret_123456"),
        ("bearerAuth", "raw_secret_123456"),
        ("basicAuth", "dXNlcjpwYXNz"),
        ("ct0", "raw_secret_123456"),
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p16-param-bad".into();
        app.slug = "bug3-p16-param-bad".into();
        app.parameters.push(ScriptParameter {
            name: pname.to_string(),
            description: "Param bypass".into(),
            required: false,
            default_value: Some(default_val.to_string()),
            is_secret: false,
        });
        let (st_p, val_p) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st_p, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_6 sensitive param `{pname}` with is_secret=false must return 422: {val_p}"
        );
        assert_eq!(val_p["error"], "secret_leak_detected");
    }

    // Phase 1 It7 Challenger p1_12 New Classes 1-6 (48 vectors) + Reviewer p1_12 >64KiB/>128KiB padding + positive controls
    let padded_ok_131k_bearer = format!("Bearer (\n{}\n{{{{api_token}}}}\n)", " ".repeat(131_080));
    for (idx, ok_desc) in [
        r#"Use "Bearer {{api_token}}" for authenticating."#,
        r#"Use "Bearer ({{api_token}})" for authenticating."#,
        "Search uses the Basic plan flow, checks no token exists, and resets docs.",
        "https://example.com/auth?bearerAuth={{api_token}}&tab=1",
        padded_ok_131k_bearer.as_str(),
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p17-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_7 positive control `{ok_desc}` should return 201: {val}"
        );
    }

    for ok_param in ["oauth2_url", "api_v2_endpoint", "oauth2url"] {
        let mut app = base_app.clone();
        let id = format!("bug3-p17-param-ok-{ok_param}");
        app.app_id = id.clone();
        app.slug = id;
        app.parameters.push(ScriptParameter {
            name: ok_param.into(),
            description: "Benign endpoint URL".into(),
            required: false,
            default_value: Some("https://example.com/v2".into()),
            is_secret: false,
        });
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_7 benign param `{ok_param}` should return 201: {val}"
        );
    }

    let pad_65k_a_sp = format!("Bearer ( a {}\nraw_secret_123456 )", " ".repeat(65_540));
    let pad_65k_q_sp = format!("Bearer \"{}\nraw_secret_123456\"", " ".repeat(65_540));
    let pad_131k_b_sp = format!("Bearer (\n{}\nraw_secret_123456\n)", " ".repeat(131_080));
    let pad_350_utf8_paren = format!("Bearer (\n{}\nraw_secret_123456\n)", "你".repeat(350));
    let pad_350_utf8_quote = format!("Bearer \"\n{}\nraw_secret_123456\n\"", "你".repeat(350));
    let pad_260_words_paren = format!("Basic ( {}\ndXNlcjpwYXNz )", "word ".repeat(260));
    let pad_260_words_quote = format!("Basic \" {}\ndXNlcjpwYXNz\"", "word ".repeat(260));
    let pad_350_utf8_kv = format!("bearerAuth: ( {}\nraw_secret_123456 )", "你".repeat(350));

    let p17_bad_texts: Vec<&str> = vec![
        "Bearer {{api_token}} my raw_secret_123456",
        "Bearer {{api_token}} key sec_12345",
        "Bearer {{api_token}} auth: raw_secret_123456",
        "Basic {{api_token}} user dXNlcjpwYXNz",
        "Basic {{api_token}} auth: dXNlcjpwYXNz",
        "Basic user dXNlcjpwYXNz",
        "Basic my dXNlcjpwYXNz",
        "Bearer my raw_secret_123456",
        r#""Bearer {{api_token}}" my raw_secret_123456"#,
        r#""Basic {{api_token}}" user dXNlcjpwYXNz"#,
        r#""Bearer ({{api_token}})" my raw_secret_123456"#,
        "Basic (auth)x: dXNlcjpwYXNz",
        "Basic (auth)x dXNlcjpwYXNz",
        r#"Basic "auth"x: dXNlcjpwYXNz"#,
        r#"Basic "auth"x dXNlcjpwYXNz"#,
        "Basic <auth>x dXNlcjpwYXNz",
        &pad_65k_a_sp,
        &pad_65k_q_sp,
        &pad_131k_b_sp,
        &pad_350_utf8_paren,
        &pad_350_utf8_quote,
        &pad_260_words_paren,
        &pad_260_words_quote,
        &pad_350_utf8_kv,
        r#"{"v2token": "raw_secret_123456"}"#,
        r#"{"v2apiKey": "raw_secret_123456"}"#,
        r#"{"v2api_key": "raw_secret_123456"}"#,
        r#"{"v2bearerAuth": "raw_secret_123456"}"#,
        r#"{"v2basicAuth": "dXNlcjpwYXNz"}"#,
        r#"{"v2password": "raw_secret_123456"}"#,
        r#"{"v2secret": "raw_secret_123456"}"#,
        r#"{"v2ct0": "raw_secret_123456"}"#,
        "https://example.com/auth?v2token=raw_secret_123456",
        "https://example.com/auth?v2api_key=raw_secret_123456",
        r#"{"Basic dXNlcjpwYXNz": ""}"#,
        r#"{"Bearer raw_secret_123456": ""}"#,
        r#"{"auth_token=raw_secret_123456": ""}"#,
        r#"Use {"Basic dXNlcjpwYXNz": ""} here"#,
        r#"Use {"Bearer raw_secret_123456": "{{api_token}}"} here"#,
        r#"Use {"auth_token=raw_secret_123456": ""} here"#,
        "bearerAuth: a&raw_secret_123456",
        "basicAuth: user;dXNlcjpwYXNz",
        "basicAuth: user&dXNlcjpwYXNz",
    ];

    for bad_text in p17_bad_texts {
        let mut app = base_app.clone();
        app.app_id = "bug3-p17-bad".into();
        app.slug = "bug3-p17-bad".into();
        app.description = bad_text.to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_7 bypass vector `{bad_text}` in description must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
    }

    // New Class 1 N_TRAIL_17 in changelog
    {
        let (st_chg, val_chg) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({
                "manifest": serde_json::to_value(&base_app).unwrap(),
                "changelog": "Bearer {{api_token}} my raw_secret_123456"
            })),
        )
        .await;
        assert_eq!(
            st_chg, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_7 N_TRAIL_17 in changelog must return 422: {val_chg}"
        );
        assert_eq!(val_chg["error"], "secret_leak_detected");
    }

    // New Class 4 url_template & extra_headers + New Class 5 JSON key in extra_headers
    {
        let mut app = base_app.clone();
        app.app_id = "bug3-p17-url".into();
        app.slug = "bug3-p17-url".into();
        app.steps = vec![ScriptStep::Navigate {
            url_template: "https://example.com/auth?v2token=raw_secret_123456".into(),
        }];
        let (st_u, val_u) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(st_u, StatusCode::UNPROCESSABLE_ENTITY, "{val_u}");
        assert_eq!(val_u["error"], "secret_leak_detected");
    }

    for extra_headers in [
        json!({ "v2token": "raw_secret_123456" }),
        json!({ "Basic dXNlcjpwYXNz": "" }),
        json!({ "Bearer raw_secret_123456": "{{api_token}}" }),
    ] {
        let (st_ex, val_ex) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({
                "manifest": serde_json::to_value(&base_app).unwrap(),
                "extra_headers": extra_headers,
            })),
        )
        .await;
        assert_eq!(
            st_ex, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_7 extra_headers `{extra_headers}` must return 422: {val_ex}"
        );
        assert_eq!(val_ex["error"], "secret_leak_detected");
    }

    for (pname, default_val) in [
        ("v2token", "raw_secret_123456"),
        ("v2bearerAuth", "raw_secret_123456"),
        ("v2basicAuth", "dXNlcjpwYXNz"),
        ("v2ct0", "raw_secret_123456"),
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p17-param-bad".into();
        app.slug = "bug3-p17-param-bad".into();
        app.parameters.push(ScriptParameter {
            name: pname.to_string(),
            description: "New Class 4 param bypass".into(),
            required: false,
            default_value: Some(default_val.to_string()),
            is_secret: false,
        });
        let (st_p, val_p) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st_p, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_7 sensitive param `{pname}` with is_secret=false must return 422: {val_p}"
        );
        assert_eq!(val_p["error"], "secret_leak_detected");
    }

    // Phase 1 It8 Challenger p1_14 N1-N5 + Reviewer p1_13 Basic 1024-byte split + Reviewer p1_14 O(N) & >1024 placeholder + Minor FP
    let ov_ph_after = format!("Bearer ( {} {{{{api_token}}}} )", "word ".repeat(300));
    let nested_basic_ok = format!("{}{}", "Basic ( ".repeat(5_000), ")".repeat(5_000));
    let quoted_bearer_ok = r#""Bearer {{api_token}}" "#.repeat(2_000);
    for (idx, ok_desc) in [
        r#"Use "Bearer {{api_token}}", then proceed."#,
        r#"Use "Bearer {{api_token}}". Next, open dashboard."#,
        r#"("Bearer {{api_token}}") is used for auth."#,
        r#"("Basic {{api_token}}") is used for auth."#,
        r#"{"auth_token": ""}"#,
        r#"{"auth_token": "{{auth_token}}"}"#,
        "https://example.com/auth?token={{auth_token}}&next=/home",
        "Set auth_token: {{auth_token}} and api_key: {{api_token}}",
        ov_ph_after.as_str(),
        nested_basic_ok.as_str(),
        quoted_bearer_ok.as_str(),
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p18-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_8 positive control `{idx}` should return 201: {val}"
        );
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_8 positive control `{idx}` took too long: {:?}",
            t0.elapsed()
        );
    }

    let ov_mismatch_b = format!("Bearer (\n{}\n]\nraw_secret_123456", "你".repeat(350));
    let ov_mismatch_ba = format!("Basic (\n{}\n]\ndXNlcjpwYXNz", "word ".repeat(260));
    let ov_quote_paren_ba = format!("Basic (\n{}\n\")\"\ndXNlcjpwYXNz", "word ".repeat(260));
    let ov_quote_paren_b = format!("Bearer (\n{}\n\")\"\nraw_secret_123456", "你".repeat(350));
    let split_basic_1 = format!("Basic ( {}dXNlcjpwYXNz )", "a ".repeat(1017));
    let split_edge_13 = format!("Basic ({}dXNlcjpwYXNz)", "你".repeat(341));
    let split_edge_15 = format!("Basic ( {}dXNlcjoxMjM0NQ )", "a ".repeat(1017));

    for bad_text in [
        r#"Use "Bearer {{api_token}}" , raw_secret_123456"#,
        r#"("Bearer {{api_token}}" ) raw_secret_123456"#,
        r#"Use "Basic auth" , dXNlcjpwYXNz"#,
        r#"Use 'Bearer {{api_token}}' , raw_secret_123456"#,
        "Use%20%22Bearer%20%7B%7Bapi_token%7D%7D%22%20%2C%20raw_secret_123456",
        ov_mismatch_b.as_str(),
        ov_mismatch_ba.as_str(),
        ov_quote_paren_ba.as_str(),
        ov_quote_paren_b.as_str(),
        split_basic_1.as_str(),
        split_edge_13.as_str(),
        split_edge_15.as_str(),
        "https://example.com/auth?auth_token[0]=raw_secret_123456",
        "https://example.com/auth?token[]=raw_secret_123456",
        "auth_token%5B0%5D=raw_secret_123456",
        "auth_token[0][1]=raw_secret_123456",
        r#"auth_token: "{{auth_token}}" raw_secret_123456"#,
        "auth_token={{auth_token}} raw_secret_123456",
        "auth_token={{auth_token}}#raw_secret_123456",
        "auth_token={{auth_token}};raw_secret_123456",
        "auth_token={{auth_token}}&raw_secret_123456",
        r#"auth_token: "" raw_secret_123456"#,
        r#"Authorization: "" raw_secret_123456"#,
        r#"Debug: {"auth_token": "" raw_secret_123456"#,
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p18-bad".into();
        app.slug = "bug3-p18-bad".into();
        app.description = bad_text.to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_8 bypass vector `{bad_text}` must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
    }

    // Phase 1 It9: D1-D7 O(N) scaling + URL #/&/; + B1-B4 + minor FPs
    let d1_kv_paren = format!("( {}", "auth_token: {{auth_token}} ".repeat(5_000));
    let d2_bearer_kv = "bearerAuth: {{api_token}} ".repeat(5_000);
    let d2_basic_kv = "basicAuth: {{api_token}} ".repeat(5_000);
    let d3_bearer_colon_kv = "Bearer auth: {{api_token}} ".repeat(4_000);
    let d4_kv_url = format!(
        "https://example.com/auth?{}x=1",
        "auth_token={{auth_token}}&".repeat(5_000)
    );
    let d5_unclosed_json = format!("{}1", "{\"a\": ".repeat(20_000));
    let d6_nested_angle = format!("{}{}", "Basic < ".repeat(5_000), ">".repeat(5_000));
    let d7_unclosed_quote = format!("{} \" )", "Basic ( ".repeat(5_000));

    for (idx, ok_desc) in [
        "(Bearer {{api_token}}) is used for auth.",
        "(Basic {{api_token}}) is used for auth.",
        "https://example.com/auth?x={{ auth_token }}&auth_token={{auth_token}}&next=/home",
        "https://example.com/auth?auth_token={{auth_token}}#top",
        "https://example.com/auth?auth_token={{auth_token}}#section",
        d1_kv_paren.as_str(),
        d2_bearer_kv.as_str(),
        d2_basic_kv.as_str(),
        d3_bearer_colon_kv.as_str(),
        d4_kv_url.as_str(),
        d5_unclosed_json.as_str(),
        d6_nested_angle.as_str(),
        d7_unclosed_quote.as_str(),
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p19-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_9 positive/O(N) control `{idx}` should return 201: {val}"
        );
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_9 positive/O(N) control `{idx}` took too long: {:?}",
            t0.elapsed()
        );
    }

    let angle_bearer = format!("Bearer < {}a > b\nraw_secret_123456", "word ".repeat(260));
    let angle_basic = format!("Basic < {}a > b\ndXNlcjpwYXNz", "word ".repeat(260));
    for bad_text in [
        "https://example.com/auth?auth_token={{auth_token}}#raw_secret_123456",
        "https://example.com/auth?auth_token={{auth_token}}&raw_secret_123456",
        "https://example.com/auth?auth_token={{auth_token}};raw_secret_123456",
        "https://example.com/auth?auth_token[0]={{auth_token}}#raw_secret_123456",
        "Use?auth_token={{auth_token}}#raw_secret_123456",
        "Use?auth_token={{auth_token}}&raw_secret_123456",
        "Use?auth_token={{auth_token}};raw_secret_123456",
        "https://example.com/auth?auth_token[a:b]=raw_secret_123456",
        "https://example.com/auth?auth_token[a=b]=raw_secret_123456",
        "auth_token[a:b]=raw_secret_123456",
        "https://example.com/auth?auth_token%5Ba%3Ab%5D=raw_secret_123456",
        "auth_token [0]=raw_secret_123456",
        "auth_token.0=raw_secret_123456",
        "auth_token.prod=raw_secret_123456",
        r#"auth_token: "{{auth_token}}" my.secret.123456: {{api_token}}"#,
        angle_bearer.as_str(),
        angle_basic.as_str(),
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p19-bad".into();
        app.slug = "bug3-p19-bad".into();
        app.description = bad_text.to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_9 bypass vector `{bad_text}` must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
    }

    // Phase 1 It10: EDGE_C O(N), depth125 450KB DoS, URL+space+secret, 12 subscript/dot bypasses, URL {{x=1}}, FPs
    let edge_c_bearer = format!(
        "https://example.com/auth?{}x=1 foo",
        "bearerAuth={{api_token}}&".repeat(5_000)
    );
    let dos_depth125_450kb = format!(
        "{}{{\"b\": [{}bad]}}{}",
        "{\"a\": ".repeat(125),
        "1,".repeat(220_000),
        "}".repeat(125)
    );
    let dos_depth125_450kb_enc = format!(
        "{}{{\"b\": [{}bad]}}{}",
        "{\"a\": ".repeat(125),
        "1%2C".repeat(110_000),
        "}".repeat(125)
    );
    for (idx, ok_desc) in [
        edge_c_bearer.as_str(),
        dos_depth125_450kb.as_str(),
        dos_depth125_450kb_enc.as_str(),
        "https://example.com/auth?bearerAuth={{api_token}} is used",
        "https://example.com/auth?bearerAuth={{api_token}}#top",
        "https://example.com/auth?bearerAuth={{api_token}}#section",
        r#"Bearer ("{{api_token}}")"#,
        r#"Basic ("{{api_token}}")"#,
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p110-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_10 positive/perf control `{idx}` should return 201: {val}"
        );
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_10 positive/perf control `{idx}` took too long: {:?}",
            t0.elapsed()
        );
    }

    for bad_text in [
        "https://example.com/auth?bearerAuth={{api_token}} raw_secret_123456",
        "https://example.com/auth?basicAuth={{api_token}} dXNlcjpwYXNz",
        "https://example.com/auth?bearerAuth={{api_token}}&basicAuth={{api_token}}&x=1 dXNlcjox",
        "auth_token[0].prod=raw_secret_123456",
        "auth_token[0].value=raw_secret_123456",
        "auth_token[0].0=raw_secret_123456",
        "https://example.com/auth?auth_token[0].prod=raw_secret_123456",
        "auth_token [0].prod=raw_secret_123456",
        "https://example.com/auth?auth_token%5B0%5D.prod=raw_secret_123456",
        "auth_token[a[b]=raw_secret_123456",
        "auth_token[0] [1]=raw_secret_123456",
        r#""auth_token"[0]=raw_secret_123456"#,
        r#""auth_token" [0]=raw_secret_123456"#,
        "auth_token[0][1].prod=raw_secret_123456",
        "bearerAuth[0].prod=raw_secret_123456",
        "auth_token.0[1].prod=raw_secret_123456",
        "https://example.com/auth?auth_token={{auth_token}}#{{x=1}}raw_secret_123456",
        "https://example.com/auth?auth_token={{auth_token}}&{{x=1}}raw_secret_123456",
        "https://example.com/auth?bearerAuth={{api_token}}#raw_secret_123456",
        r#"Bearer ("{{api_token}}""#,
        r#"Bearer ("{{api_token}}" raw_secret_123456)"#,
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p110-bad".into();
        app.slug = "bug3-p110-bad".into();
        app.description = bad_text.to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_10 bypass vector `{bad_text}` must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
    }

    // Phase 1 It11: failed_parse_bytes bomb exhaustion & 11+ extract_kv_key_before_sep vectors
    let utf8_d18_400kb = format!(
        "{}{{\"b\": \"{}\", \"x\": bad}}{}",
        "{\"a\": ".repeat(17),
        "🚀".repeat(50_000),
        "}".repeat(17)
    );
    let mut late_error_disjoint_layers = format!("{{\"p\":\"{}\",\"x\":bad}}", "1".repeat(24_000));
    for _ in 0..17 {
        late_error_disjoint_layers = format!(
            "{{\"p\":\"{}\",\"y\":{late_error_disjoint_layers},\"x\":bad}}",
            "1".repeat(24_000)
        );
    }
    for (idx, ok_desc) in [
        utf8_d18_400kb.as_str(),
        late_error_disjoint_layers.as_str(),
        "Use auth_token. Note: {{auth_token}}",
        "Use auth_token. Note: ok",
        "Use auth_token [note: ok]",
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p111-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_11 positive/perf control `{idx}` should return 201: {val}"
        );
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_11 positive/perf control `{idx}` took too long: {:?}",
            t0.elapsed()
        );
    }

    let mut bomb_16k_18l = format!("{{\"p\":\"{}\"}}", "1".repeat(16_000));
    for _ in 0..17 {
        bomb_16k_18l = format!("{{\"x\":bad,\"y\":{bomb_16k_18l}}}");
    }
    let mut bomb_70k_5l = format!("{{\"p\":\"{}\"}}", "1".repeat(70_000));
    for _ in 0..5 {
        bomb_70k_5l = format!("{{\"x\":bad,\"y\":{bomb_70k_5l}}}");
    }
    let d17_tail = format!("{}\"ok\"{}", "{\"a\": ".repeat(16), "}".repeat(16));
    let nl_tail = r#"{"auth\ntoken": "raw_secret_123456"}"#;
    let q_tail = r#"{"api\"key": "raw_secret_123456"}"#;
    let b16_nl = format!("{bomb_16k_18l} {nl_tail}");
    let b16_q = format!("{bomb_16k_18l} {q_tail}");
    let b16_d17 = format!("{bomb_16k_18l} {d17_tail}");
    let b70_nl = format!("{bomb_70k_5l} {nl_tail}");
    let b70_q = format!("{bomb_70k_5l} {q_tail}");
    let b70_d17 = format!("{bomb_70k_5l} {d17_tail}");
    let late_nl = format!("{late_error_disjoint_layers} {nl_tail}");

    for bad_text in [
        b16_nl.as_str(),
        b16_q.as_str(),
        b16_d17.as_str(),
        b70_nl.as_str(),
        b70_q.as_str(),
        b70_d17.as_str(),
        late_nl.as_str(),
        "auth_token[[0]=raw_secret_123456",
        "auth_token[[a]=raw_secret_123456",
        "auth_token[0][[1]=raw_secret_123456",
        "https://example.com/auth?auth_token[[0]=raw_secret_123456",
        "bearerAuth[[0]=raw_secret_123456",
        "https://example.com/auth?auth_token%5B%5B0%5D=raw_secret_123456",
        "auth_token[0] [a[b]=raw_secret_123456",
        r#"auth_token["]"]=raw_secret_123456"#,
        "auth_token[']']=raw_secret_123456",
        r#"auth_token["["]=raw_secret_123456"#,
        r#"auth_token["a]b"]=raw_secret_123456"#,
        "auth_token[0] .prod=raw_secret_123456",
        r#"auth_token[0] "prod"=raw_secret_123456"#,
        "auth_token..prod=raw_secret_123456",
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p111-bad".into();
        app.slug = "bug3-p111-bad".into();
        app.description = bad_text.to_string();
        let t0 = std::time::Instant::now();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_11 bypass vector must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_11 bypass check took too long: {:?}",
            t0.elapsed()
        );
    }

    // Phase 1 It12: fallback_attempts>=8, array-depth >18, and 13+ extract_kv_key_before_sep vectors
    for (idx, ok_desc) in [
        r#"Use auth_token. "Note": ok"#,
        r#"Use auth_token. "Note": {{auth_token}}"#,
        r#"Use "auth_token" "Note": ok"#,
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p112-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_12 prose positive control `{idx}` should return 201: {val}"
        );
    }

    let prefix_8_ok = format!("{{\"p\": \"{}", r#"{"a":"1"}"#.repeat(8));
    let prefix_20_bad = format!("{{\"p\": \"{}", r#"{"x":bad,"y":1}"#.repeat(20));
    let arr19 = format!("{}1{}", "[".repeat(19), "]".repeat(19));
    let d1_8_nl = format!("{prefix_8_ok}{nl_tail}");
    let d1_8_d17 = format!("{prefix_8_ok}{d17_tail}");
    let d1_20_bad_nl = format!("{prefix_20_bad}{nl_tail}");
    let d2_arr19_nl = format!("prefix {{\"auth\\ntoken\": \"raw_secret_123456\", \"pad\": {arr19}}}");
    let d2_arr19_d20 = format!("prefix {{\"a\": {arr19}}}");
    let d3_long_sub = format!("auth_token[{}]=raw_secret_123456", "a".repeat(260));
    let d3_long_qsub = format!("auth_token[\"{}\"]=raw_secret_123456", "a".repeat(260));

    for bad_text in [
        d1_8_nl.as_str(),
        d1_8_d17.as_str(),
        d1_20_bad_nl.as_str(),
        d2_arr19_nl.as_str(),
        d2_arr19_d20.as_str(),
        r#"auth_token. "prod"=raw_secret_123456"#,
        r#"auth_token[0]. "prod"=raw_secret_123456"#,
        r#"auth_token[0]. "prod"[1]=raw_secret_123456"#,
        r#"auth_token . "prod"=raw_secret_123456"#,
        r#"auth_token."a" "b"=raw_secret_123456"#,
        r#"auth_token"prod"=raw_secret_123456"#,
        r#"auth_token_"prod"=raw_secret_123456"#,
        r#"auth_token[0]a"prod"=raw_secret_123456"#,
        "auth_token[0]]=raw_secret_123456",
        "https://example.com/auth?auth_token[0]]=raw_secret_123456",
        "auth_token[0] ]=raw_secret_123456",
        "auth_token[0] a=raw_secret_123456",
        d3_long_sub.as_str(),
        d3_long_qsub.as_str(),
        "auth_token.=raw_secret_123456",
        "auth_token[=raw_secret_123456",
        "auth_token[0].=raw_secret_123456",
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p112-bad".into();
        app.slug = "bug3-p112-bad".into();
        app.description = bad_text.to_string();
        let t0 = std::time::Instant::now();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_12 bypass vector `{bad_text}` must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_12 bypass check took too long: {:?}",
            t0.elapsed()
        );
    }

    // Phase 1 It13: Defect A (2x O(N^2) DoS), Defect B (128-recursion-limit), Defect C + Findings 1-3, Defect D (dup \n JSON)
    let dos_path_a = format!(
        "{}\" {}{}",
        "[".repeat(5_000),
        "a".repeat(20_000),
        "]=1,".repeat(5_000)
    );
    let dos_path_b_sp = format!("a{}{}", "[ ".repeat(5_000), "]=1,".repeat(5_000));
    let dos_path_b_nosp = format!("a{}{}", "[".repeat(5_000), "]=1,".repeat(5_000));
    let h3_dos_d200_450kb = format!(
        "{}{{\"b\": [{}bad]}}{}",
        "{\"a\": ".repeat(200),
        "1,".repeat(220_000),
        "}".repeat(200)
    );

    for (idx, ok_desc) in [
        "Use auth_token. Note: {{auth_token}}",
        "Use auth_token [note: ok]",
        r#"Use auth_token. "Note": ok"#,
        r#"Use auth_token. "Note": {{auth_token}}"#,
        r#"Use "auth_token" "Note": ok"#,
        "Use auth_token [note: {{auth_token}}]",
        dos_path_a.as_str(),
        dos_path_b_sp.as_str(),
        dos_path_b_nosp.as_str(),
        h3_dos_d200_450kb.as_str(),
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p113-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_13 positive/perf control `{idx}` should return 201: {val}"
        );
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_13 positive/perf control `{idx}` took too long: {:?}",
            t0.elapsed()
        );
    }

    let arr130 = format!("{}1{}", "[".repeat(130), "]".repeat(130));
    let arr200 = format!("{}1{}", "[".repeat(200), "]".repeat(200));
    let d_b1 = format!("prefix {{\"auth\\ntoken\": \"raw_secret_123456\", \"pad\": {arr130}}}");
    let d_b2 = format!("prefix {{\"pad\": {arr130}, \"auth\\ntoken\": \"raw_secret_123456\"}}");
    let d_b3 = format!("prefix {{\"a\": {arr200}}}");

    for bad_text in [
        d_b1.as_str(),
        d_b2.as_str(),
        d_b3.as_str(),
        r#"prefix {"api\nkey": "raw_secret_123456", "api\nkey": "{{auth_token}}"}"#,
        r#"{"api\nkey": "raw_secret_123456", "api\nkey": "{{auth_token}}"}"#,
        "auth_token ]: raw_secret_123456",
        "auth_token ]:raw_secret_123456",
        r#"auth_token "prod":raw_secret_123456"#,
        r#"auth_token "prod": raw_secret_123456"#,
        r#"auth_token "a" "b":raw_secret_123456"#,
        r#"auth_token[0] "a" b=raw_secret_123456"#,
        r#"auth_token."a" b=raw_secret_123456"#,
        "auth_token[0]a b=raw_secret_123456",
        "auth_token[0] a b=raw_secret_123456",
        "\"\".auth_token=raw_secret_123456",
        "\".auth_token=raw_secret_123456",
        "\"\".\"auth_token\"=raw_secret_123456",
        "\"\"[0].auth_token=raw_secret_123456",
        "https://example.com/auth?\"\".auth_token=raw_secret_123456",
        r#"auth_token[ "prod":raw_secret_123456"#,
        "auth_token[ prod:raw_secret_123456",
        r#"auth_token[0] [ "prod":raw_secret_123456"#,
        "auth_token[0] [ prod:raw_secret_123456",
        r#"auth_token[0] [ "prod": raw_secret_123456"#,
        "auth_token[0] [ prod: raw_secret_123456",
        r#"auth_token [ "prod"=raw_secret_123456"#,
        "auth_token [ prod=raw_secret_123456",
        r#"auth_token [[[[[[[[ "prod":raw_secret_123456"#,
        "https://example.com/auth?auth_token%5B%20prod:raw_secret_123456",
        "https://example.com/auth?auth_token%5B%20%22prod%22:raw_secret_123456",
        "https://example.com/auth?auth_token%20%5B%20prod=raw_secret_123456",
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p113-bad".into();
        app.slug = "bug3-p113-bad".into();
        app.description = bad_text.to_string();
        let t0 = std::time::Instant::now();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_13 bypass vector `{bad_text}` must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_13 bypass check took too long: {:?}",
            t0.elapsed()
        );
    }

    // Phase 1 It14: Decreasing-key_start O(N) DoS, Top-level JSON Array Duplicate Keys (H1), and H2-H5 KV Bypasses
    let dec_dos_5k = format!(
        "{}{}",
        ",auth_token[".repeat(5_000),
        "]= {{auth_token}}, ".repeat(5_000)
    );
    let dec_dos_200k_5k = format!(
        "{}{}{}",
        "a".repeat(200_000),
        ",auth_token[".repeat(5_000),
        "]= {{auth_token}}, ".repeat(5_000)
    );

    for (idx, ok_desc) in [
        "Use auth_token. Note: {{auth_token}}",
        "Use auth_token [note: ok]",
        r#"Use auth_token. "Note": ok"#,
        r#"Use auth_token. "Note": {{auth_token}}"#,
        r#"Use "auth_token" "Note": ok"#,
        "Use auth_token [note: {{auth_token}}]",
        "Use auth_token[0]. Note: {{auth_token}}",
        "https://example.com/auth_token.html?user=alice",
        dec_dos_5k.as_str(),
        dec_dos_200k_5k.as_str(),
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p114-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_14 positive/perf control `{idx}` should return 201: {val}"
        );
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_14 positive/perf control `{idx}` took too long: {:?}",
            t0.elapsed()
        );
    }

    for bad_text in [
        r#"[{"auth_token": "raw_secret_123456", "auth_token": "{{auth_token}}"}]"#,
        r#"[{"api\nkey": "raw_secret_123456", "api\nkey": "{{auth_token}}"}]"#,
        r#"[{"auth\ntoken": "raw_secret_123456", "auth\ntoken": "{{auth_token}}"}]"#,
        r#"[[[{"auth_token": "raw_secret_123456", "auth_token": "{{auth_token}}"}]]]"#,
        r#"[{"outer": {"api\nkey": "raw_secret_123456", "api\nkey": "{{auth_token}}"}}]"#,
        r#"[{\"api\\nkey\": \"raw_secret_123456\", \"api\\nkey\": \"{{auth_token}}\"}]"#,
        r#"prefix {"pad": [bad], "api\nkey": "raw_secret_123456"}"#,
        "auth_token.a b=raw_secret_123456",
        "auth_token.a b:raw_secret_123456",
        r#"auth_token."a" b: raw_secret_123456"#,
        r#"auth_token."a"c b=raw_secret_123456"#,
        r#"auth_token."a" b c=raw_secret_123456"#,
        r#"auth_token[0]."a=b"d c=raw_secret_123456"#,
        r#"auth_token."a"b c=raw_secret_123456"#,
        "auth_token[0] a b: raw_secret_123456",
        "https://example.com/auth?auth_token.a%20b=raw_secret_123456",
        "auth_token[0]a [ prod: raw_secret_123456",
        "auth_token.a [ prod: raw_secret_123456",
        r#"auth_token[0]a [ "prod": raw_secret_123456"#,
        r#"auth_token.a [ "prod": raw_secret_123456"#,
        r#""auth_token" "prod": raw_secret_123456"#,
        r#"auth_token "a" "b": raw_secret_123456"#,
        r#"auth_token "a" "b" "c": raw_secret_123456"#,
        r#""auth_token" "a" "b": raw_secret_123456"#,
        "https://example.com/auth?auth_token%20%22a%22%20%22b%22:%20raw_secret_123456",
        "[auth_token]=raw_secret_123456",
        "https://example.com?[auth_token]=raw_secret_123456",
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p114-bad".into();
        app.slug = "bug3-p114-bad".into();
        app.description = bad_text.to_string();
        let t0 = std::time::Instant::now();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_14 bypass vector `{bad_text}` must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_14 bypass check took too long: {:?}",
            t0.elapsed()
        );
    }

    // Phase 1 It15: Quote desync, tight quoted-infix, quote+unquoted+: , dot+space multi-token, and H4 non-sensitive prefix
    for (idx, ok_desc) in [
        "Use auth_token Note: ok",
        r#"Use "auth_token" Note: ok"#,
        r#"Use "auth_token" "Note": ok"#,
        "Use 中文 auth_token. Note: {{auth_token}}",
        "https://example.com/中文/auth_token.html?user=alice",
    ]
    .iter()
    .enumerate()
    {
        let mut app = base_app.clone();
        let id = format!("bug3-p115-ok-{idx}");
        app.app_id = id.clone();
        app.slug = id;
        app.description = (*ok_desc).to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::CREATED,
            "p1_15 positive control `{idx}` should return 201: {val}"
        );
    }

    for bad_text in [
        r#"' auth_token."a=b" c: raw_secret_123456"#,
        r#"" auth_token.'a=b' c: raw_secret_123456"#,
        r#"' auth_token[0]."a=b" c: raw_secret_123456"#,
        r#"' auth_token."a. b" c: raw_secret_123456"#,
        r#"' auth_token."a/b" c: raw_secret_123456"#,
        r#"https://example.com/auth?'auth_token."a=b"%20c:%20raw_secret_123456"#,
        r#"auth_token"a"b [ prod: raw_secret_123456"#,
        r#"auth_token"a"b c: raw_secret_123456"#,
        r#"auth_token"a" b: raw_secret_123456"#,
        r#""auth_token" b: raw_secret_123456"#,
        r#"auth_token "a" b: raw_secret_123456"#,
        r#""auth_token" "a" b: raw_secret_123456"#,
        r#"auth_token "a" "b" c: raw_secret_123456"#,
        r#"auth_token "a" [ b prod: raw_secret_123456"#,
        "https://example.com/auth?%22auth_token%22%20b:%20raw_secret_123456",
        "https://example.com/auth?auth_token%20%22a%22%20b:%20raw_secret_123456",
        "auth_token[0]. a b: raw_secret_123456",
        "auth_token. a b: raw_secret_123456",
        r#"auth_token. "a" b: raw_secret_123456"#,
        r#"auth_token[0]. "a" b: raw_secret_123456"#,
        r#"auth_token "a". b: raw_secret_123456"#,
        "https://example.com/auth?auth_token[0].%20a%20b:%20raw_secret_123456",
        r#"foo "auth_token" "prod": raw_secret_123456"#,
        "https://example.com/auth?foo%20%22auth_token%22%20%22prod%22:%20raw_secret_123456",
    ] {
        let mut app = base_app.clone();
        app.app_id = "bug3-p115-bad".into();
        app.slug = "bug3-p115-bad".into();
        app.description = bad_text.to_string();
        let (st, val) = api(
            &router,
            Method::POST,
            "/api/v1/automation-apps",
            Some(&alice_tok),
            Some(json!({ "manifest": serde_json::to_value(&app).unwrap() })),
        )
        .await;
        assert_eq!(
            st, StatusCode::UNPROCESSABLE_ENTITY,
            "p1_15 bypass vector `{bad_text}` must return 422: {val}"
        );
        assert_eq!(val["error"], "secret_leak_detected");
    }
}
