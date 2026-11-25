use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bot_browser::{
    AutomationScriptApp, BrowserActionTrace, BrowserControlMode, BrowserProfileData,
    BrowserProfileStore, BrowserSessionBackend, CdpBrowserBackend,
    CredentialVault, CredentialVaultSecretResolver, DeterministicScriptRunner,
    MemoryMasterKeyStore, NormalizedOrigin, RecordedActionKind, SCRIPT_APP_SCHEMA_VERSION,
    ScriptAppMetadata, ScriptCompiler, ScriptStep, SemanticLocator, TraceParameterBinding,
    import_cookies_from_json, validate_script_app_safe_for_publish,
};
use e2e_suite::browser_e2e_fixtures::{
    DEFAULT_X_HANDLE, DEFAULT_X_PASSWORD, DEFAULT_X_USERNAME, LatencyBudget, MockOpenAiServer,
    MockWebAppServer, MockXServer, fake_secrets, generate_cdp_cookies_json, isolated_temp_dir,
    mock_openai_text, mock_openai_tool_call, x_auth_cookies,
};
use e2e_suite::common::EphemeralTestServer;
use gpui_telegram_agent::fabushi::FabushiBotHost;
use gpui_telegram_agent::hermes::providers::ProviderConfig;
use gpui_telegram_agent::{
    BrowserAutomationEvent, CommandUiEffect, HermesTurnRequest, ToolsetPreset,
};
use gpui_telegram_client::config::ClientConfig;
use gpui_telegram_client::net::RegisterRequest;
use gpui_telegram_client::TelegramClient;
use gpui_telegram_proto::automation_apps::PublishAutomationAppRequest;
use serde_json::json;

async fn launch_chrome_in(dir: &std::path::Path) -> CdpBrowserBackend {
    e2e_suite::gpui_browser_harness::launch_chrome_in(dir).await
}

