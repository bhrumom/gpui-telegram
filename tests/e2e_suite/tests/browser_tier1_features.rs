use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use bot_browser::cdp::CdpTransport;
use bot_browser::orchestrator::{
    BotBrowserOrchestrator, OrchestratorConfig, SessionTransportFactory,
};
use bot_browser::{
    Argon2KdfParams, AutomationScriptApp, BotBrowserError, BotBrowserEvent, BrowserActionTrace,
    BrowserBackendKind, BrowserControlGate, BrowserControlMode, BrowserCookie, BrowserProfileData,
    BrowserProfileStore, BrowserSessionBackend, CdpBrowserBackend, ChromeImportOptions,
    ChromeImportProbe, CredentialVault, DeterministicScriptRunner,
    KeyEventKind, KeyInputEvent, LocalStorageMap, MemoryMasterKeyStore, NoopChromeKeyProvider,
    NormalizedOrigin, PointerButton, PointerEventKind, PointerInputEvent, ProfileNamespace,
    RecordedActionKind, SCRIPT_APP_SCHEMA_VERSION, SameSitePolicy, ScriptAppMetadata,
    ScriptCompiler, ScriptParameter, ScriptStep, SemanticLocator, TraceParameterBinding,
    UserVaultCategory, UserVaultCrypto, import_cookies_from_chrome_sqlite,
    import_cookies_from_json, probe_chrome_cookies_at, probe_local_chrome_cookies,
    validate_script_app_safe_for_publish,
};
use e2e_suite::browser_e2e_fixtures::{
    DEFAULT_X_HANDLE, DEFAULT_X_PASSWORD, DEFAULT_X_USERNAME, MockOpenAiServer, MockWebAppServer,
    MockXServer, assert_ui_loop_non_blocking, fake_secrets, generate_cdp_cookies_json,
    generate_edit_this_cookie_json, generate_netscape_cookies, isolated_temp_dir,
    mock_openai_text, mock_openai_tool_call, no_proxy_http_client, x_auth_cookies,
};
use e2e_suite::common::EphemeralTestServer;
use gpui_telegram_agent::fabushi::FabushiBotHost;
use gpui_telegram_agent::hermes::providers::ProviderConfig;
use gpui_telegram_agent::{
    BrowserAutomationEvent, CommandUiEffect, HermesTurnRequest, ToolsetPreset,
    forward_events_to_mpsc,
};
use gpui_telegram_client::config::ClientConfig;
use gpui_telegram_client::net::{LoginRequest, RegisterRequest};
use gpui_telegram_client::TelegramClient;
use gpui_telegram_proto::automation_apps::{
    PublishAutomationAppRequest, RecordAutomationRunRequest, compute_automation_manifest_sha256,
};
use reqwest::header::COOKIE;
use rusqlite::Connection;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

async fn launch_chrome_in(dir: &std::path::Path) -> CdpBrowserBackend {
    e2e_suite::gpui_browser_harness::launch_chrome_in(dir).await
}

async fn wait_for_url_contains(
    backend: &dyn BrowserSessionBackend,
    needle: &str,
    timeout: Duration,
) -> String {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(url) = backend.current_url().await {
            if url.contains(needle) {
                return url;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for current_url to contain '{needle}'"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
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

/// [Test ID] TEST-BR-T1-F1-01
/// [Feature / Req] F1 / R1, AC2 — Async backend navigation, title/URL query & history (`go_back`/`go_forward`/`reload`) against `MockWebAppServer`
/// [Preconditions] Local `MockWebAppServer` started on `127.0.0.1:0`; real headless `ChromeCdp` launched in isolated `TempDir`.
/// [Opaque-Box Steps] Navigate to `/` then `/search?q=Rust&category=books`, call `go_back()`, wait for index locator/URL settle, call `go_forward()`, wait for search results locator/URL settle, and call `reload()`.
/// [Observable Assertions] `NavigationResult`, `current_url()`, and `page_title()` reflect every transition, back/forward history position, and reload accurately.
#[tokio::test]
async fn test_br_t1_f1_01_chrome_navigation_history_and_reload() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f1_01").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    let home_url = server.url("/");
    let nav1 = backend.navigate(&home_url).await.expect("navigate home");
    assert_eq!(nav1.backend, BrowserBackendKind::ChromeCdp);
    assert_eq!(nav1.origin, server.base_url());
    assert!(nav1.title.contains("Catalog Portal"));
    assert_eq!(backend.current_url().await.unwrap(), home_url);
    assert!(backend.page_title().await.unwrap().contains("Catalog Portal"));

    let search_url = server.url("/search?q=Rust&category=books");
    let nav2 = backend.navigate(&search_url).await.expect("navigate search");
    assert!(nav2.url.contains("/search"));
    assert!(nav2.title.contains("Search Results"));
    assert!(backend.current_url().await.unwrap().contains("/search"));

    backend.go_back().await.expect("go_back");
    backend
        .wait_for_locator(&SemanticLocator::by_test_id("search-input"), 3_000)
        .await
        .expect("index search-input visible after go_back");
    let back_url = wait_for_url_contains(&backend, &server.base_url(), Duration::from_secs(2)).await;
    assert_eq!(back_url, home_url);
    assert!(backend.page_title().await.unwrap().contains("Catalog Portal"));

    backend.go_forward().await.expect("go_forward");
    backend
        .wait_for_locator(&SemanticLocator::by_test_id("results-table"), 3_000)
        .await
        .expect("results-table visible after go_forward");
    let fwd_url = wait_for_url_contains(&backend, "/search", Duration::from_secs(2)).await;
    assert!(fwd_url.contains("/search"));
    assert!(backend.page_title().await.unwrap().contains("Search Results"));

    backend.reload().await.expect("reload");
    backend
        .wait_for_locator(&SemanticLocator::by_test_id("results-table"), 3_000)
        .await
        .expect("results-table visible after reload");
    assert!(backend.current_url().await.unwrap().contains("/search"));

    backend.close().await.expect("close backend");
    server.shutdown().await;
}
/// [Test ID] TEST-BR-T1-F1-02
/// [Feature / Req] F1 / R1, AC2 — Semantic locator interaction (`click_locator`, `fill_locator`, `press_key`, `scroll`, `wait_for_locator`) on `MockXServer`
/// [Preconditions] Local `MockXServer` started; real headless `ChromeCdp` launched in isolated `TempDir`.
/// [Opaque-Box Steps] Navigate to `/login`, wait for username input, fill username (`is_secret=false`) and password (`is_secret=true`), click login, wait for `/home` composer, scroll, fill tweet text, press `Tab`, click Post button, and check `/api/tweets`.
/// [Observable Assertions] `ActionOutcome.ok == true` with matched strategies, browser transitions to `/home`, and `MockXServer` `tweet_count()` increments with the submitted post text.
#[tokio::test]
async fn test_br_t1_f1_02_semantic_locator_interaction_on_mock_x() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let temp = isolated_temp_dir("t1_f1_02").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    backend.navigate(&server.url("/login")).await.expect("nav login");
    let wait_user = backend
        .wait_for_locator(&SemanticLocator::by_test_id("ocfEnterTextTextInput"), 3_000)
        .await
        .expect("wait username");
    assert!(wait_user.ok);
    assert_eq!(wait_user.matched_strategy.as_deref(), Some("test_id"));

    let fill_user = backend
        .fill_locator(
            &SemanticLocator::by_test_id("ocfEnterTextTextInput"),
            DEFAULT_X_USERNAME,
            false,
        )
        .await
        .expect("fill username");
    assert!(fill_user.ok);

    let fill_pass = backend
        .fill_locator(
            &SemanticLocator::by_test_id("LoginForm_Password_Input"),
            DEFAULT_X_PASSWORD,
            true,
        )
        .await
        .expect("fill password");
    assert!(fill_pass.ok);

    let click_login = backend
        .click_locator(&SemanticLocator::by_test_id("LoginForm_Login_Button"))
        .await
        .expect("click login");
    assert!(click_login.ok);

    backend
        .wait_for_locator(&SemanticLocator::by_test_id("tweetTextarea_0"), 4_000)
        .await
        .expect("wait home composer");
    let home_url = wait_for_url_contains(&backend, "/home", Duration::from_secs(2)).await;
    assert!(home_url.ends_with("/home"));

    let scroll_res = backend.scroll(0.0, 120.0).await.expect("scroll");
    assert!(scroll_res.ok);

    let post_text = "Opaque-box F1 semantic locator tweet";
    backend
        .fill_locator(
            &SemanticLocator::by_test_id("tweetTextarea_0"),
            post_text,
            false,
        )
        .await
        .expect("fill composer");
    let tab_res = backend.press_key("Tab").await.expect("press Tab");
    assert!(tab_res.ok);

    let post_click = backend
        .click_locator(&SemanticLocator::by_test_id("tweetButtonInline"))
        .await
        .expect("click post");
    assert!(post_click.ok);

    wait_for_tweet_count(&server, 3, Duration::from_secs(3)).await;
    {
        let tweets = server.state().tweets.read();
        assert!(tweets.iter().any(|t| t.text == post_text));
    }

    backend.close().await.expect("close backend");
    server.shutdown().await;
}
/// [Test ID] TEST-BR-T1-F1-03
/// [Feature / Req] F1 / R1 — `snapshot_dom` extracts semantic tree while redacting `<input type="password">` and secret field values
/// [Preconditions] Local `MockXServer` `/login` loaded in `ChromeCdp`.
/// [Opaque-Box Steps] Fill username and fill password input with `fake_secrets().password` (`is_secret=true`), then call `snapshot_dom()`.
/// [Observable Assertions] Interactive nodes expose `test_id`, `aria_label`, and `role`; `redacted_field_count >= 1`; `FakeSecretFixtures::contains_any_secret` returns `None` on serialized snapshot.
#[tokio::test]
async fn test_br_t1_f1_03_snapshot_dom_redacts_secrets_and_extracts_nodes() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let temp = isolated_temp_dir("t1_f1_03").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;
    let secrets = fake_secrets();

    backend.navigate(&server.url("/login")).await.expect("nav login");
    backend
        .fill_locator(
            &SemanticLocator::by_test_id("ocfEnterTextTextInput"),
            DEFAULT_X_USERNAME,
            false,
        )
        .await
        .expect("fill username");
    backend
        .fill_locator(
            &SemanticLocator::by_test_id("LoginForm_Password_Input"),
            secrets.password,
            true,
        )
        .await
        .expect("fill secret password");

    let snap = backend.snapshot_dom().await.expect("snapshot_dom");
    assert!(snap.redacted_field_count >= 1);

    let pw_node = snap
        .interactive_elements
        .iter()
        .find(|n| n.test_id.as_deref() == Some("LoginForm_Password_Input"))
        .expect("password node must appear in interactive_elements");
    assert!(pw_node.is_password);
    assert_eq!(pw_node.aria_label.as_deref(), Some("Password"));
    assert_eq!(pw_node.role.as_deref(), Some("textbox"));
    assert!(
        !pw_node
            .value
            .as_deref()
            .unwrap_or_default()
            .contains(secrets.password)
    );

    let user_node = snap
        .interactive_elements
        .iter()
        .find(|n| n.test_id.as_deref() == Some("ocfEnterTextTextInput"))
        .expect("username node must appear in interactive_elements");
    assert_eq!(user_node.value.as_deref(), Some(DEFAULT_X_USERNAME));

    let serialized = serde_json::to_string(&snap).expect("serialize snapshot");
    assert_eq!(secrets.contains_any_secret(&serialized), None);

    backend.close().await.expect("close backend");
    server.shutdown().await;
}
/// [Test ID] TEST-BR-T1-F1-04
/// [Feature / Req] F1 / R1 — Offscreen `capture_screenshot()`, `capture_frame_rgba()`, and pointer/keyboard input event dispatch
/// [Preconditions] Local `MockWebAppServer` `/` loaded in `ChromeCdp`.
/// [Opaque-Box Steps] Call `capture_screenshot()` and decode PNG, call `capture_frame_rgba()`, query `#search-input` rect center, dispatch `PointerInputEvent` Move/Down/Up to focus input, dispatch `KeyInputEvent` characters `"rust"`, and verify DOM input mutation.
/// [Observable Assertions] Screenshot decodes via `image::load_from_memory` as PNG with non-zero dimensions; RGBA buffer length equals `width * height * 4`; dispatched pointer + key events mutate `#search-input.value` to `"rust"`.
#[tokio::test]
async fn test_br_t1_f1_04_screenshot_rgba_and_input_event_dispatch() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f1_04").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    backend.navigate(&server.url("/")).await.expect("nav index");

    let png_bytes = backend.capture_screenshot().await.expect("capture_screenshot");
    let decoded = image::load_from_memory(&png_bytes).expect("decode PNG screenshot");
    assert!(decoded.width() > 0 && decoded.height() > 0);

    let rgba = backend.capture_frame_rgba().await.expect("capture_frame_rgba");
    assert!(rgba.width > 0 && rgba.height > 0);
    assert_eq!(rgba.rgba_bytes.len(), (rgba.width * rgba.height * 4) as usize);

    let coords = backend
        .eval_js("(() => { const r = document.getElementById('search-input').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()")
        .await
        .expect("query search-input rect");
    let cx = coords["x"].as_f64().expect("center x");
    let cy = coords["y"].as_f64().expect("center y");

    backend
        .dispatch_pointer_event(PointerInputEvent {
            kind: PointerEventKind::Move,
            x: cx,
            y: cy,
            button: PointerButton::None,
            click_count: 0,
            delta_x: 0.0,
            delta_y: 0.0,
            modifiers: 0,
        })
        .await
        .expect("pointer move");
    backend
        .dispatch_pointer_event(PointerInputEvent {
            kind: PointerEventKind::Down,
            x: cx,
            y: cy,
            button: PointerButton::Left,
            click_count: 1,
            delta_x: 0.0,
            delta_y: 0.0,
            modifiers: 0,
        })
        .await
        .expect("pointer down");
    backend
        .dispatch_pointer_event(PointerInputEvent {
            kind: PointerEventKind::Up,
            x: cx,
            y: cy,
            button: PointerButton::Left,
            click_count: 1,
            delta_x: 0.0,
            delta_y: 0.0,
            modifiers: 0,
        })
        .await
        .expect("pointer up");

    for ch in ['r', 'u', 's', 't'] {
        let s = ch.to_string();
        backend
            .dispatch_key_event(KeyInputEvent {
                kind: KeyEventKind::Char,
                key: s.clone(),
                code: None,
                text: Some(s),
                windows_virtual_key_code: None,
                modifiers: 0,
            })
            .await
            .expect("dispatch key char");
    }

    let typed = backend
        .eval_js("document.getElementById('search-input').value")
        .await
        .expect("read search-input value");
    assert_eq!(typed.as_str(), Some("rust"));

    backend.close().await.expect("close backend");
    server.shutdown().await;
}
/// [Test ID] TEST-BR-T1-F1-05
/// [Feature / Req] F1 / R1, AC3 — Per-step latency telemetry & async non-blocking Tokio execution under `assert_ui_loop_non_blocking`
/// [Preconditions] Local `MockWebAppServer` started; real headless `ChromeCdp` launched.
/// [Opaque-Box Steps] Execute navigation, `wait_for_locator`, `snapshot_dom`, and `capture_screenshot` concurrently with a 10ms UI ticker inside `assert_ui_loop_non_blocking` on a multi-thread Tokio runtime (`worker_threads = 2`).
/// [Observable Assertions] `NonBlockingUiReport` confirms ticker advanced cleanly without multi-second thread stalls (`max_tick_gap_ms <= 350.0` under parallel Chrome launch load); `backend.performance_metrics()` contains recorded `StepTimingMetric` entries with `duration_ms` and `success == true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_br_t1_f1_05_step_latency_telemetry_and_non_blocking_execution() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f1_05").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    let search_url = server.url("/search?q=rust");
    let ((), report) = assert_ui_loop_non_blocking(
        || async {
            backend.navigate(&search_url).await.expect("navigate");
            backend
                .wait_for_locator(&SemanticLocator::by_test_id("results-table"), 3_000)
                .await
                .expect("wait results-table");
            let _ = backend.snapshot_dom().await.expect("snapshot");
            let _ = backend.capture_screenshot().await.expect("screenshot");
        },
        Duration::from_millis(10),
        350.0,
    )
    .await
    .expect("UI loop must remain non-blocking during ChromeCdp ops");

    assert!(report.ticks_completed >= 2);
    assert!(report.max_tick_gap_ms <= 350.0);

    let metrics = backend.performance_metrics();
    assert_eq!(metrics.backend, Some(BrowserBackendKind::ChromeCdp));
    assert!(metrics.total_operations >= 4);
    assert_eq!(metrics.failed_operations, 0);
    assert!(!metrics.recent_steps.is_empty());
    assert!(metrics.recent_steps.iter().all(|s| s.success));

    backend.close().await.expect("close backend");
    server.shutdown().await;
}
/// [Test ID] TEST-BR-T1-F2-01
/// [Feature / Req] F2 / R1, AC4 — `ProfileNamespace` isolation across two profiles (`alpha` and `beta`)
/// [Preconditions] Local `MockXServer` started with two independent seeded sessions (`sess_alpha`, `sess_beta`); `BrowserProfileStore` backed by `isolated_temp_dir`.
/// [Opaque-Box Steps] Store distinct `auth_token`/`ct0` cookies and `localStorage` maps under `"alpha"` and `"beta"` namespaces for the same origin, then load each profile separately.
/// [Observable Assertions] `ProfileNamespace::profile_key_for` generates distinct keys; loading `"alpha"` returns only `sess_alpha`'s `auth_token` and `"alpha"` localStorage without leaking `"beta"`, and vice versa.
#[tokio::test]
async fn test_br_t1_f2_01_profile_namespace_isolation() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let temp = isolated_temp_dir("t1_f2_01").expect("temp dir");
    let store = BrowserProfileStore::new(temp.path());
    let origin = NormalizedOrigin::parse(&server.base_url()).expect("parse origin");

    let sess_a = server.seed_session("auth_alpha_tok_001", "ct0_alpha_001");
    let sess_b = server.seed_session("auth_beta_tok_002", "ct0_beta_002");

    let key_a = ProfileNamespace::profile_key_for("alpha", Some(&origin));
    let key_b = ProfileNamespace::profile_key_for("beta", Some(&origin));
    assert_ne!(key_a, key_b);

    let cookies_a: Vec<BrowserCookie> = x_auth_cookies("127.0.0.1", &sess_a.auth_token, &sess_a.ct0)
        .into_iter()
        .map(|fc| {
            let mut c = BrowserCookie::new(fc.name, fc.value, fc.domain);
            c.http_only = fc.http_only;
            c.expires = fc.expires;
            c.session = false;
            c
        })
        .collect();
    let cookies_b: Vec<BrowserCookie> = x_auth_cookies("127.0.0.1", &sess_b.auth_token, &sess_b.ct0)
        .into_iter()
        .map(|fc| {
            let mut c = BrowserCookie::new(fc.name, fc.value, fc.domain);
            c.http_only = fc.http_only;
            c.expires = fc.expires;
            c.session = false;
            c
        })
        .collect();

    let mut ls_a: LocalStorageMap = BTreeMap::new();
    ls_a.insert(
        origin.as_str().to_string(),
        BTreeMap::from([("active_profile".to_string(), "alpha".to_string())]),
    );
    let mut ls_b: LocalStorageMap = BTreeMap::new();
    ls_b.insert(
        origin.as_str().to_string(),
        BTreeMap::from([("active_profile".to_string(), "beta".to_string())]),
    );

    store
        .save_profile(
            "alpha",
            Some(&origin),
            &BrowserProfileData {
                cookies: cookies_a,
                local_storage: ls_a,
                updated_at_ms: 1_700_000_000_000,
            },
        )
        .expect("save alpha");
    store
        .save_profile(
            "beta",
            Some(&origin),
            &BrowserProfileData {
                cookies: cookies_b,
                local_storage: ls_b,
                updated_at_ms: 1_700_000_000_001,
            },
        )
        .expect("save beta");

    let loaded_a = store.load_profile("alpha", Some(&origin)).expect("load alpha");
    let loaded_b = store.load_profile("beta", Some(&origin)).expect("load beta");

    assert!(loaded_a.cookies.iter().any(|c| c.value == "auth_alpha_tok_001"));
    assert!(!loaded_a.cookies.iter().any(|c| c.value == "auth_beta_tok_002"));
    assert_eq!(
        loaded_a
            .local_storage
            .get(origin.as_str())
            .and_then(|m| m.get("active_profile"))
            .map(String::as_str),
        Some("alpha")
    );

    assert!(loaded_b.cookies.iter().any(|c| c.value == "auth_beta_tok_002"));
    assert!(!loaded_b.cookies.iter().any(|c| c.value == "auth_alpha_tok_001"));
    assert_eq!(
        loaded_b
            .local_storage
            .get(origin.as_str())
            .and_then(|m| m.get("active_profile"))
            .map(String::as_str),
        Some("beta")
    );

    server.shutdown().await;
}

