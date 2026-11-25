use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bot_browser::{
    AutomationScriptApp, BrowserProfileData, BrowserProfileStore, BrowserSessionBackend,
    CdpBrowserBackend, DeterministicScriptRunner, NormalizedOrigin,
    ScriptAppMetadata, import_cookies_from_json,
};
use e2e_suite::browser_e2e_fixtures::{
    MockOpenAiServer, MockWebAppServer, MockXServer, fake_secrets, generate_cdp_cookies_json,
    isolated_temp_dir, mock_openai_text, mock_openai_tool_call, x_auth_cookies,
};
use e2e_suite::common::EphemeralTestServer;
use gpui_telegram_agent::fabushi::FabushiBotHost;
use gpui_telegram_agent::hermes::providers::ProviderConfig;
use gpui_telegram_agent::{
    BrowserAutomationEvent, CommandUiEffect, HermesTurnRequest, ToolsetPreset,
};
use gpui_telegram_client::TelegramClient;
use gpui_telegram_client::config::ClientConfig;
use gpui_telegram_client::net::{LoginRequest, RegisterRequest};
use gpui_telegram_proto::automation_apps::{
    PublishAutomationAppRequest, RecordAutomationRunRequest,
};
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
            "timed out waiting for MockXServer tweet_count >= {expected} (got {})",
            server.tweet_count()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn full_turn_req(prompt: &str, session_id: &str) -> HermesTurnRequest {
    let mut req = HermesTurnRequest::new(prompt).with_session(session_id);
    req.preset = Some(ToolsetPreset::Full);
    req
}