async fn wait_for_tweet_count(server: &MockXServer, expected: usize, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if server.tweet_count() >= expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for MockXServer tweet_count >= {expected}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// [Test ID] TEST-BR-T3-01
/// [Feature / Req] F1 + F2 / R1, AC3, AC4 — End-to-end `ChromeCdp` login, `BrowserProfileStore` persistence across restart, authenticated tweet post, and `LatencyBudget` check
/// [Preconditions] `MockXServer` started; two separate `ChromeCdp` temp dirs and `BrowserProfileStore` dir.
/// [Opaque-Box Steps] Backend #1 logs into `/login`, exports cookies, saves to `BrowserProfileStore`, closes; Backend #2 loads cookies, imports them, navigates `/home` without re-login, composes & submits a tweet under `LatencyBudget`.
/// [Observable Assertions] Restored backend accesses `/home` authenticated, `server.tweet_count()` increments to 3 with the posted text, `LatencyBudget::assert_within_budget()` passes, and `performance_metrics()` records non-empty telemetry.
#[tokio::test]
async fn test_br_t3_01_f1_f2_chrome_login_persist_restore_and_post_within_budget() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let store_dir = isolated_temp_dir("t3_01_store").expect("store dir");
    let c1_dir = isolated_temp_dir("t3_01_c1").expect("c1 dir");
    let c2_dir = isolated_temp_dir("t3_01_c2").expect("c2 dir");

    let store = BrowserProfileStore::new(store_dir.path());
    let origin = NormalizedOrigin::parse(&server.base_url()).expect("origin");
    let mut budget = LatencyBudget::new(10_000.0, 30_000.0);

    // Phase 1 (F1 + F2): Authenticate via UI & persist session cookies
    {
        let b1 = launch_chrome_in(c1_dir.path()).await;
        let login_url = server.url("/login");
        budget
            .measure("login_navigate", || b1.navigate(&login_url))
            .await
            .expect("nav login");
        b1.fill_locator(
            &SemanticLocator::by_test_id("ocfEnterTextTextInput"),
            DEFAULT_X_USERNAME,
            false,
        )
        .await
        .expect("fill user");
        b1.fill_locator(
            &SemanticLocator::by_test_id("LoginForm_Password_Input"),
            DEFAULT_X_PASSWORD,
            true,
        )
        .await
        .expect("fill pw");
        b1.click_locator(&SemanticLocator::by_test_id("LoginForm_Login_Button"))
            .await
            .expect("click login");
        b1.wait_for_locator(&SemanticLocator::by_test_id("tweetTextarea_0"), 4_000)
            .await
            .expect("wait home");

        let cookies = b1.export_cookies(None).await.expect("export cookies");
        store
            .save_profile(
                "t3-x-session",
                Some(&origin),
                &BrowserProfileData {
                    cookies,
                    local_storage: BTreeMap::new(),
                    updated_at_ms: 1_700_000_000_000,
                },
            )
            .expect("save profile");
        b1.close().await.expect("close b1");
    }

    // Phase 2 (F2 + F1): Fresh backend restores session & posts authenticated
    let b2 = launch_chrome_in(c2_dir.path()).await;
    let restored = store
        .load_profile("t3-x-session", Some(&origin))
        .expect("load profile");
    budget
        .measure("import_cookies", || b2.import_cookies(&restored.cookies))
        .await
        .expect("import cookies");
    let home_url = server.url("/home");
    budget
        .measure("restore_nav_home", || b2.navigate(&home_url))
        .await
        .expect("nav /home");
    b2.wait_for_locator(&SemanticLocator::by_test_id("tweetTextarea_0"), 4_000)
        .await
        .expect("authenticated home");

    let post_text = "T3-01 cross-feature persisted session post";
    let composer_loc = SemanticLocator::by_test_id("tweetTextarea_0");
    budget
        .measure("fill_post", || {
            b2.fill_locator(&composer_loc, post_text, false)
        })
        .await
        .expect("fill post");
    let post_btn_loc = SemanticLocator::by_test_id("tweetButtonInline");
    budget
        .measure("click_post", || b2.click_locator(&post_btn_loc))
        .await
        .expect("click post");

    wait_for_tweet_count(&server, 3, Duration::from_secs(3)).await;
    assert!(
        server
            .state()
            .tweets
            .read()
            .iter()
            .any(|t| t.text == post_text)
    );

    budget.assert_within_budget().expect("within LatencyBudget");
    let metrics = b2.performance_metrics();
    assert!(metrics.total_operations >= 4);
    assert_eq!(metrics.failed_operations, 0);

    b2.close().await.expect("close b2");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T3-02
/// [Feature / Req] F1 + F3 / R1, R3, AC5 — `DeterministicScriptRunner` drives SPA DOM transitions on `MockXServer` using `SemanticLocator` fallback (`test_id` -> `aria_label` -> `css_selector`)
/// [Preconditions] `MockXServer` started; `ChromeCdp` launched; `AutomationScriptApp` constructed with mixed and fallback `SemanticLocator`s.
/// [Opaque-Box Steps] Execute `DeterministicScriptRunner::run` to log in using `test_id`, fallback `test_id -> aria_label` for password, and fallback `test_id -> aria_label -> css_selector` for submit, then post a tweet on `/home`.
/// [Observable Assertions] `report.used_llm == false`, `report.success == true`, `report.step_timings.len() == app.steps.len()`, and `MockXServer` persists the newly posted tweet.
#[tokio::test]
async fn test_br_t3_02_f1_f3_script_runner_spa_locator_fallback_on_mock_x() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let temp = isolated_temp_dir("t3_02").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    let pw_fallback_locator = SemanticLocator {
        test_id: Some("stale_password_testid".into()),
        aria_label: Some("Password".into()),
        role: Some("textbox".into()),
        text_anchor: None,
        css_selector: Some("#x-password".into()),
        name_attr: None,
    };
    let submit_css_fallback_locator = SemanticLocator {
        test_id: Some("stale_login_btn_testid".into()),
        aria_label: Some("Stale Login Label".into()),
        role: None,
        text_anchor: None,
        css_selector: Some("#x-login-btn".into()),
        name_attr: None,
    };

    // Verify direct F1 fallback strategy order on /login before running full script
    backend.navigate(&server.url("/login")).await.expect("nav login");
    let pw_check = backend
        .wait_for_locator(&pw_fallback_locator, 2_000)
        .await
        .expect("pw fallback");
    assert_eq!(pw_check.matched_strategy.as_deref(), Some("aria_label"));
    let btn_check = backend
        .wait_for_locator(&submit_css_fallback_locator, 2_000)
        .await
        .expect("btn fallback");
    assert_eq!(btn_check.matched_strategy.as_deref(), Some("css_selector"));

    let post_text = "T3-02 SPA locator fallback replay";
    let app = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "app_t3_02".into(),
        slug: "t3-02-x-fallback".into(),
        name: "X Fallback Replay".into(),
        version: "1.0.0".into(),
        description: "Drives MockX SPA transitions via SemanticLocator fallback".into(),
        category: "social".into(),
        target_domains: vec!["127.0.0.1".into()],
        parameters: vec![],
        steps: vec![
            ScriptStep::Navigate {
                url_template: server.url("/login"),
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_test_id("ocfEnterTextTextInput"),
                value_template: DEFAULT_X_USERNAME.into(),
                secret_vault_key: None,
            },
            ScriptStep::Fill {
                locator: pw_fallback_locator,
                value_template: DEFAULT_X_PASSWORD.into(),
                secret_vault_key: None,
            },
            ScriptStep::Click {
                locator: submit_css_fallback_locator,
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("tweetTextarea_0"),
                timeout_ms: 4_000,
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_aria_label("Post text"),
                value_template: post_text.into(),
                secret_vault_key: None,
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_css("#tweet-submit"),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_text(post_text),
                timeout_ms: 4_000,
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };

    let report = DeterministicScriptRunner::run(&backend, &app, &BTreeMap::new(), None, None)
        .await
        .expect("run fallback script");
    assert!(!report.used_llm);
    assert!(report.success);
    assert_eq!(report.steps_executed, app.steps.len());
    assert_eq!(report.step_timings.len(), app.steps.len());
    assert!(report.step_timings.iter().all(|s| s.success));

    wait_for_tweet_count(&server, 3, Duration::from_secs(3)).await;
    assert!(
        server
            .state()
            .tweets
            .read()
            .iter()
            .any(|t| t.text == post_text)
    );

    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T3-03
/// [Feature / Req] F2 + F3 / R1, R3, AC4, AC5 — `CredentialVault` resolves `secret_vault_key` at replay time without exposing secret in manifest or telemetry
/// [Preconditions] `MockXServer` started with custom secret password (`fake_secrets().password`); `CredentialVault` stores credential for `server.base_url()`.
/// [Opaque-Box Steps] Build `AutomationScriptApp` with `Navigate` (`/login`) followed by `Fill` (`secret_vault_key: Some("{id}:password")`), validate publish safety, and replay on `ChromeCdp` with `CredentialVaultSecretResolver`.
/// [Observable Assertions] `validate_script_app_safe_for_publish` passes; serialized manifest, error/report strings, and `performance_metrics()` contain zero secret material (`contains_any_secret == None`); browser authenticates to `/home` and extracts `DEFAULT_X_HANDLE`.
#[tokio::test]
async fn test_br_t3_03_f2_f3_credential_vault_secret_resolution_during_replay() {
    let secrets = fake_secrets();
    let server = MockXServer::start_with_credentials(DEFAULT_X_USERNAME, secrets.password)
        .await
        .expect("start MockXServer");
    let temp = isolated_temp_dir("t3_03").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    let vault = Arc::new(
        CredentialVault::open_in_memory(&MemoryMasterKeyStore::new()).expect("open vault"),
    );
    let meta = vault
        .store_credential(
            &server.base_url(),
            DEFAULT_X_USERNAME,
            secrets.password,
            None,
            Some("mock_x_account"),
            None,
        )
        .expect("store credential");

    let app = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "app_t3_03_vault".into(),
        slug: "t3-03-vault-login".into(),
        name: "Vault Login Replay".into(),
        version: "1.0.0".into(),
        description: "Logs into MockX using CredentialVaultSecretResolver".into(),
        category: "social".into(),
        target_domains: vec!["127.0.0.1".into()],
        parameters: vec![],
        steps: vec![
            ScriptStep::Navigate {
                url_template: server.url("/login"),
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_test_id("ocfEnterTextTextInput"),
                value_template: String::new(),
                secret_vault_key: Some(format!("{}:username", meta.credential_id)),
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_test_id("LoginForm_Password_Input"),
                value_template: String::new(),
                secret_vault_key: Some(format!("{}:password", meta.credential_id)),
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("LoginForm_Login_Button"),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("user-handle"),
                timeout_ms: 4_000,
            },
            ScriptStep::ExtractText {
                locator: SemanticLocator::by_test_id("user-handle"),
                output_key: "handle".into(),
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };

    validate_script_app_safe_for_publish(&app).expect("app must be publish-safe");
    let app_json = serde_json::to_string(&app).expect("serialize app");
    assert_eq!(secrets.contains_any_secret(&app_json), None);

    let resolver = CredentialVaultSecretResolver::new(Arc::clone(&vault));
    let report =
        DeterministicScriptRunner::run(&backend, &app, &BTreeMap::new(), Some(&resolver), None)
            .await
            .expect("replay with CredentialVaultSecretResolver");

    assert!(report.success);
    assert!(!report.used_llm);
    assert_eq!(
        report.extracted_outputs.get("handle").map(String::as_str),
        Some(DEFAULT_X_HANDLE)
    );

    let report_json = serde_json::to_string(&report).expect("serialize report");
    let metrics_dbg = format!("{:?}", backend.performance_metrics());
    assert_eq!(secrets.contains_any_secret(&report_json), None);
    assert_eq!(secrets.contains_any_secret(&metrics_dbg), None);

    backend.close().await.expect("close backend");
    server.shutdown().await;
}

// ============================================================================
// F4–F6 Cross-Feature Tests (TEST-BR-T3-04..07)
// ============================================================================

fn full_turn_req(prompt: &str, session_id: &str) -> HermesTurnRequest {
    let mut req = HermesTurnRequest::new(prompt).with_session(session_id);
    req.preset = Some(ToolsetPreset::Full);
    req
}

/// [Test ID] TEST-BR-T3-04
/// [Feature / Req] F3 + F4 / R3, R4, AC5, AC6 — `ScriptCompiler` output publishes to Server App Store, passes secret validation, persists in SQLite v41, and fetches identically
/// [Preconditions] `EphemeralTestServer` started; `MockWebAppServer` started; User A & User B `TelegramClient`s registered.
/// [Opaque-Box Steps] Compile a `BrowserActionTrace` (with both query parameter binding and secret password binding from `fake_secrets()`) via `ScriptCompiler::compile_from_trace`, verify zero secrets, publish via User A's `TelegramClient`, inspect SQLite v41, and fetch via User B's `TelegramClient`.
/// [Observable Assertions] Published manifest passes server secret validation (`201`/`Ok`), persists in SQLite `automation_apps` + `automation_app_versions`, and deserializes identically on User B with `steps`, `parameters`, and `target_domains` intact.
#[tokio::test]
async fn test_br_t3_04_f3_f4_script_compiler_to_store_publish_validate_fetch_roundtrip() {
    let mut store_server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let webapp = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let secrets = fake_secrets();

    let mut trace = BrowserActionTrace::new("trace-t3-04");
    trace.parameter_bindings = vec![
        TraceParameterBinding {
            param_name: "query".into(),
            literal_value: "Rust".into(),
            description: "Catalog search query".into(),
            required: true,
            is_secret: false,
        },
        TraceParameterBinding {
            param_name: "account_password".into(),
            literal_value: secrets.password.into(),
            description: "Account secret".into(),
            required: true,
            is_secret: true,
        },
    ];
    trace.push_action(
        None,
        RecordedActionKind::Navigate {
            url: webapp.url("/"),
        },
        Some(12),
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("search-input"),
            value: "Rust".into(),
            is_secret: false,
            vault_key_hint: None,
            parameter_name_hint: Some("query".into()),
        },
        Some(8),
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("account-password"),
            value: secrets.password.into(),
            is_secret: true,
            vault_key_hint: None,
            parameter_name_hint: Some("account_password".into()),
        },
        Some(8),
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("search-submit"),
        },
        Some(10),
    );
    trace.mark_completed();

    let compiled = ScriptCompiler::compile_from_trace(
        &trace,
        ScriptAppMetadata {
            app_id: Some("app-t3-04-compiled".into()),
            name: "T3-04 Compiled Catalog App".into(),
            description: "Compiled from trace and published to App Store".into(),
            category: "productivity".into(),
            target_domains: vec!["127.0.0.1".into()],
            ..Default::default()
        },
    )
    .expect("compile_from_trace");

    let compiled_json = serde_json::to_string(&compiled).unwrap();
    assert_eq!(secrets.contains_any_secret(&compiled_json), None);

    let a_dir = isolated_temp_dir("t3_04_a").expect("a dir");
    let b_dir = isolated_temp_dir("t3_04_b").expect("b dir");
    let cfg = ClientConfig {
        server_url: store_server.base_url(),
        ..Default::default()
    };
    let client_a = TelegramClient::open_with_dir(cfg.clone(), a_dir.path()).expect("client_a");
    client_a
        .register(&RegisterRequest {
            username: "t3_04_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register alice");

    let client_b = TelegramClient::open_with_dir(cfg, b_dir.path()).expect("client_b");
    client_b
        .register(&RegisterRequest {
            username: "t3_04_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        })
        .await
        .expect("register bob");

    let pub_detail = client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&compiled).unwrap(),
            is_published: true,
            changelog: Some("T3-04 compiled publish".into()),
        })
        .await
        .expect("publish compiled script app");
    assert_eq!(pub_detail.app_id, "app-t3-04-compiled");

    // Verify SQLite v41 row exists
    {
        let db = &store_server.state().expect("state").db;
        let persisted = db
            .get_automation_app("app-t3-04-compiled")
            .expect("db get")
            .expect("persisted row");
        assert_eq!(persisted.latest_version, "1.0.0");
    }

    let fetched = client_b
        .get_automation_app("app-t3-04-compiled")
        .await
        .expect("fetch by Bob");
    let roundtripped: AutomationScriptApp =
        serde_json::from_value(fetched.manifest).expect("deserialize");
    assert_eq!(roundtripped.steps, compiled.steps);
    assert_eq!(roundtripped.parameters, compiled.parameters);
    assert_eq!(roundtripped.target_domains, compiled.target_domains);

    client_a.clear_session();
    client_b.clear_session();
    tokio::task::spawn_blocking(move || drop((client_a, client_b)))
        .await
        .unwrap();
    webapp.shutdown().await;
    store_server.stop().await;
}