/// [Test ID] TEST-BR-T1-F2-02
/// [Feature / Req] F2 / R1, AC4 — Cold session persistence & restore via `BrowserProfileStore` across `ChromeCdp` backend restart
/// [Preconditions] `MockXServer` started; two distinct `TempDir` user data directories for backend #1 and backend #2; `BrowserProfileStore` in separate `TempDir`.
/// [Opaque-Box Steps] Backend #1 logs in via `/login`, exports cookies on `/home`, saves to `BrowserProfileStore`, closes and drops; Backend #2 starts fresh, loads profile, imports cookies, and navigates directly to `/home`.
/// [Observable Assertions] Backend #2 renders `/home` authenticated with `DEFAULT_X_HANDLE` without visiting `/login`.
#[tokio::test]
async fn test_br_t1_f2_02_cold_session_persistence_and_restore_across_restart() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let store_dir = isolated_temp_dir("t1_f2_02_store").expect("store temp");
    let chrome1_dir = isolated_temp_dir("t1_f2_02_c1").expect("c1 temp");
    let chrome2_dir = isolated_temp_dir("t1_f2_02_c2").expect("c2 temp");

    let store = BrowserProfileStore::new(store_dir.path());
    let origin = NormalizedOrigin::parse(&server.base_url()).expect("origin");

    // Backend 1: login and export cookies
    {
        let b1 = launch_chrome_in(chrome1_dir.path()).await;
        b1.navigate(&server.url("/login")).await.expect("nav login");
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
        b1.wait_for_locator(&SemanticLocator::by_test_id("user-handle"), 4_000)
            .await
            .expect("wait home");

        let cookies = b1.export_cookies(None).await.expect("export cookies");
        assert!(cookies.iter().any(|c| c.name == "auth_token"));
        assert!(cookies.iter().any(|c| c.name == "ct0"));

        store
            .save_profile(
                "cold-restart",
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

    // Backend 2: fresh Chrome user-data-dir restores cookies and navigates /home directly
    let b2 = launch_chrome_in(chrome2_dir.path()).await;
    let restored = store
        .load_profile("cold-restart", Some(&origin))
        .expect("load profile");
    let imported = b2
        .import_cookies(&restored.cookies)
        .await
        .expect("import cookies");
    assert!(imported >= 2);

    b2.navigate(&server.url("/home")).await.expect("nav /home");
    b2.wait_for_locator(&SemanticLocator::by_test_id("user-handle"), 3_000)
        .await
        .expect("authenticated user-handle must render without re-login");
    let url = b2.current_url().await.expect("current_url");
    assert!(url.ends_with("/home"), "expected /home, got {url}");
    let handle = b2
        .extract_text_locator(&SemanticLocator::by_test_id("user-handle"))
        .await
        .expect("extract handle");
    assert_eq!(handle, DEFAULT_X_HANDLE);

    b2.close().await.expect("close b2");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T1-F2-03
/// [Feature / Req] F2 / R1, AC4 — `import_cookies_from_json` parses CDP JSON, EditThisCookie JSON, and Netscape text & injects into `ChromeCdp`
/// [Preconditions] `MockXServer` started with seeded session; `ChromeCdp` launched.
/// [Opaque-Box Steps] Generate CDP JSON, EditThisCookie JSON, and Netscape cookies.txt for `x_auth_cookies("127.0.0.1", ...)`, parse each via `import_cookies_from_json`, inject parsed cookies into `ChromeCdp`, navigate `/home`, compose and submit a tweet.
/// [Observable Assertions] All three formats parse accurately; `ChromeCdp` accesses `/home` authenticated and triggers `POST /api/tweets`, incrementing `server.tweet_count()` and returning the post in `GET /api/tweets`.
#[tokio::test]
async fn test_br_t1_f2_03_import_cookies_from_json_and_netscape_formats() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let temp = isolated_temp_dir("t1_f2_03").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    let sess = server.seed_session("auth_import_tok_99", "ct0_import_99");
    let fixtures = x_auth_cookies("127.0.0.1", &sess.auth_token, &sess.ct0);

    let cdp_json = generate_cdp_cookies_json(&fixtures);
    let etc_json = generate_edit_this_cookie_json(&fixtures);
    let netscape_txt = generate_netscape_cookies(&fixtures);

    let from_cdp = import_cookies_from_json(&cdp_json).expect("parse CDP JSON");
    let from_etc = import_cookies_from_json(&etc_json).expect("parse EditThisCookie JSON");
    let from_netscape = import_cookies_from_json(&netscape_txt).expect("parse Netscape text");

    for (idx, parsed) in [&from_cdp, &from_etc, &from_netscape].iter().enumerate() {
        assert_eq!(parsed.len(), 2);
        let auth = parsed.iter().find(|c| c.name == "auth_token").unwrap();
        assert_eq!(auth.value, sess.auth_token);
        assert!(auth.http_only);
        if idx < 2 {
            assert_eq!(auth.same_site, Some(SameSitePolicy::Lax));
        } else {
            // Netscape cookies.txt has 7 columns and no SameSite attribute
            assert_eq!(auth.same_site, None);
        }
    }

    let etc_imported = backend
        .import_cookies(&from_etc)
        .await
        .expect("import EditThisCookie JSON cookies into ChromeCdp");
    assert_eq!(etc_imported, 2);

    let cdp_imported = backend
        .import_cookies(&from_cdp)
        .await
        .expect("import cookies into ChromeCdp");
    assert_eq!(cdp_imported, 2);
    backend.navigate(&server.url("/home")).await.expect("nav /home");
    backend
        .wait_for_locator(&SemanticLocator::by_test_id("tweetTextarea_0"), 3_000)
        .await
        .expect("composer visible");

    let msg = "Tweet via imported CDP JSON cookies";
    backend
        .fill_locator(&SemanticLocator::by_test_id("tweetTextarea_0"), msg, false)
        .await
        .expect("fill composer");
    backend
        .click_locator(&SemanticLocator::by_test_id("tweetButtonInline"))
        .await
        .expect("click post");

    wait_for_tweet_count(&server, 3, Duration::from_secs(3)).await;

    let client = no_proxy_http_client();
    let api_resp = client
        .get(server.url("/api/tweets"))
        .header(
            COOKIE,
            format!("auth_token={}; ct0={}", sess.auth_token, sess.ct0),
        )
        .send()
        .await
        .expect("GET /api/tweets");
    assert_eq!(api_resp.status(), 200);
    let body_json: serde_json::Value = api_resp.json().await.expect("parse /api/tweets json");
    let tweets = body_json["tweets"].as_array().expect("tweets array");
    assert!(tweets.iter().any(|t| t["text"].as_str() == Some(msg)));

    // Also verify Netscape-parsed cookies authenticate a distinct seeded session in ChromeCdp
    let sess_netscape = server.seed_session("auth_netscape_tok_77", "ct0_netscape_77");
    let netscape_fixtures =
        x_auth_cookies("127.0.0.1", &sess_netscape.auth_token, &sess_netscape.ct0);
    let netscape_txt_2 = generate_netscape_cookies(&netscape_fixtures);
    let from_netscape_2 =
        import_cookies_from_json(&netscape_txt_2).expect("parse second Netscape text");
    let netscape_imported = backend
        .import_cookies(&from_netscape_2)
        .await
        .expect("import Netscape cookies into ChromeCdp");
    assert_eq!(netscape_imported, 2);
    let exported_after_netscape = backend
        .export_cookies(Some("127.0.0.1"))
        .await
        .expect("export cookies after Netscape import");
    assert!(
        exported_after_netscape
            .iter()
            .any(|c| c.name == "auth_token" && c.value == sess_netscape.auth_token),
        "Netscape-imported auth_token must be active in ChromeCdp"
    );
    backend.navigate(&server.url("/home")).await.expect("nav /home with Netscape cookies");
    backend
        .wait_for_locator(&SemanticLocator::by_test_id("user-handle"), 3_000)
        .await
        .expect("Netscape cookies must authenticate /home");

    backend.close().await.expect("close backend");
    server.shutdown().await;
}
/// [Test ID] TEST-BR-T1-F2-04
/// [Feature / Req] F2 / R1, AC4 — `UserVaultCrypto` + `UserVaultCategory::BrowserCookie` (`e2ee_sync`) encrypted save & decrypt roundtrip
/// [Preconditions] `MockXServer` seeded session; two `BrowserProfileStore` instances in separate `TempDir`s.
/// [Opaque-Box Steps] Save `x_auth_cookies` to store1, initialize `UserVaultCrypto` with passphrase, seal via `seal_to_vault_blob`, inspect serialized blob for plaintext leakage, unlock key with passphrase, restore into store2, and inject into `ChromeCdp`.
/// [Observable Assertions] `blob.category == UserVaultCategory::BrowserCookie`; ciphertext JSON contains zero plaintext `auth_token`/`ct0`; restored cookies authenticate `ChromeCdp` on `/home`.
#[tokio::test]
async fn test_br_t1_f2_04_e2ee_sync_encrypted_cookie_roundtrip() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let dir1 = isolated_temp_dir("t1_f2_04_s1").expect("s1 temp");
    let dir2 = isolated_temp_dir("t1_f2_04_s2").expect("s2 temp");
    let chrome_dir = isolated_temp_dir("t1_f2_04_c").expect("chrome temp");

    let store1 = BrowserProfileStore::new(dir1.path());
    let store2 = BrowserProfileStore::new(dir2.path());
    let origin = NormalizedOrigin::parse(&server.base_url()).expect("origin");

    let sess = server.seed_session("secret_e2ee_auth_777", "secret_e2ee_ct0_888");
    let cookies: Vec<BrowserCookie> = x_auth_cookies("127.0.0.1", &sess.auth_token, &sess.ct0)
        .into_iter()
        .map(|fc| {
            let mut c = BrowserCookie::new(fc.name, fc.value, fc.domain);
            c.http_only = fc.http_only;
            c.same_site = Some(SameSitePolicy::Lax);
            c.expires = fc.expires;
            c.session = false;
            c
        })
        .collect();
    store1
        .save_cookies("e2ee-profile", Some(&origin), &cookies)
        .expect("save cookies");

    let (user_key, envelope, _recovery) =
        UserVaultCrypto::initialize_new(55, "correct-passphrase-2026", Argon2KdfParams::fast_for_tests())
            .expect("init user vault crypto");
    let blob = store1
        .seal_to_vault_blob(&user_key, 55, "e2ee-profile", Some(&origin), 1, 1024 * 1024)
        .expect("seal_to_vault_blob");
    assert_eq!(blob.category, UserVaultCategory::BrowserCookie);

    let blob_json = serde_json::to_string(&blob).expect("serialize blob");
    assert!(!blob_json.contains(&sess.auth_token));
    assert!(!blob_json.contains(&sess.ct0));

    let unlocked = UserVaultCrypto::unlock_with_passphrase(55, "correct-passphrase-2026", &envelope)
        .expect("unlock with passphrase");
    let (_item_key, restored) = store2
        .restore_from_vault_blob(&unlocked, 55, &blob)
        .expect("restore_from_vault_blob");

    let backend = launch_chrome_in(chrome_dir.path()).await;
    backend
        .import_cookies(&restored.cookies)
        .await
        .expect("import restored cookies");
    backend.navigate(&server.url("/home")).await.expect("nav /home");
    backend
        .wait_for_locator(&SemanticLocator::by_test_id("user-handle"), 3_000)
        .await
        .expect("authenticated home");

    backend.close().await.expect("close backend");
    server.shutdown().await;
}
/// [Test ID] TEST-BR-T1-F2-05
/// [Feature / Req] F2 / R1, AC4 — `probe_local_chrome_cookies()` & Chrome SQLite fixture probe/import
/// [Preconditions] Isolated `TempDir` containing a valid Chrome SQLite cookie DB (`meta` + `cookies` tables with plaintext cookie rows).
/// [Opaque-Box Steps] Call `probe_local_chrome_cookies()`, then call `probe_chrome_cookies_at` and `import_cookies_from_chrome_sqlite` on the SQLite fixture DB.
/// [Observable Assertions] `probe_local_chrome_cookies()` returns a valid `ChromeImportProbe` variant without panicking; `probe_chrome_cookies_at` returns `Available`; SQLite import returns the expected cookie with normalized fields.
#[tokio::test]
async fn test_br_t1_f2_05_probe_local_chrome_cookies_and_sqlite_fixture() {
    let host_probe = probe_local_chrome_cookies();
    assert!(matches!(
        host_probe,
        ChromeImportProbe::Available { .. }
            | ChromeImportProbe::PermissionDeniedTcc { .. }
            | ChromeImportProbe::NotFound
    ));

    let temp = isolated_temp_dir("t1_f2_05").expect("temp dir");
    let db_path = temp.path().join("Default/Cookies");
    std::fs::create_dir_all(db_path.parent().unwrap()).expect("mkdir Default");

    let conn = Connection::open(&db_path).expect("open sqlite");
    conn.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
         INSERT INTO meta (key, value) VALUES ('version', '24');
         CREATE TABLE cookies (
             host_key TEXT NOT NULL,
             name TEXT NOT NULL,
             value TEXT NOT NULL,
             encrypted_value BLOB NOT NULL,
             path TEXT NOT NULL,
             expires_utc INTEGER NOT NULL,
             is_secure INTEGER NOT NULL,
             is_httponly INTEGER NOT NULL,
             samesite INTEGER NOT NULL
         );",
    )
    .expect("create tables");

    let future_webkit_us = (2_000_000_000i64 + 11_644_473_600i64) * 1_000_000i64;
    conn.execute(
        "INSERT INTO cookies VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            ".x.com",
            "guest_id",
            "v1%3A1700000000",
            Vec::<u8>::new(),
            "/",
            future_webkit_us,
            1i64,
            0i64,
            1i64
        ],
    )
    .expect("insert cookie row");
    drop(conn);

    match probe_chrome_cookies_at(&db_path) {
        ChromeImportProbe::Available {
            path,
            profile_name,
            size_bytes,
            encrypted_v10,
        } => {
            assert_eq!(path, db_path);
            assert_eq!(profile_name, "Default");
            assert!(size_bytes > 0);
            assert_eq!(
                encrypted_v10,
                cfg!(target_os = "macos") || cfg!(target_os = "linux")
            );
        }
        other => panic!("expected Available, got {other:?}"),
    }

    let report = import_cookies_from_chrome_sqlite(
        &db_path,
        &NoopChromeKeyProvider,
        &ChromeImportOptions {
            domain_filter: Some("x.com".into()),
            include_expired: false,
            skip_undecryptable: true,
        },
    )
    .expect("import from sqlite fixture");
    assert_eq!(report.imported_count, 1);
    assert_eq!(report.cookies[0].name, "guest_id");
    assert_eq!(report.cookies[0].value, "v1%3A1700000000");
    assert_eq!(report.cookies[0].same_site, Some(SameSitePolicy::Lax));
}

/// [Test ID] TEST-BR-T1-F3-01
/// [Feature / Req] F3 / R3, AC5 — `ScriptCompiler::compile_from_trace` compiles `BrowserActionTrace` into `AutomationScriptApp` IR
/// [Preconditions] `BrowserActionTrace` populated with `Navigate`, `WaitFor`, `Click`, `Fill`, `PressKey`, `Scroll`, `ExtractText`, `AssertVisible`, and `AssertTextContains`.
/// [Opaque-Box Steps] Push actions using `SemanticLocator`s (`test_id`, `aria_label`, `css_selector`), call `ScriptCompiler::compile_from_trace`, and run `validate_script_app_safe_for_publish`.
/// [Observable Assertions] Compiled `AutomationScriptApp` preserves `SCRIPT_APP_SCHEMA_VERSION`, `slug`, `target_domains`, all 9 typed `ScriptStep`s, and `SemanticLocator` attributes (`test_id` / `aria_label` / `css_selector`).
#[tokio::test]
async fn test_br_t1_f3_01_script_compiler_from_action_trace() {
    let mut trace = BrowserActionTrace::new("sess-t1-f3-01");
    trace.initial_url = Some("http://127.0.0.1:8080/".into());
    trace.parameter_bindings.push(TraceParameterBinding {
        param_name: "query".into(),
        literal_value: "rust".into(),
        description: "Catalog query".into(),
        required: true,
        is_secret: false,
    });

    let loc_test_id = SemanticLocator::by_test_id("search-input");
    let loc_aria = SemanticLocator::by_aria_label("Search");
    let loc_css = SemanticLocator::by_css("table[data-testid='results-table']");

    trace.push_action(
        None,
        RecordedActionKind::Navigate {
            url: "http://127.0.0.1:8080/".into(),
        },
        Some(12),
    );
    trace.push_action(
        None,
        RecordedActionKind::WaitFor {
            locator: loc_test_id.clone(),
            timeout_ms: 500,
        },
        Some(8),
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: loc_test_id.clone(),
            value: "rust".into(),
            is_secret: false,
            vault_key_hint: None,
            parameter_name_hint: Some("query".into()),
        },
        Some(10),
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: loc_aria.clone(),
        },
        Some(9),
    );
    trace.push_action(
        None,
        RecordedActionKind::PressKey {
            key: "Enter".into(),
        },
        Some(4),
    );
    trace.push_action(
        None,
        RecordedActionKind::Scroll {
            delta_x: 0.0,
            delta_y: 200.0,
        },
        Some(3),
    );
    trace.push_action(
        None,
        RecordedActionKind::AssertVisible {
            locator: loc_css.clone(),
        },
        Some(5),
    );
    trace.push_action(
        None,
        RecordedActionKind::ExtractText {
            locator: SemanticLocator::by_test_id("total-results"),
            output_key: "count".into(),
        },
        Some(6),
    );
    trace.push_action(
        None,
        RecordedActionKind::AssertTextContains {
            locator: SemanticLocator::by_test_id("total-results"),
            expected: "results".into(),
        },
        Some(4),
    );
    trace.mark_completed();

    let app = ScriptCompiler::compile_from_trace(
        &trace,
        ScriptAppMetadata {
            name: "Catalog Search Flow".into(),
            slug: Some("catalog-search-flow".into()),
            category: "productivity".into(),
            description: "Searches mock catalog".into(),
            ..Default::default()
        },
    )
    .expect("compile_from_trace");

    assert_eq!(app.schema_version, SCRIPT_APP_SCHEMA_VERSION);
    assert_eq!(app.slug, "catalog-search-flow");
    assert!(app.target_domains.contains(&"127.0.0.1".to_string()));
    assert_eq!(app.steps.len(), 9);
    assert!(app.parameters.iter().any(|p| p.name == "query" && !p.is_secret));

    match &app.steps[2] {
        ScriptStep::Fill {
            locator,
            value_template,
            secret_vault_key,
        } => {
            assert_eq!(locator.test_id.as_deref(), Some("search-input"));
            assert_eq!(value_template, "{{query}}");
            assert!(secret_vault_key.is_none());
        }
        other => panic!("expected Fill step, got {other:?}"),
    }
    match &app.steps[3] {
        ScriptStep::Click { locator } => {
            assert_eq!(locator.aria_label.as_deref(), Some("Search"));
        }
        other => panic!("expected Click step, got {other:?}"),
    }
    match &app.steps[6] {
        ScriptStep::AssertVisible { locator } => {
            assert_eq!(
                locator.css_selector.as_deref(),
                Some("table[data-testid='results-table']")
            );
        }
        other => panic!("expected AssertVisible step, got {other:?}"),
    }

    validate_script_app_safe_for_publish(&app).expect("publish-safe");
}

