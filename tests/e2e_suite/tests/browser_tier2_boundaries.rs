use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::Router;
use axum::response::Html;
use axum::routing::get;
use bot_browser::cdp::CdpTransport;
use bot_browser::{
    Argon2KdfParams, AutomationScriptApp, BotBrowserError, BrowserActionTrace, BrowserBackendKind,
    BrowserControlGate, BrowserControlMode, BrowserCookie, BrowserProfileStore,
    BrowserSessionBackend, CdpBrowserBackend, ChromeImportOptions, ChromeImportProbe,
    DeterministicScriptRunner, InMemoryVaultSecretResolver,
    NoopChromeKeyProvider, NormalizedOrigin, RecordedActionKind, SCRIPT_APP_SCHEMA_VERSION,
    ScriptAppMetadata, ScriptCompiler, ScriptParameter, ScriptStep, SemanticLocator,
    UserVaultCategory, UserVaultCrypto, classify_probe_io_error, import_cookies_from_chrome_sqlite,
    import_cookies_from_json, probe_chrome_cookies_at, validate_script_app_safe_for_publish,
};
use e2e_suite::browser_e2e_fixtures::{
    MockOpenAiServer, MockWebAppServer, MockXServer, assert_ui_loop_non_blocking, fake_secrets,
    html_escape, isolated_temp_dir, mock_openai_text, mock_openai_tool_call, no_proxy_http_client,
    x_auth_cookies,
};
use e2e_suite::common::EphemeralTestServer;
use futures::{SinkExt, StreamExt};
use gpui_telegram_agent::fabushi::FabushiBotHost;
use gpui_telegram_agent::hermes::providers::ProviderConfig;
use gpui_telegram_agent::{
    BrowserAutomationEvent, BrowserSessionConfig, BrowserSessionRegistry, HermesToolContext,
    HermesToolExecutor, HermesTurnRequest, ToolsetPreset,
};
use gpui_telegram_client::config::ClientConfig;
use gpui_telegram_client::net::{HttpClientError, RegisterRequest};
use gpui_telegram_client::TelegramClient;
use gpui_telegram_proto::automation_apps::PublishAutomationAppRequest;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

async fn launch_chrome_in(dir: &std::path::Path) -> CdpBrowserBackend {
    e2e_suite::gpui_browser_harness::launch_chrome_in(dir).await
}

/// [Test ID] TEST-BR-T2-F1-01
/// [Feature / Req] F1 / R1 — Invalid URLs & CDP `Page.navigate` errors (including real closed-port Chrome navigation) rejected cleanly; backend recovers for valid navigation
/// [Preconditions] Local `MockWebAppServer` started; real `ChromeCdp` launched; an unreachable closed localhost port bound & dropped; an injected `FailingNavTransport` (`MockCdp`) for JSON-RPC error propagation.
/// [Opaque-Box Steps] Call `navigate` with `""`, `"javascript:alert(1)"`, `"file:///etc/passwd"`, and `"http://169.254.169.254/latest/meta-data/"` on `ChromeCdp`; call `navigate` on real `ChromeCdp` against the closed localhost port; verify `FailingNavTransport` (`MockCdp`); then call `navigate(&server.base_url())` on `ChromeCdp`.
/// [Observable Assertions] Invalid schemes/empty/metadata URLs return `Err(BotBrowserError::InvalidOrigin(_))`; real closed-port `ChromeCdp` navigation returns `Err(BotBrowserError::Cdp(_))` containing `ERR_CONNECTION_REFUSED`; `FailingNavTransport` propagates `Err(BotBrowserError::Cdp(_))`; subsequent `navigate(&server.base_url())` succeeds (`Ok`).
#[tokio::test]
async fn test_br_t2_f1_01_invalid_urls_rejected_and_backend_recovers() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f1_01").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    for bad in [
        "",
        "javascript:alert(1)",
        "file:///etc/passwd",
        "http://169.254.169.254/latest/meta-data/",
    ] {
        let err = backend
            .navigate(bad)
            .await
            .expect_err("invalid URL must fail");
        assert!(
            matches!(err, BotBrowserError::InvalidOrigin(_)),
            "expected InvalidOrigin for `{bad}`, got {err:?}"
        );
    }

    // Real ChromeCdp closed-port navigation must surface Page.navigate result.errorText as Err(Cdp)
    let closed_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral closed port");
    let closed_port = closed_listener.local_addr().unwrap().port();
    drop(closed_listener);
    let closed_err = backend
        .navigate(&format!("http://127.0.0.1:{closed_port}/unreachable"))
        .await
        .expect_err("real ChromeCdp closed-port navigation must return Err");
    assert!(
        matches!(closed_err, BotBrowserError::Cdp(ref msg) if msg.contains("ERR_CONNECTION_REFUSED")),
        "expected Cdp error containing ERR_CONNECTION_REFUSED, got {closed_err:?}"
    );

    // Also verify JSON-RPC error propagation via FailingNavTransport (MockCdp)
    struct FailingNavTransport;
    #[async_trait]
    impl CdpTransport for FailingNavTransport {
        async fn send_command(&self, method: &str, _params: Value) -> Result<Value, BotBrowserError> {
            if method == "Page.navigate" {
                return Err(BotBrowserError::Cdp("net::ERR_CONNECTION_REFUSED".into()));
            }
            Ok(json!({}))
        }
        async fn current_endpoint(&self) -> Result<String, BotBrowserError> {
            Ok("mock://fail-nav".into())
        }
    }
    let mock_backend = CdpBrowserBackend::new_mock(Arc::new(FailingNavTransport));
    let cdp_err = mock_backend
        .navigate("http://127.0.0.1:9/cdp-error-injection")
        .await
        .expect_err("CDP nav JSON-RPC error must return Err");
    assert!(matches!(cdp_err, BotBrowserError::Cdp(_)));

    let ok_res = backend
        .navigate(&server.base_url())
        .await
        .expect("backend must remain healthy after invalid navigate calls");
    assert_eq!(ok_res.origin, server.base_url());

    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F1-02
/// [Feature / Req] F1 / R1 — `wait_for_locator` on non-existent element with 50ms timeout fails deterministically without hanging
/// [Preconditions] `MockWebAppServer` `/` loaded in `ChromeCdp`.
/// [Opaque-Box Steps] Call `wait_for_locator(&SemanticLocator::by_test_id("does-not-exist"), 50)` and measure wall-clock duration.
/// [Observable Assertions] Returns `Err(BotBrowserError::LocatorNotFound(_))`; elapsed duration is `>= 45ms` and `< 1000ms`.
#[tokio::test]
async fn test_br_t2_f1_02_wait_for_locator_timeout_is_bounded_and_deterministic() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f1_02").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    backend.navigate(&server.base_url()).await.expect("nav index");

    let started = Instant::now();
    let err = backend
        .wait_for_locator(&SemanticLocator::by_test_id("does-not-exist"), 50)
        .await
        .expect_err("missing locator must time out");
    let elapsed = started.elapsed();

    assert!(
        matches!(err, BotBrowserError::LocatorNotFound(_)),
        "expected LocatorNotFound, got {err:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(45) && elapsed < Duration::from_millis(1_000),
        "expected bounded timeout near 50ms, elapsed = {elapsed:?}"
    );

    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F1-03
/// [Feature / Req] F1 / R1 — `click_locator`, `fill_locator`, and `wait_for_locator` with empty `SemanticLocator` fail immediately before CDP roundtrip
/// [Preconditions] Call-counting `MockCdp` transport + real `ChromeCdp` backend.
/// [Opaque-Box Steps] Invoke `click_locator`, `fill_locator`, and `wait_for_locator` with `SemanticLocator::default()` on both `MockCdp` (checking call count) and `ChromeCdp`.
/// [Observable Assertions] All calls return `Err(BotBrowserError::LocatorNotFound(_))` with zero CDP commands issued.
#[tokio::test]
async fn test_br_t2_f1_03_empty_semantic_locator_rejected_immediately() {
    struct CountingTransport(AtomicUsize);
    #[async_trait]
    impl CdpTransport for CountingTransport {
        async fn send_command(&self, _method: &str, _params: Value) -> Result<Value, BotBrowserError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(json!({}))
        }
        async fn current_endpoint(&self) -> Result<String, BotBrowserError> {
            Ok("mock://counting".into())
        }
    }

    let transport = Arc::new(CountingTransport(AtomicUsize::new(0)));
    let mock_backend = CdpBrowserBackend::new_mock(Arc::clone(&transport) as Arc<dyn CdpTransport>);
    let empty = SemanticLocator::default();

    assert!(matches!(
        mock_backend.click_locator(&empty).await,
        Err(BotBrowserError::LocatorNotFound(_))
    ));
    assert!(matches!(
        mock_backend.fill_locator(&empty, "abc", false).await,
        Err(BotBrowserError::LocatorNotFound(_))
    ));
    assert!(matches!(
        mock_backend.wait_for_locator(&empty, 50).await,
        Err(BotBrowserError::LocatorNotFound(_))
    ));
    assert_eq!(
        transport.0.load(Ordering::SeqCst),
        0,
        "empty locator must reject before issuing any CDP command"
    );

    let temp = isolated_temp_dir("t2_f1_03").expect("temp dir");
    let chrome = launch_chrome_in(temp.path()).await;
    assert!(matches!(
        chrome.click_locator(&empty).await,
        Err(BotBrowserError::LocatorNotFound(_))
    ));
    assert!(matches!(
        chrome.fill_locator(&empty, "abc", false).await,
        Err(BotBrowserError::LocatorNotFound(_))
    ));
    chrome.close().await.expect("close chrome");
}

/// [Test ID] TEST-BR-T2-F1-04
/// [Feature / Req] F1 / R1, AC3 — Oversized 512KB+ DOM with embedded secrets & delayed response is bounded, redacted, and non-blocking
/// [Preconditions] Local Axum server serving a >550KB HTML page containing 350 interactive buttons, `<input type="password">` + `FakeSecretFixtures`, and delayed route response.
/// [Opaque-Box Steps] Launch `ChromeCdp`, navigate to the oversized DOM route and call `snapshot_dom()` inside `assert_ui_loop_non_blocking`.
/// [Observable Assertions] `text_excerpt.chars().count() <= 12_000`, `interactive_elements.len() <= 200`, `FakeSecretFixtures::contains_any_secret` returns `None`, and UI ticker never stalls.
#[tokio::test]
async fn test_br_t2_f1_04_large_dom_snapshot_bounded_redacted_and_non_blocking() {
    let secrets = fake_secrets();
    let pw_esc = html_escape(secrets.password);
    let tok_esc = html_escape(secrets.auth_token);

    let mut buttons = String::new();
    for i in 0..350 {
        buttons.push_str(&format!(
            r#"<button type="button" data-testid="btn-{i}">Action {i}</button>"#
        ));
    }
    let filler_paragraph = "Large DOM payload chunk for snapshot bounding verification. ".repeat(10_000);
    let large_html = format!(
        r#"<!DOCTYPE html>
<html><head><title>Large DOM Stress</title></head>
<body>
  <form>
    <input type="password" name="password" data-testid="pw-field" value="{pw_esc}" />
    <input type="text" name="auth_token" data-testid="tok-field" value="{tok_esc}" />
    {buttons}
  </form>
  <main>{filler_paragraph}</main>
</body></html>"#
    );
    assert!(large_html.len() > 512 * 1024);

    let html_shared = Arc::new(large_html);
    let app = Router::new().route(
        "/large",
        get({
            let html_shared = Arc::clone(&html_shared);
            move || {
                let html_shared = Arc::clone(&html_shared);
                async move {
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    Html((*html_shared).clone())
                }
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let srv_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    let temp = isolated_temp_dir("t2_f1_04").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;
    let url = format!("http://{addr}/large");

    let (snap, ui_report) = assert_ui_loop_non_blocking(
        || async {
            backend.navigate(&url).await.expect("navigate large DOM");
            backend.snapshot_dom().await.expect("snapshot large DOM")
        },
        Duration::from_millis(10),
        150.0,
    )
    .await
    .expect("large DOM must not block UI ticker");

    assert!(ui_report.ticks_completed >= 2);
    assert!(snap.text_excerpt.chars().count() <= 12_000);
    assert!(snap.interactive_elements.len() <= 200);
    assert!(snap.redacted_field_count >= 2);

    let snap_json = serde_json::to_string(&snap).expect("serialize snapshot");
    assert_eq!(secrets.contains_any_secret(&snap_json), None);

    backend.close().await.expect("close backend");
    let _ = shutdown_tx.send(());
    let _ = srv_handle.await;
}

/// [Test ID] TEST-BR-T2-F1-05
/// [Feature / Req] F1 / R1 — `eval_js` syntax/runtime exceptions on `ChromeCdp` and mid-call CDP WS transport disconnect
/// [Preconditions] Real `ChromeCdp` backend + local WS mock CDP server that abruptly closes socket on `Runtime.evaluate`.
/// [Opaque-Box Steps] Call `eval_js` with invalid syntax and a thrown JS Error on `ChromeCdp`, then verify `eval_js("1 + 1")` recovers; connect via `connect_cdp` to a WS server that drops connection mid-call on `Runtime.evaluate`.
/// [Observable Assertions] JS errors return `Err(BotBrowserError::Cdp(_))` and subsequent `eval_js("1 + 1")` returns `2`; mid-call WS disconnect returns clean `Err` without panicking or deadlocking.
#[tokio::test]
async fn test_br_t2_f1_05_eval_js_errors_and_cdp_transport_disconnect() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f1_05").expect("temp dir");
    let chrome = launch_chrome_in(temp.path()).await;

    chrome.navigate(&server.base_url()).await.expect("nav index");

    let syntax_err = chrome
        .eval_js("function ( { broken")
        .await
        .expect_err("syntax error must return Err");
    assert!(matches!(syntax_err, BotBrowserError::Cdp(_)));

    let throw_err = chrome
        .eval_js("(() => { throw new Error('boom_eval_err'); })()")
        .await
        .expect_err("thrown exception must return Err");
    assert!(matches!(throw_err, BotBrowserError::Cdp(_)));

    let recovered = chrome.eval_js("1 + 1").await.expect("eval_js must recover");
    assert_eq!(recovered, json!(2));

    chrome.close().await.expect("close chrome");
    server.shutdown().await;

    // Mid-call transport disconnect via local WS mock CDP server
    let ws_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ws");
    let ws_port = ws_listener.local_addr().unwrap().port();
    let ws_url = format!("ws://127.0.0.1:{ws_port}/devtools/page/disconnect-test");
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                Ok((stream, _)) = ws_listener.accept() => {
                    tokio::spawn(async move {
                        let Ok(mut ws) = accept_async(stream).await else { return; };
                        while let Some(Ok(Message::Text(txt))) = ws.next().await {
                            let Ok(req) = serde_json::from_str::<Value>(&txt) else { continue; };
                            let id = req.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                            let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
                            if method == "Runtime.evaluate" {
                                // Simulate abrupt mid-call transport drop
                                let _ = ws.close(None).await;
                                break;
                            }
                            let _ = ws.send(Message::Text(json!({ "id": id, "result": {} }).to_string())).await;
                        }
                    });
                }
            }
        }
    });

    let ws_backend = CdpBrowserBackend::connect_cdp(
        BrowserBackendKind::ChromeCdp,
        ws_url,
        Duration::from_secs(2),
    );
    let disc_err = ws_backend
        .eval_js("document.title")
        .await
        .expect_err("mid-call WS disconnect must return Err");
    assert!(
        matches!(disc_err, BotBrowserError::Io(_) | BotBrowserError::Cdp(_)),
        "expected Io or Cdp error on disconnect, got {disc_err:?}"
    );

    // Issue one more call on `ws_backend` after the disconnect to prove `WsCdpTransport`
    // releases its connection lock and fails cleanly without deadlocking.
    let post_disc_err = tokio::time::timeout(Duration::from_secs(2), ws_backend.eval_js("1 + 1"))
        .await
        .expect("subsequent call after WS disconnect must not deadlock")
        .expect_err("subsequent call after WS disconnect must return Err");
    assert!(
        matches!(
            post_disc_err,
            BotBrowserError::Io(_) | BotBrowserError::Cdp(_)
        ),
        "expected clean Io or Cdp error on post-disconnect call, got {post_disc_err:?}"
    );
    let _ = shutdown_tx.send(());
}

/// [Test ID] TEST-BR-T2-F2-01
/// [Feature / Req] F2 / R1, AC4 — Corrupt/truncated/non-JSON/invalid-SameSite cookie payloads fail cleanly without corrupting `BrowserProfileStore`
/// [Preconditions] `BrowserProfileStore` seeded with valid `x_auth_cookies`.
/// [Opaque-Box Steps] Pass empty string, `"{bad json"`, invalid `sameSite`, invalid `domain`, and CRLF cookie-injection payloads to `import_cookies_from_json`.
/// [Observable Assertions] Every malformed payload returns `Err(BotBrowserError::CookieImport(_))` without leaking secret values; existing `BrowserProfileStore` cookies remain intact.
#[tokio::test]
async fn test_br_t2_f2_01_corrupt_cookie_import_preserves_existing_profile() {
    let temp = isolated_temp_dir("t2_f2_01").expect("temp dir");
    let store = BrowserProfileStore::new(temp.path());
    let origin = NormalizedOrigin::parse("https://x.com").expect("origin");

    let valid: Vec<BrowserCookie> = x_auth_cookies("x.com", "keep_auth_123", "keep_ct0_456")
        .into_iter()
        .map(|fc| BrowserCookie::new(fc.name, fc.value, fc.domain))
        .collect();
    store
        .save_cookies("main", Some(&origin), &valid)
        .expect("save initial valid cookies");

    let malformed_inputs = [
        "",
        "   ",
        "{bad_secret_json_payload",
        "not_a_valid_netscape_line_without_tabs",
        r#"[{"name":"a","value":"v","domain":"x.com","sameSite":"bogus_policy"}]"#,
        r#"[{"name":"a","value":"v","domain":"bad domain;"}]"#,
        r#"[{"name":"a","value":"LEAK_CRLF\r\nSet-Cookie: evil=1","domain":"x.com"}]"#,
    ];

    for bad in malformed_inputs {
        let err = import_cookies_from_json(bad).expect_err("corrupt payload must fail");
        assert!(
            matches!(err, BotBrowserError::CookieImport(_)),
            "expected CookieImport for `{bad}`, got {err:?}"
        );
        assert!(!err.to_string().contains("LEAK_CRLF"));
    }

    let persisted = store.load_cookies("main", Some(&origin)).expect("load existing");
    assert_eq!(persisted.len(), 2);
    assert!(persisted.iter().any(|c| c.value == "keep_auth_123"));
}