/// [Test ID] TEST-BR-T3-05
/// [Feature / Req] F1 + F5 / R1, R2, AC5 — `HermesEngine` (`ToolsetId::Browser`) + real Chrome `CdpBrowserBackend` drives `MockWebAppServer` with live step events
/// [Preconditions] `MockWebAppServer` started; real Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI executes `browser_navigate`, `browser_snapshot`, `browser_type`, `browser_click`, `browser_wait_for`, and `browser_extract_text` on `MockWebAppServer`.
/// [Observable Assertions] All tool calls succeed against live Chrome DOM, `StepStarted`/`StepCompleted`/`TraceCompleted` events stream cleanly, and `extracted_outputs` reads the target item title.
#[tokio::test]
async fn test_br_t3_05_f1_f5_hermes_drives_chrome_cdp_on_mock_webapp_with_events() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t3_05").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.register_browser_session(
        "tab_t3_05",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );
    let mut ev_rx = host.subscribe_browser_events();

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call("c1", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_tool_call("c2", "browser_snapshot", json!({})),
        mock_openai_tool_call(
            "c3",
            "browser_type",
            json!({ "test_id": "search-input", "text": "Flamegraph" }),
        ),
        mock_openai_tool_call(
            "c4",
            "browser_click",
            json!({ "test_id": "search-submit" }),
        ),
        mock_openai_tool_call(
            "c5",
            "browser_wait_for",
            json!({ "test_id": "item-link-item-09", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "c6",
            "browser_extract_text",
            json!({ "test_id": "item-link-item-09", "output_key": "found_item" }),
        ),
        mock_openai_text("Found Rust Flamegraph Profiler License."),
    ])
    .await
    .expect("start MockOpenAiServer");

    let engine = host.core_engine();
    engine.providers().register_provider(ProviderConfig::openai_compatible(
        "mock_openai",
        "Mock OpenAI",
        llm.base_url(),
        "mock-gpt",
        "sk-mock-test",
    ));
    engine
        .providers()
        .set_active("mock_openai", Some("mock-gpt"))
        .expect("set active");

    let res = engine
        .run_turn(full_turn_req("Search Flamegraph", "tab_t3_05"))
        .await
        .expect("run_turn");
    assert_eq!(res.tool_calls.len(), 6);
    assert!(res.tool_calls.iter().all(|tc| tc.success));
    assert!(
        res.tool_calls[5]
            .output
            .contains("Rust Flamegraph Profiler License")
    );

    let mut completed_count = 0usize;
    let mut saw_trace = false;
    while let Ok(ev) = ev_rx.try_recv() {
        match ev {
            BrowserAutomationEvent::StepCompleted { .. } => completed_count += 1,
            BrowserAutomationEvent::TraceCompleted { .. } => saw_trace = true,
            _ => {}
        }
    }
    assert_eq!(completed_count, 6);
    assert!(saw_trace);

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T3-06
/// [Feature / Req] F5 + F6 / R2, AC2 — AI automation + `BrowserControlGate` takeover pause/resume during live `MockXServer` `/challenge` flow
/// [Preconditions] `MockXServer` started with seeded session; Chrome `CdpBrowserBackend` imported with cookies and registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI navigates `/challenge`, calls `browser_request_takeover`; user takes over, checks `human-captcha-checkbox` and clicks `challenge-continue-btn` (redirecting to `/home`), calls `user_resume_ai()`; resumed AI posts tweet on `/home`.
/// [Observable Assertions] `MockXServer::state().challenge_verified_tokens` contains the session token, `MockXServer::tweet_count()` increments to 3 with AI's post, and trace includes `HumanCheckpoint`.
#[tokio::test]
async fn test_br_t3_06_f5_f6_ai_takeover_resume_during_mock_x_challenge_flow() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let seed = server.seed_session("tok_t3_06", "ct0_t3_06");
    let temp = isolated_temp_dir("t3_06").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let cookies = import_cookies_from_json(&generate_cdp_cookies_json(&x_auth_cookies(
        "127.0.0.1",
        &seed.auth_token,
        &seed.ct0,
    )))
    .expect("cookies");
    backend
        .import_cookies(&cookies)
        .await
        .expect("import cookies");

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t3_06",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );
    let mut ev_rx = host.subscribe_browser_events();

    let post_text = "T3-06 post after human challenge takeover";
    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call(
            "x1",
            "browser_navigate",
            json!({ "url": server.url("/challenge") }),
        ),
        mock_openai_tool_call(
            "x2",
            "browser_request_takeover",
            json!({ "reason": "Solve human checkpoint", "wait_for_finish": true }),
        ),
        mock_openai_tool_call(
            "x3",
            "browser_wait_for",
            json!({ "test_id": "tweetTextarea_0", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "x4",
            "browser_type",
            json!({ "test_id": "tweetTextarea_0", "text": post_text }),
        ),
        mock_openai_tool_call(
            "x5",
            "browser_click",
            json!({ "test_id": "tweetButtonInline" }),
        ),
        mock_openai_text("Challenge passed and post published."),
    ])
    .await
    .expect("start MockOpenAiServer");

    let engine = host.core_engine();
    engine.providers().register_provider(ProviderConfig::openai_compatible(
        "mock_openai",
        "Mock OpenAI",
        llm.base_url(),
        "mock-gpt",
        "sk-mock-test",
    ));
    engine
        .providers()
        .set_active("mock_openai", Some("mock-gpt"))
        .expect("set active");

    let engine_task = Arc::clone(&engine);
    let handle = tokio::spawn(async move {
        engine_task
            .run_turn(full_turn_req(
                "Pass challenge and post on X",
                "tab_t3_06",
            ))
            .await
    });

    loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), ev_rx.recv())
            .await
            .expect("timely takeover")
            .expect("ev_rx");
        if matches!(ev, BrowserAutomationEvent::TakeoverRequested { .. }) {
            break;
        }
    }

    sess.user_take_over();
    assert!(matches!(
        sess.control_mode(),
        BrowserControlMode::UserTakeover { .. }
    ));

    // User solves /challenge manually on live Chrome
    backend
        .click_locator(&SemanticLocator::by_test_id("human-captcha-checkbox"))
        .await
        .expect("check captcha checkbox");
    backend
        .click_locator(&SemanticLocator::by_test_id("challenge-continue-btn"))
        .await
        .expect("click continue");
    backend
        .wait_for_locator(&SemanticLocator::by_test_id("tweetTextarea_0"), 4_000)
        .await
        .expect("redirected to /home");

    sess.user_resume_ai();

    let res = tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("resumed turn finishes")
        .expect("join")
        .expect("turn succeeds");
    assert!(res.tool_calls.iter().all(|tc| tc.success));

    wait_for_tweet_count(&server, 3, Duration::from_secs(3)).await;
    assert!(
        server
            .state()
            .challenge_verified_tokens
            .read()
            .contains(&seed.auth_token)
    );
    assert!(
        server
            .state()
            .tweets
            .read()
            .iter()
            .any(|t| t.text == post_text)
    );
    let trace = sess.last_completed_trace().expect("completed trace");
    assert!(
        trace
            .actions
            .iter()
            .any(|a| matches!(a.action, RecordedActionKind::HumanCheckpoint { .. }))
    );

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T3-07
/// [Feature / Req] F6 + F3 / R3, AC5 — Trace-to-script distillation (`/distill`) + zero-LLM `DeterministicScriptRunner` replay on fresh Chrome profile
/// [Preconditions] `MockWebAppServer` started; Profile 1 & Profile 2 Chrome temp dirs isolated.
/// [Opaque-Box Steps] AI executes multi-step checkout on Profile 1 Chrome (`parameter_name_hint`: `customer_name`, `customer_email`) and invokes `/distill`; launch Profile 2 fresh Chrome backend + `BrowserProfileStore` dir and run `DeterministicScriptRunner::run` with new customer parameters.
/// [Observable Assertions] `DeterministicScriptRunner` returns `used_llm == false`, `success == true`, mock LLM call count delta == 0, and `MockWebAppServer` records both orders (`order_count() == 2`).
#[tokio::test]
async fn test_br_t3_07_f6_f3_distill_and_zero_llm_replay_on_fresh_chrome_profile() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let p1_dir = isolated_temp_dir("t3_07_p1").expect("p1 dir");
    let p2_dir = isolated_temp_dir("t3_07_p2").expect("p2 dir");
    let b1 = Arc::new(launch_chrome_in(p1_dir.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.register_browser_session("tab_t3_07", Arc::clone(&b1) as Arc<dyn BrowserSessionBackend>);

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call(
            "s1",
            "browser_navigate",
            json!({ "url": server.url("/item/item-06") }),
        ),
        mock_openai_tool_call(
            "s2",
            "browser_click",
            json!({ "test_id": "add-to-cart-btn" }),
        ),
        mock_openai_tool_call(
            "s3",
            "browser_wait_for",
            json!({ "test_id": "cart-status", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "s4",
            "browser_click",
            json!({ "test_id": "go-to-checkout" }),
        ),
        mock_openai_tool_call(
            "s5",
            "browser_wait_for",
            json!({ "test_id": "checkout-name", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "s6",
            "browser_type",
            json!({
                "test_id": "checkout-name",
                "text": "Order One",
                "parameter_name_hint": "customer_name"
            }),
        ),
        mock_openai_tool_call(
            "s7",
            "browser_type",
            json!({
                "test_id": "checkout-email",
                "text": "one@example.test",
                "parameter_name_hint": "customer_email"
            }),
        ),
        mock_openai_tool_call(
            "s8",
            "browser_click",
            json!({ "test_id": "checkout-submit" }),
        ),
        mock_openai_tool_call(
            "s9",
            "browser_wait_for",
            json!({ "test_id": "confirmation-id", "timeout_ms": 4000 }),
        ),
        mock_openai_text("Order #1 submitted."),
    ])
    .await
    .expect("start MockOpenAiServer");

    let engine = host.core_engine();
    engine.providers().register_provider(ProviderConfig::openai_compatible(
        "mock_openai",
        "Mock OpenAI",
        llm.base_url(),
        "mock-gpt",
        "sk-mock-test",
    ));
    engine
        .providers()
        .set_active("mock_openai", Some("mock-gpt"))
        .expect("set active");

    let ai_res = engine
        .run_turn(full_turn_req("Buy item-06", "tab_t3_07"))
        .await
        .expect("AI order #1");
    assert!(ai_res.tool_calls.iter().all(|tc| tc.success));
    assert_eq!(server.order_count(), 1);

    let distill_res = engine
        .run_turn(HermesTurnRequest::new("/distill Checkout Item 06").with_session("tab_t3_07"))
        .await
        .expect("/distill");
    let app_json = distill_res
        .ui_effects
        .iter()
        .find_map(|eff| match eff {
            CommandUiEffect::DistillBrowserScript { app_json, .. } => Some(app_json.clone()),
            _ => None,
        })
        .expect("DistillBrowserScript effect");
    let app: AutomationScriptApp = serde_json::from_str(&app_json).expect("parse distilled app");

    let llm_calls_before_replay = llm.call_count();
    b1.close().await.expect("close b1");

    // Replay on completely fresh Chrome profile with zero LLM calls
    let b2 = launch_chrome_in(p2_dir.path()).await;
    let params = BTreeMap::from([
        ("customer_name".to_string(), "Order Two".to_string()),
        ("customer_email".to_string(), "two@example.test".to_string()),
    ]);
    let report = DeterministicScriptRunner::run(&b2, &app, &params, None, None)
        .await
        .expect("replay on fresh profile");
    assert!(report.success);
    assert!(!report.used_llm);
    assert_eq!(llm.call_count(), llm_calls_before_replay);
    assert_eq!(server.order_count(), 2);

    b2.close().await.expect("close b2");
    llm.shutdown().await;
    server.shutdown().await;
}

// ============================================================================
// F7 & F8 Headless GPUI Subprocess Harness + 3 Tier 3 Cross-Feature Tests
// ============================================================================

const T3_CHILD_ENV: &str = "GPUI_E2E_T3_HEADLESS_CHILD";
const T3_CHILD_TARGET_ENV: &str = "GPUI_E2E_T3_HEADLESS_TARGET";
const T3_CHILD_AFTER_CTORS_ENV: &str = "GPUI_E2E_T3_HEADLESS_AFTER_CTORS";

fn t3_dispatch_headless_target(target: &str, use_underscore_exit: bool) -> ! {
    type AsyncBody = fn(gpui::AsyncApp) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>;
    let body: AsyncBody = match target {
        "test_br_t3_08_f6_f7_control_gate_gpui_workspace_interactive_takeover" => {
            |app| Box::pin(body_br_t3_08(app))
        }
        "test_br_t3_09_f7_f2_workspace_profile_store_session_continuity_across_reopen" => {
            |app| Box::pin(body_br_t3_09(app))
        }
        "test_br_t3_10_f4_f8_store_panel_search_install_local_replay_and_run_stats" => {
            |app| Box::pin(body_br_t3_10(app))
        }
        other => {
            eprintln!("unknown e2e tier3 headless target: {other}");
            e2e_suite::gpui_browser_harness::exit_headless_child(101, use_underscore_exit);
        }
    };
    e2e_suite::gpui_browser_harness::run_headless_gpui_body(target, use_underscore_exit, body);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_T3_HEADLESS_MAIN_THREAD_ENTRY: extern "C" fn() = t3_macos_mod_init_headless_entry;

#[cfg(target_os = "macos")]
extern "C" fn t3_macos_atexit_headless_entry() {
    if let Ok(target) = std::env::var(T3_CHILD_TARGET_ENV) {
        t3_dispatch_headless_target(&target, true);
    }
}

#[cfg(target_os = "macos")]
extern "C" fn t3_macos_mod_init_headless_entry() {
    if std::env::var(T3_CHILD_ENV).as_deref() == Ok("1") {
        if std::env::var(T3_CHILD_AFTER_CTORS_ENV).as_deref() == Ok("1") {
            unsafe extern "C" {
                fn atexit(func: extern "C" fn()) -> i32;
            }
            unsafe {
                atexit(t3_macos_atexit_headless_entry);
            }
            return;
        }
        if let Ok(target) = std::env::var(T3_CHILD_TARGET_ENV) {
            t3_dispatch_headless_target(&target, false);
        }
    }
}

fn run_t3_in_headless_subprocess(test_fn_name: &str) {
    if std::env::var(T3_CHILD_ENV).as_deref() == Ok("1") {
        t3_dispatch_headless_target(test_fn_name, false);
    }
    e2e_suite::gpui_browser_harness::spawn_headless_subprocess(
        T3_CHILD_ENV,
        T3_CHILD_TARGET_ENV,
        T3_CHILD_AFTER_CTORS_ENV,
        test_fn_name,
        false,
        None,
        None,
    );
}

/// [Test ID] TEST-BR-T3-08
/// [Feature / Req] F6 + F7 / R1, R2, AC2 — `BrowserControlGate` + GPUI `BrowserWorkspace` interactive takeover & `CdpViewportSurface` click-through
/// [Preconditions] `MockXServer` started with seeded session; Chrome `CdpBrowserBackend` with cookies attached to `AppShell`; `FabushiBotHost` + `MockOpenAiServer` wired to `shell.bot_browser.browser_registry`.
/// [Opaque-Box Steps] Trigger copilot prompt to hit `/challenge` and call `browser_request_takeover`; invoke `browser_user_takeover`, click `human-captcha-checkbox` & `challenge-continue-btn` via `CdpViewportSurface::handle_click`, then call `browser_resume_ai`.
/// [Observable Assertions] `UserTakeover` enables `CdpViewportSurface` interaction, real pointer clicks verify `/challenge` & navigate to `/home`, resumed AI posts tweet (`tweet_count == 3`), and mode returns to `Idle`.
#[test]
fn test_br_t3_08_f6_f7_control_gate_gpui_workspace_interactive_takeover() {
    run_t3_in_headless_subprocess(
        "test_br_t3_08_f6_f7_control_gate_gpui_workspace_interactive_takeover",
    );
}

async fn body_br_t3_08(mut app: gpui::AsyncApp) {
    use bot_browser::PointerButton;

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let x_server = rt.block_on(MockXServer::start()).expect("MockXServer");
    let seed = x_server.seed_session("tok_t3_08", "ct0_t3_08");
    let chrome_dir = isolated_temp_dir("t3_08").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let cookies = import_cookies_from_json(&generate_cdp_cookies_json(&x_auth_cookies(
        "127.0.0.1",
        &seed.auth_token,
        &seed.ct0,
    )))
    .expect("cookies");
    rt.block_on(chrome.import_cookies(&cookies))
        .expect("import cookies");

    let post_text = "T3-08 GPUI interactive takeover post";
    let llm = rt
        .block_on(MockOpenAiServer::start(vec![
            mock_openai_tool_call(
                "t3_08_1",
                "browser_navigate",
                json!({ "url": x_server.url("/challenge") }),
            ),
            mock_openai_tool_call(
                "t3_08_2",
                "browser_request_takeover",
                json!({ "reason": "Solve human checkpoint", "wait_for_finish": true }),
            ),
            mock_openai_tool_call(
                "t3_08_3",
                "browser_wait_for",
                json!({ "test_id": "tweetTextarea_0", "timeout_ms": 5000 }),
            ),
            mock_openai_tool_call(
                "t3_08_4",
                "browser_type",
                json!({ "test_id": "tweetTextarea_0", "text": post_text }),
            ),
            mock_openai_tool_call(
                "t3_08_5",
                "browser_click",
                json!({ "test_id": "tweetButtonInline" }),
            ),
            mock_openai_text("Challenge passed and post published."),
        ]))
        .expect("MockOpenAiServer");

    let host = Arc::new(FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host"));
    e2e_suite::gpui_browser_harness::configure_host_with_mock_llm(&host, &llm.base_url());

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                308,
                "t3_08",
            )
        })
        .unwrap();

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            host.set_browser_registry(Arc::clone(&s.bot_browser.browser_registry));
            s.attach_local_fabushi_host_with_cx(Arc::clone(&host), scx);
            s.set_browser_backend_for_test(
                Arc::clone(&chrome) as Arc<dyn BrowserSessionBackend>,
                scx,
            );
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.recorder.copilot_prompt_draft = "Pass challenge and post tweet".to_string();
                w.trigger_copilot_prompt(wcx);
            });
        });
    })
    .unwrap();

    let waiting = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            matches!(
                shell.read(cx).browser_control_mode(cx),
                BrowserControlMode::WaitingForTakeover { .. }
            )
        },
    )
    .await;
    assert!(waiting, "AI must pause in WaitingForTakeover at /challenge");

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_user_takeover(scx);
            assert!(matches!(
                s.browser_control_mode(scx),
                BrowserControlMode::UserTakeover { .. }
            ));
            let surf = s
                .browser_workspace()
                .unwrap()
                .read(scx)
                .cdp_surface()
                .unwrap()
                .read(scx);
            assert!(surf.is_interactive_enabled());
        });
    })
    .unwrap();

    let (cb_x, cb_y) = rt.block_on(e2e_suite::gpui_browser_harness::query_element_center_css(
        chrome.as_ref(),
        "[data-testid=\"human-captcha-checkbox\"]",
    ));
    let (cont_x, cont_y) = rt.block_on(
        e2e_suite::gpui_browser_harness::query_element_center_css(
            chrome.as_ref(),
            "[data-testid=\"challenge-continue-btn\"]",
        ),
    );

    app.update(|cx| {
        let surf = shell
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |sr, scx| {
            sr.handle_click(cb_x, cb_y, PointerButton::Left, 1, 0, scx);
        });
    })
    .unwrap();

    let verified = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |_| {
            x_server
                .state()
                .challenge_verified_tokens
                .read()
                .contains(&seed.auth_token)
        },
    )
    .await;
    assert!(verified, "CdpViewportSurface click on checkbox must verify challenge");

    app.update(|cx| {
        let surf = shell
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |sr, scx| {
            sr.handle_click(cont_x, cont_y, PointerButton::Left, 1, 0, scx);
        });
    })
    .unwrap();

    let redirected = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |_| {
            rt.block_on(chrome.current_url())
                .map(|u| u.contains("/home"))
                .unwrap_or(false)
        },
    )
    .await;
    assert!(redirected, "clicking challenge-continue-btn must navigate to /home");

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_resume_ai(scx);
        });
    })
    .unwrap();

    let completed = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let s = shell.read(cx);
            let w = s.browser_workspace().unwrap().read(cx);
            !w.recorder.copilot_running
                && matches!(s.browser_control_mode(cx), BrowserControlMode::Idle)
        },
    )
    .await;
    assert!(completed, "resumed AI turn must finish and return to Idle");

    rt.block_on(e2e_suite::gpui_browser_harness::wait_for_tweet_count(
        &x_server,
        3,
        Duration::from_secs(15),
    ));
    assert!(
        x_server
            .state()
            .tweets
            .read()
            .iter()
            .any(|t| t.text == post_text)
    );

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(llm.shutdown());
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(x_server.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T3-09
/// [Feature / Req] F7 + F2 / R1, AC2, AC4 — GPUI `BrowserWorkspace` + `BrowserProfileStore` session continuity across workspace/app reopen
/// [Preconditions] `MockXServer` started; shared client data dir (`TempDir`) used by Shell #1 and Shell #2 with two separate Chrome instances.
/// [Opaque-Box Steps] Shell #1 switches to profile `"persist-prof"`, logs in via `/login`, calls `save_active_profile_cookies`, closes Shell #1 & Chrome #1; Shell #2 opens with fresh Chrome #2, calls `switch_profile("persist-prof")`, and navigates `/home`.
/// [Observable Assertions] Shell #2 automatically restores `"persist-prof"` cookies and renders authenticated `/home` (`tweetTextarea_0` present) without re-login.
#[test]
fn test_br_t3_09_f7_f2_workspace_profile_store_session_continuity_across_reopen() {
    run_t3_in_headless_subprocess(
        "test_br_t3_09_f7_f2_workspace_profile_store_session_continuity_across_reopen",
    );
}

async fn body_br_t3_09(mut app: gpui::AsyncApp) {
    let rt_boot = tokio::runtime::Runtime::new().unwrap();
    let x_server = rt_boot.block_on(MockXServer::start()).expect("MockXServer");
    let shared_cfg_dir = isolated_temp_dir("t3_09_shared").expect("shared dir");

    // Phase 1: Shell #1 + Chrome #1 logs in via /login and saves profile cookies
    {
        let client1 =
            e2e_suite::gpui_browser_harness::open_client_in_dir("http://127.0.0.1:9", shared_cfg_dir.path());
        let rt1 = client1.runtime().handle().clone();
        let c1_dir = isolated_temp_dir("t3_09_c1").expect("c1 dir");
        let chrome1 = Arc::new(rt1.block_on(launch_chrome_in(c1_dir.path())));

        let shell1 = app
            .update(|cx| {
                e2e_suite::gpui_browser_harness::build_authenticated_shell(
                    cx,
                    client1.clone(),
                    309,
                    "t3_09",
                )
            })
            .unwrap();

        let login_url = x_server.url("/login");
        app.update(|cx| {
            shell1.update(cx, |s, scx| {
                s.set_browser_backend_for_test(
                    Arc::clone(&chrome1) as Arc<dyn BrowserSessionBackend>,
                    scx,
                );
                let bw = s.browser_workspace().unwrap().clone();
                bw.update(scx, |w, wcx| {
                    w.switch_profile("persist-prof".to_string(), wcx);
                });
                s.browser_navigate(&login_url, scx);
            });
        })
        .unwrap();

        assert!(
            e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
                let w = shell1.read(cx).browser_workspace().unwrap().read(cx);
                !w.is_loading && w.current_url.contains("/login")
            })
            .await
        );

        rt1.block_on(async {
            chrome1
                .fill_locator(
                    &SemanticLocator::by_test_id("ocfEnterTextTextInput"),
                    DEFAULT_X_USERNAME,
                    false,
                )
                .await
                .unwrap();
            chrome1
                .fill_locator(
                    &SemanticLocator::by_test_id("LoginForm_Password_Input"),
                    DEFAULT_X_PASSWORD,
                    true,
                )
                .await
                .unwrap();
            chrome1
                .click_locator(&SemanticLocator::by_test_id("LoginForm_Login_Button"))
                .await
                .unwrap();
            chrome1
                .wait_for_locator(&SemanticLocator::by_test_id("tweetTextarea_0"), 4_000)
                .await
                .unwrap();
        });

        app.update(|cx| {
            let bw = shell1.read(cx).browser_workspace().unwrap().clone();
            bw.update(cx, |w, wcx| {
                w.save_active_profile_cookies(wcx);
            });
        })
        .unwrap();

        let saved = e2e_suite::gpui_browser_harness::pump_until(
            &mut app,
            Duration::from_secs(15),
            |cx| {
                let w = shell1.read(cx).browser_workspace().unwrap().read(cx);
                w.recorder.last_persisted_cookie_count >= 2 && w.recorder.cookie_error_message.is_none()
            },
        )
        .await;
        if !saved {
            let diag = app
                .update(|cx| {
                    let w = shell1.read(cx).browser_workspace().unwrap().read(cx);
                    format!(
                        "last_persisted_cookie_count={}, cookie_error_message={:?}",
                        w.recorder.last_persisted_cookie_count,
                        w.recorder.cookie_error_message,
                    )
                })
                .unwrap_or_else(|e| format!("failed to read diag: {e:?}"));
            panic!("save_active_profile_cookies must persist auth cookies: {diag}");
        }

        app.update(|_| drop(shell1)).unwrap();
        rt1.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
            &chrome1,
            c1_dir.path(),
        ))
        .expect("close chrome1");
        drop(client1);
    }

    // Phase 2: Fresh Shell #2 + fresh Chrome #2 restores session via switch_profile
    {
        let client2 =
            e2e_suite::gpui_browser_harness::open_client_in_dir("http://127.0.0.1:9", shared_cfg_dir.path());
        let rt2 = client2.runtime().handle().clone();
        let c2_dir = isolated_temp_dir("t3_09_c2").expect("c2 dir");
        let chrome2 = Arc::new(rt2.block_on(launch_chrome_in(c2_dir.path())));

        let shell2 = app
            .update(|cx| {
                e2e_suite::gpui_browser_harness::build_authenticated_shell(
                    cx,
                    client2.clone(),
                    309,
                    "t3_09",
                )
            })
            .unwrap();

        app.update(|cx| {
            shell2.update(cx, |s, scx| {
                s.set_browser_backend_for_test(
                    Arc::clone(&chrome2) as Arc<dyn BrowserSessionBackend>,
                    scx,
                );
                let bw = s.browser_workspace().unwrap().clone();
                bw.update(scx, |w, wcx| {
                    w.switch_profile("persist-prof".to_string(), wcx);
                });
            });
        })
        .unwrap();

        let restored = e2e_suite::gpui_browser_harness::pump_until(
            &mut app,
            Duration::from_secs(15),
            |cx| {
                let w = shell2.read(cx).browser_workspace().unwrap().read(cx);
                w.recorder.last_imported_cookie_count >= 2 && w.recorder.cookie_error_message.is_none()
            },
        )
        .await;
        if !restored {
            let diag = app
                .update(|cx| {
                    let w = shell2.read(cx).browser_workspace().unwrap().read(cx);
                    format!(
                        "last_imported_cookie_count={}, cookie_error_message={:?}",
                        w.recorder.last_imported_cookie_count,
                        w.recorder.cookie_error_message,
                    )
                })
                .unwrap_or_else(|e| format!("failed to read diag: {e:?}"));
            panic!("switch_profile must restore persisted cookies into Chrome #2: {diag}");
        }

        let home_url = x_server.url("/home");
        app.update(|cx| {
            shell2.update(cx, |s, scx| {
                s.browser_navigate(&home_url, scx);
            });
        })
        .unwrap();

        assert!(
            e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
                let w = shell2.read(cx).browser_workspace().unwrap().read(cx);
                !w.is_loading && w.current_url.contains("/home")
            })
            .await
        );

        let snap = rt2.block_on(chrome2.snapshot_dom()).expect("snapshot_dom");
        assert!(
            snap.interactive_elements
                .iter()
                .any(|el| el.test_id.as_deref() == Some("tweetTextarea_0")),
            "reopened workspace must access authenticated /home without re-login"
        );

        app.update(|_| drop(shell2)).unwrap();
        rt2.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
            &chrome2,
            c2_dir.path(),
        ))
        .expect("close chrome2");
        drop(client2);
    }

    rt_boot.block_on(x_server.shutdown());
}