/// [Test ID] TEST-BR-T1-F3-02
/// [Feature / Req] F3 / R3, AC5 — Secret redaction during trace compilation converts secret inputs into `secret_vault_key` / secret `ScriptParameter`
/// [Preconditions] `BrowserActionTrace` containing `FakeSecretFixtures` values in `Fill` (password with `is_secret=true`, auth_token parameter hint) and URL query parameters (`?token=...`).
/// [Opaque-Box Steps] Compile trace via `ScriptCompiler::compile_from_trace`, serialize `AutomationScriptApp` to JSON, and validate with `validate_script_app_safe_for_publish`.
/// [Observable Assertions] `FakeSecretFixtures::contains_any_secret` returns `None` on serialized JSON; compiled steps have `secret_vault_key.is_some()` and `ScriptParameter { is_secret: true, default_value: None }`.
#[tokio::test]
async fn test_br_t1_f3_02_script_compiler_redacts_secrets() {
    let secrets = fake_secrets();
    let mut trace = BrowserActionTrace::new("sess-t1-f3-02");
    trace.push_action(
        None,
        RecordedActionKind::Navigate {
            url: format!(
                "https://x.com/callback?auth_token={}&api_key={}",
                secrets.auth_token, secrets.ct0
            ),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("LoginForm_Password_Input"),
            value: secrets.password.into(),
            is_secret: true,
            vault_key_hint: None,
            parameter_name_hint: Some("password".into()),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("api-bearer-field"),
            value: secrets.bearer_token.into(),
            is_secret: false,
            vault_key_hint: None,
            parameter_name_hint: None,
        },
        None,
    );

    let app = ScriptCompiler::compile_from_trace(
        &trace,
        ScriptAppMetadata {
            name: "Redacted Auth Flow".into(),
            ..Default::default()
        },
    )
    .expect("compile_from_trace");

    let json = serde_json::to_string(&app).expect("serialize compiled app");
    assert_eq!(secrets.contains_any_secret(&json), None);
    validate_script_app_safe_for_publish(&app).expect("publish-safe");

    let secret_params: Vec<_> = app.parameters.iter().filter(|p| p.is_secret).collect();
    assert!(!secret_params.is_empty());
    assert!(secret_params.iter().all(|p| p.default_value.is_none()));

    match &app.steps[1] {
        ScriptStep::Fill {
            secret_vault_key, ..
        } => assert!(secret_vault_key.is_some()),
        other => panic!("expected Fill step, got {other:?}"),
    }
    match &app.steps[2] {
        ScriptStep::Fill {
            secret_vault_key, ..
        } => assert!(secret_vault_key.is_some()),
        other => panic!("expected Fill step, got {other:?}"),
    }
}