/// [Test ID] TEST-BR-T2-F2-02
/// [Feature / Req] F2 / R1, AC4 — Expired cookies are filtered by `load_active_cookies` and rejected by `MockXServer`
/// [Preconditions] `MockXServer` started with seeded session; `BrowserProfileStore` and `ChromeCdp` in isolated `TempDir`s.
/// [Opaque-Box Steps] Store expired + active cookies in `BrowserProfileStore` and call `load_active_cookies`; separately import ONLY expired cookies (`expires = 1_000_000`) into `ChromeCdp` and navigate to `MockXServer` `/home`.
/// [Observable Assertions] `load_active_cookies` excludes expired cookies; `ChromeCdp` navigation to `/home` is unauthenticated and redirects to `/login`.
#[tokio::test]
async fn test_br_t2_f2_02_expired_cookies_filtered_and_rejected_by_mock_x() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let store_dir = isolated_temp_dir("t2_f2_02_store").expect("store dir");
    let chrome_dir = isolated_temp_dir("t2_f2_02_chrome").expect("chrome dir");

    let store = BrowserProfileStore::new(store_dir.path());
    let origin = NormalizedOrigin::parse(&server.base_url()).expect("origin");
    let sess = server.seed_session("expired_auth_tok", "expired_ct0_tok");

    let mut expired_auth = BrowserCookie::new("auth_token", &sess.auth_token, "127.0.0.1");
    expired_auth.http_only = true;
    expired_auth.expires = Some(1_000_000);
    expired_auth.session = false;

    let mut expired_ct0 = BrowserCookie::new("ct0", &sess.ct0, "127.0.0.1");
    expired_ct0.expires = Some(1_000_000);
    expired_ct0.session = false;

    let mut fresh_pref = BrowserCookie::new("lang", "en", "127.0.0.1");
    fresh_pref.expires = Some(2_500_000_000);
    fresh_pref.session = false;

    store
        .save_cookies(
            "exp-test",
            Some(&origin),
            &[expired_auth.clone(), expired_ct0.clone(), fresh_pref],
        )
        .expect("save cookies");

    let active = store
        .load_active_cookies("exp-test", Some(&origin), 1_700_000_000)
        .expect("load_active_cookies");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].name, "lang");

    let backend = launch_chrome_in(chrome_dir.path()).await;
    backend
        .import_cookies(&[expired_auth, expired_ct0])
        .await
        .expect("import expired cookies");
    backend.navigate(&server.url("/home")).await.expect("nav /home");
    let final_url = backend.current_url().await.expect("current_url");
    assert!(
        final_url.contains("/login"),
        "expired cookies must not authenticate /home; got {final_url}"
    );

    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F2-03
/// [Feature / Req] F2 / R1, AC4 — Simulated macOS TCC `EPERM` on Chrome cookie DB path returns `PermissionDeniedTcc` and `PermissionDenied` error
/// [Preconditions] `isolated_temp_dir` containing a mock Chrome `Cookies` file.
/// [Opaque-Box Steps] Classify `EPERM (1)` and `EACCES (13)` via `classify_probe_io_error`; chmod a temp `Cookies` file to `0o000` (when non-root) and call `probe_chrome_cookies_at` + `import_cookies_from_chrome_sqlite`.
/// [Observable Assertions] Returns `ChromeImportProbe::PermissionDeniedTcc { path, guidance }` with actionable non-empty `guidance` and `Err(BotBrowserError::PermissionDenied(_))` without panicking.
#[tokio::test]
async fn test_br_t2_f2_03_macos_tcc_eperm_chrome_cookie_probe_handling() {
    let temp = isolated_temp_dir("t2_f2_03").expect("temp dir");
    let prof_dir = temp.path().join("Default");
    std::fs::create_dir_all(&prof_dir).expect("mkdir Default");
    let cookies_path = prof_dir.join("Cookies");
    std::fs::write(&cookies_path, b"SQLite format 3\0").expect("write fixture");

    for raw_code in [1, 13] {
        let io_err = std::io::Error::from_raw_os_error(raw_code);
        match classify_probe_io_error(&cookies_path, &io_err) {
            ChromeImportProbe::PermissionDeniedTcc { path, guidance } => {
                assert_eq!(path, cookies_path);
                assert!(!guidance.trim().is_empty());
            }
            other => panic!("expected PermissionDeniedTcc for errno {raw_code}, got {other:?}"),
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cookies_path, std::fs::Permissions::from_mode(0o000))
            .expect("chmod 000");
        if std::fs::File::open(&cookies_path).is_err() {
            match probe_chrome_cookies_at(&cookies_path) {
                ChromeImportProbe::PermissionDeniedTcc { path, guidance } => {
                    assert_eq!(path, cookies_path);
                    assert!(!guidance.is_empty());
                }
                other => panic!("expected PermissionDeniedTcc on 0o000 file, got {other:?}"),
            }
            let import_err = import_cookies_from_chrome_sqlite(
                &cookies_path,
                &NoopChromeKeyProvider,
                &ChromeImportOptions::default(),
            )
            .expect_err("TCC denied sqlite import must return Err");
            assert!(
                matches!(import_err, BotBrowserError::PermissionDenied(_)),
                "expected PermissionDenied, got {import_err:?}"
            );
        }
        let _ = std::fs::set_permissions(&cookies_path, std::fs::Permissions::from_mode(0o600));
    }
}

/// [Test ID] TEST-BR-T2-F2-04
/// [Feature / Req] F2 / R1, AC4 — Domain filter boundary `export_cookies(Some("x.com"))` excludes lookalike `evil-x.com` & `example.com`
/// [Preconditions] `MockCdp` transport seeded with cookies for `x.com`, `.x.com`, `sub.x.com`, `evil-x.com`, and `example.com`.
/// [Opaque-Box Steps] Call `export_cookies(Some("x.com"))` and evaluate `BrowserCookie::matches_host("x.com")` across all cookie domains.
/// [Observable Assertions] Only `x.com`, `.x.com`, and `sub.x.com` are returned; `evil-x.com` and `example.com` are strictly excluded.
#[tokio::test]
async fn test_br_t2_f2_04_domain_filter_boundary_excludes_lookalike_domains() {
    struct MultiDomainCookieTransport;
    #[async_trait]
    impl CdpTransport for MultiDomainCookieTransport {
        async fn send_command(&self, method: &str, _params: Value) -> Result<Value, BotBrowserError> {
            if method == "Network.getCookies" {
                return Ok(json!({
                    "cookies": [
                        { "name": "c1", "value": "v1", "domain": "x.com", "path": "/" },
                        { "name": "c2", "value": "v2", "domain": ".x.com", "path": "/" },
                        { "name": "c3", "value": "v3", "domain": "sub.x.com", "path": "/" },
                        { "name": "c4", "value": "v4", "domain": "evil-x.com", "path": "/" },
                        { "name": "c5", "value": "v5", "domain": "example.com", "path": "/" }
                    ]
                }));
            }
            Ok(json!({}))
        }
        async fn current_endpoint(&self) -> Result<String, BotBrowserError> {
            Ok("mock://cookies".into())
        }
    }

    let backend = CdpBrowserBackend::new_mock(Arc::new(MultiDomainCookieTransport));
    let filtered = backend
        .export_cookies(Some("x.com"))
        .await
        .expect("export filtered cookies");

    let domains: Vec<&str> = filtered.iter().map(|c| c.domain.as_str()).collect();
    assert_eq!(domains.len(), 3);
    assert!(domains.contains(&"x.com"));
    assert!(domains.contains(&".x.com"));
    assert!(domains.contains(&"sub.x.com"));
    assert!(!domains.contains(&"evil-x.com"));
    assert!(!domains.contains(&"example.com"));

    assert!(BrowserCookie::new("a", "1", "x.com").matches_host("x.com"));
    assert!(BrowserCookie::new("a", "1", ".x.com").matches_host("api.x.com"));
    assert!(!BrowserCookie::new("a", "1", "evil-x.com").matches_host("x.com"));
    assert!(!BrowserCookie::new("a", "1", "x.com").matches_host("evil-x.com"));
}

/// [Test ID] TEST-BR-T2-F2-05
/// [Feature / Req] F2 / R1, AC4 — Wrong passphrase, wrong `user_id`, and tampered ciphertext fail E2EE `BrowserCookie` restore with `Crypto` error
/// [Preconditions] `BrowserProfileStore` sealed into `EncryptedVaultBlob` (`UserVaultCategory::BrowserCookie`) for `user_id = 77`.
/// [Opaque-Box Steps] Attempt `UserVaultCrypto::unlock_with_passphrase` with wrong passphrase, `restore_from_vault_blob` with wrong `user_id = 88`, and `restore_from_vault_blob` with tampered `ciphertext_b64`.
/// [Observable Assertions] All three negative attempts return `Err(BotBrowserError::Crypto(_))`; destination `BrowserProfileStore` directory contains zero written profile files.
#[tokio::test]
async fn test_br_t2_f2_05_e2ee_wrong_passphrase_and_tampered_ciphertext() {
    let src_dir = isolated_temp_dir("t2_f2_05_src").expect("src temp");
    let dst_dir = isolated_temp_dir("t2_f2_05_dst").expect("dst temp");
    let src_store = BrowserProfileStore::new(src_dir.path());
    let dst_store = BrowserProfileStore::new(dst_dir.path());
    let origin = NormalizedOrigin::parse("https://x.com").expect("origin");

    let cookie = BrowserCookie::new("auth_token", "super_secret_cookie_val", ".x.com");
    src_store
        .save_cookies("e2ee-neg", Some(&origin), &[cookie])
        .expect("save cookies");

    let (key, envelope, _) =
        UserVaultCrypto::initialize_new(77, "right-passphrase", Argon2KdfParams::fast_for_tests())
            .expect("init crypto");
    let blob = src_store
        .seal_to_vault_blob(&key, 77, "e2ee-neg", Some(&origin), 1, 1024 * 1024)
        .expect("seal blob");
    assert_eq!(blob.category, UserVaultCategory::BrowserCookie);

    let wrong_pw = UserVaultCrypto::unlock_with_passphrase(77, "wrong-passphrase", &envelope)
        .expect_err("wrong passphrase must fail");
    assert!(matches!(wrong_pw, BotBrowserError::Crypto(_)));

    let wrong_uid = dst_store
        .restore_from_vault_blob(&key, 88, &blob)
        .expect_err("wrong user_id must fail");
    assert!(matches!(wrong_uid, BotBrowserError::Crypto(_)));

    let mut tampered = blob.clone();
    let mut chars: Vec<char> = tampered.ciphertext_b64.chars().collect();
    chars[0] = if chars[0] == 'A' { 'B' } else { 'A' };
    tampered.ciphertext_b64 = chars.into_iter().collect();
    let tamper_err = dst_store
        .restore_from_vault_blob(&key, 77, &tampered)
        .expect_err("tampered ciphertext must fail");
    assert!(matches!(tamper_err, BotBrowserError::Crypto(_)));

    assert!(
        dst_store.load_cookies("e2ee-neg", Some(&origin)).unwrap().is_empty(),
        "destination store must remain empty after failed E2EE restore"
    );
}

/// [Test ID] TEST-BR-T2-F3-01
/// [Feature / Req] F3 / R3, AC5 — `ScriptCompiler::compile_from_trace` rejects empty trace and empty metadata name
/// [Preconditions] Empty `BrowserActionTrace` (`actions.is_empty()`).
/// [Opaque-Box Steps] Call `ScriptCompiler::compile_from_trace` on an empty trace, and on a non-empty trace with blank `meta.name`.
/// [Observable Assertions] Both calls return `Err(BotBrowserError::ScriptCompilation(_))`.
#[tokio::test]
async fn test_br_t2_f3_01_compile_empty_trace_rejected() {
    let empty_trace = BrowserActionTrace::new("empty-sess");
    let err_empty = ScriptCompiler::compile_from_trace(
        &empty_trace,
        ScriptAppMetadata {
            name: "Should Fail".into(),
            ..Default::default()
        },
    )
    .expect_err("empty trace must be rejected");
    assert!(matches!(err_empty, BotBrowserError::ScriptCompilation(_)));

    let mut non_empty = BrowserActionTrace::new("non-empty");
    non_empty.push_action(
        None,
        RecordedActionKind::PressKey {
            key: "Enter".into(),
        },
        None,
    );
    let err_name = ScriptCompiler::compile_from_trace(
        &non_empty,
        ScriptAppMetadata {
            name: "   ".into(),
            ..Default::default()
        },
    )
    .expect_err("blank app name must be rejected");
    assert!(matches!(err_name, BotBrowserError::ScriptCompilation(_)));
}