/// [Test ID] TEST-BR-T4-01
/// [Feature / Req] F1, F2, F3, F5, F6 / R1, R2, R3, AC4, AC5 — X.com social media operations: cookie reuse -> AI post/like/repost -> `/distill` -> zero-LLM parameterized replay on fresh profile
/// [Preconditions] `MockXServer` started; seeded session using `fake_secrets()` tokens persisted into `BrowserProfileStore` via `generate_cdp_cookies_json` + `import_cookies_from_json`.
/// [Opaque-Box Steps] Profile 1 Chrome loads restored session, AI drives `/home` to publish Post A (`post_text`), like, and repost; `/distill` compiles `AutomationScriptApp`; Profile 2 fresh Chrome restores cookies from `BrowserProfileStore` and replays via `DeterministicScriptRunner` with `post_text = "Scripted Post B"`.
/// [Observable Assertions] `MockXServer` persists both tweets with `likes == 1` and `reposts == 1`, `used_llm == false`, mock LLM calls delta == 0, `FakeSecretFixtures::contains_any_secret` is `None`, and `step_timings` are complete and valid.
#[tokio::test]
async fn test_br_t4_01_x_social_cookie_reuse_ai_automation_distill_and_zero_llm_replay() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let secrets = fake_secrets();
    let seed_1 = server.seed_session(secrets.auth_token, secrets.ct0);
    let seed_2 = server.seed_session("tok_t4_01_replay_session", "ct0_t4_01_replay_session");

    let store_dir = isolated_temp_dir("t4_01_store").expect("store dir");
    let p1_dir = isolated_temp_dir("t4_01_p1").expect("p1 dir");
    let p2_dir = isolated_temp_dir("t4_01_p2").expect("p2 dir");

    let origin = NormalizedOrigin::parse(&server.base_url()).expect("origin");
    let store = BrowserProfileStore::new(store_dir.path());
    let imported_1 = import_cookies_from_json(&generate_cdp_cookies_json(&x_auth_cookies(
        "127.0.0.1",
        &seed_1.auth_token,
        &seed_1.ct0,
    )))
    .expect("parse CDP cookies 1");
    store
        .save_profile(
            "x_operator_profile_1",
            Some(&origin),
            &BrowserProfileData {
                cookies: imported_1,
                local_storage: BTreeMap::new(),
                updated_at_ms: 1_700_000_000_000,
            },
        )
        .expect("save profile 1");

    let imported_2 = import_cookies_from_json(&generate_cdp_cookies_json(&x_auth_cookies(
        "127.0.0.1",
        &seed_2.auth_token,
        &seed_2.ct0,
    )))
    .expect("parse CDP cookies 2");
    store
        .save_profile(
            "x_operator_profile_2",
            Some(&origin),
            &BrowserProfileData {
                cookies: imported_2,
                local_storage: BTreeMap::new(),
                updated_at_ms: 1_700_000_000_001,
            },
        )
        .expect("save profile 2");

    let b1 = Arc::new(launch_chrome_in(p1_dir.path()).await);
    let loaded_p1 = store
        .load_profile("x_operator_profile_1", Some(&origin))
        .expect("load p1");
    b1.import_cookies(&loaded_p1.cookies)
        .await
        .expect("import cookies p1");

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.register_browser_session("tab_t4_01", Arc::clone(&b1) as Arc<dyn BrowserSessionBackend>);
    let mut ev_rx = host.subscribe_browser_events();

    let post_a = "AI Operational Post A";
    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call(
            "x1",
            "browser_navigate",
            json!({ "url": server.url("/home") }),
        ),
        mock_openai_tool_call(
            "x2",
            "browser_wait_for",
            json!({ "test_id": "tweetTextarea_0", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "x3",
            "browser_click",
            json!({ "css_selector": "[data-testid='like'][data-tweet-id='1001']" }),
        ),
        mock_openai_tool_call(
            "x4",
            "browser_click",
            json!({ "css_selector": "[data-testid='repost'][data-tweet-id='1002']" }),
        ),
        mock_openai_tool_call(
            "x5",
            "browser_type",
            json!({
                "test_id": "tweetTextarea_0",
                "text": post_a,
                "parameter_name_hint": "post_text"
            }),
        ),
        mock_openai_tool_call(
            "x6",
            "browser_click",
            json!({ "test_id": "tweetButtonInline" }),
        ),
        mock_openai_tool_call(
            "x7",
            "browser_wait_for",
            json!({ "text_anchor": post_a, "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "x8",
            "browser_extract_text",
            json!({ "css_selector": "#timeline article:first-child [data-testid='tweetText']", "output_key": "published_text" }),
        ),
        mock_openai_text("Published, liked, and reposted on MockX."),
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

    let ai_turn = engine
        .run_turn(full_turn_req("Post, like, and repost on X", "tab_t4_01"))
        .await
        .expect("AI turn succeeds");
    assert!(ai_turn.tool_calls.iter().all(|tc| tc.success));
    wait_for_tweet_count(&server, 3, Duration::from_secs(3)).await;
    tokio::time::sleep(Duration::from_millis(80)).await;

    let mut step_started = 0usize;
    let mut step_completed = 0usize;
    while let Ok(ev) = ev_rx.try_recv() {
        match ev {
            BrowserAutomationEvent::StepStarted { .. } => step_started += 1,
            BrowserAutomationEvent::StepCompleted { .. } => step_completed += 1,
            _ => {}
        }
    }
    assert_eq!(step_started, 8);
    assert_eq!(step_completed, 8);

    // Distill via /distill
    let distill_turn = engine
        .run_turn(HermesTurnRequest::new("/distill X Social Operations").with_session("tab_t4_01"))
        .await
        .expect("/distill");
    let app_json = distill_turn
        .ui_effects
        .iter()
        .find_map(|eff| match eff {
            CommandUiEffect::DistillBrowserScript { app_json, .. } => Some(app_json.clone()),
            _ => None,
        })
        .expect("DistillBrowserScript effect");
    assert_eq!(
        secrets.contains_any_secret(&app_json),
        None,
        "distilled app must contain zero secret material"
    );
    let app: AutomationScriptApp = serde_json::from_str(&app_json).expect("parse distilled app");
    assert!(app.parameters.iter().any(|p| p.name == "post_text"));

    let llm_calls_before_replay = llm.call_count();
    b1.close().await.expect("close b1");

    let b2 = launch_chrome_in(p2_dir.path()).await;
    let loaded_p2 = store
        .load_profile("x_operator_profile_2", Some(&origin))
        .expect("load p2");
    b2.import_cookies(&loaded_p2.cookies)
        .await
        .expect("import cookies p2");

    let post_b = "Scripted Post B";
    let params = BTreeMap::from([("post_text".to_string(), post_b.to_string())]);
    let report = DeterministicScriptRunner::run(&b2, &app, &params, None, None)
        .await
        .expect("deterministic replay succeeds");
    assert!(report.success);
    assert!(!report.used_llm);
    assert_eq!(llm.call_count(), llm_calls_before_replay);
    assert_eq!(report.step_timings.len(), report.steps_executed);
    assert!(report.step_timings.iter().all(|st| st.success));

    wait_for_tweet_count(&server, 4, Duration::from_secs(3)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let (likes_1001, reposts_1002) = {
            let tweets = server.state().tweets.read();
            let tw_1001 = tweets.iter().find(|t| t.id == "1001").expect("tweet 1001");
            let tw_1002 = tweets.iter().find(|t| t.id == "1002").expect("tweet 1002");
            (tw_1001.likes, tw_1002.reposts)
        };
        if likes_1001 == 2 && reposts_1002 == 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for likes=2 (got {likes_1001}) and reposts=2 (got {reposts_1002})"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    {
        let tweets = server.state().tweets.read();
        assert!(tweets.iter().any(|t| t.text == post_a));
        assert!(tweets.iter().any(|t| t.text == post_b));
        let tw_1001 = tweets.iter().find(|t| t.id == "1001").expect("tweet 1001");
        let tw_1002 = tweets.iter().find(|t| t.id == "1002").expect("tweet 1002");
        assert_eq!(tw_1001.likes, 2);
        assert_eq!(tw_1002.reposts, 2);
    }

    b2.close().await.expect("close b2");
    llm.shutdown().await;
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T4-02
/// [Feature / Req] F1, F3, F5, F6 / R2, R3, AC5 — General web workflow: search, category filter, pagination, item select, cart & checkout -> distill -> zero-LLM parameterized replay
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI browses paginated search page 2 (`next-page`), returns to `/` to filter category (`books`) & search `"rust"` (`query`), selects first result row, adds to cart, checks out (`customer_name`, `customer_email`) for Order #1; distills via `/distill`; replays on fresh Chrome with `query="browser"`, `customer_name="Alice"`, `customer_email="alice@example.test"` (`used_llm == false`).
/// [Observable Assertions] `server.order_count() == 2`, both orders have distinct `ORD-...` IDs, Order #2 reflects `Alice` / `alice@example.test` / `item-02` (`"Browser Automation Engineering"`), and mock LLM call count delta == 0 during replay.
#[tokio::test]
async fn test_br_t4_02_general_web_search_filter_paginate_checkout_distill_replay() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let p1_dir = isolated_temp_dir("t4_02_p1").expect("p1 dir");
    let p2_dir = isolated_temp_dir("t4_02_p2").expect("p2 dir");
    let b1 = Arc::new(launch_chrome_in(p1_dir.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.register_browser_session("tab_t4_02", Arc::clone(&b1) as Arc<dyn BrowserSessionBackend>);

    let llm = MockOpenAiServer::start(vec![
        // Step 1: Multi-page browse -> paginate to page 2
        mock_openai_tool_call(
            "g1",
            "browser_navigate",
            json!({ "url": server.url("/search?q=&category=all&page_size=5") }),
        ),
        mock_openai_tool_call(
            "g2",
            "browser_wait_for",
            json!({ "test_id": "next-page", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call("g3", "browser_click", json!({ "test_id": "next-page" })),
        mock_openai_tool_call(
            "g4",
            "browser_wait_for",
            json!({ "text_anchor": "Page 2 of 3", "timeout_ms": 4000 }),
        ),
        // Step 2: Search + category filter ('books') + parameterized query
        mock_openai_tool_call("g5", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_tool_call(
            "g6",
            "browser_type",
            json!({
                "test_id": "search-input",
                "text": "rust",
                "parameter_name_hint": "query"
            }),
        ),
        mock_openai_tool_call(
            "g7",
            "browser_click",
            json!({ "test_id": "category-select" }),
        ),
        mock_openai_tool_call("g8", "browser_press_key", json!({ "key": "ArrowDown" })),
        mock_openai_tool_call("g9", "browser_press_key", json!({ "key": "Tab" })),
        mock_openai_tool_call(
            "g10",
            "browser_click",
            json!({ "test_id": "search-submit" }),
        ),
        mock_openai_tool_call(
            "g11",
            "browser_wait_for",
            json!({ "css_selector": "table[data-testid='results-table'] tbody tr a", "timeout_ms": 4000 }),
        ),
        // Step 3: Select first matching result -> Add to Cart -> Checkout
        mock_openai_tool_call(
            "g12",
            "browser_click",
            json!({ "css_selector": "table[data-testid='results-table'] tbody tr a" }),
        ),
        mock_openai_tool_call(
            "g13",
            "browser_wait_for",
            json!({ "test_id": "add-to-cart-btn", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "g14",
            "browser_click",
            json!({ "test_id": "add-to-cart-btn" }),
        ),
        mock_openai_tool_call(
            "g15",
            "browser_wait_for",
            json!({ "test_id": "cart-status", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "g16",
            "browser_click",
            json!({ "test_id": "go-to-checkout" }),
        ),
        mock_openai_tool_call(
            "g17",
            "browser_wait_for",
            json!({ "test_id": "checkout-name", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "g18",
            "browser_type",
            json!({
                "test_id": "checkout-name",
                "text": "Operator One",
                "parameter_name_hint": "customer_name"
            }),
        ),
        mock_openai_tool_call(
            "g19",
            "browser_type",
            json!({
                "test_id": "checkout-email",
                "text": "operator1@example.test",
                "parameter_name_hint": "customer_email"
            }),
        ),
        mock_openai_tool_call(
            "g20",
            "browser_click",
            json!({ "test_id": "checkout-submit" }),
        ),
        mock_openai_tool_call(
            "g21",
            "browser_wait_for",
            json!({ "test_id": "confirmation-id", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "g22",
            "browser_extract_text",
            json!({ "test_id": "confirmation-id", "output_key": "confirmation_id" }),
        ),
        mock_openai_text("Completed general web workflow order #1."),
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
        .run_turn(full_turn_req(
            "Browse, search, filter, and checkout",
            "tab_t4_02",
        ))
        .await
        .expect("AI turn succeeds");
    assert_eq!(res.tool_calls.len(), 22);
    assert!(res.tool_calls.iter().all(|tc| tc.success));
    assert_eq!(server.order_count(), 1);

    let distill_res = engine
        .run_turn(HermesTurnRequest::new("/distill Catalog Order Workflow").with_session("tab_t4_02"))
        .await
        .expect("/distill");
    let app_json = distill_res
        .ui_effects
        .iter()
        .find_map(|eff| match eff {
            CommandUiEffect::DistillBrowserScript { app_json, .. } => Some(app_json.clone()),
            _ => None,
        })
        .expect("DistillBrowserScript");
    let app: AutomationScriptApp = serde_json::from_str(&app_json).expect("parse distilled app");
    assert!(app.parameters.iter().any(|p| p.name == "query"));
    assert!(app.parameters.iter().any(|p| p.name == "customer_name"));
    assert!(app.parameters.iter().any(|p| p.name == "customer_email"));

    let llm_calls_before_replay = llm.call_count();
    b1.close().await.expect("close b1");

    // Zero-LLM replay on fresh Chrome with new parameters
    let b2 = launch_chrome_in(p2_dir.path()).await;
    let params = BTreeMap::from([
        ("query".to_string(), "browser".to_string()),
        ("customer_name".to_string(), "Alice".to_string()),
        ("customer_email".to_string(), "alice@example.test".to_string()),
    ]);
    let report = DeterministicScriptRunner::run(&b2, &app, &params, None, None)
        .await
        .expect("zero-LLM replay order #2");
    assert!(report.success);
    assert!(!report.used_llm);
    assert_eq!(llm.call_count(), llm_calls_before_replay);
    assert!(
        report
            .extracted_outputs
            .get("confirmation_id")
            .is_some_and(|id| id.starts_with("ORD-"))
    );
    assert_eq!(server.order_count(), 2);

    {
        let orders = server.state().orders.read();
        let ord1 = orders
            .values()
            .find(|o| o.customer_name == "Operator One")
            .expect("Order #1");
        let ord2 = orders
            .values()
            .find(|o| o.customer_name == "Alice")
            .expect("Order #2");
        assert_ne!(ord1.confirmation_id, ord2.confirmation_id);
        assert!(ord1.confirmation_id.starts_with("ORD-"));
        assert!(ord2.confirmation_id.starts_with("ORD-"));
        assert_eq!(ord2.customer_email, "alice@example.test");
        assert_eq!(ord2.items[0].item_id, "item-02");
    }

    b2.close().await.expect("close b2");
    llm.shutdown().await;
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T4-04
/// [Feature / Req] F3, F4, F5, F6 / R3, R4, AC5, AC6 — Cross-client App Store lifecycle: User A distills & publishes -> `server.restart()` -> User B searches, installs, replays zero-LLM & reports run telemetry
/// [Preconditions] `EphemeralTestServer` (SQLite v41) and `MockWebAppServer` started; User A & User B registered.
/// [Opaque-Box Steps] User A records & distills a `MockWebAppServer` checkout Script App via `FabushiBotHost`, publishes via `TelegramClient::publish_automation_app`, restarts `EphemeralTestServer`; User B (fresh `LocalStore` & Chrome profile) logs in, searches catalog (`q`, `category`, `domain`), fetches detail, installs app, executes zero-LLM replay on `MockWebAppServer`, and submits `record_automation_app_run`.
/// [Observable Assertions] `install_count == 1`, `run_count == 1`, User B's zero-LLM replay (`used_llm == false`) mutates `MockWebAppServer` order ledger with an `ORD-...` confirmation ID, and metrics persist across restart.
#[tokio::test]
async fn test_br_t4_04_cross_client_app_store_publish_restart_install_and_replay() {
    let mut store_server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let webapp = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");

    let alice_client_dir = isolated_temp_dir("t4_04_alice_client").expect("alice client dir");
    let bob_client_dir = isolated_temp_dir("t4_04_bob_client").expect("bob client dir");
    let alice_chrome_dir = isolated_temp_dir("t4_04_alice_chrome").expect("alice chrome dir");
    let bob_chrome_dir = isolated_temp_dir("t4_04_bob_chrome").expect("bob chrome dir");

    let cfg1 = ClientConfig {
        server_url: store_server.base_url(),
        ..Default::default()
    };
    let client_a =
        TelegramClient::open_with_dir(cfg1.clone(), alice_client_dir.path()).expect("client_a");
    client_a
        .register(&RegisterRequest {
            username: "t4_04_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register alice");

    // Pre-register Bob before restart so Bob exists in persisted SQLite DB
    let client_b_reg =
        TelegramClient::open_with_dir(cfg1, bob_client_dir.path()).expect("client_b_reg");
    client_b_reg
        .register(&RegisterRequest {
            username: "t4_04_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        })
        .await
        .expect("register bob");
    client_b_reg.clear_session();
    tokio::task::spawn_blocking(move || drop(client_b_reg))
        .await
        .unwrap();

    // User A drives MockWebAppServer via AI & distills Script App
    let chrome_a = Arc::new(launch_chrome_in(alice_chrome_dir.path()).await);
    let host_a = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host_a");
    host_a.register_browser_session(
        "tab_alice",
        Arc::clone(&chrome_a) as Arc<dyn BrowserSessionBackend>,
    );

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call(
            "a1",
            "browser_navigate",
            json!({ "url": webapp.url("/item/item-03") }),
        ),
        mock_openai_tool_call(
            "a2",
            "browser_click",
            json!({ "test_id": "add-to-cart-btn" }),
        ),
        mock_openai_tool_call(
            "a3",
            "browser_wait_for",
            json!({ "test_id": "cart-status", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "a4",
            "browser_click",
            json!({ "test_id": "go-to-checkout" }),
        ),
        mock_openai_tool_call(
            "a5",
            "browser_wait_for",
            json!({ "test_id": "checkout-name", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "a6",
            "browser_type",
            json!({
                "test_id": "checkout-name",
                "text": "Alice Author",
                "parameter_name_hint": "customer_name"
            }),
        ),
        mock_openai_tool_call(
            "a7",
            "browser_type",
            json!({
                "test_id": "checkout-email",
                "text": "alice@author.test",
                "parameter_name_hint": "customer_email"
            }),
        ),
        mock_openai_tool_call(
            "a8",
            "browser_click",
            json!({ "test_id": "checkout-submit" }),
        ),
        mock_openai_tool_call(
            "a9",
            "browser_wait_for",
            json!({ "test_id": "confirmation-id", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "a10",
            "browser_extract_text",
            json!({ "test_id": "confirmation-id", "output_key": "order_id" }),
        ),
        mock_openai_text("Alice completed initial run."),
    ])
    .await
    .expect("start MockOpenAiServer");

    let engine_a = host_a.core_engine();
    engine_a.providers().register_provider(ProviderConfig::openai_compatible(
        "mock_openai",
        "Mock OpenAI",
        llm.base_url(),
        "mock-gpt",
        "sk-mock-test",
    ));
    engine_a
        .providers()
        .set_active("mock_openai", Some("mock-gpt"))
        .expect("set active");

    let alice_turn = engine_a
        .run_turn(full_turn_req("Order item-03", "tab_alice"))
        .await
        .expect("Alice turn");
    assert!(alice_turn.tool_calls.iter().all(|tc| tc.success));
    assert_eq!(webapp.order_count(), 1);

    let distilled = host_a
        .distill_browser_script(
            "tab_alice",
            ScriptAppMetadata {
                app_id: Some("app-t4-04-webapp-checkout".into()),
                name: "WebApp Checkout Runner".into(),
                description: "Replays MockWebApp item-03 checkout without LLM".into(),
                category: "productivity".into(),
                target_domains: vec!["127.0.0.1".into()],
                ..Default::default()
            },
        )
        .expect("distill Alice trace");

    let published = client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&distilled).unwrap(),
            is_published: true,
            changelog: Some("v1.0.0".into()),
        })
        .await
        .expect("Alice publishes app");
    assert_eq!(published.app_id, "app-t4-04-webapp-checkout");

    let llm_calls_after_alice = llm.call_count();
    chrome_a.close().await.expect("close chrome_a");
    client_a.clear_session();
    tokio::task::spawn_blocking(move || drop(client_a))
        .await
        .unwrap();

    // Restart EphemeralTestServer to verify durable SQLite persistence
    store_server.restart().await.expect("restart store_server");

    // User B with fresh LocalStore & clean Chrome profile discovers, installs, and replays
    let bob_fresh_store_dir = isolated_temp_dir("t4_04_bob_fresh_store").expect("bob fresh store");
    let client_b = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: store_server.base_url(),
            ..Default::default()
        },
        bob_fresh_store_dir.path(),
    )
    .expect("client_b");
    client_b
        .login(&LoginRequest {
            username: "t4_04_bob".into(),
            password: "StrongPassword!123".into(),
            device_name: "mac-b".into(),
        })
        .await
        .expect("login bob");

    let search = client_b
        .search_automation_apps(
            Some("Checkout"),
            Some("productivity"),
            Some("127.0.0.1"),
            None,
            None,
        )
        .await
        .expect("Bob searches catalog");
    assert_eq!(search.total, 1);
    assert_eq!(search.apps[0].app_id, "app-t4-04-webapp-checkout");

    let fetched = client_b
        .get_automation_app("app-t4-04-webapp-checkout")
        .await
        .expect("Bob fetches manifest");
    assert_eq!(fetched.latest_version, "1.0.0");

    let installed = client_b
        .install_automation_app("app-t4-04-webapp-checkout")
        .await
        .expect("Bob installs app");
    assert!(installed.newly_installed);
    assert_eq!(installed.install_count, 1);

    let bob_app: AutomationScriptApp =
        serde_json::from_value(installed.app.manifest).expect("deserialize installed manifest");

    let chrome_b = launch_chrome_in(bob_chrome_dir.path()).await;
    let params = BTreeMap::from([
        ("customer_name".to_string(), "Bob Buyer".to_string()),
        ("customer_email".to_string(), "bob@buyer.test".to_string()),
    ]);
    let report = DeterministicScriptRunner::run(&chrome_b, &bob_app, &params, None, None)
        .await
        .expect("Bob replays zero-LLM");
    assert!(report.success);
    assert!(!report.used_llm);
    assert_eq!(llm.call_count(), llm_calls_after_alice);
    let bob_order_id = report
        .extracted_outputs
        .get("order_id")
        .cloned()
        .expect("extracted order_id");
    assert!(bob_order_id.starts_with("ORD-"));
    assert_eq!(webapp.order_count(), 2);

    {
        let orders = webapp.state().orders.read();
        let bob_order = orders.get(&bob_order_id).expect("Bob order recorded");
        assert_eq!(bob_order.customer_name, "Bob Buyer");
        assert_eq!(bob_order.customer_email, "bob@buyer.test");
    }

    let run_stat = client_b
        .record_automation_app_run(
            "app-t4-04-webapp-checkout",
            &RecordAutomationRunRequest {
                version: Some("1.0.0".into()),
                success: report.success,
                steps_executed: report.steps_executed,
                total_duration_ms: report.total_duration_ms,
                used_llm: report.used_llm,
                error: None,
            },
        )
        .await
        .expect("Bob records run telemetry");
    assert_eq!(run_stat.run_count, 1);

    let final_detail = client_b
        .get_automation_app("app-t4-04-webapp-checkout")
        .await
        .expect("final detail");
    assert_eq!(final_detail.install_count, 1);
    assert_eq!(final_detail.run_count, 1);

    chrome_b.close().await.expect("close chrome_b");
    client_b.clear_session();
    tokio::task::spawn_blocking(move || drop(client_b))
        .await
        .unwrap();
    llm.shutdown().await;
    webapp.shutdown().await;
    store_server.stop().await;
}

// ============================================================================
// F7 & F8 Headless GPUI Subprocess Harness + Tier 4 Scenarios (T4-03, T4-05)
// ============================================================================

const T4_CHILD_ENV: &str = "GPUI_E2E_T4_HEADLESS_CHILD";
const T4_CHILD_TARGET_ENV: &str = "GPUI_E2E_T4_HEADLESS_TARGET";
const T4_CHILD_AFTER_CTORS_ENV: &str = "GPUI_E2E_T4_HEADLESS_AFTER_CTORS";

fn t4_dispatch_headless_target(target: &str, use_underscore_exit: bool) -> ! {
    type AsyncBody = fn(gpui::AsyncApp) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>;
    let body: AsyncBody = match target {
        "test_br_t4_03_gpui_workspace_ai_to_user_takeover_distill_and_replay" => {
            |app| Box::pin(body_br_t4_03(app))
        }
        "test_br_t4_05_credential_reuse_e2ee_tcc_fallback_and_nonblocking_latency_audit" => {
            |app| Box::pin(body_br_t4_05(app))
        }
        other => {
            eprintln!("unknown e2e tier4 headless target: {other}");
            e2e_suite::gpui_browser_harness::exit_headless_child(101, use_underscore_exit);
        }
    };
    e2e_suite::gpui_browser_harness::run_headless_gpui_body(target, use_underscore_exit, body);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_T4_HEADLESS_MAIN_THREAD_ENTRY: extern "C" fn() = t4_macos_mod_init_headless_entry;

#[cfg(target_os = "macos")]
extern "C" fn t4_macos_atexit_headless_entry() {
    if let Ok(target) = std::env::var(T4_CHILD_TARGET_ENV) {
        t4_dispatch_headless_target(&target, true);
    }
}

#[cfg(target_os = "macos")]
extern "C" fn t4_macos_mod_init_headless_entry() {
    if std::env::var(T4_CHILD_ENV).as_deref() == Ok("1") {
        if std::env::var(T4_CHILD_AFTER_CTORS_ENV).as_deref() == Ok("1") {
            unsafe extern "C" {
                fn atexit(func: extern "C" fn()) -> i32;
            }
            unsafe {
                atexit(t4_macos_atexit_headless_entry);
            }
            return;
        }
        if let Ok(target) = std::env::var(T4_CHILD_TARGET_ENV) {
            t4_dispatch_headless_target(&target, false);
        }
    }
}

fn run_t4_in_headless_subprocess(test_fn_name: &str) {
    if std::env::var(T4_CHILD_ENV).as_deref() == Ok("1") {
        t4_dispatch_headless_target(test_fn_name, false);
    }
    e2e_suite::gpui_browser_harness::spawn_headless_subprocess(
        T4_CHILD_ENV,
        T4_CHILD_TARGET_ENV,
        T4_CHILD_AFTER_CTORS_ENV,
        test_fn_name,
        false,
        None,
        None,
    );
}

/// [Test ID] TEST-BR-T4-03
/// [Feature / Req] F5, F6, F7, F8 / R1, R2, R3, AC2, AC5 — Full AI-to-user takeover, `CdpViewportSurface` interaction, trace distillation (`HumanCheckpoint`), and zero-LLM replay in GPUI `BrowserWorkspace`
/// [Preconditions] `MockXServer` (`/challenge` + `/home`) + `MockOpenAiServer` + Chrome `CdpBrowserBackend` wired through `AppShell::open_browser_workspace` & `FabushiBotHost`.
/// [Opaque-Box Steps] Trigger AI via `trigger_copilot_prompt` -> pause at `WaitingForTakeover` -> invoke `browser_user_takeover` -> click `human-captcha-checkbox` & `challenge-continue-btn` via `CdpViewportSurface::handle_click` while verifying non-blocking GPUI ticks -> invoke `browser_resume_ai` -> AI posts tweet -> `browser_distill_last_trace` -> replay distilled app (handling `HumanCheckpoint` takeover) with `used_llm == false`.
/// [Observable Assertions] `MockXServer` records verified challenge token & both tweets (AI + zero-LLM replay), `max_tick_gap_ms < 150`, distilled script includes `ScriptStep::HumanCheckpoint`, zero secrets leaked, and `llm.call_count()` stays unchanged during replay.
#[test]
fn test_br_t4_03_gpui_workspace_ai_to_user_takeover_distill_and_replay() {
    run_t4_in_headless_subprocess(
        "test_br_t4_03_gpui_workspace_ai_to_user_takeover_distill_and_replay",
    );
}

async fn body_br_t4_03(mut app: gpui::AsyncApp) {
    use bot_browser::{BrowserControlMode, PointerButton, ScriptStep};

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let x_server = rt.block_on(MockXServer::start()).expect("MockXServer");
    let seed = x_server.seed_session("tok_t4_03", "ct0_t4_03");
    let chrome_dir = isolated_temp_dir("t4_03").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let cookies = import_cookies_from_json(&generate_cdp_cookies_json(&x_auth_cookies(
        "127.0.0.1",
        &seed.auth_token,
        &seed.ct0,
    )))
    .expect("cookies");
    rt.block_on(chrome.import_cookies(&cookies))
        .expect("import cookies");

    let ai_post = "T4-03 AI takeover post";
    let llm = rt
        .block_on(MockOpenAiServer::start(vec![
            mock_openai_tool_call(
                "t4_03_1",
                "browser_navigate",
                json!({ "url": x_server.url("/challenge") }),
            ),
            mock_openai_tool_call(
                "t4_03_2",
                "browser_request_takeover",
                json!({ "reason": "Solve human checkpoint", "wait_for_finish": true }),
            ),
            mock_openai_tool_call(
                "t4_03_3",
                "browser_wait_for",
                json!({ "test_id": "tweetTextarea_0", "timeout_ms": 5000 }),
            ),
            mock_openai_tool_call(
                "t4_03_4",
                "browser_type",
                json!({
                    "test_id": "tweetTextarea_0",
                    "text": ai_post,
                    "parameter_name_hint": "post_text"
                }),
            ),
            mock_openai_tool_call(
                "t4_03_5",
                "browser_click",
                json!({ "test_id": "tweetButtonInline" }),
            ),
            mock_openai_text("Completed human checkpoint and posted tweet."),
        ]))
        .expect("MockOpenAiServer");

    let host = Arc::new(FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host"));
    e2e_suite::gpui_browser_harness::configure_host_with_mock_llm(&host, &llm.base_url());

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                403,
                "t4_03",
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
                w.recorder.copilot_prompt_draft = "Solve challenge and post tweet".to_string();
                w.trigger_copilot_prompt(wcx);
            });
        });
    })
    .unwrap();

    // Wait for AI to hit HumanCheckpoint
    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
            matches!(
                shell.read(cx).browser_control_mode(cx),
                BrowserControlMode::WaitingForTakeover { .. }
            )
        })
        .await
    );

    // User invokes takeover and manually solves /challenge via CdpViewportSurface
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

    let (verified_done, _ticks, max_gap_cb) =
        e2e_suite::gpui_browser_harness::pump_with_ticks(
            &mut app,
            Duration::from_secs(15),
            Duration::from_millis(15),
            |_, _| {
                x_server
                    .state()
                    .challenge_verified_tokens
                    .read()
                    .contains(&seed.auth_token)
            },
        )
        .await;
    assert!(verified_done, "checkbox click must verify challenge");
    assert!(max_gap_cb < 150, "GPUI UI tick gap too high during takeover click: {max_gap_cb}ms");

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

    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |_| {
            rt.block_on(chrome.current_url())
                .map(|u| u.contains("/home"))
                .unwrap_or(false)
        })
        .await
    );

    // Resume AI -> AI posts tweet on /home
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_resume_ai(scx);
        });
    })
    .unwrap();

    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
            let s = shell.read(cx);
            let w = s.browser_workspace().unwrap().read(cx);
            !w.recorder.copilot_running
                && matches!(s.browser_control_mode(cx), BrowserControlMode::Idle)
        })
        .await
    );
    rt.block_on(e2e_suite::gpui_browser_harness::wait_for_tweet_count(
        &x_server,
        3,
        Duration::from_secs(15),
    ));

    // Distill trace and assert HumanCheckpoint preserved & zero secrets
    let distilled = app
        .update(|cx| {
            shell.update(cx, |s, scx| {
                let app = s
                    .browser_distill_last_trace(
                        "T4-03 Challenge Poster",
                        "Takeover + post workflow",
                        "social",
                        scx,
                    )
                    .expect("distill_last_trace");
                let w = s.browser_workspace().unwrap().read(scx);
                assert!(
                    w.recorder
                        .event_logs
                        .iter()
                        .any(|e| e.title.contains("Takeover requested"))
                );
                assert!(
                    w.recorder
                        .event_logs
                        .iter()
                        .any(|e| e.duration_ms.is_some())
                );
                app
            })
        })
        .unwrap();

    assert!(
        distilled
            .steps
            .iter()
            .any(|st| matches!(st, ScriptStep::HumanCheckpoint { .. })),
        "distilled ScriptApp must preserve HumanCheckpoint"
    );
    let distilled_json = serde_json::to_string(&distilled).unwrap();
    assert!(fake_secrets().contains_any_secret(&distilled_json).is_none());

    // Zero-LLM replay of distilled script (reset challenge state so HumanCheckpoint is genuinely solved)
    let llm_calls_before_replay = llm.call_count();
    x_server
        .state()
        .challenge_verified_tokens
        .write()
        .remove(&seed.auth_token);

    let replay_post = "T4-03 Zero-LLM Replay Post";
    let params = BTreeMap::from([("post_text".to_string(), replay_post.to_string())]);
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_run_script_app(distilled.clone(), params, scx);
        });
    })
    .unwrap();

    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
            matches!(
                shell.read(cx).browser_control_mode(cx),
                BrowserControlMode::WaitingForTakeover { .. }
            )
        })
        .await
    );

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_user_takeover(scx);
        });
    })
    .unwrap();
    let (cb_x2, cb_y2) = rt.block_on(e2e_suite::gpui_browser_harness::query_element_center_css(
        chrome.as_ref(),
        "[data-testid=\"human-captcha-checkbox\"]",
    ));
    let (cont_x2, cont_y2) = rt.block_on(
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
            sr.handle_click(cb_x2, cb_y2, PointerButton::Left, 1, 0, scx);
        });
    })
    .unwrap();
    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |_| {
            x_server
                .state()
                .challenge_verified_tokens
                .read()
                .contains(&seed.auth_token)
        })
        .await
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
            sr.handle_click(cont_x2, cont_y2, PointerButton::Left, 1, 0, scx);
        });
    })
    .unwrap();
    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |_| {
            rt.block_on(chrome.current_url())
                .map(|u| u.contains("/home"))
                .unwrap_or(false)
        })
        .await
    );

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_resume_ai(scx);
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
    })
    .unwrap();
    assert_eq!(llm.call_count(), llm_calls_before_replay);
    rt.block_on(e2e_suite::gpui_browser_harness::wait_for_tweet_count(
        &x_server,
        4,
        Duration::from_secs(15),
    ));
    assert!(
        x_server
            .state()
            .tweets
            .read()
            .iter()
            .any(|t| t.text == replay_post)
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

/// [Test ID] TEST-BR-T4-05
/// [Feature / Req] F1, F2, F3, F7, F8 / R1, AC2, AC3, AC4 — Credential/Session reuse (`e2ee_sync` `UserVaultCategory::BrowserCookie` + TCC `EPERM` fallback + JSON import) & non-blocking GPUI UI latency telemetry audit
/// [Preconditions] Isolated temp `HOME` with `0o000` Chrome `Cookies` file; `MockXServer`, `MockWebAppServer`, `DelayedHtmlServer`, and real Chrome `CdpBrowserBackend`s.
/// [Opaque-Box Steps] Probe local Chrome (`PermissionDeniedTcc`), import JSON cookies into `BrowserWorkspace`, seal via `UserVaultCategory::BrowserCookie`, backup to `CredentialVault`, restore into fresh Chrome #2 + `AppShell` #2, verify authenticated `MockXServer` `/home` SPA & `MockWebAppServer` `/search`, then run delayed page load + zero-LLM checkout script concurrently with 15ms GPUI ticks.
/// [Observable Assertions] `ChromeProbeUiState::PermissionDeniedTcc` reported gracefully; E2EE vault blob hides all `fake_secrets()`; restored Chrome #2 renders authenticated `/home`; `LatencyBudget` and GPUI UI tick gap remain bounded (`max_gap_ms < 150`); structured latency & SPA report printed via `eprintln!`.
#[test]
fn test_br_t4_05_credential_reuse_e2ee_tcc_fallback_and_nonblocking_latency_audit() {
    run_t4_in_headless_subprocess(
        "test_br_t4_05_credential_reuse_e2ee_tcc_fallback_and_nonblocking_latency_audit",
    );
}

async fn body_br_t4_05(mut app: gpui::AsyncApp) {
    use std::os::unix::fs::PermissionsExt;
    use bot_browser::{
        probe_chrome_cookies_at, Argon2KdfParams, ScriptParameter, ScriptStep, SemanticLocator,
        UserVaultCategory, UserVaultCrypto, SCRIPT_APP_SCHEMA_VERSION,
    };
    use e2e_suite::browser_e2e_fixtures::LatencyBudget;
    use gpui_telegram_desktop::views::browser::ChromeProbeUiState;

    let temp_chrome_profile = isolated_temp_dir("t4_05_chrome_tcc").expect("temp tcc dir");
    let cookie_path = temp_chrome_profile
        .path()
        .join("Google/Chrome/Default/Cookies");
    std::fs::create_dir_all(cookie_path.parent().unwrap()).expect("create chrome dir");
    std::fs::write(&cookie_path, b"SQLite format 3\0").expect("write dummy cookie db");
    std::fs::set_permissions(&cookie_path, std::fs::Permissions::from_mode(0o000))
        .expect("chmod 000");

    struct RestorePerms(std::path::PathBuf);
    impl Drop for RestorePerms {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o600));
        }
    }
    let _perm_guard = RestorePerms(cookie_path.clone());

    let rt_boot = tokio::runtime::Runtime::new().unwrap();
    let x_server = rt_boot.block_on(MockXServer::start()).expect("MockXServer");
    let web = rt_boot
        .block_on(MockWebAppServer::start())
        .expect("MockWebAppServer");
    let delayed = rt_boot
        .block_on(e2e_suite::gpui_browser_harness::DelayedHtmlServer::start(300))
        .expect("DelayedHtmlServer");

    let secrets = fake_secrets();
    let seed = x_server.seed_session(secrets.auth_token, secrets.ct0);
    let cdp_json = generate_cdp_cookies_json(&x_auth_cookies(
        "127.0.0.1",
        &seed.auth_token,
        &seed.ct0,
    ));

    let (vault_key, _env, _rec) =
        UserVaultCrypto::initialize_new(505, "t4-05-passphrase", Argon2KdfParams::fast_for_tests())
            .expect("UserVaultCrypto::initialize_new");
    let key_arc = Arc::new(vault_key);

    // Phase 1: TCC probe fallback + JSON cookie import + E2EE BrowserCookie seal + CredentialVault backup
    let sealed_blob;
    {
        let (_cdir1, client1) =
            e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
        let rt1 = client1.runtime().handle().clone();
        let ch1_dir = isolated_temp_dir("t4_05_ch1").expect("ch1 dir");
        let chrome1 = Arc::new(rt1.block_on(launch_chrome_in(ch1_dir.path())));

        let shell1 = app
            .update(|cx| {
                e2e_suite::gpui_browser_harness::build_authenticated_shell(
                    cx,
                    client1.clone(),
                    505,
                    "t4_05_a",
                )
            })
            .unwrap();

        let tcc_probe = probe_chrome_cookies_at(&cookie_path);
        app.update(|cx| {
            shell1.update(cx, |s, scx| {
                s.set_browser_backend_for_test(
                    Arc::clone(&chrome1) as Arc<dyn BrowserSessionBackend>,
                    scx,
                );
                let bw = s.browser_workspace().unwrap().clone();
                bw.update(scx, |w, wcx| {
                    w.set_unlocked_vault_key(Some(Arc::clone(&key_arc)));
                    w.apply_chrome_probe_result(tcc_probe, wcx);
                    w.import_cookies_from_text_async(cdp_json.clone(), wcx);
                });
            });
        })
        .unwrap();

        let phase1_imported = e2e_suite::gpui_browser_harness::pump_until(
            &mut app,
            Duration::from_secs(15),
            |cx| {
                let w = shell1.read(cx).browser_workspace().unwrap().read(cx);
                !matches!(
                    w.recorder.chrome_probe,
                    ChromeProbeUiState::NotProbed | ChromeProbeUiState::Probing
                ) && w.recorder.last_imported_cookie_count >= 2
                    && w.recorder.last_persisted_cookie_count >= 2
            },
        )
        .await;
        if !phase1_imported {
            let diag = app
                .update(|cx| {
                    let w = shell1.read(cx).browser_workspace().unwrap().read(cx);
                    format!(
                        "chrome_probe={:?}, last_imported_cookie_count={}, last_persisted_cookie_count={}, cookie_error_message={:?}",
                        w.recorder.chrome_probe,
                        w.recorder.last_imported_cookie_count,
                        w.recorder.last_persisted_cookie_count,
                        w.recorder.cookie_error_message,
                    )
                })
                .unwrap_or_else(|e| format!("failed to read diag: {e:?}"));
            panic!("T4-05 Phase 1 probe/cookie import did not converge within 15s: {diag}");
        }

        let src_data_dir = app
            .update(|cx| {
                let bw = shell1.read(cx).browser_workspace().unwrap().clone();
                bw.update(cx, |w, wcx| {
                    assert!(matches!(
                        w.recorder.chrome_probe,
                        ChromeProbeUiState::PermissionDeniedTcc { .. }
                    ));
                    w.backup_profile_to_vault(wcx);
                    w.data_dir.clone()
                })
            })
            .unwrap();

        let vault_backed_up = e2e_suite::gpui_browser_harness::pump_until(
            &mut app,
            Duration::from_secs(15),
            |cx| {
                shell1
                    .read(cx)
                    .browser_workspace()
                    .unwrap()
                    .read(cx)
                    .recorder
                    .vault_sync_status
                    .is_some()
            },
        )
        .await;
        assert!(
            vault_backed_up,
            "T4-05 Phase 1 backup_profile_to_vault must populate vault_sync_status within 15s"
        );

        sealed_blob = BrowserProfileStore::new(&src_data_dir)
            .seal_to_vault_blob(key_arc.as_ref(), 505, "default", None, 1, 1024 * 1024)
            .expect("seal_to_vault_blob");
        assert_eq!(sealed_blob.category, UserVaultCategory::BrowserCookie);
        let blob_json = serde_json::to_string(&sealed_blob).unwrap();
        assert!(
            secrets.contains_any_secret(&blob_json).is_none(),
            "encrypted vault blob must contain zero plaintext secrets"
        );

        app.update(|_| drop(shell1)).unwrap();
        rt1.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
            &chrome1,
            ch1_dir.path(),
        ))
        .expect("close chrome1");
        drop(client1);
    }

    // Phase 2: Fresh AppShell #2 + Chrome #2 restores from E2EE blob, verifies SPA DOM & audits non-blocking latency
    let (_cdir2, client2) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt2 = client2.runtime().handle().clone();
    let ch2_dir = isolated_temp_dir("t4_05_ch2").expect("ch2 dir");
    let chrome2 = Arc::new(rt2.block_on(launch_chrome_in(ch2_dir.path())));

    let shell2 = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client2.clone(),
                505,
                "t4_05_b",
            )
        })
        .unwrap();

    let (restored_scope, restored_data) = BrowserProfileStore::new(&ch2_dir.path().join("tmp_restore"))
        .restore_from_vault_blob(key_arc.as_ref(), 505, &sealed_blob)
        .expect("restore_from_vault_blob");
    assert_eq!(restored_scope, "default__global");

    app.update(|cx| {
        shell2.update(cx, |s, scx| {
            s.set_browser_backend_for_test(
                Arc::clone(&chrome2) as Arc<dyn BrowserSessionBackend>,
                scx,
            );
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                BrowserProfileStore::new(&w.data_dir)
                    .save_profile("default", None, &restored_data)
                    .expect("save restored profile");
                w.restore_active_profile_cookies(wcx);
            });
        });
    })
    .unwrap();

    let phase2_restored = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            shell2
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .recorder
                .last_imported_cookie_count
                >= 2
        },
    )
    .await;
    if !phase2_restored {
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
        panic!("T4-05 Phase 2 restore_active_profile_cookies did not converge within 15s: {diag}");
    }

    let mut budget = LatencyBudget::new(10_000.0, 30_000.0);

    // Navigate MockXServer /home SPA
    let x_home = x_server.url("/home");
    let t_x = std::time::Instant::now();
    app.update(|cx| {
        shell2.update(cx, |s, scx| s.browser_navigate(&x_home, scx));
    })
    .unwrap();
    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
            let w = shell2.read(cx).browser_workspace().unwrap().read(cx);
            !w.is_loading && w.current_url.contains("/home")
        })
        .await
    );
    budget.record("mock_x_spa_home_nav", t_x.elapsed().as_secs_f64() * 1000.0);

    let x_snap = rt2.block_on(chrome2.snapshot_dom()).expect("x snapshot_dom");
    assert!(
        x_snap
            .interactive_elements
            .iter()
            .any(|el| el.test_id.as_deref() == Some("tweetTextarea_0")),
        "E2EE-restored session must render authenticated MockXServer /home SPA"
    );
    let x_snap_json = serde_json::to_string(&x_snap).unwrap();
    assert!(secrets.contains_any_secret(&x_snap_json).is_none());

    // Navigate MockWebAppServer /search SPA
    let web_search = web.url("/search?q=rust&category=books");
    let t_w = std::time::Instant::now();
    app.update(|cx| {
        shell2.update(cx, |s, scx| s.browser_navigate(&web_search, scx));
    })
    .unwrap();
    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
            let w = shell2.read(cx).browser_workspace().unwrap().read(cx);
            !w.is_loading && w.current_url.contains("/search")
        })
        .await
    );
    budget.record("mock_webapp_search_nav", t_w.elapsed().as_secs_f64() * 1000.0);

    // Non-blocking audit during 300ms delayed load
    let slow_url = delayed.url("/slow");
    app.update(|cx| {
        shell2.update(cx, |s, scx| s.browser_navigate(&slow_url, scx));
    })
    .unwrap();
    let (slow_done, slow_ticks, max_gap_slow) =
        e2e_suite::gpui_browser_harness::pump_with_ticks(
            &mut app,
            Duration::from_secs(15),
            Duration::from_millis(15),
            |cx, _| {
                let w = shell2.read(cx).browser_workspace().unwrap().read(cx);
                !w.is_loading && w.current_url == slow_url
            },
        )
        .await;
    assert!(slow_done);
    assert!(slow_ticks >= 5);
    assert!(
        max_gap_slow < 150,
        "GPUI main thread stalled during slow load: {max_gap_slow}ms"
    );

    // Concurrent multi-step Script App replay + GPUI tick audit
    let script = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "t4-05-audit-script".to_string(),
        slug: "t4-05-audit-script".to_string(),
        name: "T4-05 Latency Audit Flow".to_string(),
        version: "1.0.0".to_string(),
        description: "Multi-step zero-LLM checkout audit".to_string(),
        category: "productivity".to_string(),
        target_domains: vec!["127.0.0.1".to_string()],
        parameters: vec![ScriptParameter {
            name: "customer_name".to_string(),
            description: "Customer name".to_string(),
            required: true,
            default_value: Some("Audit User".to_string()),
            is_secret: false,
        }],
        steps: vec![
            ScriptStep::Navigate {
                url_template: web.url("/item/item-01"),
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
                value_template: "audit@example.test".to_string(),
                secret_vault_key: None,
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("checkout-submit"),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("confirmation-id"),
                timeout_ms: 4_000,
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };
    assert!(
        secrets
            .contains_any_secret(&serde_json::to_string(&script).unwrap())
            .is_none()
    );

    let t_rep = std::time::Instant::now();
    app.update(|cx| {
        shell2.update(cx, |s, scx| {
            s.browser_run_script_app(script, BTreeMap::new(), scx);
        });
    })
    .unwrap();
    let (rep_done, rep_ticks, max_gap_rep) = e2e_suite::gpui_browser_harness::pump_with_ticks(
        &mut app,
        Duration::from_secs(15),
        Duration::from_millis(15),
        |cx, _| {
            let w = shell2.read(cx).browser_workspace().unwrap().read(cx);
            !w.recorder.is_replaying && w.recorder.last_replay_report.is_some()
        },
    )
    .await;
    assert!(rep_done);
    assert!(rep_ticks >= 5);
    assert!(
        max_gap_rep < 150,
        "GPUI main thread stalled during replay: {max_gap_rep}ms"
    );
    budget.record("zero_llm_checkout_replay", t_rep.elapsed().as_secs_f64() * 1000.0);

    app.update(|cx| {
        let w = shell2.read(cx).browser_workspace().unwrap().read(cx);
        let report = w.recorder.last_replay_report.as_ref().unwrap();
        assert!(report.success);
        assert!(!report.used_llm);
        assert!(!report.step_timings.is_empty());
        for st in &report.step_timings {
            budget.record(
                format!("step_{}", st.step_index.unwrap_or(0)),
                st.duration_ms as f64,
            );
        }
        eprintln!(
            "[T4-05 Latency & SPA Audit] total_budget_ms={:.1}, max_step_ms={:.1}, slow_ticks={}, max_gap_slow_ms={}, replay_ticks={}, max_gap_replay_ms={}, replay_steps={}, x_spa_elements={}",
            budget.total_ms(),
            budget.max_observed_step_ms(),
            slow_ticks,
            max_gap_slow,
            rep_ticks,
            max_gap_rep,
            report.steps_executed,
            x_snap.interactive_elements.len(),
        );
    })
    .unwrap();
    budget
        .assert_within_budget()
        .expect("T4-05 latencies must remain within budget");
    assert_eq!(web.order_count(), 1);

    app.update(|_| drop(shell2)).unwrap();
    rt2.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome2,
        ch2_dir.path(),
    ))
    .expect("close chrome2");
    drop(client2);
    rt_boot.block_on(delayed.shutdown());
    rt_boot.block_on(web.shutdown());
    rt_boot.block_on(x_server.shutdown());
}