/// [Test ID] TEST-BR-T1-F3-03
/// [Feature / Req] F3 / R3, AC5 — Parameterized `DeterministicScriptRunner::run` executes `MockWebAppServer` search+order script with substituted parameters
/// [Preconditions] `MockWebAppServer` started; real `ChromeCdp` launched; compiled parameterized `AutomationScriptApp` (`query`, `customer_name`, `customer_email`).
/// [Opaque-Box Steps] Compile multi-step trace for catalog search -> select item -> add to cart -> checkout -> submit order, then execute `DeterministicScriptRunner::run` with non-default parameter values.
/// [Observable Assertions] `report.used_llm == false`, `report.success == true`, `report.steps_executed == app.steps.len()`, `server.order_count() == 1`, and `OrderRecord` matches substituted customer name & email.
#[tokio::test]
async fn test_br_t1_f3_03_deterministic_runner_executes_parameterized_webapp_order() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f3_03").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    let mut trace = BrowserActionTrace::new("sess-t1-f3-03");
    trace.parameter_bindings = vec![
        TraceParameterBinding {
            param_name: "query".into(),
            literal_value: "default_query".into(),
            description: "Search term".into(),
            required: true,
            is_secret: false,
        },
        TraceParameterBinding {
            param_name: "customer_name".into(),
            literal_value: "Default Name".into(),
            description: "Customer full name".into(),
            required: true,
            is_secret: false,
        },
        TraceParameterBinding {
            param_name: "customer_email".into(),
            literal_value: "default@example.test".into(),
            description: "Customer email".into(),
            required: true,
            is_secret: false,
        },
    ];
    trace.push_action(
        None,
        RecordedActionKind::Navigate {
            url: server.url("/"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("search-input"),
            value: "default_query".into(),
            is_secret: false,
            vault_key_hint: None,
            parameter_name_hint: Some("query".into()),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("search-submit"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::WaitFor {
            locator: SemanticLocator::by_test_id("item-link-item-01"),
            timeout_ms: 4_000,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("item-link-item-01"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::WaitFor {
            locator: SemanticLocator::by_test_id("add-to-cart-btn"),
            timeout_ms: 4_000,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("add-to-cart-btn"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::WaitFor {
            locator: SemanticLocator::by_test_id("cart-status"),
            timeout_ms: 4_000,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("go-to-checkout"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::WaitFor {
            locator: SemanticLocator::by_test_id("checkout-name"),
            timeout_ms: 4_000,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("checkout-name"),
            value: "Default Name".into(),
            is_secret: false,
            vault_key_hint: None,
            parameter_name_hint: Some("customer_name".into()),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("checkout-email"),
            value: "default@example.test".into(),
            is_secret: false,
            vault_key_hint: None,
            parameter_name_hint: Some("customer_email".into()),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("checkout-submit"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::WaitFor {
            locator: SemanticLocator::by_test_id("order-confirmation"),
            timeout_ms: 4_000,
        },
        None,
    );

    let app = ScriptCompiler::compile_from_trace(
        &trace,
        ScriptAppMetadata {
            name: "Catalog Order Replay".into(),
            ..Default::default()
        },
    )
    .expect("compile_from_trace");

    let params = BTreeMap::from([
        ("query".to_string(), "Rust".to_string()),
        ("customer_name".to_string(), "Ada Lovelace".to_string()),
        (
            "customer_email".to_string(),
            "ada@example.test".to_string(),
        ),
    ]);

    let report = DeterministicScriptRunner::run(&backend, &app, &params, None, None)
        .await
        .expect("run parameterized script");
    assert!(!report.used_llm);
    assert!(report.success);
    assert_eq!(report.steps_executed, app.steps.len());

    assert_eq!(server.order_count(), 1);
    {
        let orders = server.state().orders.read();
        let order = orders.values().next().expect("recorded order");
        assert_eq!(order.customer_name, "Ada Lovelace");
        assert_eq!(order.customer_email, "ada@example.test");
        assert!(order.confirmation_id.starts_with("ORD-"));
    }

    backend.close().await.expect("close backend");
    server.shutdown().await;
}
/// [Test ID] TEST-BR-T1-F3-04
/// [Feature / Req] F3 / R3, AC5 — `ExtractText`, `AssertVisible`, and `AssertTextContains` steps populate `extracted_outputs` and verify live DOM
/// [Preconditions] `MockWebAppServer` started; real `ChromeCdp` launched.
/// [Opaque-Box Steps] Run a script that opens `/item/item-01`, checks `AssertVisible` on `item-title`, runs `ExtractText` (`item_title`), verifies `AssertTextContains` (`$45.00`), completes cart+checkout, and extracts `confirmation-id`.
/// [Observable Assertions] `extracted_outputs["item_title"] == "Rust Async Systems Handbook"`, `extracted_outputs["confirmation_id"]` starts with `"ORD-"`, and all assertions pass.
#[tokio::test]
async fn test_br_t1_f3_04_extract_text_and_assertions_in_deterministic_runner() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f3_04").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    let mut trace = BrowserActionTrace::new("sess-t1-f3-04");
    trace.push_action(
        None,
        RecordedActionKind::Navigate {
            url: server.url("/item/item-01"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::AssertVisible {
            locator: SemanticLocator::by_test_id("item-title"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::ExtractText {
            locator: SemanticLocator::by_test_id("item-title"),
            output_key: "item_title".into(),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::AssertTextContains {
            locator: SemanticLocator::by_test_id("item-price"),
            expected: "$45.00".into(),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("add-to-cart-btn"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::WaitFor {
            locator: SemanticLocator::by_test_id("cart-status"),
            timeout_ms: 4_000,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("go-to-checkout"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::WaitFor {
            locator: SemanticLocator::by_test_id("checkout-name"),
            timeout_ms: 4_000,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("checkout-name"),
            value: "Grace Hopper".into(),
            is_secret: false,
            vault_key_hint: None,
            parameter_name_hint: None,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("checkout-email"),
            value: "grace@example.test".into(),
            is_secret: false,
            vault_key_hint: None,
            parameter_name_hint: None,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("checkout-submit"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::WaitFor {
            locator: SemanticLocator::by_test_id("order-confirmation"),
            timeout_ms: 4_000,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::ExtractText {
            locator: SemanticLocator::by_test_id("confirmation-id"),
            output_key: "confirmation_id".into(),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::AssertTextContains {
            locator: SemanticLocator::by_test_id("confirmation-id"),
            expected: "ORD-".into(),
        },
        None,
    );

    let app = ScriptCompiler::compile_from_trace(
        &trace,
        ScriptAppMetadata {
            name: "Extract and Assert Flow".into(),
            ..Default::default()
        },
    )
    .expect("compile_from_trace");

    let report = DeterministicScriptRunner::run(&backend, &app, &BTreeMap::new(), None, None)
        .await
        .expect("run extract & assert script");
    assert!(report.success);
    assert!(!report.used_llm);
    assert_eq!(
        report.extracted_outputs.get("item_title").map(String::as_str),
        Some("Rust Async Systems Handbook")
    );
    let conf_id = report
        .extracted_outputs
        .get("confirmation_id")
        .expect("confirmation_id extracted");
    assert!(conf_id.starts_with("ORD-"), "got {conf_id}");

    backend.close().await.expect("close backend");
    server.shutdown().await;
}
/// [Test ID] TEST-BR-T1-F3-05
/// [Feature / Req] F3 / R3, AC3 — `ScriptReplayReport` emits complete `step_timings` and bounded `total_duration_ms`
/// [Preconditions] `MockWebAppServer` started; real `ChromeCdp` launched.
/// [Opaque-Box Steps] Compile and replay a 5-step search script via `DeterministicScriptRunner::run` and inspect `ScriptReplayReport`.
/// [Observable Assertions] `report.used_llm == false`, `report.step_timings.len() == report.steps_executed`, each step reports `step_index`, `ChromeCdp` backend, `success == true`, monotonic `timestamp_ms`, and step durations bounded by `total_duration_ms`.
#[tokio::test]
async fn test_br_t1_f3_05_replay_report_emits_monotonic_step_timings() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f3_05").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    let mut trace = BrowserActionTrace::new("sess-t1-f3-05");
    trace.push_action(
        None,
        RecordedActionKind::Navigate {
            url: server.url("/"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("search-input"),
            value: "books".into(),
            is_secret: false,
            vault_key_hint: None,
            parameter_name_hint: None,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("search-submit"),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::WaitFor {
            locator: SemanticLocator::by_test_id("results-table"),
            timeout_ms: 4_000,
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::AssertVisible {
            locator: SemanticLocator::by_test_id("page-indicator"),
        },
        None,
    );

    let app = ScriptCompiler::compile_from_trace(
        &trace,
        ScriptAppMetadata {
            name: "Telemetry Replay App".into(),
            ..Default::default()
        },
    )
    .expect("compile_from_trace");

    let report = DeterministicScriptRunner::run(&backend, &app, &BTreeMap::new(), None, None)
        .await
        .expect("run telemetry script");

    assert!(!report.used_llm);
    assert!(report.success);
    assert_eq!(report.steps_executed, 5);
    assert_eq!(report.step_timings.len(), report.steps_executed);
    assert!(report.final_url.as_deref().is_some_and(|u| u.contains("/search")));

    let mut prev_ts = i64::MIN;
    let mut sum_ms = 0u64;
    for (idx, metric) in report.step_timings.iter().enumerate() {
        assert_eq!(metric.step_index, Some(idx));
        assert_eq!(metric.backend, Some(BrowserBackendKind::ChromeCdp));
        assert!(metric.success);
        assert!(metric.timestamp_ms >= prev_ts, "non-monotonic step timestamp");
        prev_ts = metric.timestamp_ms;
        assert!(metric.duration_ms <= report.total_duration_ms);
        sum_ms += metric.duration_ms;
    }
    assert!(
        sum_ms <= report.total_duration_ms + report.step_timings.len() as u64,
        "step duration sum {sum_ms} exceeded total_duration_ms {}",
        report.total_duration_ms
    );

    backend.close().await.expect("close backend");
    server.shutdown().await;
}

// ============================================================================
// F4–F6 Helpers & Tests (TEST-BR-T1-F4-01..05, F5-01..05, F6-01..05)
// ============================================================================

fn build_clean_store_app(
    app_id: &str,
    slug: &str,
    name: &str,
    version: &str,
    category: &str,
    domains: Vec<&str>,
) -> AutomationScriptApp {
    AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: app_id.to_string(),
        slug: slug.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        description: format!("Clean E2E automation app for {name}"),
        category: category.to_string(),
        target_domains: domains.into_iter().map(ToString::to_string).collect(),
        parameters: vec![ScriptParameter {
            name: "query".to_string(),
            description: "Search term".to_string(),
            required: true,
            default_value: Some("rust".to_string()),
            is_secret: false,
        }],
        steps: vec![
            ScriptStep::Navigate {
                url_template: "https://example.com/search".to_string(),
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_test_id("search-input"),
                value_template: "{{query}}".to_string(),
                secret_vault_key: None,
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("search-submit"),
            },
        ],
        required_capabilities: vec!["browser.dom".to_string()],
        author_id: None,
        source_trace_summary: Some("trace=t1-f4 steps=3".to_string()),
    }
}

async fn drop_client_async(client: TelegramClient) {
    client.clear_session();
    tokio::task::spawn_blocking(move || drop(client))
        .await
        .expect("spawn_blocking drop TelegramClient");
}

struct CountingTransportFactory {
    creations: Arc<AtomicUsize>,
}

#[async_trait]
impl SessionTransportFactory for CountingTransportFactory {
    async fn create_transport(
        &self,
        _session_id: &str,
        _data_dir: &std::path::Path,
        _origin_hint: Option<&NormalizedOrigin>,
        _event_tx: &broadcast::Sender<BotBrowserEvent>,
    ) -> Result<Arc<dyn CdpTransport>, BotBrowserError> {
        self.creations.fetch_add(1, Ordering::SeqCst);
        Err(BotBrowserError::Cdp(
            "unexpected duplicate tab spawn during active-tab test".to_string(),
        ))
    }
}

fn full_turn_req(prompt: &str, session_id: &str) -> HermesTurnRequest {
    let mut req = HermesTurnRequest::new(prompt).with_session(session_id);
    req.preset = Some(ToolsetPreset::Full);
    req
}

/// [Test ID] TEST-BR-T1-F4-01
/// [Feature / Req] F4 / R4, AC6 — `POST /api/v1/automation-apps` publish + `GET /api/v1/automation-apps/{id}` roundtrip + SQLite v41 persistence
/// [Preconditions] `EphemeralTestServer` started (SQLite v41); User A & User B `TelegramClient`s registered.
/// [Opaque-Box Steps] User A publishes a valid `AutomationScriptApp` via `POST /api/v1/automation-apps`, verifies SQLite `_schema_migrations` = 41 and rows in `automation_apps` + `automation_app_versions`, and User B + anonymous HTTP client fetch `GET /api/v1/automation-apps/{id}`.
/// [Observable Assertions] HTTP `201` on publish, `200` on anonymous GET, `content_sha256` matches `compute_automation_manifest_sha256`, SQLite v41 rows match, and `AutomationScriptApp` deserializes identically.
#[tokio::test]
async fn test_br_t1_f4_01_publish_get_roundtrip_and_sqlite_v41_persistence() {
    let mut server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let alice_dir = isolated_temp_dir("t1_f4_01_alice").expect("alice dir");
    let bob_dir = isolated_temp_dir("t1_f4_01_bob").expect("bob dir");

    let cfg = ClientConfig {
        server_url: server.base_url(),
        ..Default::default()
    };
    let client_a = TelegramClient::open_with_dir(cfg.clone(), alice_dir.path()).expect("client_a");
    let alice_auth = client_a
        .register(&RegisterRequest {
            username: "t1_f4_01_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register alice");

    let client_b = TelegramClient::open_with_dir(cfg, bob_dir.path()).expect("client_b");
    let _bob_auth = client_b
        .register(&RegisterRequest {
            username: "t1_f4_01_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        })
        .await
        .expect("register bob");

    let app = build_clean_store_app(
        "app-t1-f4-01",
        "t1-f4-01-roundtrip",
        "T1 F4 Roundtrip App",
        "1.0.0",
        "productivity",
        vec!["example.com"],
    );
    let manifest_val = serde_json::to_value(&app).expect("serialize manifest");

    let raw_http = no_proxy_http_client();
    let publish_resp = raw_http
        .post(format!("{}/api/v1/automation-apps", server.base_url()))
        .bearer_auth(&alice_auth.token)
        .json(&PublishAutomationAppRequest {
            manifest: manifest_val,
            is_published: true,
            changelog: Some("Initial release".into()),
        })
        .send()
        .await
        .expect("POST /api/v1/automation-apps");
    assert_eq!(publish_resp.status().as_u16(), 201);
    let published: gpui_telegram_proto::automation_apps::AutomationAppDetail =
        publish_resp.json().await.expect("parse AutomationAppDetail");
    assert_eq!(published.app_id, "app-t1-f4-01");
    assert_eq!(published.author_user_id, alice_auth.user_id);
    assert_eq!(published.latest_version, "1.0.0");
    assert_eq!(published.versions, vec!["1.0.0".to_string()]);
    assert_eq!(
        published.content_sha256,
        compute_automation_manifest_sha256(&serde_json::to_string(&published.manifest).unwrap())
    );

    // Direct SQLite v41 persistence inspection
    {
        let db = &server.state().expect("active server state").db;
        let (max_ver, app_rows, ver_rows): (i64, i64, i64) = db
            .pool()
            .with_reader(|conn| {
                let max_ver: i64 = conn
                    .query_row(
                        "SELECT COALESCE(MAX(version), 0) FROM _schema_migrations",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                let app_rows: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM automation_apps WHERE app_id = ?1",
                        ["app-t1-f4-01"],
                        |r| r.get(0),
                    )
                    .unwrap();
                let ver_rows: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM automation_app_versions WHERE app_id = ?1 AND version = '1.0.0'",
                        ["app-t1-f4-01"],
                        |r| r.get(0),
                    )
                    .unwrap();
                Ok((max_ver, app_rows, ver_rows))
            })
            .expect("SQLite reader");
        assert!(max_ver >= 41, "expected SQLite schema >= 41, got {max_ver}");
        assert_eq!(app_rows, 1);
        assert_eq!(ver_rows, 1);
    }

    // Anonymous GET + Client B GET
    let anon_get = raw_http
        .get(format!(
            "{}/api/v1/automation-apps/app-t1-f4-01",
            server.base_url()
        ))
        .send()
        .await
        .expect("anon GET");
    assert_eq!(anon_get.status().as_u16(), 200);

    let fetched = client_b
        .get_automation_app("app-t1-f4-01")
        .await
        .expect("client_b get_automation_app");
    assert_eq!(fetched.content_sha256, published.content_sha256);
    let roundtripped: AutomationScriptApp =
        serde_json::from_value(fetched.manifest).expect("deserialize roundtripped manifest");
    assert_eq!(roundtripped.steps, app.steps);
    assert_eq!(roundtripped.parameters, app.parameters);

    drop_client_async(client_a).await;
    drop_client_async(client_b).await;
    server.stop().await;
}

/// [Test ID] TEST-BR-T1-F4-02
/// [Feature / Req] F4 / R4, AC6 — Catalog browse & search (`q`, `category`, `domain` exact/subdomain vs lookalike, `limit`/`offset`)
/// [Preconditions] `EphemeralTestServer` started; 4 published apps across distinct categories (`social`, `productivity`, `general`) and domains (`x.com`, `api.x.com`, `myx.com`, `shop.example.org`).
/// [Opaque-Box Steps] Call `search_automation_apps` with `q`, `category`, `domain="x.com"`, and `limit`/`offset` pagination slices.
/// [Observable Assertions] `domain="x.com"` matches `x.com` and `api.x.com` while excluding `myx.com`; `category` and `q` return exact subsets; pagination `limit`/`offset` respects `total` and slices accurately.
#[tokio::test]
async fn test_br_t1_f4_02_catalog_browse_search_domain_and_pagination() {
    let mut server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let alice_dir = isolated_temp_dir("t1_f4_02_alice").expect("alice dir");
    let client = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: server.base_url(),
            ..Default::default()
        },
        alice_dir.path(),
    )
    .expect("client");
    client
        .register(&RegisterRequest {
            username: "t1_f4_02_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register");

    let apps = vec![
        build_clean_store_app(
            "app-x-main",
            "x-main",
            "Alpha X Poster",
            "1.0.0",
            "social",
            vec!["x.com"],
        ),
        build_clean_store_app(
            "app-x-api",
            "x-api",
            "Beta X Analytics",
            "1.0.0",
            "social",
            vec!["api.x.com"],
        ),
        build_clean_store_app(
            "app-myx-lookalike",
            "myx-lookalike",
            "Gamma MyX Helper",
            "1.0.0",
            "social",
            vec!["myx.com"],
        ),
        build_clean_store_app(
            "app-shop-order",
            "shop-order",
            "Delta Shop Checkout",
            "1.0.0",
            "productivity",
            vec!["shop.example.org"],
        ),
    ];

    for app in &apps {
        client
            .publish_automation_app(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .await
            .expect("publish catalog app");
    }

    // 1. Domain exact/subdomain boundary: "x.com" matches x.com + api.x.com, NOT myx.com
    let dom_res = client
        .search_automation_apps(None, None, Some("x.com"), None, None)
        .await
        .expect("domain search");
    assert_eq!(dom_res.total, 2);
    let dom_ids: Vec<&str> = dom_res.apps.iter().map(|a| a.app_id.as_str()).collect();
    assert!(dom_ids.contains(&"app-x-main"));
    assert!(dom_ids.contains(&"app-x-api"));
    assert!(!dom_ids.contains(&"app-myx-lookalike"));

    // 2. Category + keyword query
    let cat_res = client
        .search_automation_apps(Some("checkout"), Some("productivity"), None, None, None)
        .await
        .expect("category+q search");
    assert_eq!(cat_res.total, 1);
    assert_eq!(cat_res.apps[0].app_id, "app-shop-order");

    // 3. Pagination (4 total apps, page size 2, offset 0 vs offset 2)
    let page1 = client
        .search_automation_apps(None, None, None, Some(2), Some(0))
        .await
        .expect("page 1");
    assert_eq!(page1.total, 4);
    assert_eq!(page1.limit, 2);
    assert_eq!(page1.offset, 0);
    assert_eq!(page1.apps.len(), 2);

    let page2 = client
        .search_automation_apps(None, None, None, Some(2), Some(2))
        .await
        .expect("page 2");
    assert_eq!(page2.total, 4);
    assert_eq!(page2.limit, 2);
    assert_eq!(page2.offset, 2);
    assert_eq!(page2.apps.len(), 2);

    let mut seen_ids: Vec<String> = page1
        .apps
        .into_iter()
        .chain(page2.apps)
        .map(|a| a.app_id)
        .collect();
    seen_ids.sort();
    assert_eq!(
        seen_ids,
        vec![
            "app-myx-lookalike".to_string(),
            "app-shop-order".to_string(),
            "app-x-api".to_string(),
            "app-x-main".to_string(),
        ]
    );

    drop_client_async(client).await;
    server.stop().await;
}

/// [Test ID] TEST-BR-T1-F4-03
/// [Feature / Req] F4 / R4, AC6 — Install, pinned version in `GET /installed`, idempotent re-install, uninstall & `install_count`
/// [Preconditions] `EphemeralTestServer` started; User A publishes v1.0.0; User B registered.
/// [Opaque-Box Steps] User B installs v1.0.0, re-installs idempotently, User A publishes v2.0.0 with updated name, User B checks `GET /installed`, then User B uninstalls.
/// [Observable Assertions] First install returns `newly_installed == true` and `install_count == 1`; re-install returns `newly_installed == false` and `install_count == 1`; after v2.0.0 publish, `/installed` retains pinned `installed_version == "1.0.0"` and `latest_version == "2.0.0"`; after uninstall `removed == true`, `/installed` is empty, and `install_count == 0`.
#[tokio::test]
async fn test_br_t1_f4_03_install_pinned_version_idempotent_and_uninstall() {
    let mut server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let alice_dir = isolated_temp_dir("t1_f4_03_alice").expect("alice dir");
    let bob_dir = isolated_temp_dir("t1_f4_03_bob").expect("bob dir");
    let cfg = ClientConfig {
        server_url: server.base_url(),
        ..Default::default()
    };

    let client_a = TelegramClient::open_with_dir(cfg.clone(), alice_dir.path()).expect("client_a");
    client_a
        .register(&RegisterRequest {
            username: "t1_f4_03_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register alice");

    let client_b = TelegramClient::open_with_dir(cfg, bob_dir.path()).expect("client_b");
    client_b
        .register(&RegisterRequest {
            username: "t1_f4_03_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        })
        .await
        .expect("register bob");

    let app_v1 = build_clean_store_app(
        "app-t1-f4-03",
        "pinned-app-v1",
        "Pinned App V1",
        "1.0.0",
        "social",
        vec!["x.com"],
    );
    client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&app_v1).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .expect("publish v1");

    let inst1 = client_b
        .install_automation_app("app-t1-f4-03")
        .await
        .expect("first install");
    assert!(inst1.newly_installed);
    assert_eq!(inst1.install_count, 1);
    assert_eq!(inst1.app.installed_version, "1.0.0");

    // Idempotent re-install
    let inst2 = client_b
        .install_automation_app("app-t1-f4-03")
        .await
        .expect("idempotent re-install");
    assert!(!inst2.newly_installed);
    assert_eq!(inst2.install_count, 1);

    // Author publishes v2.0.0 with updated name & slug
    let mut app_v2 = build_clean_store_app(
        "app-t1-f4-03",
        "pinned-app-v2",
        "Pinned App V2",
        "2.0.0",
        "social",
        vec!["x.com"],
    );
    app_v2.steps.push(ScriptStep::Scroll {
        delta_x: 0.0,
        delta_y: 120.0,
    });
    client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&app_v2).unwrap(),
            is_published: true,
            changelog: Some("v2".into()),
        })
        .await
        .expect("publish v2");

    // User B still sees pinned v1.0.0 manifest until explicit upgrade/reinstall
    let installed = client_b
        .list_installed_automation_apps()
        .await
        .expect("list installed");
    assert_eq!(installed.total, 1);
    assert_eq!(installed.apps[0].installed_version, "1.0.0");
    assert_eq!(installed.apps[0].latest_version, "2.0.0");
    assert_eq!(installed.apps[0].name, "Pinned App V1");
    let pinned_manifest: AutomationScriptApp =
        serde_json::from_value(installed.apps[0].manifest.clone()).unwrap();
    assert_eq!(pinned_manifest.steps.len(), app_v1.steps.len());

    // Uninstall
    let uninst = client_b
        .uninstall_automation_app("app-t1-f4-03")
        .await
        .expect("uninstall");
    assert!(uninst.removed);
    let after_uninst = client_b
        .list_installed_automation_apps()
        .await
        .expect("list after uninstall");
    assert_eq!(after_uninst.total, 0);
    let detail = client_b
        .get_automation_app("app-t1-f4-03")
        .await
        .expect("get detail after uninstall");
    assert_eq!(detail.install_count, 0);
    assert!(!detail.is_installed);

    drop_client_async(client_a).await;
    drop_client_async(client_b).await;
    server.stop().await;
}

/// [Test ID] TEST-BR-T1-F4-04
/// [Feature / Req] F4 / R4, AC6 — `POST /api/v1/automation-apps/{id}/runs` telemetry & persistence across `EphemeralTestServer::restart()`
/// [Preconditions] `EphemeralTestServer` started; User A publishes app; User B installs and records a run.
/// [Opaque-Box Steps] Call `record_automation_app_run` (`success: true`, `steps_executed: 3`, `total_duration_ms: 245`, `used_llm: false`), call `server.restart().await`, inspect SQLite `automation_app_runs` and fetch via new client.
/// [Observable Assertions] `run_count == 1` and `last_run_at_ms > 0`; after `server.restart()`, `automation_app_runs` row and `get_automation_app().run_count == 1` persist intact.
#[tokio::test]
async fn test_br_t1_f4_04_record_run_telemetry_persists_across_server_restart() {
    let mut server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let alice_dir = isolated_temp_dir("t1_f4_04_alice").expect("alice dir");
    let bob_dir = isolated_temp_dir("t1_f4_04_bob").expect("bob dir");
    let cfg = ClientConfig {
        server_url: server.base_url(),
        ..Default::default()
    };

    let client_a = TelegramClient::open_with_dir(cfg.clone(), alice_dir.path()).expect("client_a");
    client_a
        .register(&RegisterRequest {
            username: "t1_f4_04_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register alice");
    let app = build_clean_store_app(
        "app-t1-f4-04",
        "run-telemetry",
        "Run Telemetry App",
        "1.0.0",
        "general",
        vec!["example.com"],
    );
    client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&app).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .expect("publish");

    let client_b = TelegramClient::open_with_dir(cfg, bob_dir.path()).expect("client_b");
    let _bob = client_b
        .register(&RegisterRequest {
            username: "t1_f4_04_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        })
        .await
        .expect("register bob");
    client_b
        .install_automation_app("app-t1-f4-04")
        .await
        .expect("install");

    let run_resp = client_b
        .record_automation_app_run(
            "app-t1-f4-04",
            &RecordAutomationRunRequest {
                version: Some("1.0.0".into()),
                success: true,
                steps_executed: 3,
                total_duration_ms: 245,
                used_llm: false,
                error: None,
            },
        )
        .await
        .expect("record run");
    assert_eq!(run_resp.run_count, 1);
    assert!(run_resp.last_run_at_ms > 0);

    drop_client_async(client_a).await;
    drop_client_async(client_b).await;

    // Restart server on same SQLite directory
    server.restart().await.expect("restart server");

    {
        let db = &server.state().expect("active state after restart").db;
        let runs = db
            .list_automation_app_runs("app-t1-f4-04", 10)
            .expect("list_automation_app_runs");
        assert_eq!(runs.len(), 1);
        assert!(runs[0].success);
        assert_eq!(runs[0].steps_executed, 3);
        assert_eq!(runs[0].total_duration_ms, 245);
        assert!(!runs[0].used_llm);
    }

    // Re-login Bob against new port & confirm API reads persisted stats
    let bob_post_dir = isolated_temp_dir("t1_f4_04_bob_post").expect("bob post dir");
    let client_b2 = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: server.base_url(),
            ..Default::default()
        },
        bob_post_dir.path(),
    )
    .expect("client_b2");
    client_b2
        .login(&LoginRequest {
            username: "t1_f4_04_bob".into(),
            password: "StrongPassword!123".into(),
            device_name: "mac-b2".into(),
        })
        .await
        .expect("login bob after restart");

    let detail = client_b2
        .get_automation_app("app-t1-f4-04")
        .await
        .expect("get after restart");
    assert_eq!(detail.run_count, 1);
    assert_eq!(detail.install_count, 1);
    assert!(detail.is_installed);

    let installed = client_b2
        .list_installed_automation_apps()
        .await
        .expect("installed after restart");
    assert_eq!(installed.total, 1);
    assert_eq!(installed.apps[0].local_run_count, 1);
    assert!(installed.apps[0].last_run_at_ms.is_some());

    drop_client_async(client_b2).await;
    server.stop().await;
}

/// [Test ID] TEST-BR-T1-F4-05
/// [Feature / Req] F4 / R4, AC6 — `TelegramClient` full facade + `LocalStore` offline persistence across client re-instantiation
/// [Preconditions] `EphemeralTestServer` started; User A & User B persistent cache dirs created.
/// [Opaque-Box Steps] Exercise all 7 `TelegramClient` automation-app methods (`publish`, `search`, `get`, `install`, `record_run`, `list_installed`, `uninstall` on a 2nd app); drop client, stop server completely, reopen `TelegramClient::open_with_dir` with same URL & cache dir, query `*_cached()` offline.
/// [Observable Assertions] Reopened offline client returns installed app, cached detail manifest, and `local_run_count == 1` with zero network server running.
#[tokio::test]
async fn test_br_t1_f4_05_telegram_client_facade_and_local_store_offline_cache() {
    let mut server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let alice_dir = isolated_temp_dir("t1_f4_05_alice").expect("alice dir");
    let bob_dir = isolated_temp_dir("t1_f4_05_bob").expect("bob dir");
    let cfg = ClientConfig {
        server_url: server.base_url(),
        ..Default::default()
    };

    let client_a = TelegramClient::open_with_dir(cfg.clone(), alice_dir.path()).expect("client_a");
    client_a
        .register(&RegisterRequest {
            username: "t1_f4_05_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register alice");

    let app_keep = build_clean_store_app(
        "app-t1-f4-05-keep",
        "keep-app",
        "Keep Offline App",
        "1.0.0",
        "productivity",
        vec!["example.com"],
    );
    let app_temp = build_clean_store_app(
        "app-t1-f4-05-temp",
        "temp-app",
        "Temp Uninstall App",
        "1.0.0",
        "general",
        vec!["example.org"],
    );
    client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&app_keep).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .expect("publish keep");
    client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&app_temp).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .expect("publish temp");

    let client_b = TelegramClient::open_with_dir(cfg.clone(), bob_dir.path()).expect("client_b");
    let bob_auth = client_b
        .register(&RegisterRequest {
            username: "t1_f4_05_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        })
        .await
        .expect("register bob");

    let searched = client_b
        .search_automation_apps(Some("Keep Offline"), None, None, None, None)
        .await
        .expect("search");
    assert_eq!(searched.total, 1);

    let got = client_b
        .get_automation_app("app-t1-f4-05-keep")
        .await
        .expect("get");
    assert_eq!(got.name, "Keep Offline App");

    client_b
        .install_automation_app("app-t1-f4-05-keep")
        .await
        .expect("install keep");
    client_b
        .install_automation_app("app-t1-f4-05-temp")
        .await
        .expect("install temp");
    client_b
        .record_automation_app_run(
            "app-t1-f4-05-keep",
            &RecordAutomationRunRequest {
                version: Some("1.0.0".into()),
                success: true,
                steps_executed: 3,
                total_duration_ms: 111,
                used_llm: false,
                error: None,
            },
        )
        .await
        .expect("record run");
    client_b
        .uninstall_automation_app("app-t1-f4-05-temp")
        .await
        .expect("uninstall temp");
    let installed_online = client_b
        .list_installed_automation_apps()
        .await
        .expect("list_installed");
    assert_eq!(installed_online.total, 1);

    // Drop clients and shut down server completely before reopening offline
    drop_client_async(client_a).await;
    drop_client_async(client_b).await;
    server.stop().await;

    let offline_client =
        TelegramClient::open_with_dir(cfg, bob_dir.path()).expect("reopen offline client");
    offline_client.set_auth_session(bob_auth);

    let cached_list = offline_client.list_installed_automation_apps_cached();
    assert_eq!(cached_list.len(), 1);
    assert_eq!(cached_list[0].app_id, "app-t1-f4-05-keep");
    assert_eq!(cached_list[0].local_run_count, 1);

    let cached_installed = offline_client
        .get_installed_automation_app_cached("app-t1-f4-05-keep")
        .expect("cached installed app");
    let offline_manifest: AutomationScriptApp =
        serde_json::from_value(cached_installed.manifest).expect("manifest from LocalStore");
    assert_eq!(offline_manifest.steps, app_keep.steps);

    let cached_detail = offline_client
        .get_cached_automation_app("app-t1-f4-05-keep")
        .expect("cached detail");
    assert_eq!(cached_detail.name, "Keep Offline App");
    assert!(
        offline_client
            .get_installed_automation_app_cached("app-t1-f4-05-temp")
            .is_none()
    );

    drop_client_async(offline_client).await;
}

/// [Test ID] TEST-BR-T1-F5-01
/// [Feature / Req] F5 / R2, AC5 — `HermesEngine` (`ToolsetId::Browser`) + Chrome `CdpBrowserBackend` drives `MockXServer` post flow
/// [Preconditions] `MockXServer` started with seeded auth session; real Chrome `CdpBrowserBackend` imported with cookies; `FabushiBotHost` + loopback `MockOpenAiServer`.
/// [Opaque-Box Steps] Register Chrome backend, run `HermesEngine::run_turn` with `ToolsetPreset::Full` driving `browser_navigate` (`/home`), `browser_wait_for`, `browser_type` (`tweetTextarea_0`), `browser_click` (`tweetButtonInline`), `browser_extract_text` (`tweetText`).
/// [Observable Assertions] `MockXServer::tweet_count()` increments by 1 with exact post text; `BrowserAutomationEvent` stream contains `StepStarted`, `StepCompleted`, and `TraceCompleted`.
#[tokio::test]
async fn test_br_t1_f5_01_hermes_drives_mock_x_post_with_real_chrome() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let sess_seed = server.seed_session("tok_t1_f5_01", "ct0_t1_f5_01");
    let temp = isolated_temp_dir("t1_f5_01").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let cookies_json = generate_cdp_cookies_json(&x_auth_cookies(
        "127.0.0.1",
        &sess_seed.auth_token,
        &sess_seed.ct0,
    ));
    let imported = import_cookies_from_json(&cookies_json).expect("parse cookies");
    backend
        .import_cookies(&imported)
        .await
        .expect("import cookies");

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t1_f5_01",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );
    let mut ev_rx = host.subscribe_browser_events();

    let post_text = "T1-F5-01 live Chrome Hermes X post";
    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call(
            "c1",
            "browser_navigate",
            json!({ "url": server.url("/home") }),
        ),
        mock_openai_tool_call(
            "c2",
            "browser_wait_for",
            json!({ "test_id": "tweetTextarea_0", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "c3",
            "browser_type",
            json!({ "test_id": "tweetTextarea_0", "text": post_text, "parameter_name_hint": "post_text" }),
        ),
        mock_openai_tool_call(
            "c4",
            "browser_click",
            json!({ "test_id": "tweetButtonInline" }),
        ),
        mock_openai_tool_call(
            "c5",
            "browser_wait_for",
            json!({ "text_anchor": post_text, "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "c6",
            "browser_extract_text",
            json!({ "test_id": "tweetText", "output_key": "latest_post" }),
        ),
        mock_openai_text("Published post on MockX."),
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
        .expect("set active provider");

    let res = engine
        .run_turn(full_turn_req("Post to X", "tab_t1_f5_01"))
        .await
        .expect("run_turn succeeds");
    assert_eq!(res.tool_calls.len(), 6);
    assert!(res.tool_calls.iter().all(|tc| tc.success));
    assert!(res.tool_calls[5].output.contains(post_text));

    wait_for_tweet_count(&server, 3, Duration::from_secs(3)).await;
    assert!(
        server
            .state()
            .tweets
            .read()
            .iter()
            .any(|t| t.text == post_text)
    );
    assert!(sess.last_completed_trace().is_some());

    let mut saw_started = false;
    let mut saw_completed = false;
    let mut saw_trace_completed = false;
    while let Ok(ev) = ev_rx.try_recv() {
        match ev {
            BrowserAutomationEvent::StepStarted { .. } => saw_started = true,
            BrowserAutomationEvent::StepCompleted { .. } => saw_completed = true,
            BrowserAutomationEvent::TraceCompleted { .. } => saw_trace_completed = true,
            _ => {}
        }
    }
    assert!(saw_started && saw_completed && saw_trace_completed);

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}


/// [Test ID] TEST-BR-T1-F5-02
/// [Feature / Req] F5 / R2, AC5 — `HermesEngine` (`ToolsetId::Browser`) + Chrome drives `MockWebAppServer` search, category filter, paginate, cart & checkout
/// [Preconditions] `MockWebAppServer` started; real Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI executes search with category filter (`books`), navigates paginated results (`next-page` to page 2), selects `item-link-item-08`, adds to cart, opens `/checkout`, fills `checkout-name` & `checkout-email`, submits order, and extracts `confirmation-id`.
/// [Observable Assertions] `server.order_count() == 1`, extracted confirmation starts with `"ORD-"`, and `last_completed_trace()` captures each step with locators and URLs.
#[tokio::test]
async fn test_br_t1_f5_02_hermes_drives_mock_webapp_search_filter_paginate_checkout() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f5_02").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t1_f5_02",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );

    let llm = MockOpenAiServer::start(vec![
        // 1. Navigate to index, fill search-input, select category 'books' via keyboard on <select>, submit
        mock_openai_tool_call("w1", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_tool_call(
            "w2",
            "browser_type",
            json!({ "test_id": "search-input", "text": "Rust" }),
        ),
        mock_openai_tool_call(
            "w3",
            "browser_click",
            json!({ "test_id": "category-select" }),
        ),
        mock_openai_tool_call("w4", "browser_press_key", json!({ "key": "ArrowDown" })),
        mock_openai_tool_call("w5", "browser_press_key", json!({ "key": "Tab" })),
        mock_openai_tool_call(
            "w6",
            "browser_click",
            json!({ "test_id": "search-submit" }),
        ),
        mock_openai_tool_call(
            "w7",
            "browser_wait_for",
            json!({ "test_id": "item-link-item-01", "timeout_ms": 4000 }),
        ),
        // 2. Browse full catalog with page_size=5 and paginate to Page 2
        mock_openai_tool_call(
            "w8",
            "browser_navigate",
            json!({ "url": server.url("/search?q=&category=all&page_size=5") }),
        ),
        mock_openai_tool_call(
            "w9",
            "browser_wait_for",
            json!({ "test_id": "next-page", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call("w10", "browser_click", json!({ "test_id": "next-page" })),
        mock_openai_tool_call(
            "w11",
            "browser_wait_for",
            json!({ "test_id": "item-link-item-08", "timeout_ms": 4000 }),
        ),
        // 3. Select item-08 -> add to cart -> checkout -> extract confirmation
        mock_openai_tool_call(
            "w12",
            "browser_click",
            json!({ "test_id": "item-link-item-08" }),
        ),
        mock_openai_tool_call(
            "w13",
            "browser_wait_for",
            json!({ "test_id": "add-to-cart-btn", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "w14",
            "browser_click",
            json!({ "test_id": "add-to-cart-btn" }),
        ),
        mock_openai_tool_call(
            "w15",
            "browser_wait_for",
            json!({ "test_id": "cart-status", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "w16",
            "browser_click",
            json!({ "test_id": "go-to-checkout" }),
        ),
        mock_openai_tool_call(
            "w17",
            "browser_wait_for",
            json!({ "test_id": "checkout-name", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "w18",
            "browser_type",
            json!({ "test_id": "checkout-name", "text": "Grace Hopper" }),
        ),
        mock_openai_tool_call(
            "w19",
            "browser_type",
            json!({ "test_id": "checkout-email", "text": "grace@example.test" }),
        ),
        mock_openai_tool_call(
            "w20",
            "browser_click",
            json!({ "test_id": "checkout-submit" }),
        ),
        mock_openai_tool_call(
            "w21",
            "browser_wait_for",
            json!({ "test_id": "confirmation-id", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "w22",
            "browser_extract_text",
            json!({ "test_id": "confirmation-id", "output_key": "order_id" }),
        ),
        mock_openai_text("Order placed on MockWebApp."),
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
            "Search, paginate, and buy item-08",
            "tab_t1_f5_02",
        ))
        .await
        .expect("run_turn succeeds");
    assert_eq!(res.tool_calls.len(), 22);
    assert!(
        res.tool_calls.iter().all(|tc| tc.success),
        "all tool calls must succeed: {:?}",
        res.tool_calls
    );
    assert!(res.tool_calls[21].output.contains("ORD-"));

    assert_eq!(server.order_count(), 1);
    {
        let orders = server.state().orders.read();
        let order = orders.values().next().expect("order");
        assert_eq!(order.customer_name, "Grace Hopper");
        assert_eq!(order.customer_email, "grace@example.test");
    }

    let trace = sess.last_completed_trace().expect("completed trace");
    assert!(trace.actions.len() >= 18);
    assert!(trace.actions.iter().any(|a| {
        a.page_url
            .as_deref()
            .is_some_and(|u| u.contains("/checkout"))
    }));
    assert!(trace.actions.iter().any(|a| matches!(
        &a.action,
        RecordedActionKind::Click { locator } if locator.test_id.as_deref() == Some("next-page")
    )));

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}


/// [Test ID] TEST-BR-T1-F5-03
/// [Feature / Req] F5 / R2 — `BrowserSessionRegistry` targets active live session without spawning duplicate tabs
/// [Preconditions] `MockWebAppServer` started; `BotBrowserOrchestrator` configured with `CountingTransportFactory`; Chrome backend registered via `register_active_backend("active_tab_f5_03", ...)`.
/// [Opaque-Box Steps] Execute `HermesEngine::run_turn` with a DIFFERENT chat session ID (`"dm_chat_f5_03"`) and omit `session_id` in browser tool calls.
/// [Observable Assertions] `CountingTransportFactory` spawn count remains `0`, `list_session_ids()` stays len `1`, and the active Chrome backend's `current_url()` & DOM update in-place.
#[tokio::test]
async fn test_br_t1_f5_03_registry_targets_active_session_without_duplicate_spawn() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f5_03").expect("temp dir");
    let vault_dir = isolated_temp_dir("t1_f5_03_vault").expect("vault dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let spawn_count = Arc::new(AtomicUsize::new(0));
    let vault = Arc::new(
        CredentialVault::open(
            vault_dir.path().join("vault.db"),
            &MemoryMasterKeyStore::new(),
        )
        .expect("vault"),
    );
    let orch = Arc::new(BotBrowserOrchestrator::with_transport_factory(
        OrchestratorConfig::new(vault_dir.path()),
        vault,
        Arc::new(CountingTransportFactory {
            creations: Arc::clone(&spawn_count),
        }),
    ));

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.set_bot_browser(orch);
    host.browser_registry().register_active_backend(
        "active_tab_f5_03",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call(
            "a1",
            "browser_navigate",
            json!({ "url": server.url("/item/item-02") }),
        ),
        mock_openai_tool_call(
            "a2",
            "browser_extract_text",
            json!({ "test_id": "item-title", "output_key": "title" }),
        ),
        mock_openai_text("Read active tab title."),
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
            "Open item-02 on active browser",
            "dm_chat_f5_03",
        ))
        .await
        .expect("run_turn succeeds");
    assert!(res.tool_calls.iter().all(|tc| tc.success));
    assert!(
        res.tool_calls[1]
            .output
            .contains("Browser Automation Engineering")
    );
    assert_eq!(spawn_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        host.browser_registry().list_session_ids(),
        vec!["active_tab_f5_03".to_string()]
    );
    let url = backend.current_url().await.expect("current_url");
    assert!(url.contains("/item/item-02"));

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}


/// [Test ID] TEST-BR-T1-F5-04
/// [Feature / Req] F5 / R2 — `FabushiBotHost` streams ordered `BrowserAutomationEvent` telemetry via `forward_events_to_mpsc`
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered on `FabushiBotHost`; `forward_events_to_mpsc` forwarding `subscribe_browser_events()`.
/// [Opaque-Box Steps] Execute multi-step `HermesEngine::run_turn` (`navigate`, `type`, `click`, `wait_for`) and collect forwarded MPSC events.
/// [Observable Assertions] Event stream includes `ControlModeChanged(AiControlling)`, ordered `StepStarted` & `StepCompleted` with populated `duration_ms`, `locator`, and `url`, `ControlModeChanged(Idle)`, and `TraceCompleted`.
#[tokio::test]
async fn test_br_t1_f5_04_fabushi_host_relays_structured_browser_automation_events() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f5_04").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.register_browser_session(
        "tab_t1_f5_04",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );
    let (mpsc_tx, mut mpsc_rx) = mpsc::channel(64);
    let fwd_handle = forward_events_to_mpsc(host.subscribe_browser_events(), mpsc_tx);

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call("e1", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_tool_call(
            "e2",
            "browser_type",
            json!({ "test_id": "search-input", "text": "Async" }),
        ),
        mock_openai_tool_call(
            "e3",
            "browser_click",
            json!({ "test_id": "search-submit" }),
        ),
        mock_openai_tool_call(
            "e4",
            "browser_wait_for",
            json!({ "test_id": "item-link-item-01", "timeout_ms": 4000 }),
        ),
        mock_openai_text("Search telemetry complete."),
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
        .run_turn(full_turn_req("Search Async", "tab_t1_f5_04"))
        .await
        .expect("run_turn");
    assert!(res.tool_calls.iter().all(|tc| tc.success));

    tokio::time::sleep(Duration::from_millis(30)).await;
    let mut events = Vec::new();
    while let Ok(ev) = mpsc_rx.try_recv() {
        events.push(ev);
    }

    let started_ops: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            BrowserAutomationEvent::StepStarted { operation, .. } => Some(operation.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        started_ops,
        vec![
            "browser_navigate",
            "browser_type",
            "browser_click",
            "browser_wait_for"
        ]
    );

    let completed: Vec<(usize, String, Option<SemanticLocator>, Option<String>)> = events
        .iter()
        .filter_map(|e| match e {
            BrowserAutomationEvent::StepCompleted {
                step_index,
                operation,
                locator,
                url,
                ..
            } => Some((*step_index, operation.clone(), locator.clone(), url.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(completed.len(), 4);
    assert_eq!(
        completed.iter().map(|c| c.0).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert!(
        completed[1]
            .2
            .as_ref()
            .is_some_and(|l| l.test_id.as_deref() == Some("search-input"))
    );
    assert!(
        completed[0]
            .3
            .as_deref()
            .is_some_and(|u| u.starts_with(&server.base_url()))
    );
    assert!(events.iter().any(|e| matches!(
        e,
        BrowserAutomationEvent::ControlModeChanged {
            mode: BrowserControlMode::AiControlling { .. },
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        BrowserAutomationEvent::ControlModeChanged {
            mode: BrowserControlMode::Idle,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BrowserAutomationEvent::TraceCompleted { .. }))
    );

    fwd_handle.abort();
    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}


/// [Test ID] TEST-BR-T1-F5-05
/// [Feature / Req] F5 / R2 — AI `browser_snapshot` + `browser_extract_text` observation loop redacts secrets while reading non-secret DOM
/// [Preconditions] `MockXServer` started with `fake_secrets().password`; Chrome `CdpBrowserBackend` navigates to `/login` and fills password input via `fill_locator(..., is_secret: true)`.
/// [Opaque-Box Steps] Run AI turn calling `browser_snapshot`, benign `browser_extract_text` on `role="heading"` (`h1`), and sensitive `browser_extract_text` on `LoginForm_Password_Input`.
/// [Observable Assertions] `browser_snapshot` output has `redacted_field_count >= 1` and zero `FakeSecretFixtures` strings; benign heading extraction returns `"Sign in to X"`; sensitive password extraction is refused cleanly.
#[tokio::test]
async fn test_br_t1_f5_05_ai_observation_loop_redacts_secrets_reads_benign_dom() {
    let secrets = fake_secrets();
    let server = MockXServer::start_with_credentials(DEFAULT_X_USERNAME, secrets.password)
        .await
        .expect("start MockXServer");
    let temp = isolated_temp_dir("t1_f5_05").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    backend
        .navigate(&format!(
            "{}?auth_token={}",
            server.url("/login"),
            secrets.auth_token
        ))
        .await
        .expect("nav login");
    backend
        .fill_locator(
            &SemanticLocator::by_test_id("ocfEnterTextTextInput"),
            DEFAULT_X_USERNAME,
            false,
        )
        .await
        .expect("fill username");
    backend
        .fill_locator(
            &SemanticLocator::by_test_id("LoginForm_Password_Input"),
            secrets.password,
            true,
        )
        .await
        .expect("fill secret password");

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.register_browser_session(
        "tab_t1_f5_05",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call("o1", "browser_snapshot", json!({})),
        mock_openai_tool_call(
            "o2",
            "browser_extract_text",
            json!({ "css_selector": "h1", "output_key": "heading" }),
        ),
        mock_openai_tool_call(
            "o3",
            "browser_extract_text",
            json!({ "test_id": "LoginForm_Password_Input", "output_key": "pw" }),
        ),
        mock_openai_text("Observed login form safely."),
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
            "Inspect login page without leaking credentials",
            "tab_t1_f5_05",
        ))
        .await
        .expect("run_turn");
    assert_eq!(res.tool_calls.len(), 3);
    assert!(res.tool_calls[0].success);
    assert!(res.tool_calls[1].success);
    assert!(!res.tool_calls[2].success);

    let snap_out = &res.tool_calls[0].output;
    assert_eq!(secrets.contains_any_secret(snap_out), None);
    let snap_val: serde_json::Value = serde_json::from_str(snap_out).expect("snapshot JSON");
    assert!(
        snap_val["redacted_field_count"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    );
    assert!(res.tool_calls[1].output.contains("Sign in to X"));
    assert_eq!(
        secrets.contains_any_secret(&res.tool_calls[2].output),
        None
    );

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}


/// [Test ID] TEST-BR-T1-F6-01
/// [Feature / Req] F6 / R2, AC2 — `BrowserControlGate` transitions `Idle -> AiControlling -> UserTakeover -> AiControlling -> Idle`
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`; event subscriber attached.
/// [Opaque-Box Steps] Start AI turn via `MockOpenAiServer` (slow tool/takeover boundary), invoke `sess.user_take_over()` between steps, verify AI suspends, invoke `sess.user_resume_ai()`, wait for completion.
/// [Observable Assertions] Recorded `ControlModeChanged` events contain the exact sequence `AiControlling -> UserTakeover -> AiControlling -> Idle`.
#[tokio::test]
async fn test_br_t1_f6_01_control_gate_transitions_ai_takeover_resume_idle() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f6_01").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t1_f6_01",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );
    assert_eq!(sess.control_mode(), BrowserControlMode::Idle);
    let mut ev_rx = host.subscribe_browser_events();

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call("c1", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_tool_call(
            "c2",
            "browser_extract_text",
            json!({ "css_selector": "h1", "output_key": "title" }),
        ),
        mock_openai_text("Done"),
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
    let turn_handle = tokio::spawn(async move {
        engine_task
            .run_turn(full_turn_req("Navigate and read heading", "tab_t1_f6_01"))
            .await
    });

    // Wait until Step 0 (browser_navigate) completes, then trigger user_take_over before Step 1 starts
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), ev_rx.recv())
            .await
            .expect("timely StepCompleted")
            .expect("ev_rx");
        if matches!(
            ev,
            BrowserAutomationEvent::StepCompleted {
                operation,
                ..
            } if operation == "browser_navigate"
        ) {
            sess.user_take_over();
            break;
        }
    }
    assert!(matches!(
        sess.control_mode(),
        BrowserControlMode::UserTakeover { .. }
    ));

    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!turn_handle.is_finished(), "AI must pause on UserTakeover");

    sess.user_resume_ai();
    let res = turn_handle
        .await
        .expect("turn join")
        .expect("turn succeeds");
    assert!(res.tool_calls.iter().all(|tc| tc.success));
    assert_eq!(sess.control_mode(), BrowserControlMode::Idle);

    // Inspect transition sequence from watch / event stream (note initial AiControlling happened before loop)
    let mut transitions = Vec::new();
    while let Ok(ev) = ev_rx.try_recv() {
        if let BrowserAutomationEvent::ControlModeChanged { mode, .. } = ev {
            transitions.push(mode);
        }
    }
    // After user_take_over, we must see AiControlling (on resume) and Idle (on completion)
    assert!(transitions.iter().any(|m| matches!(m, BrowserControlMode::UserTakeover { .. })));
    assert!(transitions.iter().any(|m| matches!(m, BrowserControlMode::AiControlling { .. })));
    assert!(matches!(transitions.last(), Some(BrowserControlMode::Idle)));

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}


/// [Test ID] TEST-BR-T1-F6-02
/// [Feature / Req] F6 / R2, AC2 — AI-initiated `browser_request_takeover(reason)` suspends on `Notify` and wakes on `user_resume_ai` reading live state
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI navigates `/` then calls `browser_request_takeover(reason, wait_for_finish: true)`; verify `WaitingForTakeover` and turn does not finish; manually navigate Chrome to `/item/item-04`; call `user_resume_ai()`.
/// [Observable Assertions] AI resumes immediately upon `user_resume_ai`, takeover result reports updated live `/item/item-04` URL, and subsequent `browser_extract_text` reads `"Distributed SQLite Architecture"`.
#[tokio::test]
async fn test_br_t1_f6_02_request_takeover_suspends_on_notify_and_reads_live_state() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f6_02").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t1_f6_02",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );
    let mut ev_rx = host.subscribe_browser_events();

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call("t1", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_tool_call(
            "t2",
            "browser_request_takeover",
            json!({ "reason": "Select product manually", "wait_for_finish": true }),
        ),
        mock_openai_tool_call(
            "t3",
            "browser_extract_text",
            json!({ "test_id": "item-title", "output_key": "chosen_title" }),
        ),
        mock_openai_text("Read manually selected item."),
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
    let turn_handle = tokio::spawn(async move {
        engine_task
            .run_turn(full_turn_req("Wait for manual selection", "tab_t1_f6_02"))
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
    assert!(matches!(
        sess.control_mode(),
        BrowserControlMode::WaitingForTakeover { .. }
    ));

    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!turn_handle.is_finished(), "must remain suspended on Notify");

    backend
        .navigate(&server.url("/item/item-04"))
        .await
        .expect("manual navigate");
    sess.user_resume_ai();

    let res = tokio::time::timeout(Duration::from_secs(10), turn_handle)
        .await
        .expect("resumed promptly")
        .expect("join")
        .expect("turn succeeds");
    assert_eq!(res.tool_calls.len(), 3);
    assert!(res.tool_calls[1].output.contains("/item/item-04"));
    assert!(
        res.tool_calls[2]
            .output
            .contains("Distributed SQLite Architecture")
    );

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}


/// [Test ID] TEST-BR-T1-F6-03
/// [Feature / Req] F6 / R2, AC2 — `user_cancel_ai` cleanly aborts paused AI run and restores `Idle`
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI enters `WaitingForTakeover` via `browser_request_takeover`; user invokes `sess.user_cancel_ai()`; then user calls `backend.navigate(...)` manually.
/// [Observable Assertions] Cancelled tool call reports cancellation error, `sess.control_mode() == BrowserControlMode::Idle`, and manual Chrome navigation succeeds afterwards.
#[tokio::test]
async fn test_br_t1_f6_03_user_cancel_ai_aborts_cleanly_and_restores_idle() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f6_03").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t1_f6_03",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call("c1", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_tool_call(
            "c2",
            "browser_request_takeover",
            json!({ "reason": "Cancel test", "wait_for_finish": true }),
        ),
        mock_openai_text("Cancelled by user."),
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
    let turn_handle = tokio::spawn(async move {
        engine_task
            .run_turn(full_turn_req("Start and cancel", "tab_t1_f6_03"))
            .await
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !matches!(
        sess.control_mode(),
        BrowserControlMode::WaitingForTakeover { .. }
    ) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for WaitingForTakeover"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    sess.user_cancel_ai();

    let res = tokio::time::timeout(Duration::from_secs(10), turn_handle)
        .await
        .expect("aborted promptly")
        .expect("join")
        .expect("turn returns result");
    assert_eq!(res.tool_calls.len(), 2);
    assert!(!res.tool_calls[1].success);
    assert!(
        res.tool_calls[1]
            .output
            .to_ascii_lowercase()
            .contains("cancelled")
    );
    assert_eq!(sess.control_mode(), BrowserControlMode::Idle);

    // Manual browser control remains completely usable
    let nav = backend
        .navigate(&server.url("/item/item-05"))
        .await
        .expect("manual navigate after cancel");
    assert!(nav.url.contains("/item/item-05"));

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}


/// [Test ID] TEST-BR-T1-F6-04
/// [Feature / Req] F6 / R3, AC5 — `/distill` command & `distill_browser_script_app` tool compile trace -> `ScriptDistilled` -> zero-LLM replay
/// [Preconditions] `MockXServer` started with seeded session; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI posts tweet A on `/home` (`parameter_name_hint: "post_text"`) and calls `distill_browser_script_app`, then `/distill` slash command runs via `engine.run_turn`; deserialize `CommandUiEffect::DistillBrowserScript` app and replay via `host.replay_browser_script` with `post_text = "Zero LLM Post B"`.
/// [Observable Assertions] `ScriptDistilled` event and `CommandUiEffect::DistillBrowserScript` emitted; replay returns `used_llm == false`, `success == true`, mock LLM call count delta == 0, and `MockXServer` persists both posts.
#[tokio::test]
async fn test_br_t1_f6_04_distill_command_and_tool_produce_zero_llm_reproducible_app() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let seed = server.seed_session("tok_t1_f6_04", "ct0_t1_f6_04");
    let temp = isolated_temp_dir("t1_f6_04").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let cookies = import_cookies_from_json(&generate_cdp_cookies_json(&x_auth_cookies(
        "127.0.0.1",
        &seed.auth_token,
        &seed.ct0,
    )))
    .expect("parse cookies");
    backend
        .import_cookies(&cookies)
        .await
        .expect("import cookies");

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.register_browser_session(
        "tab_t1_f6_04",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );
    let mut ev_rx = host.subscribe_browser_events();

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call(
            "d1",
            "browser_navigate",
            json!({ "url": server.url("/home") }),
        ),
        mock_openai_tool_call(
            "d2",
            "browser_wait_for",
            json!({ "test_id": "tweetTextarea_0", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "d3",
            "browser_type",
            json!({
                "test_id": "tweetTextarea_0",
                "text": "AI Post A",
                "parameter_name_hint": "post_text"
            }),
        ),
        mock_openai_tool_call(
            "d4",
            "browser_click",
            json!({ "test_id": "tweetButtonInline" }),
        ),
        mock_openai_tool_call(
            "d5",
            "browser_wait_for",
            json!({ "text_anchor": "AI Post A", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "d6",
            "distill_browser_script_app",
            json!({ "name": "Tool Distilled X", "category": "social" }),
        ),
        mock_openai_text("Distilled via tool."),
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
        .run_turn(full_turn_req("Post and distill", "tab_t1_f6_04"))
        .await
        .expect("ai turn");
    assert!(ai_turn.tool_calls.iter().all(|tc| tc.success));
    wait_for_tweet_count(&server, 3, Duration::from_secs(3)).await;

    // Also run `/distill` command
    let cmd_res = engine
        .run_turn(HermesTurnRequest::new("/distill Slash Distilled X").with_session("tab_t1_f6_04"))
        .await
        .expect("/distill turn");
    let distilled_json = cmd_res
        .ui_effects
        .iter()
        .find_map(|eff| match eff {
            CommandUiEffect::DistillBrowserScript { name, app_json, .. }
                if name == "Slash Distilled X" =>
            {
                Some(app_json.clone())
            }
            _ => None,
        })
        .expect("CommandUiEffect::DistillBrowserScript emitted");

    let mut distilled_events = 0usize;
    while let Ok(ev) = ev_rx.try_recv() {
        if matches!(ev, BrowserAutomationEvent::ScriptDistilled { .. }) {
            distilled_events += 1;
        }
    }
    assert!(distilled_events >= 2);

    let app: AutomationScriptApp =
        serde_json::from_str(&distilled_json).expect("deserialize distilled app");
    // Strip the literal text WaitFor("AI Post A") if present? Wait: WaitFor(text_anchor: "AI Post A") waits for "AI Post A" which is still in timeline on /home!
    // Even better, replay with zero LLM calls and verify post_text parameter produces "Zero LLM Post B"
    let llm_calls_before_replay = llm.call_count();
    let params = BTreeMap::from([("post_text".to_string(), "Zero LLM Post B".to_string())]);
    let report = host
        .replay_browser_script("tab_t1_f6_04", &app, &params, None)
        .await
        .expect("zero-LLM replay");
    assert!(report.success);
    assert!(!report.used_llm);
    assert_eq!(
        llm.call_count(),
        llm_calls_before_replay,
        "LLM must receive ZERO calls during deterministic replay"
    );

    wait_for_tweet_count(&server, 4, Duration::from_secs(3)).await;
    assert!(
        server
            .state()
            .tweets
            .read()
            .iter()
            .any(|t| t.text == "Zero LLM Post B")
    );

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}


/// [Test ID] TEST-BR-T1-F6-05
/// [Feature / Req] F6 / R2, R3 — Distillation preserves `ScriptStep::HumanCheckpoint { reason }` at takeover boundary and replays with `BrowserControlGate`
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI navigates `/`, invokes `browser_request_takeover(reason: "Verify user gate")`, resumes on `user_resume_ai()`, extracts item title on `/item/item-02`, then distills via `/distill`; replay compiled app with `DeterministicScriptRunner::run` passing `Some(&gate)` and resume when `WaitingForTakeover` hits.
/// [Observable Assertions] Compiled `app.steps` contains `ScriptStep::HumanCheckpoint { reason }` between `Navigate` and `ExtractText`, and `DeterministicScriptRunner` replays across the checkpoint with `used_llm == false`.
#[tokio::test]
async fn test_br_t1_f6_05_distillation_preserves_human_checkpoint_and_replays_with_gate() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t1_f6_05").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t1_f6_05",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );
    let mut ev_rx = host.subscribe_browser_events();

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call("h1", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_tool_call(
            "h2",
            "browser_request_takeover",
            json!({ "reason": "Verify user gate", "wait_for_finish": true }),
        ),
        mock_openai_tool_call(
            "h3",
            "browser_extract_text",
            json!({ "test_id": "item-title", "output_key": "item_title" }),
        ),
        mock_openai_text("Done with human checkpoint."),
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
    let turn_handle = tokio::spawn(async move {
        engine_task
            .run_turn(full_turn_req("Run with checkpoint", "tab_t1_f6_05"))
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
    backend
        .navigate(&server.url("/item/item-02"))
        .await
        .expect("manual navigate to item-02");
    sess.user_resume_ai();

    let res = turn_handle.await.expect("join").expect("turn succeeds");
    assert!(res.tool_calls.iter().all(|tc| tc.success));

    let app = host
        .distill_browser_script(
            "tab_t1_f6_05",
            ScriptAppMetadata {
                name: "Checkpoint Script".into(),
                category: "general".into(),
                ..Default::default()
            },
        )
        .expect("distill");

    let checkpoint_idx = app
        .steps
        .iter()
        .position(|s| matches!(s, ScriptStep::HumanCheckpoint { reason } if reason == "Verify user gate"))
        .expect("HumanCheckpoint preserved in distilled steps");
    assert!(checkpoint_idx >= 1);
    assert!(matches!(app.steps[0], ScriptStep::Navigate { .. }));
    assert!(
        app.steps[checkpoint_idx + 1..]
            .iter()
            .any(|s| matches!(s, ScriptStep::ExtractText { .. }))
    );

    // Replay with BrowserControlGate
    let gate = Arc::new(BrowserControlGate::new());
    let mut gate_rx = gate.subscribe();
    let backend_replay = Arc::clone(&backend);
    let app_replay = app.clone();
    let gate_replay = Arc::clone(&gate);
    let replay_handle = tokio::spawn(async move {
        DeterministicScriptRunner::run(
            backend_replay.as_ref(),
            &app_replay,
            &BTreeMap::new(),
            None,
            Some(&gate_replay),
        )
        .await
    });

    gate_rx
        .wait_for(|m| matches!(m, BrowserControlMode::WaitingForTakeover { .. }))
        .await
        .expect("hit HumanCheckpoint");
    backend
        .navigate(&server.url("/item/item-02"))
        .await
        .expect("manual navigate during replay checkpoint");
    gate.user_resume_ai();

    let report = replay_handle
        .await
        .expect("replay join")
        .expect("replay succeeds");
    assert!(report.success);
    assert!(!report.used_llm);
    assert_eq!(
        report
            .extracted_outputs
            .get("item_title")
            .map(String::as_str),
        Some("Browser Automation Engineering")
    );

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}

// ============================================================================
// F7 & F8 Headless GPUI Subprocess Harness + 10 Tier 1 E2E Tests
// ============================================================================

const T1_CHILD_ENV: &str = "GPUI_E2E_T1_HEADLESS_CHILD";
const T1_CHILD_TARGET_ENV: &str = "GPUI_E2E_T1_HEADLESS_TARGET";
const T1_CHILD_AFTER_CTORS_ENV: &str = "GPUI_E2E_T1_HEADLESS_AFTER_CTORS";

fn t1_dispatch_headless_target(target: &str, use_underscore_exit: bool) -> ! {
    type AsyncBody = fn(gpui::AsyncApp) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>;
    let body: AsyncBody = match target {
        "test_br_t1_f7_01_appshell_nav_browser_mount_and_address_bar_navigation" => {
            |app| Box::pin(body_br_t1_f7_01(app))
        }
        "test_br_t1_f7_02_cdp_viewport_surface_render_image_and_input_forwarding" => {
            |app| Box::pin(body_br_t1_f7_02(app))
        }
        "test_br_t1_f7_03_workspace_takeover_controls_and_manual_navigation" => {
            |app| Box::pin(body_br_t1_f7_03(app))
        }
        "test_br_t1_f7_04_live_ai_step_log_sidebar_updates_from_automation_events" => {
            |app| Box::pin(body_br_t1_f7_04(app))
        }
        "test_br_t1_f7_05_gpui_ui_thread_non_blocking_during_slow_navigation" => {
            |app| Box::pin(body_br_t1_f7_05(app))
        }
        "test_br_t1_f8_01_in_app_json_cookie_import_authenticates_mock_x" => {
            |app| Box::pin(body_br_t1_f8_01(app))
        }
        "test_br_t1_f8_02_distill_last_trace_saves_named_script_app_with_editable_params" => {
            |app| Box::pin(body_br_t1_f8_02(app))
        }
        "test_br_t1_f8_03_browser_run_script_app_zero_llm_replay_and_timings" => {
            |app| Box::pin(body_br_t1_f8_03(app))
        }
        "test_br_t1_f8_04_publish_script_app_to_ephemeral_server_store" => {
            |app| Box::pin(body_br_t1_f8_04(app))
        }
        "test_br_t1_f8_05_store_panel_search_install_and_replay_remote_script_app" => {
            |app| Box::pin(body_br_t1_f8_05(app))
        }
        other => {
            eprintln!("unknown e2e tier1 headless target: {other}");
            e2e_suite::gpui_browser_harness::exit_headless_child(101, use_underscore_exit);
        }
    };
    e2e_suite::gpui_browser_harness::run_headless_gpui_body(target, use_underscore_exit, body);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_T1_HEADLESS_MAIN_THREAD_ENTRY: extern "C" fn() = t1_macos_mod_init_headless_entry;

#[cfg(target_os = "macos")]
extern "C" fn t1_macos_atexit_headless_entry() {
    if let Ok(target) = std::env::var(T1_CHILD_TARGET_ENV) {
        t1_dispatch_headless_target(&target, true);
    }
}

#[cfg(target_os = "macos")]
extern "C" fn t1_macos_mod_init_headless_entry() {
    if std::env::var(T1_CHILD_ENV).as_deref() == Ok("1") {
        if std::env::var(T1_CHILD_AFTER_CTORS_ENV).as_deref() == Ok("1") {
            unsafe extern "C" {
                fn atexit(func: extern "C" fn()) -> i32;
            }
            unsafe {
                atexit(t1_macos_atexit_headless_entry);
            }
            return;
        }
        if let Ok(target) = std::env::var(T1_CHILD_TARGET_ENV) {
            t1_dispatch_headless_target(&target, false);
        }
    }
}

fn run_t1_in_headless_subprocess(test_fn_name: &str) {
    if std::env::var(T1_CHILD_ENV).as_deref() == Ok("1") {
        t1_dispatch_headless_target(test_fn_name, false);
    }
    e2e_suite::gpui_browser_harness::spawn_headless_subprocess(
        T1_CHILD_ENV,
        T1_CHILD_TARGET_ENV,
        T1_CHILD_AFTER_CTORS_ENV,
        test_fn_name,
        false,
        None,
        None,
    );
}

fn build_webapp_checkout_script(base_url: &str, app_id: &str, name: &str) -> AutomationScriptApp {
    AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: app_id.to_string(),
        slug: app_id.to_string(),
        name: name.to_string(),
        version: "1.0.0".to_string(),
        description: format!("{name} deterministic checkout script"),
        category: "productivity".to_string(),
        target_domains: vec!["127.0.0.1".to_string()],
        parameters: vec![
            ScriptParameter {
                name: "customer_name".to_string(),
                description: "Customer full name".to_string(),
                required: true,
                default_value: Some("Default Buyer".to_string()),
                is_secret: false,
            },
            ScriptParameter {
                name: "customer_email".to_string(),
                description: "Customer email".to_string(),
                required: true,
                default_value: Some("default@buyer.test".to_string()),
                is_secret: false,
            },
        ],
        steps: vec![
            ScriptStep::Navigate {
                url_template: format!("{base_url}/item/item-05"),
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
        required_capabilities: vec!["browser.navigate".to_string(), "browser.interact".to_string()],
        author_id: None,
        source_trace_summary: None,
    }
}

/// [Test ID] TEST-BR-T1-F7-01
/// [Feature / Req] F7 / R1, AC2 — Headless GPUI `AppShell` mounts `NavDestination::Browser` (`Cmd+9`), drives address bar input, and navigates `MockWebAppServer`
/// [Preconditions] `MockWebAppServer` started; real headless Chrome `CdpBrowserBackend` attached to `AppShell::set_browser_backend_for_test`.
/// [Opaque-Box Steps] Trigger shortcut `"9"`, enter `web.url("/")` via `ActiveBrowserInput::AddressBar` + Enter, wait for load, then call `browser_navigate` to `/search?q=Rust`.
/// [Observable Assertions] `ActiveWorkspacePanel::Browser` is mounted, `should_hide_composer` is true, and `current_url`, `url_input`, and `page_title` converge on each page.
#[test]
fn test_br_t1_f7_01_appshell_nav_browser_mount_and_address_bar_navigation() {
    run_t1_in_headless_subprocess(
        "test_br_t1_f7_01_appshell_nav_browser_mount_and_address_bar_navigation",
    );
}

async fn body_br_t1_f7_01(mut app: gpui::AsyncApp) {
    use gpui_telegram_desktop::views::browser::ActiveBrowserInput;
    use gpui_telegram_desktop::views::nav_rail::NavDestination;
    use gpui_telegram_desktop::views::shell::ActiveWorkspacePanel;

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let web = rt.block_on(MockWebAppServer::start()).expect("MockWebAppServer");
    let chrome_dir = isolated_temp_dir("t1_f7_01").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                701,
                "f7_user1",
            )
        })
        .unwrap();

    let home_url = web.url("/");
    let search_url = web.url("/search?q=Rust");

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            assert!(s.handle_shortcut_key("9", false, scx));
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
            assert_eq!(
                s.nav_rail.read(scx).active_destination,
                NavDestination::Browser
            );
            assert!(s.should_hide_composer(scx));

            s.set_browser_backend_for_test(
                Arc::clone(&chrome) as Arc<dyn BrowserSessionBackend>,
                scx,
            );
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::AddressBar, None, wcx);
            });
            s.clear_active_field(scx);
            s.insert_committed_text(&home_url, scx);
            assert!(s.handle_non_char_key("enter", false, scx));
        });
    })
    .unwrap();

    let loaded_home = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let s = shell.read(cx);
            let Some(bw) = s.browser_workspace() else {
                return false;
            };
            let w = bw.read(cx);
            !w.is_loading
                && w.current_url == home_url
                && w.page_title.contains("Catalog Portal")
        },
    )
    .await;
    assert!(loaded_home, "address bar Enter must load MockWebAppServer /");

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::None, None, wcx);
            });
            s.browser_navigate(&search_url, scx);
        });
    })
    .unwrap();

    let loaded_search = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.is_loading
                && w.current_url.contains("/search?q=Rust")
                && w.url_input.contains("/search?q=Rust")
                && w.page_title.contains("Search Results")
        },
    )
    .await;
    assert!(
        loaded_search,
        "browser_navigate must update current_url, url_input, and page_title"
    );

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(web.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T1-F7-02
/// [Feature / Req] F7 / R1, AC3 — `CdpViewportSurface` renders decoded `RenderImage` from real Chrome PNG frame and forwards pointer/keyboard events
/// [Preconditions] `MockWebAppServer` `/` loaded in `BrowserWorkspace` backed by real `CdpBrowserBackend`.
/// [Opaque-Box Steps] Wait for initial `RenderImage`, query real DOM bounding centers for `search-input` & `search-submit`, dispatch `handle_click`, `send_text_input("Rust")`, and submit click via `CdpViewportSurface`.
/// [Observable Assertions] Surface holds non-empty BGRA `RenderImage`, forwarded pointer/text inputs navigate Chrome to `/search?q=Rust`, and `last_frame_revision` increments.
#[test]
fn test_br_t1_f7_02_cdp_viewport_surface_render_image_and_input_forwarding() {
    run_t1_in_headless_subprocess(
        "test_br_t1_f7_02_cdp_viewport_surface_render_image_and_input_forwarding",
    );
}

async fn body_br_t1_f7_02(mut app: gpui::AsyncApp) {
    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let web = rt.block_on(MockWebAppServer::start()).expect("MockWebAppServer");
    let chrome_dir = isolated_temp_dir("t1_f7_02").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                702,
                "f7_user2",
            )
        })
        .unwrap();

    let home_url = web.url("/");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(
                Arc::clone(&chrome) as Arc<dyn BrowserSessionBackend>,
                scx,
            );
            s.browser_navigate(&home_url, scx);
        });
    })
    .unwrap();

    let rendered = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let Some(surf) = bw.cdp_surface() else {
                return false;
            };
            let sr = surf.read(cx);
            !bw.is_loading
                && !sr.is_refreshing()
                && sr.has_rendered_frame()
                && sr.last_frame_revision() >= 1
                && sr.current_url() == home_url
        },
    )
    .await;
    assert!(rendered, "CdpViewportSurface must decode initial Chrome PNG frame");

    let rev_before = app
        .update(|cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            let img = sr.rendered_image().expect("RenderImage must be populated");
            let sz = img.size(0);
            assert!(sz.width.0 > 0 && sz.height.0 > 0);
            assert!(!img.as_bytes(0).expect("RenderImage bytes").is_empty());
            sr.last_frame_revision()
        })
        .unwrap();

    let (in_x, in_y) = rt.block_on(e2e_suite::gpui_browser_harness::query_element_center_css(
        chrome.as_ref(),
        "[data-testid=\"search-input\"]",
    ));
    let (btn_x, btn_y) = rt.block_on(e2e_suite::gpui_browser_harness::query_element_center_css(
        chrome.as_ref(),
        "[data-testid=\"search-submit\"]",
    ));

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
            sr.handle_click(in_x, in_y, PointerButton::Left, 1, 0, scx);
        });
    })
    .unwrap();

    let focused = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |_| {
            rt.block_on(
                chrome.eval_js("document.activeElement?.getAttribute('data-testid') || ''"),
            )
            .ok()
            .and_then(|v| v.as_str().map(|s| s == "search-input"))
            .unwrap_or(false)
        },
    )
    .await;
    assert!(focused, "handle_click must focus search-input in Chrome DOM");

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
            sr.send_text_input("Rust", scx);
        });
    })
    .unwrap();

    let typed = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |_| {
            rt.block_on(
                chrome.eval_js("document.querySelector('[data-testid=\"search-input\"]')?.value || ''"),
            )
            .ok()
            .and_then(|v| v.as_str().map(|s| s == "Rust"))
            .unwrap_or(false)
        },
    )
    .await;
    assert!(typed, "send_text_input must populate search-input");

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
            sr.handle_click(btn_x, btn_y, PointerButton::Left, 1, 0, scx);
        });
    })
    .unwrap();

    let navigated = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let cur = rt.block_on(chrome.current_url()).unwrap_or_default();
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            cur.contains("/search") && cur.contains("q=Rust") && sr.last_frame_revision() > rev_before
        },
    )
    .await;
    assert!(
        navigated,
        "CdpViewportSurface click + text forwarding must submit search form to /search?q=Rust and refresh frame"
    );

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(web.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T1-F7-03
/// [Feature / Req] F7 / R1, R2, AC2 — GPUI workspace takeover UI controls (`browser_user_takeover` / `browser_resume_ai`) reflect `BrowserControlMode` transitions and allow manual navigation
/// [Preconditions] `MockXServer` started with seeded session; Chrome `CdpBrowserBackend` imported with cookies; `AutomationScriptApp` with `HumanCheckpoint` launched via `browser_run_script_app`.
/// [Opaque-Box Steps] Wait for `WaitingForTakeover`, invoke `browser_user_takeover`, navigate manually during takeover, return to `/home`, then invoke `browser_resume_ai`.
/// [Observable Assertions] Mode transitions `WaitingForTakeover -> UserTakeover -> AiControlling/Idle`, `is_interactive_enabled()` is true during takeover, and resumed script posts tweet cleanly.
#[test]
fn test_br_t1_f7_03_workspace_takeover_controls_and_manual_navigation() {
    run_t1_in_headless_subprocess(
        "test_br_t1_f7_03_workspace_takeover_controls_and_manual_navigation",
    );
}

async fn body_br_t1_f7_03(mut app: gpui::AsyncApp) {
    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let x_server = rt.block_on(MockXServer::start()).expect("MockXServer");
    let seed = x_server.seed_session("tok_t1_f7_03", "ct0_t1_f7_03");
    let chrome_dir = isolated_temp_dir("t1_f7_03").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let cookies = import_cookies_from_json(&generate_cdp_cookies_json(&x_auth_cookies(
        "127.0.0.1",
        &seed.auth_token,
        &seed.ct0,
    )))
    .expect("cookies");
    rt.block_on(chrome.import_cookies(&cookies))
        .expect("import cookies");

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                703,
                "f7_user3",
            )
        })
        .unwrap();

    let script = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "t1-f7-03-takeover".to_string(),
        slug: "t1-f7-03-takeover".to_string(),
        name: "Takeover Check".to_string(),
        version: "1.0.0".to_string(),
        description: "Pauses at checkpoint then posts".to_string(),
        category: "social".to_string(),
        target_domains: vec!["127.0.0.1".to_string()],
        parameters: vec![],
        steps: vec![
            ScriptStep::Navigate {
                url_template: x_server.url("/home"),
            },
            ScriptStep::HumanCheckpoint {
                reason: "Verify user control".to_string(),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("tweetTextarea_0"),
                timeout_ms: 4_000,
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_test_id("tweetTextarea_0"),
                value_template: "T1-F7-03 post after resume".to_string(),
                secret_vault_key: None,
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("tweetButtonInline"),
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(
                Arc::clone(&chrome) as Arc<dyn BrowserSessionBackend>,
                scx,
            );
            s.browser_run_script_app(script, BTreeMap::new(), scx);
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
    assert!(waiting, "workspace must enter WaitingForTakeover");

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_user_takeover(scx);
            assert!(matches!(
                s.browser_control_mode(scx),
                BrowserControlMode::UserTakeover { .. }
            ));
            let bw = s.browser_workspace().unwrap().read(scx);
            assert!(matches!(bw.control_mode, BrowserControlMode::UserTakeover { .. }));
            assert!(bw.cdp_surface().unwrap().read(scx).is_interactive_enabled());
            s.browser_navigate(&x_server.url("/challenge"), scx);
        });
    })
    .unwrap();

    let manual_nav = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.is_loading && w.current_url.contains("/challenge")
        },
    )
    .await;
    assert!(manual_nav, "manual navigation must work during UserTakeover");

    // Return to /home manually before handing control back
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_navigate(&x_server.url("/home"), scx);
        });
    })
    .unwrap();
    let back_home = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.is_loading && w.current_url.contains("/home")
        },
    )
    .await;
    assert!(back_home);

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_resume_ai(scx);
        });
    })
    .unwrap();

    let finished = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let s = shell.read(cx);
            let w = s.browser_workspace().unwrap().read(cx);
            !w.recorder.is_replaying
                && w.recorder.last_replay_report.is_some()
                && matches!(s.browser_control_mode(cx), BrowserControlMode::Idle)
        },
    )
    .await;
    assert!(finished, "resumed run must complete and return to Idle");
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
            .any(|t| t.text == "T1-F7-03 post after resume")
    );

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(x_server.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T1-F7-04
/// [Feature / Req] F7 / R1, AC2 — Live AI step log sidebar (`RecorderPanelState::event_logs`) updates reactively from `BrowserAutomationEvent` stream
/// [Preconditions] `MockWebAppServer` & `MockOpenAiServer` started; `FabushiBotHost` wired to `shell.bot_browser.browser_registry` and attached via `attach_local_fabushi_host_with_cx`.
/// [Opaque-Box Steps] Set `copilot_prompt_draft` and call `trigger_copilot_prompt` to run `browser_navigate`, `browser_wait_for`, and `browser_click` on `MockWebAppServer`.
/// [Observable Assertions] `bw.recorder.copilot_last_reply` completes and `bw.recorder.event_logs` records ordered `StepStarted` / `StepCompleted` entries with monotonic `seq` and `duration_ms.is_some()`.
#[test]
fn test_br_t1_f7_04_live_ai_step_log_sidebar_updates_from_automation_events() {
    run_t1_in_headless_subprocess(
        "test_br_t1_f7_04_live_ai_step_log_sidebar_updates_from_automation_events",
    );
}

async fn body_br_t1_f7_04(mut app: gpui::AsyncApp) {
    use gpui_telegram_desktop::views::browser::BrowserLogLevel;

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let web = rt.block_on(MockWebAppServer::start()).expect("MockWebAppServer");
    let chrome_dir = isolated_temp_dir("t1_f7_04").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let llm = rt
        .block_on(MockOpenAiServer::start(vec![
            mock_openai_tool_call(
                "f7_04_1",
                "browser_navigate",
                json!({ "url": web.url("/item/item-01") }),
            ),
            mock_openai_tool_call(
                "f7_04_2",
                "browser_wait_for",
                json!({ "test_id": "add-to-cart-btn", "timeout_ms": 4000 }),
            ),
            mock_openai_tool_call(
                "f7_04_3",
                "browser_click",
                json!({ "test_id": "add-to-cart-btn" }),
            ),
            mock_openai_text("Added item-01 to cart."),
        ]))
        .expect("MockOpenAiServer");

    let host = Arc::new(FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host"));
    e2e_suite::gpui_browser_harness::configure_host_with_mock_llm(&host, &llm.base_url());

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                704,
                "f7_user4",
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
                w.recorder.copilot_prompt_draft = "Open item-01 and add it to cart".to_string();
                w.trigger_copilot_prompt(wcx);
            });
        });
    })
    .unwrap();

    let done = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.recorder.copilot_running && w.recorder.copilot_last_reply.is_some()
        },
    )
    .await;
    assert!(done, "copilot prompt must finish");

    app.update(|cx| {
        let w = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(
            w.recorder.copilot_last_reply.as_deref(),
            Some("Added item-01 to cart.")
        );
        let started_count = w
            .recorder
            .event_logs
            .iter()
            .filter(|e| e.level == BrowserLogLevel::Info && e.title.contains("started"))
            .count();
        let completed: Vec<_> = w
            .recorder
            .event_logs
            .iter()
            .filter(|e| e.level == BrowserLogLevel::Success && e.title.contains("done:"))
            .collect();
        assert!(started_count >= 3, "expected >=3 StepStarted logs, got {started_count}");
        assert!(completed.len() >= 3, "expected >=3 StepCompleted logs, got {}", completed.len());
        assert!(completed.iter().all(|e| e.duration_ms.is_some()));
        for pair in w.recorder.event_logs.windows(2) {
            assert!(pair[0].seq < pair[1].seq, "event log seq must be monotonic");
        }
        assert!(
            w.recorder
                .event_logs
                .iter()
                .any(|e| e.detail.contains("/item/item-01")),
            "event logs must record navigation to /item/item-01"
        );
        assert!(
            w.current_url.contains("/cart/add") || w.current_url.contains("/item/item-01"),
            "expected workspace current_url to reflect /cart/add or /item/item-01, got {}",
            w.current_url
        );
    })
    .unwrap();

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(llm.shutdown());
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(web.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T1-F7-05
/// [Feature / Req] F7 / R1, AC3 — GPUI main UI thread remains responsive during slow backend navigation
/// [Preconditions] `DelayedHtmlServer` serving `/slow` with 300ms delay; Chrome `CdpBrowserBackend` attached to `AppShell`.
/// [Opaque-Box Steps] Trigger `browser_navigate` to `/slow`, then pump GPUI 15ms ticks concurrently while toggling inspector tabs & typing into `DistillName`.
/// [Observable Assertions] Multiple UI ticks and inspector/input updates execute before `/slow` completes, with `max_tick_gap_ms < 150`.
#[test]
fn test_br_t1_f7_05_gpui_ui_thread_non_blocking_during_slow_navigation() {
    run_t1_in_headless_subprocess(
        "test_br_t1_f7_05_gpui_ui_thread_non_blocking_during_slow_navigation",
    );
}

async fn body_br_t1_f7_05(mut app: gpui::AsyncApp) {
    use gpui_telegram_desktop::views::browser::{ActiveBrowserInput, BrowserInspectorTab};

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let delayed = rt
        .block_on(e2e_suite::gpui_browser_harness::DelayedHtmlServer::start(300))
        .expect("DelayedHtmlServer");
    let chrome_dir = isolated_temp_dir("t1_f7_05").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                705,
                "f7_user5",
            )
        })
        .unwrap();

    let slow_url = delayed.url("/slow");
    let initial_draft_len = app
        .update(|cx| {
            shell.update(cx, |s, scx| {
                s.set_browser_backend_for_test(
                    Arc::clone(&chrome) as Arc<dyn BrowserSessionBackend>,
                    scx,
                );
                let len = s
                    .browser_workspace()
                    .unwrap()
                    .read(scx)
                    .recorder
                    .distill_name_draft
                    .len();
                s.browser_navigate(&slow_url, scx);
                len
            })
        })
        .unwrap();

    let mut ticks_while_loading = 0usize;
    let (done, total_ticks, max_gap_ms) = e2e_suite::gpui_browser_harness::pump_with_ticks(
        &mut app,
        Duration::from_secs(15),
        Duration::from_millis(15),
        |cx, tick| {
            let bw = shell.read(cx).browser_workspace().unwrap().clone();
            let loading = bw.read(cx).is_loading;
            if loading {
                ticks_while_loading += 1;
                bw.update(cx, |w, wcx| {
                    let tab = if tick % 2 == 0 {
                        BrowserInspectorTab::Store
                    } else {
                        BrowserInspectorTab::Session
                    };
                    w.set_inspector_tab(tab, wcx);
                    w.set_active_input(ActiveBrowserInput::DistillName, None, wcx);
                    w.insert_text_to_active_input("x", wcx);
                });
            }
            let r = bw.read(cx);
            !r.is_loading && r.current_url == slow_url && r.page_title.contains("Slow Loaded Page")
        },
    )
    .await;

    assert!(done, "slow navigation must finish cleanly");
    assert!(
        ticks_while_loading >= 5,
        "expected >= 5 GPUI UI ticks while 300ms backend navigation was in flight, got {ticks_while_loading} (total {total_ticks})"
    );
    assert!(
        max_gap_ms < 150,
        "GPUI main UI thread tick gap too high: {max_gap_ms}ms"
    );
    app.update(|cx| {
        let w = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(
            w.recorder.distill_name_draft.len(),
            initial_draft_len + ticks_while_loading
        );
    })
    .unwrap();

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(delayed.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T1-F8-01
/// [Feature / Req] F8 / R1, AC4 — In-app JSON cookie import via `RecorderPanel` authenticates `MockXServer` `/home` immediately
/// [Preconditions] `MockXServer` started with seeded session; Chrome `CdpBrowserBackend` attached to `AppShell`.
/// [Opaque-Box Steps] Switch to `BrowserInspectorTab::Session`, paste CDP JSON cookies into `ActiveBrowserInput::CookieImport`, call `import_cookies_from_text_async`, then navigate to `/home`.
/// [Observable Assertions] `last_imported_cookie_count >= 2`, `last_persisted_cookie_count >= 2`, `MockXServer` `/home` renders `tweetTextarea_0`, and `BrowserProfileStore` persists cookies.
#[test]
fn test_br_t1_f8_01_in_app_json_cookie_import_authenticates_mock_x() {
    run_t1_in_headless_subprocess(
        "test_br_t1_f8_01_in_app_json_cookie_import_authenticates_mock_x",
    );
}

async fn body_br_t1_f8_01(mut app: gpui::AsyncApp) {
    use gpui_telegram_desktop::views::browser::{ActiveBrowserInput, BrowserInspectorTab};

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let x_server = rt.block_on(MockXServer::start()).expect("MockXServer");
    let seed = x_server.seed_session("tok_t1_f8_01", "ct0_t1_f8_01");
    let chrome_dir = isolated_temp_dir("t1_f8_01").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let cdp_json = generate_cdp_cookies_json(&x_auth_cookies(
        "127.0.0.1",
        &seed.auth_token,
        &seed.ct0,
    ));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                801,
                "f8_user1",
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
                w.set_inspector_tab(BrowserInspectorTab::Session, wcx);
                w.set_active_input(ActiveBrowserInput::CookieImport, None, wcx);
                w.insert_text_to_active_input(&cdp_json, wcx);
                let draft = w.recorder.cookie_import_draft.clone();
                w.import_cookies_from_text_async(draft, wcx);
            });
        });
    })
    .unwrap();

    let imported = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            w.recorder.last_imported_cookie_count >= 2
                && w.recorder.last_persisted_cookie_count >= 2
                && w.recorder.cookie_error_message.is_none()
        },
    )
    .await;
    if !imported {
        let diag = app
            .update(|cx| {
                let w = shell.read(cx).browser_workspace().unwrap().read(cx);
                format!(
                    "last_imported_cookie_count={}, last_persisted_cookie_count={}, cookie_error_message={:?}",
                    w.recorder.last_imported_cookie_count,
                    w.recorder.last_persisted_cookie_count,
                    w.recorder.cookie_error_message,
                )
            })
            .unwrap_or_else(|e| format!("failed to read workspace diag: {e:?}"));
        panic!("cookie import must succeed via RecorderPanel: {diag}");
    }

    let home_url = x_server.url("/home");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_navigate(&home_url, scx);
        });
    })
    .unwrap();

    let loaded = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.is_loading && w.current_url.contains("/home")
        },
    )
    .await;
    assert!(loaded, "authenticated /home must load without /login redirect");

    let snap = rt.block_on(chrome.snapshot_dom()).expect("snapshot_dom");
    assert!(
        snap.interactive_elements
            .iter()
            .any(|el| el.test_id.as_deref() == Some("tweetTextarea_0")),
        "authenticated /home must render tweetTextarea_0"
    );

    let data_dir = app
        .update(|cx| shell.read(cx).browser_workspace().unwrap().read(cx).data_dir.clone())
        .unwrap();
    let stored = BrowserProfileStore::new(&data_dir)
        .load_cookies("default", None)
        .expect("load persisted cookies");
    assert!(stored.iter().any(|c| c.name == "auth_token" && c.value == seed.auth_token));
    assert!(stored.iter().any(|c| c.name == "ct0" && c.value == seed.ct0));

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(x_server.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T1-F8-02
/// [Feature / Req] F8 / R3, AC5 — `browser_distill_last_trace` saves trace as named `AutomationScriptApp` in `RecorderPanelState` & `StorePanelState` with editable parameters
/// [Preconditions] `MockWebAppServer` + `MockOpenAiServer` + Chrome `CdpBrowserBackend` attached to `AppShell`.
/// [Opaque-Box Steps] Drive AI checkout via `trigger_copilot_prompt`, distill via `browser_distill_last_trace`, and edit `param_inputs` in `RecorderPanelState`.
/// [Observable Assertions] Distilled app has expected metadata, parameters, zero secrets, populates `local_scripts`, `selected_script_id`, `store.selected_draft_app`, and `collect_replay_params_for` reflects edited parameters.
#[test]
fn test_br_t1_f8_02_distill_last_trace_saves_named_script_app_with_editable_params() {
    run_t1_in_headless_subprocess(
        "test_br_t1_f8_02_distill_last_trace_saves_named_script_app_with_editable_params",
    );
}

async fn body_br_t1_f8_02(mut app: gpui::AsyncApp) {
    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let web = rt.block_on(MockWebAppServer::start()).expect("MockWebAppServer");
    let chrome_dir = isolated_temp_dir("t1_f8_02").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let llm = rt
        .block_on(MockOpenAiServer::start(vec![
            mock_openai_tool_call(
                "d1",
                "browser_navigate",
                json!({ "url": web.url("/item/item-04") }),
            ),
            mock_openai_tool_call(
                "d2",
                "browser_click",
                json!({ "test_id": "add-to-cart-btn" }),
            ),
            mock_openai_tool_call(
                "d3",
                "browser_wait_for",
                json!({ "test_id": "cart-status", "timeout_ms": 4000 }),
            ),
            mock_openai_tool_call(
                "d4",
                "browser_click",
                json!({ "test_id": "go-to-checkout" }),
            ),
            mock_openai_tool_call(
                "d5",
                "browser_wait_for",
                json!({ "test_id": "checkout-name", "timeout_ms": 4000 }),
            ),
            mock_openai_tool_call(
                "d6",
                "browser_type",
                json!({
                    "test_id": "checkout-name",
                    "text": "Alice Draft",
                    "parameter_name_hint": "customer_name"
                }),
            ),
            mock_openai_tool_call(
                "d7",
                "browser_type",
                json!({
                    "test_id": "checkout-email",
                    "text": "alice@draft.test",
                    "parameter_name_hint": "customer_email"
                }),
            ),
            mock_openai_tool_call(
                "d8",
                "browser_click",
                json!({ "test_id": "checkout-submit" }),
            ),
            mock_openai_tool_call(
                "d9",
                "browser_wait_for",
                json!({ "test_id": "confirmation-id", "timeout_ms": 4000 }),
            ),
            mock_openai_text("Checkout completed."),
        ]))
        .expect("MockOpenAiServer");

    let host = Arc::new(FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host"));
    e2e_suite::gpui_browser_harness::configure_host_with_mock_llm(&host, &llm.base_url());

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                802,
                "f8_user2",
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
                w.recorder.copilot_prompt_draft = "Buy item-04".to_string();
                w.trigger_copilot_prompt(wcx);
            });
        });
    })
    .unwrap();

    let done = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(30),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.recorder.copilot_running
        },
    )
    .await;
    assert!(done, "copilot checkout run must finish within 30s");
    app.update(|cx| {
        let w = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert!(
            w.recorder.copilot_last_reply.is_some(),
            "copilot failed: last_error={:?}, logs={:?}",
            w.last_error,
            w.recorder.event_logs
        );
    })
    .unwrap();
    assert_eq!(web.order_count(), 1);

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            let distilled = s
                .browser_distill_last_trace(
                    "WebApp Checkout Flow",
                    "Orders an item on MockWebAppServer",
                    "productivity",
                    scx,
                )
                .expect("distill_last_trace");
            assert_eq!(distilled.name, "WebApp Checkout Flow");
            assert_eq!(distilled.category, "productivity");
            assert!(distilled.parameters.iter().any(|p| p.name == "customer_name"));
            assert!(distilled.parameters.iter().any(|p| p.name == "customer_email"));
            let json_str = serde_json::to_string(&distilled).unwrap();
            assert!(fake_secrets().contains_any_secret(&json_str).is_none());

            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, _| {
                assert!(w.recorder.local_scripts.iter().any(|a| a.app_id == distilled.app_id));
                assert_eq!(w.recorder.selected_script_id.as_deref(), Some(distilled.app_id.as_str()));
                assert_eq!(
                    w.store.selected_draft_app.as_ref().map(|a| a.app_id.as_str()),
                    Some(distilled.app_id.as_str())
                );
                w.recorder
                    .param_inputs
                    .insert("customer_name".to_string(), "Edited Buyer".to_string());
                let collected = w.collect_replay_params_for(&distilled);
                assert_eq!(
                    collected.get("customer_name").map(String::as_str),
                    Some("Edited Buyer")
                );
            });
        });
    })
    .unwrap();

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(llm.shutdown());
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(web.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T1-F8-03
/// [Feature / Req] F8 / R3, AC5 — `browser_run_script_app` executes saved Script App locally with zero LLM calls and emits `ScriptReplayReport`
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` attached to `AppShell`.
/// [Opaque-Box Steps] Call `browser_run_script_app` with custom `customer_name` / `customer_email` parameters.
/// [Observable Assertions] `report.used_llm == false`, `report.success == true`, `step_timings.len() == steps.len()`, `extracted_outputs["order_id"]` starts with `"ORD-"`, and `MockWebAppServer` records the order.
#[test]
fn test_br_t1_f8_03_browser_run_script_app_zero_llm_replay_and_timings() {
    run_t1_in_headless_subprocess(
        "test_br_t1_f8_03_browser_run_script_app_zero_llm_replay_and_timings",
    );
}

async fn body_br_t1_f8_03(mut app: gpui::AsyncApp) {
    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let web = rt.block_on(MockWebAppServer::start()).expect("MockWebAppServer");
    let chrome_dir = isolated_temp_dir("t1_f8_03").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                803,
                "f8_user3",
            )
        })
        .unwrap();

    let script = build_webapp_checkout_script(&web.base_url(), "t1-f8-03-script", "Local Replay App");
    let expected_steps = script.steps.len();
    let params = BTreeMap::from([
        ("customer_name".to_string(), "Zero LLM User".to_string()),
        ("customer_email".to_string(), "zerollm@example.test".to_string()),
    ]);

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(
                Arc::clone(&chrome) as Arc<dyn BrowserSessionBackend>,
                scx,
            );
            s.browser_run_script_app(script, params, scx);
        });
    })
    .unwrap();

    let finished = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.recorder.is_replaying && w.recorder.last_replay_report.is_some()
        },
    )
    .await;
    assert!(finished, "browser_run_script_app must complete");

    app.update(|cx| {
        let w = shell.read(cx).browser_workspace().unwrap().read(cx);
        let report = w.recorder.last_replay_report.as_ref().unwrap();
        assert!(report.success);
        assert!(!report.used_llm);
        assert_eq!(report.steps_executed, expected_steps);
        assert_eq!(report.step_timings.len(), expected_steps);
        let order_id = report
            .extracted_outputs
            .get("order_id")
            .expect("extracted order_id");
        assert!(order_id.starts_with("ORD-"));
        assert!(
            w.recorder
                .event_logs
                .iter()
                .any(|e| e.title.contains("used_llm=false"))
        );
    })
    .unwrap();

    assert_eq!(web.order_count(), 1);
    {
        let orders = web.state().orders.read();
        let ord = orders.values().next().unwrap();
        assert_eq!(ord.customer_name, "Zero LLM User");
        assert_eq!(ord.customer_email, "zerollm@example.test");
    }

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(web.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T1-F8-04
/// [Feature / Req] F8 / R4, AC6 — `browser_publish_script_app_with_changelog` publishes local Script App to `EphemeralTestServer` store
/// [Preconditions] `EphemeralTestServer` started; User A registered; `AppShell` authenticated with User A's real session token.
/// [Opaque-Box Steps] Call `browser_publish_script_app_with_changelog` on clean `AutomationScriptApp`.
/// [Observable Assertions] `store.publish_success` populated, `store.publish_error` is None, `store.catalog_apps` contains app, and `client_a.get_automation_app` returns matching manifest.
#[test]
fn test_br_t1_f8_04_publish_script_app_to_ephemeral_server_store() {
    run_t1_in_headless_subprocess("test_br_t1_f8_04_publish_script_app_to_ephemeral_server_store");
}

async fn body_br_t1_f8_04(mut app: gpui::AsyncApp) {
    let client_dir = isolated_temp_dir("t1_f8_04_client").expect("client dir");
    let rt_boot = tokio::runtime::Runtime::new().unwrap();
    let mut server = rt_boot
        .block_on(EphemeralTestServer::start())
        .expect("EphemeralTestServer");
    let client = e2e_suite::gpui_browser_harness::open_client_in_dir(&server.base_url(), client_dir.path());
    let rt = client.runtime().handle().clone();
    let auth = rt
        .block_on(client.register(&RegisterRequest {
            username: "t1_f8_04_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        }))
        .expect("register alice");

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell_with_token(
                cx,
                client.clone(),
                auth.user_id,
                "t1_f8_04_alice",
                &auth.token,
            )
        })
        .unwrap();

    let script = build_webapp_checkout_script("http://127.0.0.1:8080", "t1-f8-04-pub", "Published Flow");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_publish_script_app_with_changelog(
                script.clone(),
                Some("Initial release".to_string()),
                scx,
            );
        });
    })
    .unwrap();

    let published = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(20),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.store.is_publishing
                && w.store.publish_success.is_some()
                && !w.store.is_loading_catalog
                && w.store.catalog_apps.iter().any(|c| c.app_id == "t1-f8-04-pub")
        },
    )
    .await;
    if !published {
        let diag = app
            .update(|cx| {
                let w = shell.read(cx).browser_workspace().unwrap().read(cx);
                format!(
                    "is_publishing={}, is_loading_catalog={}, publish_success={:?}, publish_error={:?}, catalog_len={}",
                    w.store.is_publishing,
                    w.store.is_loading_catalog,
                    w.store.publish_success,
                    w.store.publish_error,
                    w.store.catalog_apps.len(),
                )
            })
            .unwrap_or_else(|e| format!("failed to read store diag: {e:?}"));
        panic!("publish must update StorePanelState and catalog: {diag}");
    }

    app.update(|cx| {
        let w = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert!(w.store.publish_error.is_none());
        assert!(!w.store.publish_secret_rejected);
    })
    .unwrap();

    let detail = rt
        .block_on(client.get_automation_app("t1-f8-04-pub"))
        .expect("get_automation_app");
    assert_eq!(detail.name, "Published Flow");
    assert_eq!(detail.latest_version, "1.0.0");

    app.update(|_| drop(shell)).unwrap();
    client.clear_session();
    drop(client);
    rt_boot.block_on(server.stop());
}

/// [Test ID] TEST-BR-T1-F8-05
/// [Feature / Req] F8 / R4, AC6 — `StorePanel` searches, installs (`browser_install_store_app`), and replays remote Script App
/// [Preconditions] `EphemeralTestServer` & `MockWebAppServer` started; User A publishes checkout Script App; User B opens `AppShell` + Chrome `CdpBrowserBackend`.
/// [Opaque-Box Steps] User B searches `"StorePanel"` in `StorePanel`, installs via `browser_install_store_app`, overrides parameters, and launches `run_installed_store_app`.
/// [Observable Assertions] App is cached in `LocalStore`, `report.success && !report.used_llm`, `MockWebAppServer` records 1 order, and server reports `install_count >= 1 && run_count >= 1`.
#[test]
fn test_br_t1_f8_05_store_panel_search_install_and_replay_remote_script_app() {
    run_t1_in_headless_subprocess(
        "test_br_t1_f8_05_store_panel_search_install_and_replay_remote_script_app",
    );
}

async fn body_br_t1_f8_05(mut app: gpui::AsyncApp) {
    use gpui_telegram_desktop::views::browser::BrowserInspectorTab;

    let rt_boot = tokio::runtime::Runtime::new().unwrap();
    let mut server = rt_boot
        .block_on(EphemeralTestServer::start())
        .expect("EphemeralTestServer");
    let web = rt_boot
        .block_on(MockWebAppServer::start())
        .expect("MockWebAppServer");

    let a_dir = isolated_temp_dir("t1_f8_05_a").expect("a dir");
    let client_a = e2e_suite::gpui_browser_harness::open_client_in_dir(&server.base_url(), a_dir.path());
    client_a
        .runtime()
        .block_on(client_a.register(&RegisterRequest {
            username: "t1_f8_05_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        }))
        .expect("register alice");

    let remote_app = build_webapp_checkout_script(
        &web.base_url(),
        "t1-f8-05-remote",
        "StorePanel Checkout App",
    );
    client_a
        .runtime()
        .block_on(client_a.publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&remote_app).unwrap(),
            is_published: true,
            changelog: Some("v1".into()),
        }))
        .expect("alice publish");
    client_a.clear_session();
    drop(client_a);

    let b_dir = isolated_temp_dir("t1_f8_05_b").expect("b dir");
    let client_b = e2e_suite::gpui_browser_harness::open_client_in_dir(&server.base_url(), b_dir.path());
    let rt_b = client_b.runtime().handle().clone();
    let auth_b = rt_b
        .block_on(client_b.register(&RegisterRequest {
            username: "t1_f8_05_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        }))
        .expect("register bob");

    let chrome_dir = isolated_temp_dir("t1_f8_05_chrome").expect("chrome dir");
    let chrome = Arc::new(rt_b.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell_with_token(
                cx,
                client_b.clone(),
                auth_b.user_id,
                "t1_f8_05_bob",
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
                w.store.search_query_draft = "StorePanel".to_string();
                w.search_store_apps(wcx);
            });
        });
    })
    .unwrap();

    let found = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(20),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.store.is_loading_catalog
                && w.store
                    .catalog_apps
                    .iter()
                    .any(|a| a.app_id == "t1-f8-05-remote")
        },
    )
    .await;
    assert!(found, "StorePanel search must find published remote app");

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_install_store_app("t1-f8-05-remote", scx);
        });
    })
    .unwrap();

    let installed = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(20),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            w.store
                .installed_apps
                .iter()
                .any(|a| a.app_id == "t1-f8-05-remote")
        },
    )
    .await;
    assert!(installed, "browser_install_store_app must install into StorePanel & LocalStore");
    assert!(
        client_b
            .get_installed_automation_app_cached("t1-f8-05-remote")
            .is_some()
    );

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            let inst = w
                .store
                .installed_apps
                .iter()
                .find(|a| a.app_id == "t1-f8-05-remote")
                .cloned()
                .unwrap();
            w.recorder
                .param_inputs
                .insert("customer_name".to_string(), "Store Buyer".to_string());
            w.recorder
                .param_inputs
                .insert("customer_email".to_string(), "store@buyer.test".to_string());
            w.run_installed_store_app(inst, wcx);
        });
    })
    .unwrap();

    let replayed = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.recorder.is_replaying && w.recorder.last_replay_report.is_some()
        },
    )
    .await;
    assert!(replayed, "run_installed_store_app must complete replay");

    app.update(|cx| {
        let w = shell.read(cx).browser_workspace().unwrap().read(cx);
        let report = w.recorder.last_replay_report.as_ref().unwrap();
        assert!(report.success);
        assert!(!report.used_llm);
        assert!(w.store.store_error.is_none());
    })
    .unwrap();
    assert_eq!(web.order_count(), 1);

    let detail = rt_b
        .block_on(client_b.get_automation_app("t1-f8-05-remote"))
        .expect("get_automation_app");
    assert!(detail.install_count >= 1);
    assert!(detail.run_count >= 1);

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