/// [Test ID] TEST-BR-T2-F3-02
/// [Feature / Req] F3 / R3, AC5 — Secret-redaction trap across all 4 `FakeSecretFixtures` values
/// [Preconditions] `BrowserActionTrace` embedding `auth_token`, `ct0`, `bearer_token`, and `password` (`is_secret=true`) across `Fill` and `Navigate` steps.
/// [Opaque-Box Steps] Compile via `ScriptCompiler::compile_from_trace`, serialize `AutomationScriptApp` to JSON, and validate with `validate_script_app_safe_for_publish`.
/// [Observable Assertions] `FakeSecretFixtures::contains_any_secret(&json)` returns `None`, and `validate_script_app_safe_for_publish` succeeds (`Ok(())`).
#[tokio::test]
async fn test_br_t2_f3_02_secret_redaction_trap_with_all_fake_secrets() {
    let secrets = fake_secrets();
    let mut trace = BrowserActionTrace::new("sess-t2-f3-02");

    trace.push_action(
        None,
        RecordedActionKind::Navigate {
            url: format!(
                "https://x.com/oauth?auth_token={}&ct0={}",
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
            locator: SemanticLocator::by_test_id("auth-token-input"),
            value: secrets.auth_token.into(),
            is_secret: true,
            vault_key_hint: None,
            parameter_name_hint: Some("auth_token".into()),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("ct0-input"),
            value: secrets.ct0.into(),
            is_secret: true,
            vault_key_hint: None,
            parameter_name_hint: Some("ct0".into()),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Fill {
            locator: SemanticLocator::by_test_id("bearer-input"),
            value: secrets.bearer_token.into(),
            is_secret: true,
            vault_key_hint: None,
            parameter_name_hint: Some("bearer_token".into()),
        },
        None,
    );

    let app = ScriptCompiler::compile_from_trace(
        &trace,
        ScriptAppMetadata {
            name: "All Fake Secrets Trap".into(),
            ..Default::default()
        },
    )
    .expect("compile_from_trace");

    let serialized = serde_json::to_string(&app).expect("serialize app");
    assert_eq!(secrets.contains_any_secret(&serialized), None);
    validate_script_app_safe_for_publish(&app).expect("compiled app must pass publish validation");
}

/// [Test ID] TEST-BR-T2-F3-03
/// [Feature / Req] F3 / R3, AC5 — Missing required `ScriptParameter` and missing `secret_vault_key` fail upfront / cleanly
/// [Preconditions] `ChromeCdp` backend; `AutomationScriptApp` with required non-secret parameter and `Fill` referencing `secret_vault_key`.
/// [Opaque-Box Steps] Run with empty `params` (missing required parameter), then run with non-secret param provided and an empty `InMemoryVaultSecretResolver` (missing vault key).
/// [Observable Assertions] Missing required parameter returns `Err(BotBrowserError::ScriptReplay(_))` with 0 backend operations executed; missing vault key returns `Err(BotBrowserError::CredentialNotFound(_))`.
#[tokio::test]
async fn test_br_t2_f3_03_missing_required_param_and_missing_vault_key_errors() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f3_03").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;

    let app_missing_param = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "app_missing_param".into(),
        slug: "missing-param".into(),
        name: "Missing Param".into(),
        version: "1.0.0".into(),
        description: "Test missing required parameter".into(),
        category: "general".into(),
        target_domains: vec!["127.0.0.1".into()],
        parameters: vec![ScriptParameter {
            name: "query".into(),
            description: "Required search query".into(),
            required: true,
            default_value: None,
            is_secret: false,
        }],
        steps: vec![ScriptStep::Navigate {
            url_template: server.url("/search?q={{query}}"),
        }],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };

    let err_param =
        DeterministicScriptRunner::run(&backend, &app_missing_param, &BTreeMap::new(), None, None)
            .await
            .expect_err("missing required param must fail upfront");
    assert!(matches!(err_param, BotBrowserError::ScriptReplay(_)));
    assert_eq!(
        backend.performance_metrics().total_operations,
        0,
        "upfront param validation must run 0 steps"
    );

    let app_missing_vault = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "app_missing_vault".into(),
        slug: "missing-vault".into(),
        name: "Missing Vault".into(),
        version: "1.0.0".into(),
        description: "Test missing vault secret".into(),
        category: "general".into(),
        target_domains: vec!["127.0.0.1".into()],
        parameters: vec![],
        steps: vec![
            ScriptStep::Navigate {
                url_template: server.url("/"),
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_test_id("search-input"),
                value_template: String::new(),
                secret_vault_key: Some("nonexistent:password".into()),
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };

    let resolver = InMemoryVaultSecretResolver::new();
    let err_vault = DeterministicScriptRunner::run(
        &backend,
        &app_missing_vault,
        &BTreeMap::new(),
        Some(&resolver),
        None,
    )
    .await
    .expect_err("missing vault key must return CredentialNotFound");
    assert!(matches!(err_vault, BotBrowserError::CredentialNotFound(_)));

    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F3-04
/// [Feature / Req] F3 / R3, AC5 — Mid-script failure at step 3 stops before step 4, returns `Ok(ScriptReplayReport { success: false, step_timings })`, redacts secret errors, and resets `BrowserControlGate` to `Idle`
/// [Preconditions] `MockWebAppServer` started; real `ChromeCdp` launched; `BrowserControlGate` created.
/// [Opaque-Box Steps] Run a 4-step script (`app_mismatch`) failing at step 3 (`AssertTextContains` mismatch with secret parameter) so step 4 (`Click(go-to-checkout)`) never runs; then run `app_missing` failing at step 3 (`Click(vanished-step-3)`).
/// [Observable Assertions] Both return `Ok(ScriptReplayReport)` with `success == false`, `used_llm == false`, `steps_executed == 3`, and `step_timings.len() == 3` (`[0]` & `[1]` `success: true`, `[2]` `success: false`, `step_index: Some(2)`, `detail: Some(...)`); `app_mismatch` redacts secret (`[REDACTED]`) in `report.error` and `step_timings[2].detail`, `backend.current_url()` does not contain `"/checkout"`, and `backend.performance_metrics().recent_steps` records the 3 pre-failure operations without step 4; `app_missing` reports `"locator not found"` / `"vanished-step-3"`; `gate.mode() == BrowserControlMode::Idle`.
#[tokio::test]
async fn test_br_t2_f3_04_mid_script_failure_records_partial_metrics_and_resets_gate() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f3_04").expect("temp dir");
    let backend = launch_chrome_in(temp.path()).await;
    let gate = BrowserControlGate::new();

    let app_mismatch = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "app_mid_fail".into(),
        slug: "mid-fail".into(),
        name: "Mid Fail Test".into(),
        version: "1.0.0".into(),
        description: "Fails at step 3".into(),
        category: "general".into(),
        target_domains: vec!["127.0.0.1".into()],
        parameters: vec![ScriptParameter {
            name: "secret_expect".into(),
            description: "Secret expected text".into(),
            required: true,
            default_value: None,
            is_secret: true,
        }],
        steps: vec![
            ScriptStep::Navigate {
                url_template: server.url("/"),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("search-submit"),
                timeout_ms: 2_000,
            },
            ScriptStep::AssertTextContains {
                locator: SemanticLocator::by_test_id("search-submit"),
                expected_template: "{{secret_expect}}".into(),
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("go-to-checkout"),
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };

    let params = BTreeMap::from([(
        "secret_expect".to_string(),
        "TOP_SECRET_ASSERT_MISMATCH_99".to_string(),
    )]);
    let report = DeterministicScriptRunner::run(&backend, &app_mismatch, &params, None, Some(&gate))
        .await
        .expect("mid-script step 3 mismatch returns Ok(ScriptReplayReport { success: false })");
    assert!(!report.success);
    assert!(!report.used_llm);
    assert_eq!(report.steps_executed, 3);
    assert_eq!(report.step_timings.len(), 3);
    assert!(report.step_timings[0].success);
    assert!(report.step_timings[1].success);
    assert!(!report.step_timings[2].success);
    assert_eq!(report.step_timings[2].step_index, Some(2));
    let err_msg = report.error.as_deref().unwrap_or("");
    let detail_msg = report.step_timings[2].detail.as_deref().unwrap_or("");
    assert!(!err_msg.contains("TOP_SECRET_ASSERT_MISMATCH_99"));
    assert!(!detail_msg.contains("TOP_SECRET_ASSERT_MISMATCH_99"));
    assert!(err_msg.contains("[REDACTED]"));
    assert!(detail_msg.contains("[REDACTED]"));
    assert_eq!(gate.mode(), BrowserControlMode::Idle);

    let url_after_mismatch = backend.current_url().await.expect("current_url");
    assert!(
        url_after_mismatch.starts_with(&server.base_url())
            && !url_after_mismatch.contains("/checkout"),
        "step 4 Click(go-to-checkout) must not execute after step 3 mismatch, got {url_after_mismatch}"
    );
    let mismatch_metrics = backend.performance_metrics();
    assert_eq!(mismatch_metrics.total_operations, 3);
    assert_eq!(mismatch_metrics.failed_operations, 0);
    let mismatch_ops: Vec<&str> = mismatch_metrics
        .recent_steps
        .iter()
        .map(|s| s.operation.as_str())
        .collect();
    assert_eq!(
        mismatch_ops,
        vec!["navigate", "wait_for_locator", "extract_text_locator"]
    );

    // Also verify missing locator mid-script records `success: false` in both report.step_timings and backend telemetry
    backend.reset_metrics();
    let app_missing = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "app_mid_missing".into(),
        slug: "mid-missing".into(),
        name: "Mid Missing Test".into(),
        version: "1.0.0".into(),
        description: "Missing locator at step 3".into(),
        category: "general".into(),
        target_domains: vec!["127.0.0.1".into()],
        parameters: vec![],
        steps: vec![
            ScriptStep::Navigate {
                url_template: server.url("/"),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("search-input"),
                timeout_ms: 2_000,
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("vanished-step-3"),
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("go-to-checkout"),
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };
    let report2 =
        DeterministicScriptRunner::run(&backend, &app_missing, &BTreeMap::new(), None, Some(&gate))
            .await
            .expect("missing locator at step 3 returns Ok(ScriptReplayReport { success: false })");
    assert!(!report2.success);
    assert!(!report2.used_llm);
    assert_eq!(report2.steps_executed, 3);
    assert_eq!(report2.step_timings.len(), 3);
    assert!(report2.step_timings[0].success);
    assert!(report2.step_timings[1].success);
    assert!(!report2.step_timings[2].success);
    assert_eq!(report2.step_timings[2].step_index, Some(2));
    let err2_msg = report2.error.as_deref().unwrap_or("");
    let detail2_msg = report2.step_timings[2].detail.as_deref().unwrap_or("");
    assert!(
        err2_msg.contains("locator not found") && err2_msg.contains("vanished-step-3"),
        "expected locator not found for vanished-step-3, got: {err2_msg}"
    );
    assert!(detail2_msg.contains("vanished-step-3"));
    assert_eq!(gate.mode(), BrowserControlMode::Idle);

    let metrics = backend.performance_metrics();
    assert_eq!(metrics.total_operations, 3);
    assert_eq!(metrics.failed_operations, 1);
    assert_eq!(metrics.recent_steps.len(), 3);
    assert!(metrics.recent_steps[0].success);
    assert!(metrics.recent_steps[1].success);
    assert!(!metrics.recent_steps[2].success);

    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F3-05
/// [Feature / Req] F3 / R3, AC5 — `BrowserControlGate` cancellation (`user_cancel_ai`) mid-`DeterministicScriptRunner::run` aborts immediately
/// [Preconditions] `MockWebAppServer` started; `ChromeCdp` launched and pre-warmed on `/search?q=warm`; `BrowserControlGate` subscribed via `watch`.
/// [Opaque-Box Steps] Start a `DeterministicScriptRunner::run` navigating to `/` followed by `HumanCheckpoint` and `Navigate` to `/checkout`; wait up to 15s (selecting on `&mut runner_handle` so early `Err` surfaces immediately) for `WaitingForTakeover`, invoke `gate.user_cancel_ai()`, and await runner result.
/// [Observable Assertions] Runner exits immediately with `Err(BotBrowserError::ControlCancelled(_))`, `final_url` is at `/` (`starts_with(&server.base_url())`, not `/checkout` or `/search`), remaining steps are skipped, and `gate.mode()` returns to `Idle`.
#[tokio::test]
async fn test_br_t2_f3_05_control_gate_cancellation_aborts_replay_cleanly() {
    let server = MockWebAppServer::start().await.expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f3_05").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);
    // Pre-warm Chrome + `WsCdpTransport` on a distinct path before spawning the runner
    // so cold `connect_ws()` + Chrome startup under parallel load never races the takeover wait.
    backend
        .navigate(&server.url("/search?q=warm"))
        .await
        .expect("pre-warm ChromeCdp");
    let gate = Arc::new(BrowserControlGate::new());

    let app = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "app_cancel_mid".into(),
        slug: "cancel-mid".into(),
        name: "Cancel Mid Replay".into(),
        version: "1.0.0".into(),
        description: "Tests cancellation at checkpoint".into(),
        category: "general".into(),
        target_domains: vec!["127.0.0.1".into()],
        parameters: vec![],
        steps: vec![
            ScriptStep::Navigate {
                url_template: server.url("/"),
            },
            ScriptStep::HumanCheckpoint {
                reason: "Confirm before checkout".into(),
            },
            ScriptStep::Navigate {
                url_template: server.url("/checkout"),
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };

    let mut rx = gate.subscribe();
    let backend_task = Arc::clone(&backend);
    let gate_task = Arc::clone(&gate);
    let app_task = app.clone();
    let mut runner_handle = tokio::spawn(async move {
        DeterministicScriptRunner::run(
            backend_task.as_ref(),
            &app_task,
            &BTreeMap::new(),
            None,
            Some(&gate_task),
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(15), async {
        tokio::select! {
            res = &mut runner_handle => {
                panic!("runner exited prematurely before WaitingForTakeover: {res:?}");
            }
            wait_res = rx.wait_for(|m| matches!(m, BrowserControlMode::WaitingForTakeover { .. })) => {
                wait_res.expect("watch channel open");
            }
        }
    })
    .await
    .expect("must reach WaitingForTakeover within 15s");

    gate.user_cancel_ai();

    let run_res = runner_handle.await.expect("runner join");
    let err = run_res.expect_err("cancelled run must return Err");
    assert!(
        matches!(err, BotBrowserError::ControlCancelled(_)),
        "expected ControlCancelled, got {err:?}"
    );
    assert_eq!(gate.mode(), BrowserControlMode::Idle);

    // Step 0 (/) must have navigated away from /search?q=warm, and Step 2 (/checkout) must NOT have executed
    let final_url = backend.current_url().await.expect("current_url");
    assert!(
        final_url.starts_with(&server.base_url())
            && !final_url.contains("/checkout")
            && !final_url.contains("/search"),
        "expected Step 0 `/` active and Step 2 `/checkout` skipped after cancel, got {final_url}"
    );

    backend.close().await.expect("close backend");
    server.shutdown().await;
}

// ============================================================================
// F4–F6 Helpers & Boundary Tests (TEST-BR-T2-F4-01..05, F5-01..05, F6-01..05)
// ============================================================================

fn build_clean_boundary_app(
    app_id: &str,
    slug: &str,
    name: &str,
    version: &str,
) -> AutomationScriptApp {
    AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: app_id.to_string(),
        slug: slug.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        description: format!("Boundary test app for {name}"),
        category: "productivity".to_string(),
        target_domains: vec!["example.com".to_string()],
        parameters: vec![],
        steps: vec![ScriptStep::Navigate {
            url_template: "https://example.com/home".to_string(),
        }],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    }
}

async fn drop_client_async(client: TelegramClient) {
    client.clear_session();
    tokio::task::spawn_blocking(move || drop(client))
        .await
        .expect("spawn_blocking drop TelegramClient");
}

fn full_turn_req(prompt: &str, session_id: &str) -> HermesTurnRequest {
    let mut req = HermesTurnRequest::new(prompt).with_session(session_id);
    req.preset = Some(ToolsetPreset::Full);
    req
}

/// [Test ID] TEST-BR-T2-F4-01
/// [Feature / Req] F4 / R4, AC6 — `POST /api/v1/automation-apps` secret-leakage rejection (`422`) vs valid prose/placeholder positive control (`201`)
/// [Preconditions] `EphemeralTestServer` started; User A registered; `FakeSecretFixtures` loaded.
/// [Opaque-Box Steps] Attempt to publish manifests leaking `FakeSecretFixtures` in `description`, `steps[0].url_template`, `steps[1].value_template`, `changelog`, and an extra JSON field; verify SQLite remains empty; then publish positive control using `{{auth_token}}` placeholder + `"Bearer authentication"` prose.
/// [Observable Assertions] All leaky payloads return HTTP `422` with `"error": "secret_leak_detected"` and write 0 rows to `automation_apps`; positive control returns HTTP `201`.
#[tokio::test]
async fn test_br_t2_f4_01_secret_leakage_rejection_and_positive_control() {
    let mut server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let dir = isolated_temp_dir("t2_f4_01").expect("temp dir");
    let client = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: server.base_url(),
            ..Default::default()
        },
        dir.path(),
    )
    .expect("client");
    let auth = client
        .register(&RegisterRequest {
            username: "t2_f4_01_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register");

    let secrets = fake_secrets();
    let http = no_proxy_http_client();
    let publish_url = format!("{}/api/v1/automation-apps", server.base_url());

    // 1. Leak in description
    let mut app1 = build_clean_boundary_app("leak-desc", "leak-desc", "Leak Desc", "1.0.0");
    app1.description = format!("auth_token={}", secrets.auth_token);

    // 2. Leak in step URL
    let mut app2 = build_clean_boundary_app("leak-url", "leak-url", "Leak Url", "1.0.0");
    app2.steps = vec![ScriptStep::Navigate {
        url_template: format!("https://example.com/home?ct0={}", secrets.ct0),
    }];

    // 3. Leak in Fill step
    let mut app3 = build_clean_boundary_app("leak-fill", "leak-fill", "Leak Fill", "1.0.0");
    app3.steps.push(ScriptStep::Fill {
        locator: SemanticLocator::by_test_id("pw"),
        value_template: format!("password={}", secrets.password),
        secret_vault_key: None,
    });

    // 4. Leak in changelog + unknown extra JSON key
    let app4 = build_clean_boundary_app("leak-chg", "leak-chg", "Leak Chg", "1.0.0");
    let mut extra_manifest =
        serde_json::to_value(&build_clean_boundary_app("leak-extra", "leak-extra", "Leak Extra", "1.0.0"))
            .unwrap();
    extra_manifest["extra_headers"] = json!({ "Authorization": secrets.bearer_token });

    let leaky_payloads = vec![
        json!({ "manifest": serde_json::to_value(&app1).unwrap(), "is_published": true }),
        json!({ "manifest": serde_json::to_value(&app2).unwrap(), "is_published": true }),
        json!({ "manifest": serde_json::to_value(&app3).unwrap(), "is_published": true }),
        json!({
            "manifest": serde_json::to_value(&app4).unwrap(),
            "is_published": true,
            "changelog": format!("Uses {}", secrets.bearer_token)
        }),
        json!({ "manifest": extra_manifest, "is_published": true }),
    ];

    for (idx, payload) in leaky_payloads.into_iter().enumerate() {
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&payload)
            .send()
            .await
            .expect("send leaky payload");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(status, 422, "case {idx} must reject with 422, got {body}");
        assert_eq!(body["error"], "secret_leak_detected");
    }

    // Confirm nothing persisted to SQLite
    {
        let db = &server.state().expect("state").db;
        let count: i64 = db
            .pool()
            .with_reader(|conn| {
                Ok(conn
                    .query_row("SELECT COUNT(*) FROM automation_apps", [], |r| r.get(0))
                    .unwrap())
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    // Positive control: {{auth_token}} secret placeholder + "Bearer authentication" prose -> 201
    let positive_app = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "safe-bearer-prose".into(),
        slug: "safe-bearer-prose".into(),
        name: "Safe Bearer Prose App".into(),
        version: "1.0.0".into(),
        description: "Uses Bearer authentication safely via Bearer ({{auth_token}}) placeholder".into(),
        category: "productivity".into(),
        target_domains: vec!["example.com".into()],
        parameters: vec![ScriptParameter {
            name: "auth_token".into(),
            description: "Token".into(),
            required: true,
            default_value: None,
            is_secret: true,
        }],
        steps: vec![
            ScriptStep::Navigate {
                url_template: "https://example.com/login".into(),
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_test_id("token-input"),
                value_template: "{{auth_token}}".into(),
                secret_vault_key: Some("secret.auth_token".into()),
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };
    let ok_resp = http
        .post(&publish_url)
        .bearer_auth(&auth.token)
        .json(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&positive_app).unwrap(),
            is_published: true,
            changelog: Some("Clean release".into()),
        })
        .send()
        .await
        .expect("send positive control");
    assert_eq!(ok_resp.status().as_u16(), 201);

    // Bug 3 regression: ScriptCompiler-sanitized URLs with query placeholders (`?token={{...}}`,
    // `&next=/home`, `#frag`) publish cleanly (201) via TelegramClient::publish_automation_app,
    // while unclosed-quote, Bearer/Authorization trailing-payload, and prefix/suffix bypasses return 422.
    let mut trace = BrowserActionTrace::new("trace-bug3-e2e");
    trace.push_action(
        None,
        RecordedActionKind::Navigate {
            url: format!(
                "https://example.com/oauth/callback?auth_token={}&ct0={}&next=/home",
                secrets.auth_token, secrets.ct0
            ),
        },
        None,
    );
    trace.push_action(
        None,
        RecordedActionKind::Click {
            locator: SemanticLocator::by_test_id("continue-btn"),
        },
        None,
    );
    let compiled = ScriptCompiler::compile_from_trace(
        &trace,
        ScriptAppMetadata {
            app_id: Some("compiled-query-ph".into()),
            slug: Some("compiled-query-ph".into()),
            name: "Compiled Query Placeholder".into(),
            version: "1.0.0".into(),
            description: "OAuth callback".into(),
            category: "general".into(),
            target_domains: vec!["example.com".into()],
            ..Default::default()
        },
    )
    .expect("compile trace with sensitive query params");
    let published_compiled = client
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&compiled).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .expect("ScriptCompiler-sanitized URL must publish with 201");
    assert_eq!(published_compiled.app_id, "compiled-query-ph");
    let fetched = client
        .get_automation_app("compiled-query-ph")
        .await
        .expect("fetch published compiled app");
    assert_eq!(fetched.app_id, "compiled-query-ph");

    for (idx, ok_url) in [
        "https://example.com/auth?token={{auth_token}}",
        "https://example.com/auth?token={{auth_token}}&next=/home",
        "https://example.com/auth?token={{auth_token}}#frag",
    ]
    .iter()
    .enumerate()
    {
        let mut ok_q = positive_app.clone();
        let id = format!("safe-query-ph-{idx}");
        ok_q.app_id = id.clone();
        ok_q.slug = id;
        ok_q.steps = vec![ScriptStep::Navigate {
            url_template: (*ok_url).to_string(),
        }];
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_q).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send valid query placeholder app");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(status, 201, "valid URL `{ok_url}` must return 201, got {body_txt}");
    }

    for bad_desc in [
        "https://example.com/auth?token=\"raw_secret",
        "https://example.com/auth?token=\"\"raw_secret_123456",
        "https://example.com/auth?token=''raw_secret_123456",
        "https://example.com/auth?token={{auth_token}}\"raw_secret_123456",
        "https://example.com/auth?token={{auth_token}}'raw_secret_123456",
        "Bearer {{auth_token}}raw_secret",
        "Authorization: Bearer {{auth_token}}raw_secret",
        "Authorization: Bearer {{auth_token}}\"raw_secret_123456",
        "Bearer {{auth_token}}#raw_secret_123456",
        "Bearer \"raw_secret\"",
        "Basic {{auth_token}}dXNlcjpwYXNz",
        "Basic dXNlcjpwYXNz",
        "https://example.com/auth?token={{auth_token}}real_secret",
        "https://example.com/auth?token=real_secret{{auth_token}}",
        r#"{"token":"raw_secret"}"#,
    ] {
        let mut bad_app = positive_app.clone();
        bad_app.app_id = "bad-bypass-e2e".into();
        bad_app.slug = "bad-bypass-e2e".into();
        bad_app.description = bad_desc.to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(status, 422, "bypass vector `{bad_desc}` must return 422: {body}");
        assert_eq!(body["error"], "secret_leak_detected");
    }

    // Section 4.2 unified matrix (Challenger 1-17, unbalanced enclosures, embedded JSON, UTF-8/emoji positive controls)
    let mut s42_base = positive_app.clone();
    s42_base.parameters.push(ScriptParameter {
        name: "api_token".into(),
        description: "API token".into(),
        required: true,
        default_value: None,
        is_secret: true,
    });

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
        let mut ok_app = s42_base.clone();
        let id = format!("s42-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send Section 4.2 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "Section 4.2 positive control `{ok_desc}` must return 201, got {body_txt}"
        );
    }

    for bad_desc in [
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
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "s42-bad-e2e".into();
        bad_app.slug = "s42-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send Section 4.2 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "Section 4.2 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    // Phase 1 It4 Challenger Classes 1-6 + Reviewer p1_5 Minor Finding (plus positive controls)
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
        let mut ok_app = s42_base.clone();
        let id = format!("p14-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_4 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_4 positive control `{ok_desc}` must return 201, got {body_txt}"
        );
    }

    for bad_desc in [
        r#"{"Bearer":"secret_1234567890"}"#,
        r#"{"Basic":"dXNlcjpwYXNz"}"#,
        "Bearer ( raw_secret_123456)",
        "Basic ( dXNlcjpwYXNz)",
        "Basic (user dXNlcjpwYXNz)",
        "https://example.com/auth?authToken=raw_secret_123456",
        "https://example.com/auth?auth_token_1=raw_secret_123456",
        r#"Debug: {"authToken": "raw_secret_123456""#,
        r#"{"Authorization": "Bearer {{api_token}} raw_secret_123456"}"#,
        "Authorization: Bearer {{api_token}} raw_secret_123456",
        "Basic auth: value: dXNlcjpwYXNz",
        "`sk-ant-api03-1234567890abcdef1234`",
        "`eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U`",
        "Bearer raw_secret_123456:",
    ] {
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p14-bad-e2e".into();
        bad_app.slug = "p14-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_4 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_4 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    // Phase 1 It5 Challenger p1_8 Classes A-F (38 vectors) + Reviewer p1_7 Minor Notes 1-2 + Positive Controls
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
        let mut ok_app = s42_base.clone();
        let id = format!("p15-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_5 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_5 positive control `{ok_desc}` must return 201, got {body_txt}"
        );
    }

    for bad_desc in [
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
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p15-bad-e2e".into();
        bad_app.slug = "p15-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_5 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_5 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    for extra_headers in [
        json!({ "basicAuth": "dXNlcjpwYXNz" }),
        json!({ "bearerAuth": "raw_secret_123456" }),
        json!({ "Bearer": "key sec_12345" }),
    ] {
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({
                "manifest": serde_json::to_value(&s42_base).unwrap(),
                "is_published": true,
                "extra_headers": extra_headers,
            }))
            .send()
            .await
            .expect("send p1_5 extra_headers bypass");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_5 extra_headers `{extra_headers}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    for chg_bad in ["Bearer (\nraw_secret_123456\n)", "Basic ( dXNlcjpwYXNz"] {
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({
                "manifest": serde_json::to_value(&s42_base).unwrap(),
                "is_published": true,
                "changelog": chg_bad,
            }))
            .send()
            .await
            .expect("send p1_5 changelog bypass");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_5 changelog `{chg_bad}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
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
        let mut ok_app = s42_base.clone();
        let id = format!("p16-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_6 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_6 positive control `{ok_desc}` must return 201, got {body_txt}"
        );
    }

    let padded_bearer_sp = format!("Bearer (\n{}\nraw_secret_123456\n)", " ".repeat(1030));
    let padded_basic_sp = format!("Basic (\n{}\ndXNlcjpwYXNz\n)", " ".repeat(1030));
    let padded_bracket_nl = format!("Bearer [\n{}\nraw_secret_123456\n]", "\n".repeat(1030));

    let p16_bad_descs: Vec<&str> = vec![
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

    for bad_desc in p16_bad_descs {
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p16-bad-e2e".into();
        bad_app.slug = "p16-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_6 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_6 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    for extra_headers in [
        json!({ "basic1": "dXNlcjpwYXNz" }),
        json!({ "authToken1": "raw_secret_123456" }),
        json!({ "ct0": "raw_secret_123456" }),
    ] {
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({
                "manifest": serde_json::to_value(&s42_base).unwrap(),
                "is_published": true,
                "extra_headers": extra_headers,
            }))
            .send()
            .await
            .expect("send p1_6 extra_headers bypass");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_6 extra_headers `{extra_headers}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    for chg_bad in [
        "Bearer ({{api_token}}) raw_secret_123456",
        "Bearer ( a\nraw_secret_123456",
        "bearerAuth: key sec_12345",
    ] {
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({
                "manifest": serde_json::to_value(&s42_base).unwrap(),
                "is_published": true,
                "changelog": chg_bad,
            }))
            .send()
            .await
            .expect("send p1_6 changelog bypass");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_6 changelog `{chg_bad}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    for (pname, default_val) in [
        ("authToken", "raw_secret_123456"),
        ("bearerAuth", "raw_secret_123456"),
        ("basicAuth", "dXNlcjpwYXNz"),
    ] {
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p16-param-bad-e2e".into();
        bad_app.slug = "p16-param-bad-e2e".into();
        bad_app.parameters.push(ScriptParameter {
            name: pname.to_string(),
            description: "Param bypass".into(),
            required: false,
            default_value: Some(default_val.to_string()),
            is_secret: false,
        });
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_6 param bypass");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_6 sensitive param `{pname}` with is_secret=false must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    // Phase 1 It7 Challenger p1_12 New Classes 1-6 + Reviewer p1_12 >64KiB/>128KiB padding + positive controls
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
        let mut ok_app = s42_base.clone();
        let id = format!("p17-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_7 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_7 positive control `{ok_desc}` must return 201, got {body_txt}"
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

    let p17_bad_descs: Vec<&str> = vec![
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

    for bad_desc in p17_bad_descs {
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p17-bad-e2e".into();
        bad_app.slug = "p17-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_7 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_7 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    for extra_headers in [
        json!({ "v2token": "raw_secret_123456" }),
        json!({ "Basic dXNlcjpwYXNz": "" }),
        json!({ "Bearer raw_secret_123456": "{{api_token}}" }),
    ] {
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({
                "manifest": serde_json::to_value(&s42_base).unwrap(),
                "is_published": true,
                "extra_headers": extra_headers,
            }))
            .send()
            .await
            .expect("send p1_7 extra_headers bypass");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_7 extra_headers `{extra_headers}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    {
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({
                "manifest": serde_json::to_value(&s42_base).unwrap(),
                "is_published": true,
                "changelog": "Bearer {{api_token}} my raw_secret_123456",
            }))
            .send()
            .await
            .expect("send p1_7 changelog bypass");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_7 changelog N_TRAIL_17 must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    for (pname, default_val) in [
        ("v2token", "raw_secret_123456"),
        ("v2bearerAuth", "raw_secret_123456"),
        ("v2basicAuth", "dXNlcjpwYXNz"),
        ("v2ct0", "raw_secret_123456"),
    ] {
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p17-param-bad-e2e".into();
        bad_app.slug = "p17-param-bad-e2e".into();
        bad_app.parameters.push(ScriptParameter {
            name: pname.to_string(),
            description: "Param bypass".into(),
            required: false,
            default_value: Some(default_val.to_string()),
            is_secret: false,
        });
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_7 param bypass");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_7 sensitive param `{pname}` with is_secret=false must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
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
        let mut ok_app = s42_base.clone();
        let id = format!("p18-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_8 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_8 positive control `{idx}` must return 201, got {body_txt}"
        );
    }

    let ov_mismatch_b = format!("Bearer (\n{}\n]\nraw_secret_123456", "你".repeat(350));
    let ov_mismatch_ba = format!("Basic (\n{}\n]\ndXNlcjpwYXNz", "word ".repeat(260));
    let ov_quote_paren_ba = format!("Basic (\n{}\n\")\"\ndXNlcjpwYXNz", "word ".repeat(260));
    let ov_quote_paren_b = format!("Bearer (\n{}\n\")\"\nraw_secret_123456", "你".repeat(350));
    let split_basic_1 = format!("Basic ( {}dXNlcjpwYXNz )", "a ".repeat(1017));
    let split_edge_13 = format!("Basic ({}dXNlcjpwYXNz)", "你".repeat(341));
    let split_edge_15 = format!("Basic ( {}dXNlcjoxMjM0NQ )", "a ".repeat(1017));

    for bad_desc in [
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
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p18-bad-e2e".into();
        bad_app.slug = "p18-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_8 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_8 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
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
        let mut ok_app = s42_base.clone();
        let id = format!("p19-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_9 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_9 positive/O(N) control `{idx}` must return 201, got {body_txt}"
        );
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_9 positive/O(N) control `{idx}` took too long: {:?}",
            t0.elapsed()
        );
    }

    let angle_bearer = format!("Bearer < {}a > b\nraw_secret_123456", "word ".repeat(260));
    let angle_basic = format!("Basic < {}a > b\ndXNlcjpwYXNz", "word ".repeat(260));
    for bad_desc in [
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
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p19-bad-e2e".into();
        bad_app.slug = "p19-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_9 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_9 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
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
        let mut ok_app = s42_base.clone();
        let id = format!("p110-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_10 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_10 positive/perf control `{idx}` must return 201, got {body_txt}"
        );
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_10 positive/perf control `{idx}` took too long: {:?}",
            t0.elapsed()
        );
    }

    for bad_desc in [
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
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p110-bad-e2e".into();
        bad_app.slug = "p110-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_10 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_10 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
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
        let mut ok_app = s42_base.clone();
        let id = format!("p111-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_11 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_11 positive/perf control `{idx}` must return 201, got {body_txt}"
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

    for bad_desc in [
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
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p111-bad-e2e".into();
        bad_app.slug = "p111-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let t0 = std::time::Instant::now();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_11 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_11 bypass vector must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
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
        let mut ok_app = s42_base.clone();
        let id = format!("p112-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_12 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_12 prose positive control `{idx}` must return 201, got {body_txt}"
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

    for bad_desc in [
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
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p112-bad-e2e".into();
        bad_app.slug = "p112-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let t0 = std::time::Instant::now();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_12 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_12 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
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
        "Use auth_token [note: {{auth_token}}]",
        dos_path_a.as_str(),
        dos_path_b_sp.as_str(),
        dos_path_b_nosp.as_str(),
        h3_dos_d200_450kb.as_str(),
    ]
    .iter()
    .enumerate()
    {
        let mut ok_app = s42_base.clone();
        let id = format!("p113-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_13 positive/perf control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_13 positive/perf control `{idx}` must return 201, got {body_txt}"
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

    for bad_desc in [
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
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p113-bad-e2e".into();
        bad_app.slug = "p113-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let t0 = std::time::Instant::now();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_13 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_13 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
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
        "Use auth_token[0]. Note: {{auth_token}}",
        "https://example.com/auth_token.html?user=alice",
        dec_dos_5k.as_str(),
        dec_dos_200k_5k.as_str(),
    ]
    .iter()
    .enumerate()
    {
        let mut ok_app = s42_base.clone();
        let id = format!("p114-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let t0 = std::time::Instant::now();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_14 positive/perf control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_14 positive/perf control `{idx}` must return 201, got {body_txt}"
        );
        assert!(
            t0.elapsed().as_millis() < 2000,
            "p1_14 positive/perf control `{idx}` took too long: {:?}",
            t0.elapsed()
        );
    }

    for bad_desc in [
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
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p114-bad-e2e".into();
        bad_app.slug = "p114-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let t0 = std::time::Instant::now();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_14 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_14 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
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
    ]
    .iter()
    .enumerate()
    {
        let mut ok_app = s42_base.clone();
        let id = format!("p115-ok-{idx}");
        ok_app.app_id = id.clone();
        ok_app.slug = id;
        ok_app.description = (*ok_desc).to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&PublishAutomationAppRequest {
                manifest: serde_json::to_value(&ok_app).unwrap(),
                is_published: true,
                changelog: None,
            })
            .send()
            .await
            .expect("send p1_15 positive control");
        let status = resp.status().as_u16();
        let body_txt = resp.text().await.unwrap_or_default();
        assert_eq!(
            status, 201,
            "p1_15 positive control `{idx}` must return 201, got {body_txt}"
        );
    }

    for bad_desc in [
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
        let mut bad_app = s42_base.clone();
        bad_app.app_id = "p115-bad-e2e".into();
        bad_app.slug = "p115-bad-e2e".into();
        bad_app.description = bad_desc.to_string();
        let resp = http
            .post(&publish_url)
            .bearer_auth(&auth.token)
            .json(&json!({ "manifest": serde_json::to_value(&bad_app).unwrap(), "is_published": true }))
            .send()
            .await
            .expect("send p1_15 bypass vector");
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.expect("error json");
        assert_eq!(
            status, 422,
            "p1_15 bypass vector `{bad_desc}` must return 422: {body}"
        );
        assert_eq!(body["error"], "secret_leak_detected");
    }

    drop_client_async(client).await;
    server.stop().await;
}

/// [Test ID] TEST-BR-T2-F4-02
/// [Feature / Req] F4 / R4, AC6 — Unauthenticated & forged Bearer token (`401`) + `TelegramClient` missing-token `HttpClientError::Unauthorized`
/// [Preconditions] `EphemeralTestServer` started; `no_proxy_http_client()` ready; unauthenticated `TelegramClient` constructed.
/// [Opaque-Box Steps] Hit `POST /automation-apps`, `GET /automation-apps/installed`, `POST /automation-apps/{id}/install`, `DELETE /automation-apps/{id}/install`, and `POST /automation-apps/{id}/runs` with no auth header and with forged Bearer; also call `TelegramClient` authenticated methods without token.
/// [Observable Assertions] Every raw HTTP probe returns `401 Unauthorized`; every unauthenticated `TelegramClient` call returns `HttpClientError::Unauthorized(_)`.
#[tokio::test]
async fn test_br_t2_f4_02_unauthenticated_and_forged_bearer_return_401() {
    let mut server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let http = no_proxy_http_client();
    let base = server.base_url();
    let sample = build_clean_boundary_app("auth-check", "auth-check", "Auth Check", "1.0.0");

    let probes = vec![
        (reqwest::Method::POST, format!("{base}/api/v1/automation-apps"), Some(json!({ "manifest": sample }))),
        (reqwest::Method::GET, format!("{base}/api/v1/automation-apps/installed"), None),
        (reqwest::Method::POST, format!("{base}/api/v1/automation-apps/auth-check/install"), None),
        (reqwest::Method::DELETE, format!("{base}/api/v1/automation-apps/auth-check/install"), None),
        (reqwest::Method::POST, format!("{base}/api/v1/automation-apps/auth-check/runs"), Some(json!({ "success": true, "steps_executed": 1, "total_duration_ms": 10, "used_llm": false }))),
    ];

    for (method, url, body) in probes {
        let mut req_no_auth = http.request(method.clone(), &url);
        if let Some(ref b) = body {
            req_no_auth = req_no_auth.json(b);
        }
        let r1 = req_no_auth.send().await.expect("no-auth request");
        assert_eq!(r1.status().as_u16(), 401, "{method} {url} without auth");

        let mut req_forged = http
            .request(method.clone(), &url)
            .bearer_auth("forged.invalid.jwt.token");
        if let Some(ref b) = body {
            req_forged = req_forged.json(b);
        }
        let r2 = req_forged.send().await.expect("forged-auth request");
        assert_eq!(r2.status().as_u16(), 401, "{method} {url} forged auth");
    }

    let dir = isolated_temp_dir("t2_f4_02").expect("temp dir");
    let unauth_client = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: base,
            ..Default::default()
        },
        dir.path(),
    )
    .expect("client");

    let err_pub = unauth_client
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&sample).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err_pub, HttpClientError::Unauthorized(_)));

    let err_inst = unauth_client
        .install_automation_app("auth-check")
        .await
        .unwrap_err();
    assert!(matches!(err_inst, HttpClientError::Unauthorized(_)));

    let err_list = unauth_client
        .list_installed_automation_apps()
        .await
        .unwrap_err();
    assert!(matches!(err_list, HttpClientError::Unauthorized(_)));

    drop_client_async(unauth_client).await;
    server.stop().await;
}

/// [Test ID] TEST-BR-T2-F4-03
/// [Feature / Req] F4 / R4, AC6 — Ownership `403`, spoofed `author_id` `403`, duplicate version `409` (`Conflict`), same-author upgrade `200`
/// [Preconditions] `EphemeralTestServer` started; User A & User B `TelegramClient`s registered.
/// [Opaque-Box Steps] User A publishes v1.0.0; User A republishes v1.0.0; User B attempts to publish v2.0.0 under User A's `app_id`; User A attempts to publish with spoofed `author_id`; User A publishes v1.1.0.
/// [Observable Assertions] Duplicate version returns `HttpClientError::Conflict(_)`; cross-author hijack & spoofed `author_id` return `HttpClientError::Api { status: 403, .. }`; same-author v1.1.0 succeeds with `versions == ["1.1.0", "1.0.0"]`.
#[tokio::test]
async fn test_br_t2_f4_03_ownership_403_conflict_409_and_same_author_upgrade() {
    let mut server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let a_dir = isolated_temp_dir("t2_f4_03_a").expect("a dir");
    let b_dir = isolated_temp_dir("t2_f4_03_b").expect("b dir");
    let cfg = ClientConfig {
        server_url: server.base_url(),
        ..Default::default()
    };

    let client_a = TelegramClient::open_with_dir(cfg.clone(), a_dir.path()).expect("client_a");
    let alice = client_a
        .register(&RegisterRequest {
            username: "t2_f4_03_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register alice");

    let client_b = TelegramClient::open_with_dir(cfg, b_dir.path()).expect("client_b");
    let _bob = client_b
        .register(&RegisterRequest {
            username: "t2_f4_03_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        })
        .await
        .expect("register bob");

    let app_v1 = build_clean_boundary_app("owned-app", "owned-app", "Owned App", "1.0.0");
    client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&app_v1).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .expect("initial publish");

    // 1. Duplicate version -> HttpClientError::Conflict (409)
    let dup_err = client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&app_v1).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(dup_err, HttpClientError::Conflict(_)),
        "expected Conflict (409), got {dup_err:?}"
    );

    // 2. Cross-author hijack -> 403
    let hijack = build_clean_boundary_app("owned-app", "owned-app", "Hijacked", "2.0.0");
    let hijack_err = client_b
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&hijack).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(hijack_err, HttpClientError::Api { status: 403, .. }),
        "expected 403 hijack error, got {hijack_err:?}"
    );

    // 3. Spoofed author_id -> 403
    let mut spoofed = build_clean_boundary_app("spoof-app", "spoof-app", "Spoofed", "1.0.0");
    spoofed.author_id = Some((alice.user_id + 9999).to_string());
    let spoof_err = client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&spoofed).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(spoof_err, HttpClientError::Api { status: 403, .. }),
        "expected 403 spoofed author_id error, got {spoof_err:?}"
    );

    // 4. Same-author upgrade -> 200 OK
    let app_v1_1 = build_clean_boundary_app("owned-app", "owned-app", "Owned App v1.1", "1.1.0");
    let upgraded = client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&app_v1_1).unwrap(),
            is_published: true,
            changelog: Some("v1.1.0".into()),
        })
        .await
        .expect("same-author upgrade succeeds");
    assert_eq!(upgraded.latest_version, "1.1.0");
    assert_eq!(upgraded.versions, vec!["1.1.0".to_string(), "1.0.0".to_string()]);

    drop_client_async(client_a).await;
    drop_client_async(client_b).await;
    server.stop().await;
}

/// [Test ID] TEST-BR-T2-F4-04
/// [Feature / Req] F4 / R4, AC6 — Nonexistent `app_id` & unpublished non-author draft (`404`) + client `app_id` guard (`400`)
/// [Preconditions] `EphemeralTestServer` started; User A publishes `is_published: false` draft; User B registered.
/// [Opaque-Box Steps] User B queries `get`, `install`, `uninstall`, and `record_run` for `"missing-app-xyz"` and for User A's unpublished draft; also invokes `get_automation_app("installed")` and `get_automation_app("../bad")`.
/// [Observable Assertions] Missing & private-draft routes return `HttpClientError::Api { status: 404, .. }`; `"installed"` and `"../bad"` fail locally with `HttpClientError::Api { status: 400, .. }`.
#[tokio::test]
async fn test_br_t2_f4_04_nonexistent_private_draft_404_and_client_guard_400() {
    let mut server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let a_dir = isolated_temp_dir("t2_f4_04_a").expect("a dir");
    let b_dir = isolated_temp_dir("t2_f4_04_b").expect("b dir");
    let cfg = ClientConfig {
        server_url: server.base_url(),
        ..Default::default()
    };

    let client_a = TelegramClient::open_with_dir(cfg.clone(), a_dir.path()).expect("client_a");
    client_a
        .register(&RegisterRequest {
            username: "t2_f4_04_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register alice");
    let draft = build_clean_boundary_app("private-draft", "private-draft", "Private Draft", "1.0.0");
    client_a
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&draft).unwrap(),
            is_published: false,
            changelog: None,
        })
        .await
        .expect("publish unpublished draft");

    let client_b = TelegramClient::open_with_dir(cfg, b_dir.path()).expect("client_b");
    client_b
        .register(&RegisterRequest {
            username: "t2_f4_04_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        })
        .await
        .expect("register bob");

    for target in ["missing-app-xyz", "private-draft"] {
        let g = client_b.get_automation_app(target).await.unwrap_err();
        assert!(matches!(g, HttpClientError::Api { status: 404, .. }), "get {target}: {g:?}");

        let i = client_b.install_automation_app(target).await.unwrap_err();
        assert!(matches!(i, HttpClientError::Api { status: 404, .. }), "install {target}: {i:?}");

        let u = client_b.uninstall_automation_app(target).await.unwrap_err();
        assert!(matches!(u, HttpClientError::Api { status: 404, .. }), "uninstall {target}: {u:?}");

        let r = client_b
            .record_automation_app_run(
                target,
                &gpui_telegram_proto::automation_apps::RecordAutomationRunRequest {
                    version: None,
                    success: true,
                    steps_executed: 1,
                    total_duration_ms: 5,
                    used_llm: false,
                    error: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(r, HttpClientError::Api { status: 404, .. }), "run {target}: {r:?}");
    }

    // Author CAN fetch their own unpublished draft
    let author_draft = client_a
        .get_automation_app("private-draft")
        .await
        .expect("author fetches own draft");
    assert!(!author_draft.is_published);

    // Client-side guard rejects reserved / traversal app_ids locally with 400
    for bad_id in ["installed", "INSTALLED", "../bad", "bad/id", ""] {
        let err = client_b.get_automation_app(bad_id).await.unwrap_err();
        assert!(
            matches!(err, HttpClientError::Api { status: 400, .. }),
            "expected local 400 for '{bad_id}', got {err:?}"
        );
    }

    drop_client_async(client_a).await;
    drop_client_async(client_b).await;
    server.stop().await;
}

/// [Test ID] TEST-BR-T2-F4-05
/// [Feature / Req] F4 / R4, AC6 — Search bounds (`limit=0`, `limit=10000`, `offset=99999`) & SQLi/XSS query safety
/// [Preconditions] `EphemeralTestServer` started; 1 clean published app in store.
/// [Opaque-Box Steps] Call `search_automation_apps` with `limit=Some(0)`, `limit=Some(10000)`, `offset=Some(99999)`, `q=Some("' OR 1=1;--")`, and `q=Some("<script>alert(1)</script>")`, then query normal catalog again.
/// [Observable Assertions] `limit=0` clamps to `1`, `limit=10000` clamps to `100`, `offset=99999` clamps to `10000` with empty `apps`, SQLi/XSS payloads return `200 OK` with `total == 0`, and database table remains intact.
#[tokio::test]
async fn test_br_t2_f4_05_search_bounds_clamping_and_sqli_xss_safety() {
    let mut server = EphemeralTestServer::start()
        .await
        .expect("start EphemeralTestServer");
    let dir = isolated_temp_dir("t2_f4_05").expect("temp dir");
    let client = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: server.base_url(),
            ..Default::default()
        },
        dir.path(),
    )
    .expect("client");
    client
        .register(&RegisterRequest {
            username: "t2_f4_05_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        })
        .await
        .expect("register");

    let app = build_clean_boundary_app("safe-app", "safe-app", "Safe Catalog Entry", "1.0.0");
    client
        .publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&app).unwrap(),
            is_published: true,
            changelog: None,
        })
        .await
        .expect("publish");

    let r_zero = client
        .search_automation_apps(None, None, None, Some(0), None)
        .await
        .expect("limit=0");
    assert_eq!(r_zero.limit, 1);
    assert_eq!(r_zero.apps.len(), 1);

    let r_huge_limit = client
        .search_automation_apps(None, None, None, Some(10_000), None)
        .await
        .expect("limit=10000");
    assert_eq!(r_huge_limit.limit, 100);

    let r_huge_offset = client
        .search_automation_apps(None, None, None, None, Some(99_999))
        .await
        .expect("offset=99999");
    assert_eq!(r_huge_offset.offset, 10_000);
    assert!(r_huge_offset.apps.is_empty());

    let r_sqli = client
        .search_automation_apps(Some("' OR 1=1;--"), None, None, None, None)
        .await
        .expect("SQLi payload");
    assert_eq!(r_sqli.total, 0);

    let r_xss = client
        .search_automation_apps(Some("<script>alert(1)</script>"), None, None, None, None)
        .await
        .expect("XSS payload");
    assert_eq!(r_xss.total, 0);

    // Database intact after injection probes
    let r_after = client
        .search_automation_apps(Some("Safe Catalog"), None, None, None, None)
        .await
        .expect("normal query after probes");
    assert_eq!(r_after.total, 1);

    drop_client_async(client).await;
    server.stop().await;
}

/// [Test ID] TEST-BR-T2-F5-01
/// [Feature / Req] F5 / R2 — `max_steps_per_run` terminates unchanging loop with `StepFailed` (`"budget exhausted"`) and `Idle`
/// [Preconditions] `MockWebAppServer` started; `BrowserSessionRegistry` configured with `max_steps_per_run: 3`; Chrome backend registered.
/// [Opaque-Box Steps] Mock LLM issues 4 distinct `browser_navigate` / `browser_scroll` calls on the same session.
/// [Observable Assertions] Steps 1..=3 succeed, step 4 fails with `"budget exhausted"`, `StepFailed` event is emitted, and `sess.control_mode() == BrowserControlMode::Idle`.
#[tokio::test]
async fn test_br_t2_f5_01_step_budget_exhaustion_terminates_loop_cleanly() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f5_01").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let reg = Arc::new(BrowserSessionRegistry::with_config(BrowserSessionConfig {
        max_steps_per_run: 3,
        auto_pause_on_sensitive_origin: false,
        event_channel_capacity: 64,
    }));
    let sess = reg.register_backend(
        "tab_t2_f5_01",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );
    let mut ev_rx = reg.subscribe();

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.set_browser_registry(reg);

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call("s1", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_tool_call("s2", "browser_scroll", json!({ "delta_x": 0.0, "delta_y": 25.0 })),
        mock_openai_tool_call("s3", "browser_scroll", json!({ "delta_x": 0.0, "delta_y": 50.0 })),
        mock_openai_tool_call("s4", "browser_scroll", json!({ "delta_x": 0.0, "delta_y": 75.0 })),
        mock_openai_text("Stopped at step budget."),
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
        .run_turn(full_turn_req("Exceed step budget", "tab_t2_f5_01"))
        .await
        .expect("turn finishes");
    assert_eq!(res.tool_calls.len(), 4);
    assert!(res.tool_calls[0].success);
    assert!(res.tool_calls[1].success);
    assert!(res.tool_calls[2].success);
    assert!(!res.tool_calls[3].success);
    assert!(res.tool_calls[3].output.contains("budget exhausted"));
    assert_eq!(sess.control_mode(), BrowserControlMode::Idle);

    let mut saw_step_failed = false;
    while let Ok(ev) = ev_rx.try_recv() {
        if let BrowserAutomationEvent::StepFailed { error, .. } = ev {
            if error.contains("budget exhausted") {
                saw_step_failed = true;
            }
        }
    }
    assert!(saw_step_failed);

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F5-02
/// [Feature / Req] F5 / R2 — Missing or closed `session_id` in `BrowserSessionRegistry` returns clean tool error without corrupting active sessions
/// [Preconditions] `MockWebAppServer` started; `BrowserSessionRegistry` with one active session (`"healthy_tab"`) and NO `active_session_id` fallback nor orchestrator; `"closed_tab"` registered then unregistered.
/// [Opaque-Box Steps] Execute `browser_navigate` targeting `"missing_tab"` and `"closed_tab"` via `HermesToolExecutor`, then execute `browser_navigate` targeting `"healthy_tab"`.
/// [Observable Assertions] Missing and unregistered targets return `success == false` tool errors without panicking, while `"healthy_tab"` continues operating cleanly.
#[tokio::test]
async fn test_br_t2_f5_02_missing_and_closed_session_id_errors_cleanly_without_corrupting_other_sessions() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f5_02").expect("temp dir");
    let backend_healthy = Arc::new(launch_chrome_in(temp.path()).await);
    let temp_closed = isolated_temp_dir("t2_f5_02_closed").expect("closed dir");
    let backend_closed = Arc::new(launch_chrome_in(temp_closed.path()).await);

    let reg = Arc::new(BrowserSessionRegistry::new());
    reg.register_backend(
        "healthy_tab",
        Arc::clone(&backend_healthy) as Arc<dyn BrowserSessionBackend>,
    );
    reg.register_backend(
        "closed_tab",
        Arc::clone(&backend_closed) as Arc<dyn BrowserSessionBackend>,
    );
    assert!(reg.unregister_backend("closed_tab").is_some());

    let ctx = HermesToolContext::new(temp.path())
        .with_session("missing_tab", 1)
        .with_browser_sessions(Arc::clone(&reg));

    let err_missing = HermesToolExecutor::execute(
        "browser_navigate",
        json!({ "session_id": "missing_tab", "url": server.url("/") }),
        &ctx,
    )
    .await;
    assert!(!err_missing.success);

    let err_closed = HermesToolExecutor::execute(
        "browser_navigate",
        json!({ "session_id": "closed_tab", "url": server.url("/") }),
        &ctx,
    )
    .await;
    assert!(!err_closed.success);

    // Healthy session in the same registry remains completely unaffected
    let ok_healthy = HermesToolExecutor::execute(
        "browser_navigate",
        json!({ "session_id": "healthy_tab", "url": server.url("/item/item-03") }),
        &ctx,
    )
    .await;
    assert!(ok_healthy.success, "healthy session failed: {}", ok_healthy.output);
    assert!(
        backend_healthy
            .current_url()
            .await
            .expect("current_url")
            .contains("/item/item-03")
    );

    backend_healthy.close().await.expect("close healthy");
    backend_closed.close().await.expect("close closed");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F5-03
/// [Feature / Req] F5 / R2 — `MockWebAppServer` `/checkout` `422` validation error captured cleanly without crashing `BrowserAutomationSession`
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI navigates `/checkout` (empty cart), clicks `checkout-submit` (server responds HTTP `422` HTML), waits for `validation-errors`, extracts `validation-errors`, then navigates `/` cleanly.
/// [Observable Assertions] Extracted error text includes `"Cart cannot be empty"` and `"Customer name is required"`, trace completes intact, and subsequent navigation succeeds on the same session.
#[tokio::test]
async fn test_br_t2_f5_03_http_422_validation_error_captured_without_crashing_session() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f5_03").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t2_f5_03",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call(
            "v1",
            "browser_navigate",
            json!({ "url": server.url("/checkout") }),
        ),
        mock_openai_tool_call(
            "v2",
            "browser_click",
            json!({ "test_id": "checkout-submit" }),
        ),
        mock_openai_tool_call(
            "v3",
            "browser_wait_for",
            json!({ "test_id": "validation-errors", "timeout_ms": 4000 }),
        ),
        mock_openai_tool_call(
            "v4",
            "browser_extract_text",
            json!({ "test_id": "validation-errors", "output_key": "checkout_errors" }),
        ),
        mock_openai_tool_call("v5", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_text("Captured 422 validation errors cleanly."),
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
            "Submit empty checkout and read errors",
            "tab_t2_f5_03",
        ))
        .await
        .expect("run_turn");
    assert!(res.tool_calls.iter().all(|tc| tc.success));
    assert!(res.tool_calls[3].output.contains("Cart cannot be empty"));
    assert!(
        res.tool_calls[3]
            .output
            .contains("Customer name is required")
    );
    assert_eq!(sess.control_mode(), BrowserControlMode::Idle);
    assert!(sess.last_completed_trace().is_some());
    assert!(
        backend
            .current_url()
            .await
            .expect("current_url")
            .starts_with(&server.base_url())
    );

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F5-04
/// [Feature / Req] F5 / R2 — Concurrent AI run and replay rejected on active `AiControlling` locked session
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] Acquire `sess.try_begin_ai_turn()` lock; concurrently invoke `engine.run_turn` and `host.replay_browser_script` on the locked `session_id`; then release guard and run a subsequent turn.
/// [Observable Assertions] Overlapping `run_turn` tool call and `replay_browser_script` both reject immediately with `"locked"` error without interleaving; after releasing guard, a new AI turn succeeds.
#[tokio::test]
async fn test_br_t2_f5_04_concurrent_ai_run_and_replay_rejected_on_locked_session() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f5_04").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t2_f5_04",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );

    let mut outer_guard = sess.try_begin_ai_turn().expect("outer lock acquires");
    assert!(sess.is_ai_locked());

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call("l1", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_text("Rejected due to lock."),
        mock_openai_tool_call("l2", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_text("Succeeded after unlock."),
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

    let locked_turn = engine
        .run_turn(full_turn_req("Attempt overlapping run", "tab_t2_f5_04"))
        .await
        .expect("turn returns tool error");
    assert_eq!(locked_turn.tool_calls.len(), 1);
    assert!(!locked_turn.tool_calls[0].success);
    assert!(locked_turn.tool_calls[0].output.contains("locked"));

    let dummy_app = build_clean_boundary_app("dummy-lock", "dummy-lock", "Dummy", "1.0.0");
    let replay_err = host
        .replay_browser_script("tab_t2_f5_04", &dummy_app, &BTreeMap::new(), None)
        .await
        .unwrap_err();
    assert!(replay_err.to_string().contains("locked"));

    outer_guard.finish();
    drop(outer_guard);
    assert!(!sess.is_ai_locked());

    let unlocked_turn = engine
        .run_turn(full_turn_req("Run after unlock", "tab_t2_f5_04"))
        .await
        .expect("turn after unlock");
    assert!(unlocked_turn.tool_calls[0].success);

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F5-05
/// [Feature / Req] F5 / R2 — >25-step trace (28 distinct steps) streams monotonic `StepCompleted` (`0..28`) with zero drops
/// [Preconditions] `MockWebAppServer` started; `BrowserSessionRegistry` with `event_channel_capacity: 256`; Chrome session configured with `set_max_steps_per_run(64)`; `req.max_iterations = Some(40)`.
/// [Opaque-Box Steps] Mock LLM executes 1 `browser_navigate` + 27 distinct `browser_scroll` steps (varying `delta_y` to avoid `ToolGuardrailTracker` 5-repeat short-circuit), collecting events via `subscribe_mpsc(256)`.
/// [Observable Assertions] All 28 `StepCompleted` events arrive in strict `step_index` sequence `0..28` with zero dropped events.
#[tokio::test]
async fn test_br_t2_f5_05_large_28_step_trace_streams_monotonic_events_zero_drops() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f5_05").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let reg = Arc::new(BrowserSessionRegistry::with_config(BrowserSessionConfig {
        max_steps_per_run: 64,
        auto_pause_on_sensitive_origin: false,
        event_channel_capacity: 256,
    }));
    let sess = reg.register_backend(
        "tab_t2_f5_05",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );
    sess.set_max_steps_per_run(64);
    let (mut mpsc_rx, fwd_handle) = sess.subscribe_mpsc(256);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.set_browser_registry(reg);

    let mut responses = Vec::with_capacity(29);
    responses.push(mock_openai_tool_call(
        "step_0",
        "browser_navigate",
        json!({ "url": server.url("/") }),
    ));
    for i in 1..28usize {
        responses.push(mock_openai_tool_call(
            &format!("step_{i}"),
            "browser_scroll",
            json!({ "delta_x": 0.0, "delta_y": 10.0 + i as f64 }),
        ));
    }
    responses.push(mock_openai_text("Completed 28 steps."));

    let llm = MockOpenAiServer::start(responses)
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

    let mut req = full_turn_req("Execute 28 browser steps", "tab_t2_f5_05");
    req.max_iterations = Some(40);
    let res = engine.run_turn(req).await.expect("28-step turn succeeds");
    assert_eq!(res.tool_calls.len(), 28);
    assert!(res.tool_calls.iter().all(|tc| tc.success));

    tokio::time::sleep(Duration::from_millis(30)).await;
    let mut completed_indices = Vec::new();
    while let Ok(ev) = mpsc_rx.try_recv() {
        if let BrowserAutomationEvent::StepCompleted { step_index, .. } = ev {
            completed_indices.push(step_index);
        }
    }
    let expected: Vec<usize> = (0..28).collect();
    assert_eq!(completed_indices, expected);

    fwd_handle.abort();
    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F6-01
/// [Feature / Req] F6 / R2, AC2 — Invalid FSM transitions (`user_resume_ai`/`user_take_over` while `Idle`, double `user_take_over`) handled safely
/// [Preconditions] `BrowserControlGate` created in `Idle`; `BrowserAutomationSession` created with `MockWebAppServer` + Chrome backend.
/// [Opaque-Box Steps] Call `user_resume_ai()` and `user_take_over()` while `Idle`; start AI (`start_ai`), call `user_take_over()` twice in a row, then `user_resume_ai()` twice, then `complete_ai()`.
/// [Observable Assertions] No panics occur; `Idle` remains `Idle` on invalid transitions; double `user_take_over()` stays `UserTakeover`; gate finishes cleanly in `Idle`.
#[tokio::test]
async fn test_br_t2_f6_01_invalid_fsm_transitions_are_safe_idempotent_no_ops() {
    let gate = BrowserControlGate::new();
    assert_eq!(gate.mode(), BrowserControlMode::Idle);

    // Calling resume or take_over while Idle must be a safe no-op
    gate.user_resume_ai();
    assert_eq!(gate.mode(), BrowserControlMode::Idle);
    gate.user_take_over();
    assert_eq!(gate.mode(), BrowserControlMode::Idle);

    // Double user_take_over during AiControlling
    gate.start_ai("working");
    gate.set_active_step(2, "clicking");
    gate.user_take_over();
    assert_eq!(
        gate.mode(),
        BrowserControlMode::UserTakeover { paused_at_step: 2 }
    );
    gate.user_take_over();
    assert_eq!(
        gate.mode(),
        BrowserControlMode::UserTakeover { paused_at_step: 2 }
    );

    // Double user_resume_ai
    gate.user_resume_ai();
    assert!(matches!(
        gate.mode(),
        BrowserControlMode::AiControlling { .. }
    ));
    gate.user_resume_ai();
    assert!(matches!(
        gate.mode(),
        BrowserControlMode::AiControlling { .. }
    ));

    gate.complete_ai();
    assert_eq!(gate.mode(), BrowserControlMode::Idle);
}

/// [Test ID] TEST-BR-T2-F6-02
/// [Feature / Req] F6 / R2, AC2 — `user_cancel_ai` during `WaitingForTakeover` wakes `Notify` immediately, restores `Idle`, and allows next turn
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] Start AI turn calling `browser_request_takeover`; wait for `WaitingForTakeover`; call `sess.user_cancel_ai()`; measure wakeup latency; run a second AI turn navigating `/`.
/// [Observable Assertions] Suspended AI task wakes and exits within 2s without hanging `Notify`, `control_mode()` is `Idle`, and subsequent AI turn succeeds.
#[tokio::test]
async fn test_br_t2_f6_02_user_cancel_during_waiting_for_takeover_wakes_notify_and_recovers() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f6_02").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t2_f6_02",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call(
            "k1",
            "browser_request_takeover",
            json!({ "reason": "Need user check", "wait_for_finish": true }),
        ),
        mock_openai_text("Aborted."),
        mock_openai_tool_call("k2", "browser_navigate", json!({ "url": server.url("/") })),
        mock_openai_text("Recovered cleanly."),
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
            .run_turn(full_turn_req("Request takeover", "tab_t2_f6_02"))
            .await
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !matches!(
        sess.control_mode(),
        BrowserControlMode::WaitingForTakeover { .. }
    ) {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let cancel_started = Instant::now();
    sess.user_cancel_ai();
    let res = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("Notify must wake immediately on cancel")
        .expect("join")
        .expect("turn returns result");
    assert!(cancel_started.elapsed() < Duration::from_secs(2));
    assert!(!res.tool_calls[0].success);
    assert_eq!(sess.control_mode(), BrowserControlMode::Idle);

    // Subsequent turn succeeds cleanly
    let next = engine
        .run_turn(full_turn_req("Navigate after cancel", "tab_t2_f6_02"))
        .await
        .expect("subsequent turn succeeds");
    assert!(next.tool_calls[0].success);

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F6-03
/// [Feature / Req] F6 / R3 — `/distill` and `distill_browser_script` on empty or cancelled-empty trace return actionable error
/// [Preconditions] Chrome `CdpBrowserBackend` registered in `FabushiBotHost` with zero recorded actions.
/// [Opaque-Box Steps] Call `host.distill_browser_script` and `/distill Empty App` on empty session; also trigger a cancelled takeover turn with no actions and re-try `/distill` & `distill_browser_script`.
/// [Observable Assertions] `host.distill_browser_script` returns `Err(BotBrowserError::ScriptCompilation(_))`, `/distill` produces no `DistillBrowserScript` UI effect and returns an informative error message.
#[tokio::test]
async fn test_br_t2_f6_03_distill_on_empty_or_cancelled_trace_returns_actionable_error() {
    let temp = isolated_temp_dir("t2_f6_03").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t2_f6_03",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );

    let err_api = host
        .distill_browser_script(
            "tab_t2_f6_03",
            ScriptAppMetadata {
                name: "Should Fail".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(err_api, BotBrowserError::ScriptCompilation(_)));

    let engine = host.core_engine();
    let cmd_empty = engine
        .run_turn(HermesTurnRequest::new("/distill Empty Trace").with_session("tab_t2_f6_03"))
        .await
        .expect("/distill command returns outcome");
    assert!(
        !cmd_empty
            .ui_effects
            .iter()
            .any(|e| matches!(e, gpui_telegram_agent::CommandUiEffect::DistillBrowserScript { .. })),
        "empty /distill must not emit DistillBrowserScript: {:?}",
        cmd_empty.ui_effects
    );
    assert!(
        cmd_empty
            .response_text
            .to_ascii_lowercase()
            .contains("no browser actions")
            || cmd_empty.response_text.to_ascii_lowercase().contains("failed")
    );

    // Cancelled empty trace (only cancel, no recorded browser steps)
    sess.user_cancel_ai();
    sess.reset_control();
    let err_after_cancel = host
        .distill_browser_script(
            "tab_t2_f6_03",
            ScriptAppMetadata {
                name: "Still Empty".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(
        err_after_cancel,
        BotBrowserError::ScriptCompilation(_)
    ));

    backend.close().await.expect("close backend");
}

/// [Test ID] TEST-BR-T2-F6-04
/// [Feature / Req] F6 / R2, AC2 — Long `UserTakeover` with multiple manual navigations refreshes live `refreshed_url` & DOM on `user_resume_ai`
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI navigates `/search?q=Rust`, user invokes `user_take_over()` before AI's next `browser_snapshot` step; user manually navigates Chrome across `/` -> `/item/item-01` -> `/checkout`; user invokes `user_resume_ai()`.
/// [Observable Assertions] Resumed `browser_snapshot` tool structured payload reports `resumed_from_takeover == true` and `refreshed_url` containing `/checkout` (not stale `/search`), and snapshot text reflects Checkout DOM.
#[tokio::test]
async fn test_br_t2_f6_04_long_takeover_multiple_manual_navigations_refreshes_live_dom() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f6_04").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    let sess = host.register_browser_session(
        "tab_t2_f6_04",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );

    let ctx = HermesToolContext::new(temp.path())
        .with_session("tab_t2_f6_04", 1)
        .with_browser_sessions(host.browser_registry())
        .with_browser_turn_scope();

    let s1 = HermesToolExecutor::execute(
        "browser_navigate",
        json!({ "url": server.url("/search?q=Rust") }),
        &ctx,
    )
    .await;
    assert!(s1.success);

    sess.user_take_over();
    let ctx_next = ctx.clone();
    let resumed_step = tokio::spawn(async move {
        HermesToolExecutor::execute("browser_snapshot", json!({}), &ctx_next).await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!resumed_step.is_finished());

    // Multiple manual Chrome navigations while paused in UserTakeover
    backend.navigate(&server.url("/")).await.expect("nav /");
    backend
        .navigate(&server.url("/item/item-01"))
        .await
        .expect("nav item-01");
    backend
        .navigate(&server.url("/checkout"))
        .await
        .expect("nav checkout");

    sess.user_resume_ai();

    let s2 = resumed_step.await.expect("join");
    assert!(s2.success, "{}", s2.output);
    let structured = s2.structured.as_ref().expect("structured payload");
    assert_eq!(structured["resumed_from_takeover"], true);
    let refreshed_url = structured["refreshed_url"].as_str().unwrap_or("");
    assert!(
        refreshed_url.contains("/checkout"),
        "expected /checkout, got {refreshed_url}"
    );
    assert!(s2.output.contains("Checkout"));

    drop(ctx);
    backend.close().await.expect("close backend");
    server.shutdown().await;
}

/// [Test ID] TEST-BR-T2-F6-05
/// [Feature / Req] F6 / R3 — Failed tool retries followed by recovery action omit dead failed step from distilled Script App
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` registered in `FabushiBotHost`.
/// [Opaque-Box Steps] AI navigates `/item/item-01`, fails `browser_click` on `"nonexistent-btn-id"`, recovers with valid `browser_click` (`"add-to-cart-btn"`), waits for `cart-status`, distills via `host.distill_browser_script`, and replays via `host.replay_browser_script`.
/// [Observable Assertions] Distilled `AutomationScriptApp` contains NO reference to `"nonexistent-btn-id"` and replays cleanly (`success == true`, `used_llm == false`).
#[tokio::test]
async fn test_br_t2_f6_05_failed_tool_retries_omitted_from_distilled_script_and_replays_cleanly() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let temp = isolated_temp_dir("t2_f6_05").expect("temp dir");
    let backend = Arc::new(launch_chrome_in(temp.path()).await);

    let host = FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("host");
    host.register_browser_session(
        "tab_t2_f6_05",
        Arc::clone(&backend) as Arc<dyn BrowserSessionBackend>,
    );

    let llm = MockOpenAiServer::start(vec![
        mock_openai_tool_call(
            "r1",
            "browser_navigate",
            json!({ "url": server.url("/item/item-01") }),
        ),
        mock_openai_tool_call(
            "r2",
            "browser_click",
            json!({ "test_id": "nonexistent-btn-id" }),
        ),
        mock_openai_tool_call(
            "r3",
            "browser_click",
            json!({ "test_id": "add-to-cart-btn" }),
        ),
        mock_openai_tool_call(
            "r4",
            "browser_wait_for",
            json!({ "test_id": "cart-status", "timeout_ms": 4000 }),
        ),
        mock_openai_text("Recovered and added to cart."),
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
            "Click button with retry recovery",
            "tab_t2_f6_05",
        ))
        .await
        .expect("turn completes");
    assert_eq!(res.tool_calls.len(), 4);
    assert!(res.tool_calls[0].success);
    assert!(!res.tool_calls[1].success);
    assert!(res.tool_calls[2].success);
    assert!(res.tool_calls[3].success);

    let app = host
        .distill_browser_script(
            "tab_t2_f6_05",
            ScriptAppMetadata {
                name: "Clean Recovery App".into(),
                category: "productivity".into(),
                ..Default::default()
            },
        )
        .expect("distill recovery trace");

    let app_json = serde_json::to_string(&app).unwrap();
    assert!(
        !app_json.contains("nonexistent-btn-id"),
        "failed tool step must not appear in distilled app: {app_json}"
    );

    let llm_calls_before = llm.call_count();
    let report = host
        .replay_browser_script("tab_t2_f6_05", &app, &BTreeMap::new(), None)
        .await
        .expect("replay clean recovery script");
    assert!(report.success);
    assert!(!report.used_llm);
    assert_eq!(llm.call_count(), llm_calls_before);

    llm.shutdown().await;
    backend.close().await.expect("close backend");
    server.shutdown().await;
}

// ============================================================================
// F7 & F8 Headless GPUI Subprocess Harness + 10 Tier 2 Boundary Tests
// ============================================================================

const T2_CHILD_ENV: &str = "GPUI_E2E_T2_HEADLESS_CHILD";
const T2_CHILD_TARGET_ENV: &str = "GPUI_E2E_T2_HEADLESS_TARGET";
const T2_CHILD_AFTER_CTORS_ENV: &str = "GPUI_E2E_T2_HEADLESS_AFTER_CTORS";

fn t2_dispatch_headless_target(target: &str, use_underscore_exit: bool) -> ! {
    type AsyncBody = fn(gpui::AsyncApp) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>;
    let body: AsyncBody = match target {
        "test_br_t2_f7_01_malformed_blank_unreachable_url_recovery" => {
            |app| Box::pin(body_br_t2_f7_01(app))
        }
        "test_br_t2_f7_02_rapid_navigations_and_zero_bounds_resize" => {
            |app| Box::pin(body_br_t2_f7_02(app))
        }
        "test_br_t2_f7_03_ui_tick_latency_under_slow_backend_step" => {
            |app| Box::pin(body_br_t2_f7_03(app))
        }
        "test_br_t2_f7_04_rapid_takeover_toggling_syncs_with_control_gate" => {
            |app| Box::pin(body_br_t2_f7_04(app))
        }
        "test_br_t2_f7_05_nav_destination_switch_preserves_browser_state" => {
            |app| Box::pin(body_br_t2_f7_05(app))
        }
        "test_br_t2_f8_01_distill_empty_trace_blank_name_and_duplicate_slug" => {
            |app| Box::pin(body_br_t2_f8_01(app))
        }
        "test_br_t2_f8_02_missing_required_param_aborts_cleanly" => {
            |app| Box::pin(body_br_t2_f8_02(app))
        }
        "test_br_t2_f8_03_publish_unreachable_and_422_secret_rejection_retains_draft" => {
            |app| Box::pin(body_br_t2_f8_03(app))
        }
        "test_br_t2_f8_04_malformed_cookie_json_and_tcc_eperm_via_workspace" => {
            |app| Box::pin(body_br_t2_f8_04(app))
        }
        "test_br_t2_f8_05_store_nonexistent_idempotent_install_and_invalid_run" => {
            |app| Box::pin(body_br_t2_f8_05(app))
        }
        other => {
            eprintln!("unknown e2e tier2 headless target: {other}");
            e2e_suite::gpui_browser_harness::exit_headless_child(101, use_underscore_exit);
        }
    };
    e2e_suite::gpui_browser_harness::run_headless_gpui_body(target, use_underscore_exit, body);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_T2_HEADLESS_MAIN_THREAD_ENTRY: extern "C" fn() = t2_macos_mod_init_headless_entry;

#[cfg(target_os = "macos")]
extern "C" fn t2_macos_atexit_headless_entry() {
    if let Ok(target) = std::env::var(T2_CHILD_TARGET_ENV) {
        t2_dispatch_headless_target(&target, true);
    }
}

#[cfg(target_os = "macos")]
extern "C" fn t2_macos_mod_init_headless_entry() {
    if std::env::var(T2_CHILD_ENV).as_deref() == Ok("1") {
        if std::env::var(T2_CHILD_AFTER_CTORS_ENV).as_deref() == Ok("1") {
            unsafe extern "C" {
                fn atexit(func: extern "C" fn()) -> i32;
            }
            unsafe {
                atexit(t2_macos_atexit_headless_entry);
            }
            return;
        }
        if let Ok(target) = std::env::var(T2_CHILD_TARGET_ENV) {
            t2_dispatch_headless_target(&target, false);
        }
    }
}

fn run_t2_in_headless_subprocess(test_fn_name: &str) {
    if std::env::var(T2_CHILD_ENV).as_deref() == Ok("1") {
        t2_dispatch_headless_target(test_fn_name, false);
    }
    e2e_suite::gpui_browser_harness::spawn_headless_subprocess(
        T2_CHILD_ENV,
        T2_CHILD_TARGET_ENV,
        T2_CHILD_AFTER_CTORS_ENV,
        test_fn_name,
        false,
        None,
        None,
    );
}

fn run_t2_in_headless_subprocess_with_window(test_fn_name: &str) {
    if std::env::var(T2_CHILD_ENV).as_deref() == Ok("1") {
        t2_dispatch_headless_target(test_fn_name, false);
    }
    e2e_suite::gpui_browser_harness::spawn_headless_subprocess(
        T2_CHILD_ENV,
        T2_CHILD_TARGET_ENV,
        T2_CHILD_AFTER_CTORS_ENV,
        test_fn_name,
        true,
        None,
        None,
    );
}

/// [Test ID] TEST-BR-T2-F7-01
/// [Feature / Req] F7 / R1, AC2 — Malformed, blank, and unreachable URLs in `BrowserWorkspace` do not crash GPUI or lose prior valid URL, and subsequent valid navigation recovers cleanly
/// [Preconditions] `MockWebAppServer` started; Chrome `CdpBrowserBackend` attached to `AppShell`.
/// [Opaque-Box Steps] Navigate to valid `/`, submit blank/whitespace address bar input, navigate to `"javascript:alert(1)"`, `"http://[invalid"`, and `"http://127.0.0.1:1/unreachable"`, then navigate to `/search?q=Rust`.
/// [Observable Assertions] Blank input is a safe no-op; invalid schemes/URLs populate `last_error` while retaining prior valid `current_url` / rendered frame; valid recovery clears `last_error`.
#[test]
fn test_br_t2_f7_01_malformed_blank_unreachable_url_recovery() {
    run_t2_in_headless_subprocess("test_br_t2_f7_01_malformed_blank_unreachable_url_recovery");
}

async fn body_br_t2_f7_01(mut app: gpui::AsyncApp) {
    use gpui_telegram_desktop::views::browser::ActiveBrowserInput;

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let web = rt.block_on(MockWebAppServer::start()).expect("MockWebAppServer");
    let chrome_dir = isolated_temp_dir("t2_f7_01").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                721,
                "t2_f7_01",
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

    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.is_loading && w.current_url == home_url && w.last_error.is_none()
        })
        .await
    );

    // Blank / whitespace address bar submission is a safe no-op
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::AddressBar, None, wcx);
                w.clear_active_input(wcx);
                w.insert_text_to_active_input("   ", wcx);
            });
            let _ = s.handle_non_char_key("enter", false, scx);
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::None, None, wcx);
                w.navigate("", wcx);
            });
            assert_eq!(bw.read(scx).current_url, home_url);
        });
    })
    .unwrap();

    // Malformed / blocked schemes surface last_error while keeping prior valid current_url
    for bad in ["javascript:alert(1)", "http://[invalid"] {
        app.update(|cx| {
            shell.update(cx, |s, scx| {
                s.browser_navigate(bad, scx);
            });
        })
        .unwrap();
        let settled = e2e_suite::gpui_browser_harness::pump_until(
            &mut app,
            Duration::from_secs(15),
            |cx| {
                let w = shell.read(cx).browser_workspace().unwrap().read(cx);
                !w.is_loading && w.last_error.is_some()
            },
        )
        .await;
        assert!(settled, "bad url `{bad}` must set last_error without hanging");
        app.update(|cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            assert_eq!(w.current_url, home_url, "prior valid URL must stay intact");
        })
        .unwrap();
    }

    // Unreachable closed port does not crash GPUI
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_navigate("http://127.0.0.1:1/unreachable", scx);
        });
    })
    .unwrap();
    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
            !shell.read(cx).browser_workspace().unwrap().read(cx).is_loading
        })
        .await
    );

    // Subsequent valid navigation recovers and clears last_error
    let search_url = web.url("/search?q=Rust");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_navigate(&search_url, scx);
        });
    })
    .unwrap();
    let recovered = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.is_loading && w.last_error.is_none() && w.current_url == search_url
        },
    )
    .await;
    assert!(recovered, "valid navigation after errors must succeed and clear last_error");

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(web.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T2-F7-02
/// [Feature / Req] F7 / R1, AC3 — Rapid sequential navigations and `0x0 -> 1440x900` degenerate bounds resize in `CdpViewportSurface`
/// [Preconditions] Headless GPUI window opened at `0x0` (`WindowBounds::Windowed(0,0,0,0)`); `MockWebAppServer` & Chrome `CdpBrowserBackend` active.
/// [Opaque-Box Steps] Draw window at `0x0`, verify `set_viewport_size(0, 0)`, `set_viewport_bounds_for_test` `0x0`, and `map_point_contain(..., (0,0), (0,0))` return finite coords; fire 3 rapid sequential `navigate` calls, resize window + surface to `1440x900`, and redraw.
/// [Observable Assertions] Zero panic or division-by-zero on `0x0`; viewport converges cleanly on the 3rd URL with a non-empty `RenderImage` at `1440x900`.
#[test]
fn test_br_t2_f7_02_rapid_navigations_and_zero_bounds_resize() {
    run_t2_in_headless_subprocess_with_window(
        "test_br_t2_f7_02_rapid_navigations_and_zero_bounds_resize",
    );
}

async fn body_br_t2_f7_02(mut app: gpui::AsyncApp) {
    use gpui::{bounds, point, px, size, AppContext, WindowBounds, WindowOptions};
    use gpui_telegram_desktop::views::browser::map_point_contain;
    use gpui_telegram_desktop::views::shell::AppShell;

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let web = rt.block_on(MockWebAppServer::start()).expect("MockWebAppServer");
    let chrome_dir = isolated_temp_dir("t2_f7_02").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let win_handle = app
        .update(|cx| {
            let _rt_guard = client.runtime().enter();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds(
                        point(px(0.0), px(0.0)),
                        size(px(0.0), px(0.0)),
                    ))),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let mut s = AppShell::new_with_client(client.clone(), cx);
                        s.is_authenticated = true;
                        s.session_sync.user_id = Some(722);
                        s.session_sync.username = Some("t2_f7_02".to_string());
                        s.session_sync.session_token = Some("tok_t2_f7_02".to_string());
                        s.magpie_local.bootstrapped_auth_generation =
                            Some(s.session_sync.auth_generation);
                        s.set_browser_backend_for_test(
                            Arc::clone(&chrome) as Arc<dyn BrowserSessionBackend>,
                            cx,
                        );
                        s
                    })
                },
            )
            .expect("open 0x0 headless window")
        })
        .unwrap();

    let shell_ent = win_handle.entity(&app).unwrap();
    app.update_window(win_handle.into(), |_, window, cx| {
        let _ = window.draw(cx);
        let surf = shell_ent
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |sr, scx| {
            sr.set_viewport_size(0, 0, scx);
            assert_eq!(sr.viewport_size(), (1, 1));
            sr.set_viewport_bounds_for_test(Some(bounds(
                point(px(0.0), px(0.0)),
                size(px(0.0), px(0.0)),
            )));
            let (vx, vy) = sr.map_window_point_to_viewport(point(px(10.0), px(10.0)));
            assert!(vx.is_finite() && vy.is_finite());
        });
        let (cx0, cy0) = map_point_contain(
            point(px(10.0), px(10.0)),
            bounds(point(px(0.0), px(0.0)), size(px(0.0), px(0.0))),
            (0, 0),
            (0, 0),
        );
        assert!(cx0.is_finite() && cy0.is_finite());
    })
    .unwrap();

    let url1 = web.url("/");
    let url2 = web.url("/search?q=books");
    let url3 = web.url("/item/item-02");

    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            s.browser_navigate(&url1, scx);
            s.browser_navigate(&url2, scx);
            s.browser_navigate(&url3, scx);
            let surf = s
                .browser_workspace()
                .unwrap()
                .read(scx)
                .cdp_surface()
                .unwrap()
                .clone();
            surf.update(scx, |sr, rcx| {
                sr.set_viewport_bounds_for_test(None);
                sr.set_viewport_size(1440, 900, rcx);
            });
        });
        window.resize(size(px(1440.0), px(900.0)));
        let _ = window.draw(cx);
        let sr = shell_ent
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .read(cx);
        let (mx, my) = sr.map_window_point_to_viewport(point(px(100.0), px(100.0)));
        assert!(
            mx.is_finite()
                && my.is_finite()
                && (0.0..=1440.0).contains(&mx)
                && (0.0..=900.0).contains(&my),
            "non-zero window point mapping at 1440x900 must be finite and in bounds, got ({mx}, {my})"
        );
    })
    .unwrap();

    let converged = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let bw = shell_ent.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading
                && !sr.is_refreshing()
                && !sr.is_navigating()
                && bw.current_url == url3
                && sr.has_rendered_frame()
                && sr.viewport_size() == (1440, 900)
        },
    )
    .await;
    assert!(
        converged,
        "rapid sequential navigations + 0x0 -> 1440x900 resize must converge on url3"
    );

    app.update_window(win_handle.into(), |_, window, cx| {
        let _ = window.draw(cx);
        let sr = shell_ent
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .read(cx);
        let (mx, my) = sr.map_window_point_to_viewport(point(px(100.0), px(100.0)));
        assert!(
            mx.is_finite()
                && my.is_finite()
                && (0.0..=1440.0).contains(&mx)
                && (0.0..=900.0).contains(&my),
            "post-render window point mapping at 1440x900 must be finite and in bounds, got ({mx}, {my})"
        );
        window.remove_window();
    })
    .unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(web.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T2-F7-03
/// [Feature / Req] F7 / R1, AC3 — `BrowserWorkspace` UI tick latency remains bounded under simulated 300ms slow backend step
/// [Preconditions] `DelayedHtmlServer` serving `/slow` with 300ms latency; Chrome `CdpBrowserBackend` attached to `AppShell`.
/// [Opaque-Box Steps] Trigger `browser_navigate` to `/slow` and pump 15ms GPUI ticks until settled.
/// [Observable Assertions] `ticks >= 10` (including ticks while loading) and `max_gap_ms < 150`.
#[test]
fn test_br_t2_f7_03_ui_tick_latency_under_slow_backend_step() {
    run_t2_in_headless_subprocess("test_br_t2_f7_03_ui_tick_latency_under_slow_backend_step");
}

async fn body_br_t2_f7_03(mut app: gpui::AsyncApp) {
    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let delayed = rt
        .block_on(e2e_suite::gpui_browser_harness::DelayedHtmlServer::start(300))
        .expect("DelayedHtmlServer");
    let chrome_dir = isolated_temp_dir("t2_f7_03").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                723,
                "t2_f7_03",
            )
        })
        .unwrap();

    let slow_url = delayed.url("/slow");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(
                Arc::clone(&chrome) as Arc<dyn BrowserSessionBackend>,
                scx,
            );
            s.browser_navigate(&slow_url, scx);
        });
    })
    .unwrap();

    let (done, ticks, max_gap_ms) = e2e_suite::gpui_browser_harness::pump_with_ticks(
        &mut app,
        Duration::from_secs(15),
        Duration::from_millis(15),
        |cx, _| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.is_loading && w.current_url == slow_url && w.page_title.contains("Slow Loaded Page")
        },
    )
    .await;

    assert!(done, "slow backend step must complete");
    assert!(ticks >= 10, "expected >= 10 GPUI ticks during 300ms step, got {ticks}");
    assert!(
        max_gap_ms < 150,
        "GPUI UI tick latency exceeded threshold: {max_gap_ms}ms"
    );

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(delayed.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T2-F7-04
/// [Feature / Req] F7 / R1, R2 — Rapid takeover toggling (`Takeover -> Resume -> Takeover -> Cancel`) stays synchronized with `BrowserControlGate`
/// [Preconditions] `MockXServer` + `seed` + Chrome `CdpBrowserBackend` attached to `AppShell`; `AutomationScriptApp` with `HumanCheckpoint` launched.
/// [Opaque-Box Steps] Wait for `WaitingForTakeover`, then call `browser_user_takeover`, `browser_resume_ai`, `browser_user_takeover`, and `browser_cancel_ai` rapidly.
/// [Observable Assertions] Replay exits cleanly, `BrowserControlMode::Idle` is restored on both `AppShell` and `BrowserWorkspace`, and `CdpViewportSurface::is_interactive_enabled()` is true.
#[test]
fn test_br_t2_f7_04_rapid_takeover_toggling_syncs_with_control_gate() {
    run_t2_in_headless_subprocess(
        "test_br_t2_f7_04_rapid_takeover_toggling_syncs_with_control_gate",
    );
}

async fn body_br_t2_f7_04(mut app: gpui::AsyncApp) {
    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let x_server = rt.block_on(MockXServer::start()).expect("MockXServer");
    let chrome_dir = isolated_temp_dir("t2_f7_04").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                724,
                "t2_f7_04",
            )
        })
        .unwrap();

    let script = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "t2-f7-04-toggle".to_string(),
        slug: "t2-f7-04-toggle".to_string(),
        name: "Takeover Toggle".to_string(),
        version: "1.0.0".to_string(),
        description: "Test rapid toggle".to_string(),
        category: "social".to_string(),
        target_domains: vec!["127.0.0.1".to_string()],
        parameters: vec![],
        steps: vec![
            ScriptStep::Navigate {
                url_template: x_server.url("/challenge"),
            },
            ScriptStep::HumanCheckpoint {
                reason: "Toggle gate".to_string(),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator::by_test_id("tweetTextarea_0"),
                timeout_ms: 4_000,
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
            s.browser_resume_ai(scx);
            s.browser_user_takeover(scx);
            s.browser_cancel_ai(scx);
        });
    })
    .unwrap();

    let settled = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let s = shell.read(cx);
            let w = s.browser_workspace().unwrap().read(cx);
            !w.recorder.is_replaying && matches!(s.browser_control_mode(cx), BrowserControlMode::Idle)
        },
    )
    .await;
    assert!(settled, "rapid takeover toggling + cancel must settle in Idle");

    app.update(|cx| {
        let s = shell.read(cx);
        let w = s.browser_workspace().unwrap().read(cx);
        assert_eq!(w.control_mode, BrowserControlMode::Idle);
        assert_eq!(w.control_mode(), BrowserControlMode::Idle);
        assert!(w.cdp_surface().unwrap().read(cx).is_interactive_enabled());
    })
    .unwrap();

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(x_server.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T2-F7-05
/// [Feature / Req] F7 / R1 — Switching away from `NavDestination::Browser` and returning preserves URL, step logs, and control mode
/// [Preconditions] `MockWebAppServer` + Chrome `CdpBrowserBackend` attached to `AppShell`.
/// [Opaque-Box Steps] Navigate to `/search?q=Rust`, push step log, enter `UserTakeover`, switch to `NavDestination::Chats`, then return via shortcut `"9"`.
/// [Observable Assertions] Switching away sets `!is_workspace_visible()` & `is_externally_occluded()`; returning restores visibility and preserves `current_url`, `event_logs`, and `UserTakeover`.
#[test]
fn test_br_t2_f7_05_nav_destination_switch_preserves_browser_state() {
    run_t2_in_headless_subprocess(
        "test_br_t2_f7_05_nav_destination_switch_preserves_browser_state",
    );
}

async fn body_br_t2_f7_05(mut app: gpui::AsyncApp) {
    use gpui_telegram_desktop::views::browser::BrowserLogLevel;
    use gpui_telegram_desktop::views::nav_rail::NavDestination;
    use gpui_telegram_desktop::views::shell::ActiveWorkspacePanel;

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let web = rt.block_on(MockWebAppServer::start()).expect("MockWebAppServer");
    let chrome_dir = isolated_temp_dir("t2_f7_05").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                725,
                "t2_f7_05",
            )
        })
        .unwrap();

    let search_url = web.url("/search?q=Rust");
    let checkpoint_script = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "t2-f7-05-switch".to_string(),
        slug: "t2-f7-05-switch".to_string(),
        name: "Panel Switch Preserves State".to_string(),
        version: "1.0.0".to_string(),
        description: "Holds at checkpoint during nav switch".to_string(),
        category: "general".to_string(),
        target_domains: vec!["127.0.0.1".to_string()],
        parameters: vec![],
        steps: vec![
            ScriptStep::Navigate {
                url_template: search_url.clone(),
            },
            ScriptStep::HumanCheckpoint {
                reason: "Hold state during nav switch".to_string(),
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
            s.browser_run_script_app(checkpoint_script, BTreeMap::new(), scx);
        });
    })
    .unwrap();

    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
            let s = shell.read(cx);
            let w = s.browser_workspace().unwrap().read(cx);
            matches!(
                s.browser_control_mode(cx),
                BrowserControlMode::WaitingForTakeover { .. }
            ) && w.current_url == search_url
        })
        .await
    );

    let log_count_before = app
        .update(|cx| {
            shell.update(cx, |s, scx| {
                s.browser_user_takeover(scx);
                assert!(matches!(
                    s.browser_control_mode(scx),
                    BrowserControlMode::UserTakeover { .. }
                ));
                let bw = s.browser_workspace().unwrap().clone();
                bw.update(scx, |w, wcx| {
                    w.recorder.push_log(
                        BrowserLogLevel::Info,
                        "Preserved step",
                        "Should survive panel switch",
                        Some(12),
                    );
                    wcx.notify();
                });
                let count = bw.read(scx).recorder.event_logs.len();

                s.activate_nav_destination(NavDestination::Chats, scx);
                assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
                let r = bw.read(scx);
                assert!(!r.is_workspace_visible());
                assert!(r.is_externally_occluded());
                count
            })
        })
        .unwrap();

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            assert!(s.handle_shortcut_key("9", false, scx));
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
            let w = s.browser_workspace().unwrap().read(scx);
            assert!(w.is_workspace_visible());
            assert!(!w.is_externally_occluded());
            assert_eq!(w.current_url, search_url);
            assert_eq!(w.recorder.event_logs.len(), log_count_before);
            assert!(
                w.recorder
                    .event_logs
                    .iter()
                    .any(|e| e.title == "Preserved step")
            );
            assert!(matches!(
                w.control_mode(),
                BrowserControlMode::UserTakeover { .. }
            ));
            s.browser_cancel_ai(scx);
        });
    })
    .unwrap();

    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(15), |cx| {
            !shell
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .recorder
                .is_replaying
        })
        .await
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