/// [Test ID] TEST-BR-T3-10
/// [Feature / Req] F4 + F8 / R3, R4, AC6 — Server App Store + `TelegramClient` + Desktop `StorePanel` search, install to `LocalStore`, zero-LLM replay, and run stats
/// [Preconditions] `EphemeralTestServer` & `MockWebAppServer` started; Client A publishes `"t3-10-store-app"`; Client B opens `AppShell` + Chrome `CdpBrowserBackend`.
/// [Opaque-Box Steps] Client B searches in `StorePanel`, installs to `LocalStore`, overrides parameters, and executes `run_installed_store_app`.
/// [Observable Assertions] `LocalStore` persists installed app, `report.success && !report.used_llm`, `MockWebAppServer::order_count() == 1`, and server reports `install_count == 1 && run_count == 1`.
#[test]
fn test_br_t3_10_f4_f8_store_panel_search_install_local_replay_and_run_stats() {
    run_t3_in_headless_subprocess(
        "test_br_t3_10_f4_f8_store_panel_search_install_local_replay_and_run_stats",
    );
}

async fn body_br_t3_10(mut app: gpui::AsyncApp) {
    use bot_browser::{ScriptParameter, SCRIPT_APP_SCHEMA_VERSION};
    use gpui_telegram_desktop::views::browser::BrowserInspectorTab;

    let rt_boot = tokio::runtime::Runtime::new().unwrap();
    let mut server = rt_boot
        .block_on(EphemeralTestServer::start())
        .expect("EphemeralTestServer");
    let web = rt_boot
        .block_on(MockWebAppServer::start())
        .expect("MockWebAppServer");

    let a_dir = isolated_temp_dir("t3_10_a").expect("a dir");
    let client_a = e2e_suite::gpui_browser_harness::open_client_in_dir(&server.base_url(), a_dir.path());
    client_a
        .runtime()
        .block_on(client_a.register(&RegisterRequest {
            username: "t3_10_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        }))
        .expect("register alice");

    let published_app = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "t3-10-store-app".to_string(),
        slug: "t3-10-store-app".to_string(),
        name: "Cross-Feature Store Checkout".to_string(),
        version: "1.0.0".to_string(),
        description: "End-to-end StorePanel replay".to_string(),
        category: "productivity".to_string(),
        target_domains: vec!["127.0.0.1".to_string()],
        parameters: vec![
            ScriptParameter {
                name: "customer_name".to_string(),
                description: "Customer name".to_string(),
                required: true,
                default_value: None,
                is_secret: false,
            },
            ScriptParameter {
                name: "customer_email".to_string(),
                description: "Customer email".to_string(),
                required: true,
                default_value: None,
                is_secret: false,
            },
        ],
        steps: vec![
            ScriptStep::Navigate {
                url_template: web.url("/item/item-07"),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("add-to-cart-btn"),
                timeout_ms: 4_000,
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("add-to-cart-btn"),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("cart-status"),
                timeout_ms: 4_000,
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("go-to-checkout"),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("checkout-name"),
                timeout_ms: 4_000,
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_test_id("checkout-name"),
                value_template: "{{customer_name}}".to_string(),
                secret_vault_key: None,
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_test_id("checkout-email"),
                value_template: "{{customer_email}}".to_string(),
                secret_vault_key: None,
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("checkout-submit"),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("confirmation-id"),
                timeout_ms: 4_000,
            },
            ScriptStep::ExtractText {
                locator: SemanticLocator::by_test_id("confirmation-id"),
                output_key: "order_id".to_string(),
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };

    client_a
        .runtime()
        .block_on(client_a.publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&published_app).unwrap(),
            is_published: true,
            changelog: Some("v1.0.0".into()),
        }))
        .expect("alice publish");
    client_a.clear_session();
    drop(client_a);

    let b_dir = isolated_temp_dir("t3_10_b").expect("b dir");
    let client_b = e2e_suite::gpui_browser_harness::open_client_in_dir(&server.base_url(), b_dir.path());
    let rt_b = client_b.runtime().handle().clone();
    let auth_b = rt_b
        .block_on(client_b.register(&RegisterRequest {
            username: "t3_10_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        }))
        .expect("register bob");

    let chrome_dir = isolated_temp_dir("t3_10_chrome").expect("chrome dir");
    let chrome = Arc::new(rt_b.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell_with_token(
                cx,
                client_b.clone(),
                auth_b.user_id,
                "t3_10_bob",
                &auth_b.token,
            )
        })
        .unwrap();

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(
                Arc::clone(&chrome) as Arc<dyn BrowserSessionBackend>,
                scx,
            );
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Store, wcx);
                w.store.search_query_draft = "Cross-Feature".to_string();
                w.search_store_apps(wcx);
            });
        });
    })
    .unwrap();

    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(20), |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.store.is_loading_catalog
                && w.store
                    .catalog_apps
                    .iter()
                    .any(|a| a.app_id == "t3-10-store-app")
        })
        .await
    );

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_install_store_app("t3-10-store-app", scx);
        });
    })
    .unwrap();

    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(20), |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            w.store
                .installed_apps
                .iter()
                .any(|a| a.app_id == "t3-10-store-app")
        })
        .await
    );

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            let inst = w
                .store
                .installed_apps
                .iter()
                .find(|a| a.app_id == "t3-10-store-app")
                .cloned()
                .unwrap();
            w.recorder
                .param_inputs
                .insert("customer_name".to_string(), "T3-10 Bob".to_string());
            w.recorder
                .param_inputs
                .insert("customer_email".to_string(), "bob@t3-10.test".to_string());
            w.run_installed_store_app(inst, wcx);
        });
    })
    .unwrap();

    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.recorder.is_replaying && w.recorder.last_replay_report.is_some()
        })
        .await
    );

    app.update(|cx| {
        let w = shell.read(cx).browser_workspace().unwrap().read(cx);
        let report = w.recorder.last_replay_report.as_ref().unwrap();
        assert!(report.success);
        assert!(!report.used_llm);
        assert!(
            report
                .extracted_outputs
                .get("order_id")
                .is_some_and(|s| s.starts_with("ORD-"))
        );
    })
    .unwrap();
    assert_eq!(web.order_count(), 1);
    assert!(
        client_b
            .get_installed_automation_app_cached("t3-10-store-app")
            .is_some()
    );

    let detail = rt_b
        .block_on(client_b.get_automation_app("t3-10-store-app"))
        .expect("get_automation_app");
    assert_eq!(detail.install_count, 1);
    assert_eq!(detail.run_count, 1);

    app.update(|_| drop(shell)).unwrap();
    rt_b.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    client_b.clear_session();
    drop(client_b);
    rt_boot.block_on(web.shutdown());
    rt_boot.block_on(server.stop());
}