/// [Test ID] TEST-BR-T2-F8-01
/// [Feature / Req] F8 / R3 — Empty trace, blank name validation (`ScriptCompiler`), empty UI name fallback, and duplicate slug disambiguation in `RecorderPanelState`
/// [Preconditions] `MockWebAppServer` + Chrome `CdpBrowserBackend` attached to `AppShell`.
/// [Opaque-Box Steps] Call `browser_distill_last_trace` on empty trace, call `ScriptCompiler::compile_from_trace` with blank name, call `browser_distill_last_trace("")` on recorded trace, then distill `"Duplicate Flow"` twice.
/// [Observable Assertions] Empty trace sets `distill_error`, blank compiler name returns `Err`, blank UI name falls back to `"Recorded Browser Script"`, and duplicate names produce distinct `app_id`s in `local_scripts`.
#[test]
fn test_br_t2_f8_01_distill_empty_trace_blank_name_and_duplicate_slug() {
    run_t2_in_headless_subprocess(
        "test_br_t2_f8_01_distill_empty_trace_blank_name_and_duplicate_slug",
    );
}

async fn body_br_t2_f8_01(app: gpui::AsyncApp) {
    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let web = rt.block_on(MockWebAppServer::start()).expect("MockWebAppServer");
    let chrome_dir = isolated_temp_dir("t2_f8_01").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                821,
                "t2_f8_01",
            )
        })
        .unwrap();

    let url = web.url("/");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(
                Arc::clone(&chrome) as Arc<dyn BrowserSessionBackend>,
                scx,
            );
            // 1. Empty trace must error
            let empty_res = s.browser_distill_last_trace("No Trace", "desc", "general", scx);
            assert!(empty_res.is_err());
            assert!(
                s.browser_workspace()
                    .unwrap()
                    .read(scx)
                    .recorder
                    .distill_error
                    .is_some()
            );

            // 2. Record a trace on the workspace's active automation_session
            let sess = s
                .browser_workspace()
                .unwrap()
                .read(scx)
                .automation_session()
                .unwrap()
                .clone();
            sess.record_navigate(&url, 10);
            let trace = sess.current_trace();
            let blank_err = ScriptCompiler::compile_from_trace(
                &trace,
                ScriptAppMetadata {
                    name: "   ".to_string(),
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert!(matches!(blank_err, BotBrowserError::ScriptCompilation(_)));

            // 3. UI empty name safely defaults to "Recorded Browser Script"
            let fallback = s
                .browser_distill_last_trace("", "Fallback name check", "general", scx)
                .expect("fallback name distill");
            assert_eq!(fallback.name, "Recorded Browser Script");
            assert_eq!(fallback.slug, "recorded-browser-script");

            // 4. Duplicate names are disambiguated by distinct UUID app_ids in local_scripts
            let dup1 = s
                .browser_distill_last_trace("Duplicate Flow", "First", "general", scx)
                .expect("dup1");
            let dup2 = s
                .browser_distill_last_trace("Duplicate Flow", "Second", "general", scx)
                .expect("dup2");
            assert_ne!(dup1.app_id, dup2.app_id);
            assert_eq!(dup1.slug, dup2.slug);

            let scripts = &s.browser_workspace().unwrap().read(scx).recorder.local_scripts;
            assert!(scripts.iter().any(|a| a.app_id == dup1.app_id));
            assert!(scripts.iter().any(|a| a.app_id == dup2.app_id));
        });
    })
    .unwrap();

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(web.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T2-F8-02
/// [Feature / Req] F8 / R3 — Running Script App with missing required parameter surfaces `replay_error` without mutating target page
/// [Preconditions] `MockWebAppServer` + Chrome `CdpBrowserBackend` attached to `AppShell`.
/// [Opaque-Box Steps] Run a Script App with `required: true, default_value: None` parameter `"required_buyer"` via both `browser_run_script_app` and `run_installed_store_app` with empty `param_inputs`.
/// [Observable Assertions] `recorder.replay_error` mentions `"missing required script parameter 'required_buyer'"`, `last_replay_report` stays `None`, and `MockWebAppServer::order_count() == 0`.
#[test]
fn test_br_t2_f8_02_missing_required_param_aborts_cleanly() {
    run_t2_in_headless_subprocess("test_br_t2_f8_02_missing_required_param_aborts_cleanly");
}

async fn body_br_t2_f8_02(mut app: gpui::AsyncApp) {
    use gpui_telegram_proto::automation_apps::InstalledAutomationApp;

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let web = rt.block_on(MockWebAppServer::start()).expect("MockWebAppServer");
    let chrome_dir = isolated_temp_dir("t2_f8_02").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                822,
                "t2_f8_02",
            )
        })
        .unwrap();

    let app_missing = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "t2-f8-02-missing-param".to_string(),
        slug: "t2-f8-02-missing-param".to_string(),
        name: "Missing Param Checkout".to_string(),
        version: "1.0.0".to_string(),
        description: "Requires required_buyer".to_string(),
        category: "productivity".to_string(),
        target_domains: vec!["127.0.0.1".to_string()],
        parameters: vec![ScriptParameter {
            name: "required_buyer".to_string(),
            description: "Required buyer name".to_string(),
            required: true,
            default_value: None,
            is_secret: false,
        }],
        steps: vec![
            ScriptStep::Navigate {
                url_template: web.url("/item/item-01"),
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("add-to-cart-btn"),
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("go-to-checkout"),
            },
            ScriptStep::Fill {
                locator: SemanticLocator::by_test_id("checkout-name"),
                value_template: "{{required_buyer}}".to_string(),
                secret_vault_key: None,
            },
            ScriptStep::Click {
                locator: SemanticLocator::by_test_id("checkout-submit"),
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
            s.browser_run_script_app(app_missing.clone(), BTreeMap::new(), scx);
        });
    })
    .unwrap();

    let errored1 = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.recorder.is_replaying && w.recorder.replay_error.is_some()
        },
    )
    .await;
    assert!(errored1);
    app.update(|cx| {
        let w = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert!(
            w.recorder
                .replay_error
                .as_deref()
                .unwrap()
                .contains("missing required script parameter 'required_buyer'")
        );
        assert!(w.recorder.last_replay_report.is_none());
    })
    .unwrap();
    assert_eq!(web.order_count(), 0);

    // Also verify run_installed_store_app with empty param_inputs fails identically
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.recorder.param_inputs.clear();
            w.run_installed_store_app(
                InstalledAutomationApp {
                    app_id: app_missing.app_id.clone(),
                    slug: app_missing.slug.clone(),
                    name: app_missing.name.clone(),
                    description: app_missing.description.clone(),
                    category: app_missing.category.clone(),
                    target_domains: app_missing.target_domains.clone(),
                    installed_version: app_missing.version.clone(),
                    latest_version: app_missing.version.clone(),
                    manifest: serde_json::to_value(&app_missing).unwrap(),
                    content_sha256: String::new(),
                    installed_at_ms: 1,
                    last_run_at_ms: None,
                    local_run_count: 0,
                },
                wcx,
            );
        });
    })
    .unwrap();

    let errored2 = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            !w.recorder.is_replaying && w.recorder.replay_error.is_some()
        },
    )
    .await;
    assert!(errored2);
    assert_eq!(web.order_count(), 0);

    app.update(|_| drop(shell)).unwrap();
    rt.block_on(e2e_suite::gpui_browser_harness::close_chrome_and_reap(
        &chrome,
        chrome_dir.path(),
    ))
    .expect("close chrome");
    rt.block_on(web.shutdown());
    drop(client);
}

/// [Test ID] TEST-BR-T2-F8-03
/// [Feature / Req] F8 / R4 — Publish failure on unreachable server & HTTP 422 secret rejection retain local draft in `RecorderPanelState` & `StorePanelState`
/// [Preconditions] Case A: unreachable port `http://127.0.0.1:9`; Case B: `EphemeralTestServer` with registered user and manifest embedding `fake_secrets().bearer_token`.
/// [Opaque-Box Steps] Attempt `browser_publish_script_app` in both Case A and Case B.
/// [Observable Assertions] Case A sets `publish_error` with `!publish_secret_rejected`; Case B sets `publish_secret_rejected == true` and `"422"` error; local drafts in `local_scripts` & `selected_draft_app` stay intact.
#[test]
fn test_br_t2_f8_03_publish_unreachable_and_422_secret_rejection_retains_draft() {
    run_t2_in_headless_subprocess(
        "test_br_t2_f8_03_publish_unreachable_and_422_secret_rejection_retains_draft",
    );
}

async fn body_br_t2_f8_03(mut app: gpui::AsyncApp) {
    let clean_draft = AutomationScriptApp {
        schema_version: SCRIPT_APP_SCHEMA_VERSION,
        app_id: "t2-f8-03-clean".to_string(),
        slug: "t2-f8-03-clean".to_string(),
        name: "Clean Local Draft".to_string(),
        version: "1.0.0".to_string(),
        description: "Must remain intact on failure".to_string(),
        category: "general".to_string(),
        target_domains: vec!["127.0.0.1".to_string()],
        parameters: vec![],
        steps: vec![ScriptStep::Navigate {
            url_template: "http://127.0.0.1:8080/".to_string(),
        }],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };

    // Case A: Unreachable server retains draft
    let (_unreach_dir, client_unreach) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let shell_a = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell_with_token(
                cx,
                client_unreach.clone(),
                823,
                "t2_f8_03_a",
                "tok_unreach",
            )
        })
        .unwrap();

    app.update(|cx| {
        shell_a.update(cx, |s, scx| {
            let bw = s.open_browser_workspace(scx);
            bw.update(scx, |w, _| {
                w.recorder.upsert_script(clean_draft.clone());
                w.store.selected_draft_app = Some(clean_draft.clone());
            });
            s.browser_publish_script_app(clean_draft.clone(), scx);
        });
    })
    .unwrap();

    let failed_unreach = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(20),
        |cx| {
            let w = shell_a.read(cx).browser_workspace().unwrap().read(cx);
            !w.store.is_publishing && w.store.publish_error.is_some()
        },
    )
    .await;
    assert!(failed_unreach);
    app.update(|cx| {
        let w = shell_a.read(cx).browser_workspace().unwrap().read(cx);
        assert!(!w.store.publish_secret_rejected);
        assert!(w.store.publish_success.is_none());
        assert!(w.recorder.local_scripts.iter().any(|a| a.app_id == clean_draft.app_id));
        assert_eq!(w.store.selected_draft_app.as_ref(), Some(&clean_draft));
    })
    .unwrap();
    app.update(|_| drop(shell_a)).unwrap();
    drop(client_unreach);

    // Case B: EphemeralTestServer HTTP 422 secret rejection
    let rt_boot = tokio::runtime::Runtime::new().unwrap();
    let mut server = rt_boot
        .block_on(EphemeralTestServer::start())
        .expect("EphemeralTestServer");
    let b_dir = isolated_temp_dir("t2_f8_03_b").expect("b dir");
    let client_b = e2e_suite::gpui_browser_harness::open_client_in_dir(&server.base_url(), b_dir.path());
    let auth_b = client_b
        .runtime()
        .block_on(client_b.register(&RegisterRequest {
            username: "t2_f8_03_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        }))
        .expect("register bob");

    let shell_b = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell_with_token(
                cx,
                client_b.clone(),
                auth_b.user_id,
                "t2_f8_03_bob",
                &auth_b.token,
            )
        })
        .unwrap();

    let mut leaky = clean_draft.clone();
    leaky.app_id = "t2-f8-03-leaky".to_string();
    leaky.slug = "t2-f8-03-leaky".to_string();
    leaky.description = format!("Authorization: Bearer {}", fake_secrets().bearer_token);

    app.update(|cx| {
        shell_b.update(cx, |s, scx| {
            let bw = s.open_browser_workspace(scx);
            bw.update(scx, |w, _| {
                w.recorder.upsert_script(clean_draft.clone());
                w.store.selected_draft_app = Some(leaky.clone());
            });
            s.browser_publish_script_app(leaky.clone(), scx);
        });
    })
    .unwrap();

    let rejected_422 = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(20),
        |cx| {
            let w = shell_b.read(cx).browser_workspace().unwrap().read(cx);
            !w.store.is_publishing && w.store.publish_secret_rejected
        },
    )
    .await;
    assert!(rejected_422, "server must reject secret-bearing manifest with 422");
    app.update(|cx| {
        let w = shell_b.read(cx).browser_workspace().unwrap().read(cx);
        assert!(w.store.publish_error.as_deref().unwrap().contains("422"));
        assert!(w.store.publish_success.is_none());
        assert!(w.recorder.local_scripts.iter().any(|a| a.app_id == clean_draft.app_id));
        assert_eq!(w.store.selected_draft_app.as_ref(), Some(&leaky));
    })
    .unwrap();

    app.update(|_| drop(shell_b)).unwrap();
    client_b.clear_session();
    drop(client_b);
    rt_boot.block_on(server.stop());
}

/// [Test ID] TEST-BR-T2-F8-04
/// [Feature / Req] F8 / R1, AC4 — Malformed cookie JSON & simulated macOS TCC `EPERM` (`chmod 0o000`) surface UI error/guidance without corrupting active cookies
/// [Preconditions] Subprocess spawned with `HOME` pointing at an isolated `TempDir` containing `0o000` Chrome `Cookies` file; valid `"stay_intact"` cookie seeded in Chrome backend.
/// [Opaque-Box Steps] Call `import_cookies_from_text_async("{bad json")`, call `probe_local_chrome`, and call `import_cookies_from_chrome_path` on the unreadable file.
/// [Observable Assertions] `cookie_error_message` is set on malformed JSON and unreadable DB, `chrome_probe` becomes `ChromeProbeUiState::PermissionDeniedTcc`, and `"stay_intact"` cookie remains uncorrupted.
#[test]
fn test_br_t2_f8_04_malformed_cookie_json_and_tcc_eperm_via_workspace() {
    run_t2_in_headless_subprocess(
        "test_br_t2_f8_04_malformed_cookie_json_and_tcc_eperm_via_workspace",
    );
}

async fn body_br_t2_f8_04(mut app: gpui::AsyncApp) {
    use std::os::unix::fs::PermissionsExt;
    use gpui_telegram_desktop::views::browser::ChromeProbeUiState;

    let temp_home = isolated_temp_dir("t2_f8_04_home").expect("temp chrome dir");
    let cookie_path = temp_home
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

    let (_client_dir, client) =
        e2e_suite::gpui_browser_harness::new_isolated_client("http://127.0.0.1:9");
    let rt = client.runtime().handle().clone();
    let x_server = rt.block_on(MockXServer::start()).expect("MockXServer");
    let _seed = x_server.seed_session("stay_intact", "ct0_intact");
    let chrome_dir = isolated_temp_dir("t2_f8_04_chrome").expect("chrome dir");
    let chrome = Arc::new(rt.block_on(launch_chrome_in(chrome_dir.path())));

    let seed_cookies = import_cookies_from_json(
        &e2e_suite::browser_e2e_fixtures::generate_cdp_cookies_json(&x_auth_cookies(
            "127.0.0.1",
            "stay_intact",
            "ct0_intact",
        )),
    )
    .expect("seed cookies");
    rt.block_on(chrome.import_cookies(&seed_cookies))
        .expect("import seed cookies");
    rt.block_on(chrome.navigate(&x_server.url("/home")))
        .expect("navigate /home on 127.0.0.1");

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell(
                cx,
                client.clone(),
                824,
                "t2_f8_04",
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
                w.import_cookies_from_text_async("{bad json".to_string(), wcx);
            });
        });
    })
    .unwrap();

    let json_errored = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            w.recorder.cookie_error_message.is_some()
        },
    )
    .await;
    assert!(json_errored);
    app.update(|cx| {
        let w = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert!(w.recorder.cookie_status_message.is_none());
    })
    .unwrap();

    // Probe 0o000 Chrome Cookies DB -> PermissionDeniedTcc applied cleanly to BrowserWorkspace
    let probe = probe_chrome_cookies_at(&cookie_path);
    let denied_path = app
        .update(|cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().clone();
            bw.update(cx, |w, wcx| {
                w.apply_chrome_probe_result(probe, wcx);
                match &w.recorder.chrome_probe {
                    ChromeProbeUiState::PermissionDeniedTcc { path, guidance } => {
                        assert!(!guidance.trim().is_empty());
                        path.clone()
                    }
                    other => panic!("expected PermissionDeniedTcc, got {other:?}"),
                }
            })
        })
        .unwrap();

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.recorder.cookie_error_message = None;
            w.import_cookies_from_chrome_path(denied_path, wcx);
        });
    })
    .unwrap();

    let tcc_errored = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(15),
        |cx| {
            let w = shell.read(cx).browser_workspace().unwrap().read(cx);
            w.recorder.cookie_error_message.is_some()
        },
    )
    .await;
    assert!(tcc_errored);

    let exported = rt.block_on(chrome.export_cookies(None)).expect("export cookies");
    assert!(
        exported
            .iter()
            .any(|c| c.name == "auth_token" && c.value == "stay_intact"),
        "existing valid cookies must remain intact after failed imports"
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

/// [Test ID] TEST-BR-T2-F8-05
/// [Feature / Req] F8 / R4 — Nonexistent app install/uninstall (`404`), idempotent re-install, and invalid manifest run do not corrupt `LocalStore`
/// [Preconditions] `EphemeralTestServer` started; User A publishes `"t2-f8-05-app"`; User B authenticated in `AppShell`.
/// [Opaque-Box Steps] Attempt install of `"nonexistent-store-app"`, install `"t2-f8-05-app"` twice, run an invalid manifest via `run_installed_store_app`, and attempt `uninstall_store_app("nonexistent-store-app")`.
/// [Observable Assertions] Missing IDs populate `store_error`, duplicate install keeps exactly 1 entry in `LocalStore` & `installed_apps`, and invalid run leaves `LocalStore` uncorrupted.
#[test]
fn test_br_t2_f8_05_store_nonexistent_idempotent_install_and_invalid_run() {
    run_t2_in_headless_subprocess(
        "test_br_t2_f8_05_store_nonexistent_idempotent_install_and_invalid_run",
    );
}

async fn body_br_t2_f8_05(mut app: gpui::AsyncApp) {
    use gpui_telegram_proto::automation_apps::InstalledAutomationApp;

    let rt_boot = tokio::runtime::Runtime::new().unwrap();
    let mut server = rt_boot
        .block_on(EphemeralTestServer::start())
        .expect("EphemeralTestServer");

    let a_dir = isolated_temp_dir("t2_f8_05_a").expect("a dir");
    let client_a = e2e_suite::gpui_browser_harness::open_client_in_dir(&server.base_url(), a_dir.path());
    client_a
        .runtime()
        .block_on(client_a.register(&RegisterRequest {
            username: "t2_f8_05_alice".into(),
            password: "StrongPassword!123".into(),
            first_name: "Alice".into(),
            device_name: "mac-a".into(),
        }))
        .expect("register alice");

    let valid_app = build_clean_boundary_app("t2-f8-05-app", "t2-f8-05-app", "Idempotent App", "1.0.0");
    client_a
        .runtime()
        .block_on(client_a.publish_automation_app(&PublishAutomationAppRequest {
            manifest: serde_json::to_value(&valid_app).unwrap(),
            is_published: true,
            changelog: None,
        }))
        .expect("publish t2-f8-05-app");
    client_a.clear_session();
    drop(client_a);

    let b_dir = isolated_temp_dir("t2_f8_05_b").expect("b dir");
    let client_b = e2e_suite::gpui_browser_harness::open_client_in_dir(&server.base_url(), b_dir.path());
    let auth_b = client_b
        .runtime()
        .block_on(client_b.register(&RegisterRequest {
            username: "t2_f8_05_bob".into(),
            password: "StrongPassword!123".into(),
            first_name: "Bob".into(),
            device_name: "mac-b".into(),
        }))
        .expect("register bob");

    let shell = app
        .update(|cx| {
            e2e_suite::gpui_browser_harness::build_authenticated_shell_with_token(
                cx,
                client_b.clone(),
                auth_b.user_id,
                "t2_f8_05_bob",
                &auth_b.token,
            )
        })
        .unwrap();

    // 1. Nonexistent install -> store_error
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_install_store_app("nonexistent-store-app", scx);
        });
    })
    .unwrap();
    assert!(
        e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(20), |cx| {
            shell
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .store
                .store_error
                .is_some()
        })
        .await
    );
    app.update(|cx| {
        assert!(
            shell
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .store
                .installed_apps
                .is_empty()
        );
    })
    .unwrap();

    // 2. Idempotent double install
    for _ in 0..2 {
        app.update(|cx| {
            shell.update(cx, |s, scx| {
                if let Some(bw) = s.browser_workspace() {
                    bw.update(scx, |w, _| {
                        w.store.last_installed_app_id = None;
                    });
                }
                s.browser_install_store_app("t2-f8-05-app", scx);
            });
        })
        .unwrap();
        assert!(
            e2e_suite::gpui_browser_harness::pump_until(&mut app, Duration::from_secs(20), |cx| {
                let w = shell.read(cx).browser_workspace().unwrap().read(cx);
                w.store.store_error.is_none()
                    && w.store.last_installed_app_id.as_deref() == Some("t2-f8-05-app")
                    && w.store
                        .installed_apps
                        .iter()
                        .any(|a| a.app_id == "t2-f8-05-app")
            })
            .await
        );
    }
    assert_eq!(client_b.list_installed_automation_apps_cached().len(), 1);
    app.update(|cx| {
        let w = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(
            w.store
                .installed_apps
                .iter()
                .filter(|a| a.app_id == "t2-f8-05-app")
                .count(),
            1
        );
    })
    .unwrap();

    // 3. Invalid manifest run sets store_error without corrupting LocalStore
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.run_installed_store_app(
                InstalledAutomationApp {
                    app_id: "uninstalled-invalid".to_string(),
                    slug: "uninstalled-invalid".to_string(),
                    name: "Bad".to_string(),
                    description: String::new(),
                    category: "general".to_string(),
                    target_domains: vec![],
                    installed_version: "1.0.0".to_string(),
                    latest_version: "1.0.0".to_string(),
                    manifest: serde_json::json!({}),
                    content_sha256: String::new(),
                    installed_at_ms: 0,
                    last_run_at_ms: None,
                    local_run_count: 0,
                },
                wcx,
            );
            assert!(w.store.store_error.is_some());
        });
    })
    .unwrap();
    assert_eq!(client_b.list_installed_automation_apps_cached().len(), 1);

    // 4. Uninstall nonexistent -> store_error
    let direct_uninst = client_b
        .runtime()
        .block_on(client_b.uninstall_automation_app("nonexistent-store-app"));
    assert!(
        direct_uninst.is_err(),
        "server DELETE nonexistent-store-app must return 404 Err, got {direct_uninst:?}"
    );
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.store.store_error = None;
            w.uninstall_store_app("nonexistent-store-app", wcx);
        });
    })
    .unwrap();
    let uninst_errored = e2e_suite::gpui_browser_harness::pump_until(
        &mut app,
        Duration::from_secs(20),
        |cx| {
            shell
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .store
                .store_error
                .is_some()
        },
    )
    .await;
    if !uninst_errored {
        let diag = app
            .update(|cx| {
                let w = shell.read(cx).browser_workspace().unwrap().read(cx);
                format!(
                    "store_error={:?}, installed_apps_len={}, catalog_apps_len={}",
                    w.store.store_error,
                    w.store.installed_apps.len(),
                    w.store.catalog_apps.len(),
                )
            })
            .unwrap_or_else(|e| format!("failed to read diag: {e:?}"));
        panic!("uninstall_store_app(nonexistent) must populate store_error: {diag}");
    }
    assert_eq!(client_b.list_installed_automation_apps_cached().len(), 1);

    app.update(|_| drop(shell)).unwrap();
    client_b.clear_session();
    drop(client_b);
    rt_boot.block_on(server.stop());
}
