//! Generic IM Platform Mini App & Bot API Integration Tests.
//!
//! Verifies Telegram-parity platform decoupling invariants:
//! - Clean server boot seeds ZERO built-in Mini Apps and ZERO `fabushi_%` tables in the IM DB.
//! - Any third-party external Mini App can register via `POST /api/v1/bots`, publish an external
//!   `web_url` via `POST /bot{token}/publishMiniApp`, configure its menu button, be discovered in
//!   catalog/search, render the generic `#tgWebAppData=...` iframe wrapper, launch with HMAC-signed
//!   `initData`, validate `initData`, persist CloudStorage, and handle generic `miniapps_action`.
//! - Standard `allows_write_to_pm` gates `POST /bot{token}/sendMessage`: blocked (`403`) until
//!   the user grants permission via `/api/v1/bots/{bot_id}/allow-send-message`, then `200 OK`
//!   with `web_app` inline keyboard preservation in DM history.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use gpui_telegram_proto::miniapp_contracts::{
    AllowSendMessageResponse, MiniAppActionRequest, MiniAppActionResponse, MiniAppLaunchContext,
    ValidateInitDataResponse,
};
use gpui_telegram_server::{build_router_with_state, config::ServerConfig, server::AppState};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("{}_{}", prefix, Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self { path: dir }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn boot_temp_server(prefix: &str) -> (TempDirGuard, Arc<AppState>, Router) {
    let dir = TempDirGuard::new(prefix);
    let db_path = dir.path.join("im_platform_miniapp.db");
    let media_dir = dir.path.join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;
    let state = Arc::new(AppState::try_new(cfg).expect("boot AppState"));
    let router = build_router_with_state(state.clone());
    (dir, state, router)
}

async fn http_raw(
    router: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
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
    (status, bytes.to_vec())
}

async fn http_json(
    router: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, bytes) = http_raw(router, method, uri, token, body).await;
    let val = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, val)
}

async fn register_user(router: &Router, username: &str) -> (u64, String) {
    let (status, val) = http_json(
        router,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": username,
            "password": "GenericPlatformPassword!2026",
            "first_name": username,
            "last_name": "Tester",
            "device_name": "MiniAppPlatformTest"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register {username}: {val}");
    (
        val["user_id"].as_u64().expect("user_id"),
        val["token"].as_str().expect("token").to_string(),
    )
}

#[tokio::test]
async fn clean_im_platform_has_zero_fabushi_tables_and_supports_generic_external_miniapps() {
    let (_dir, state, router) = boot_temp_server("generic_miniapp_e2e");

    // 1. Verify IM SQLite DB has ZERO `fabushi_%` tables
    let fabushi_table_count: i64 = state
        .db
        .pool()
        .with_reader(|conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE 'fabushi_%'",
                    [],
                    |row| row.get(0),
                )
                .expect("query sqlite_master");
            Ok(count)
        })
        .expect("read sqlite_master");
    assert_eq!(
        fabushi_table_count, 0,
        "core IM database must not create app-specific fabushi_* tables"
    );

    // 2. Register publisher and end user
    let (_pub_uid, pub_token) = register_user(&router, "acme_publisher").await;
    let (alice_uid, alice_token) = register_user(&router, "alice_consumer").await;

    // Fresh boot has 0 published miniapps in catalog
    let (st_empty, empty_catalog) =
        http_json(&router, "GET", "/api/v1/miniapps", Some(&alice_token), None).await;
    assert_eq!(st_empty, StatusCode::OK, "list empty catalog: {empty_catalog}");
    assert_eq!(empty_catalog["total"].as_u64(), Some(0));

    // 3. Register a generic third-party bot and publish an external Mini App + menu button
    let (st_bot, bot_val) = http_json(
        &router,
        "POST",
        "/api/v1/bots",
        Some(&pub_token),
        Some(json!({
            "display_name": "Acme Helper",
            "username": "acme_helper_bot",
            "description": "Generic external Mini App bot",
            "about": "External portal companion bot",
            "allow_attach_menu": true,
            "attach_peer_types": ["users", "bots", "groups"]
        })),
    )
    .await;
    assert_eq!(st_bot, StatusCode::CREATED, "create @acme_helper_bot: {bot_val}");
    let bot_token = bot_val["bot_token"].as_str().expect("bot_token").to_string();

    let external_url = "https://miniapp.acme.example/app";
    let (st_pub, pub_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/publishMiniApp"),
        None,
        Some(json!({
            "short_name": "portal",
            "title": "Acme Portal",
            "description": "External hosted workspace",
            "category": "tools",
            "icon_emoji": "🧰",
            "web_url": external_url,
            "is_main_app": true,
            "is_attach_enabled": true
        })),
    )
    .await;
    assert_eq!(st_pub, StatusCode::OK, "publishMiniApp failed: {pub_val}");
    assert_eq!(pub_val["ok"], true);

    let (st_menu, menu_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/setChatMenuButton"),
        None,
        Some(json!({
            "menu_button": {
                "type": "web_app",
                "text": "Open Acme Portal",
                "web_app": { "url": external_url }
            }
        })),
    )
    .await;
    assert_eq!(st_menu, StatusCode::OK, "setChatMenuButton failed: {menu_val}");
    assert_eq!(menu_val["ok"], true);

    // 4. Discovery & install
    let (st_search, search_val) = http_json(
        &router,
        "GET",
        "/api/v1/miniapps?q=acme",
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(st_search, StatusCode::OK, "search miniapps: {search_val}");
    let apps = search_val["apps"].as_array().expect("apps array");
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0]["bot_username"].as_str(), Some("acme_helper_bot"));
    assert_eq!(apps[0]["short_name"].as_str(), Some("portal"));
    assert_eq!(apps[0]["web_url"].as_str(), Some(external_url));

    let (st_inst, inst_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/acme_helper_bot/portal/install",
        Some(&alice_token),
        Some(json!({})),
    )
    .await;
    assert_eq!(st_inst, StatusCode::OK, "install miniapp: {inst_val}");

    // 5. Generic external HTML wrapper renders iframe pointing to external_url#tgWebAppData=
    let (st_html, html_bytes) = http_raw(
        &router,
        "GET",
        "/miniapps/acme_helper_bot/portal?init_data=demo_init",
        None,
        None,
    )
    .await;
    assert_eq!(st_html, StatusCode::OK);
    let html = String::from_utf8(html_bytes).expect("utf8 html");
    assert!(
        html.contains("https://miniapp.acme.example/app#tgWebAppData=demo_init"),
        "expected external iframe wrapper with tgWebAppData hash, got: {html}"
    );

    // 6. Launch produces external web_url and valid HMAC-SHA256 initData
    let (st_launch, launch_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/acme_helper_bot/portal/launch",
        Some(&alice_token),
        Some(json!({})),
    )
    .await;
    assert_eq!(st_launch, StatusCode::OK, "launch failed: {launch_val}");
    let launch_ctx: MiniAppLaunchContext =
        serde_json::from_value(launch_val).expect("parse MiniAppLaunchContext");
    assert_eq!(launch_ctx.app.web_url, external_url);
    assert!(!launch_ctx.init_data.raw_query_string.is_empty());
    let bot_id = launch_ctx.companion_bot.bot_user_id;

    // 7. validate-init-data accepts valid HMAC string and rejects tampered string
    let (st_val_ok, val_ok_json) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/acme_helper_bot/portal/validate-init-data",
        Some(&alice_token),
        Some(json!({ "init_data": launch_ctx.init_data.raw_query_string })),
    )
    .await;
    assert_eq!(st_val_ok, StatusCode::OK);
    let val_ok: ValidateInitDataResponse = serde_json::from_value(val_ok_json).unwrap();
    assert!(val_ok.valid, "valid initData must pass validation");
    assert_eq!(val_ok.user_id, Some(alice_uid as i64));

    let tampered = format!("{}&extra=tampered", launch_ctx.init_data.raw_query_string);
    let (st_val_bad, val_bad_json) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/acme_helper_bot/portal/validate-init-data",
        Some(&alice_token),
        Some(json!({ "init_data": tampered })),
    )
    .await;
    assert_eq!(st_val_bad, StatusCode::OK);
    let val_bad: ValidateInitDataResponse = serde_json::from_value(val_bad_json).unwrap();
    assert!(!val_bad.valid, "tampered initData must fail validation");

    // 8. CloudStorage CRUD
    let (st_put, put_val) = http_json(
        &router,
        "PUT",
        "/api/v1/miniapps/acme_helper_bot/portal/storage",
        Some(&alice_token),
        Some(json!({ "key": "theme_pref", "value": "solarized" })),
    )
    .await;
    assert_eq!(st_put, StatusCode::OK, "storage put: {put_val}");

    let (st_get, get_val) = http_json(
        &router,
        "GET",
        "/api/v1/miniapps/acme_helper_bot/portal/storage",
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(st_get, StatusCode::OK, "storage get: {get_val}");
    assert_eq!(get_val["entries"]["theme_pref"].as_str(), Some("solarized"));

    let (st_del, _) = http_json(
        &router,
        "DELETE",
        "/api/v1/miniapps/acme_helper_bot/portal/storage?key=theme_pref",
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(st_del, StatusCode::OK);

    let (st_get2, get_val2) = http_json(
        &router,
        "GET",
        "/api/v1/miniapps/acme_helper_bot/portal/storage",
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(st_get2, StatusCode::OK);
    assert!(get_val2["entries"].get("theme_pref").is_none());

    // 9. Generic miniapps_action main_button_click
    let (st_act, act_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/acme_helper_bot/portal/action",
        Some(&alice_token),
        Some(
            serde_json::to_value(MiniAppActionRequest {
                query_id: Some(launch_ctx.init_data.query_id.clone()),
                chat_id: None,
                action_type: "main_button_click".to_string(),
                widget_id: None,
                value: None,
                data: None,
                prompt: None,
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(st_act, StatusCode::OK, "generic action failed: {act_val}");
    let act_res: MiniAppActionResponse = serde_json::from_value(act_val).unwrap();
    assert!(act_res.ok);

    // 10. Telegram `allows_write_to_pm` gating on /bot{token}/sendMessage
    let (st_blocked_dm, blocked_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/sendMessage"),
        None,
        Some(json!({
            "chat_id": alice_uid,
            "text": "Should fail before write permission"
        })),
    )
    .await;
    assert_eq!(
        st_blocked_dm,
        StatusCode::FORBIDDEN,
        "sendMessage before allow-send-message must return 403: {blocked_val}"
    );

    // Grant write permission via /api/v1/bots/{bot_id}/allow-send-message
    let (st_allow, allow_raw) = http_json(
        &router,
        "POST",
        &format!("/api/v1/bots/{bot_id}/allow-send-message"),
        Some(&alice_token),
        Some(json!({})),
    )
    .await;
    assert_eq!(st_allow, StatusCode::OK, "allow-send-message failed: {allow_raw}");
    let allow_resp: AllowSendMessageResponse = serde_json::from_value(allow_raw).unwrap();
    assert!(allow_resp.ok);
    assert!(allow_resp.chat_id > 0);

    // Repeat sendMessage with inline web_app keyboard -> 200 OK & visible in DM history
    let receipt_text = "Acme Portal task completed successfully";
    let (st_dm_ok, dm_ok_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/sendMessage"),
        None,
        Some(json!({
            "chat_id": alice_uid,
            "text": receipt_text,
            "reply_markup": {
                "inline_keyboard": [[
                    { "text": "Open Acme Portal", "web_app": { "url": external_url } }
                ]]
            }
        })),
    )
    .await;
    assert_eq!(st_dm_ok, StatusCode::OK, "sendMessage after allow failed: {dm_ok_val}");
    assert_eq!(dm_ok_val["ok"], true);

    let (st_hist, hist_val) = http_json(
        &router,
        "GET",
        &format!("/api/v1/chats/{}/messages?limit=50", allow_resp.chat_id),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(st_hist, StatusCode::OK, "fetch DM history: {hist_val}");
    let msgs = hist_val["messages"].as_array().expect("messages array");
    let receipt_msg = msgs
        .iter()
        .find(|m| m["text"].as_str() == Some(receipt_text))
        .expect("bot receipt message should be persisted in companion DM chat");
    let kb = receipt_msg["inline_keyboard"]
        .as_array()
        .expect("inline_keyboard must be preserved");
    let btn = &kb[0][0];
    assert_eq!(btn["text"].as_str(), Some("Open Acme Portal"));
    assert_eq!(btn["web_app_url"].as_str(), Some(external_url));
}

#[tokio::test]
async fn external_miniapp_iframe_wrapper_relays_webmcp_and_forwards_hash_init_data() {
    let (_dir, _state, router) = boot_temp_server("external_miniapp_iframe_webmcp");
    let (_alice_uid, alice_token) = register_user(&router, "alice_iframe_webmcp").await;

    let (st_bot, bot_val) = http_json(
        &router,
        "POST",
        "/api/v1/bots",
        Some(&alice_token),
        Some(json!({
            "username": "acme_iframe_bot",
            "display_name": "Acme Iframe Bot",
            "description": "External Mini App Iframe Test",
            "about": "Tests cross-frame WebMCP relay"
        })),
    )
    .await;
    assert_eq!(st_bot, StatusCode::CREATED, "create bot: {bot_val}");
    let bot_token = bot_val["bot_token"].as_str().unwrap().to_string();

    let external_url = "https://miniapp.acme.example/app";
    let (st_pub, pub_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/publishMiniApp"),
        None,
        Some(json!({
            "short_name": "portal",
            "title": "Acme Portal",
            "description": "External WebMCP Portal",
            "category": "tools",
            "icon_emoji": "🌐",
            "web_url": external_url,
            "is_main_app": true
        })),
    )
    .await;
    assert_eq!(st_pub, StatusCode::OK, "publishMiniApp: {pub_val}");

    let (st_html, html_bytes) = http_raw(
        &router,
        "GET",
        "/miniapps/acme_iframe_bot/portal",
        None,
        None,
    )
    .await;
    assert_eq!(st_html, StatusCode::OK);
    let html = String::from_utf8(html_bytes).expect("utf8 html");
    assert!(html.contains("<iframe id=\"tg-app-frame\""), "missing iframe");
    for marker in [
        "gpui_webmcp_invoke",
        "gpui_webmcp_response",
        "gpui_webmcp_ready",
        "gpui_webmcp_tools_changed",
        "gpui_webmcp_unready",
        "window.__GPUI_WEBMCP__",
        "tgWebAppData=",
    ] {
        assert!(html.contains(marker), "wrapper HTML missing '{marker}'");
    }

    // Extract the inline relay <script> block and execute it under node (when available)
    let frame_tag_pos = html.find("<iframe id=\"tg-app-frame\"").unwrap();
    let script_open = html[frame_tag_pos..].find("<script>").unwrap() + frame_tag_pos + "<script>".len();
    let script_close = html[script_open..].find("</script>").unwrap() + script_open;
    let wrapper_js = &html[script_open..script_close];

    let node_check = std::process::Command::new("node").arg("--version").output();
    if let Ok(ver) = node_check {
        if ver.status.success() {
            let test_driver = format!(
                r#"
const assert = require('assert');
const listeners = {{}};
const frameListeners = {{}};
const postedToFrame = [];
const bridgeEvents = [];
const mockFrameWin = {{
  postMessage(msg, targetOrigin) {{
    postedToFrame.push({{ msg, targetOrigin }});
  }}
}};
const mockFrame = {{
  src: {initial_src},
  getAttribute(k) {{ return k === 'src' ? this.src : null; }},
  contentWindow: mockFrameWin,
  addEventListener(type, fn) {{
    (frameListeners[type] = frameListeners[type] || []).push(fn);
  }}
}};
global.document = {{
  getElementById(id) {{ return id === 'tg-app-frame' ? mockFrame : null; }}
}};
global.window = {{
  location: {{ hash: '#tgWebAppData=user%3D1001&auth_date%3D1' }},
  parent: null,
  TelegramWebviewProxy: {{
    postEvent(evType, evData) {{
      bridgeEvents.push({{ eventType: evType, eventData: JSON.parse(evData) }});
    }}
  }},
  addEventListener(type, fn) {{
    (listeners[type] = listeners[type] || []).push(fn);
  }}
}};
global.window.parent = global.window;

{wrapper_js}

(async () => {{
  // 1. Hash tgWebAppData forwarded into frame.src
  assert(mockFrame.src.includes('#tgWebAppData=user%3D1001'), 'expected tgWebAppData forwarded to frame.src: ' + mockFrame.src);

  // 2. invoke() before gpui_webmcp_ready queues instead of failing immediately
  const p1 = global.window.__GPUI_WEBMCP__.invoke('req_1', 'call_tool', 'fabushi_send_dharma', {{ sutra: '金刚经' }});
  assert.strictEqual(postedToFrame.length, 0, 'invoke before ready must queue');

  // 3. gpui_webmcp_ready from frame flushes queued invoke and bridges __gpui_webmcp_page_ready
  const onMsg = (listeners.message || [])[0];
  onMsg({{
    source: mockFrameWin,
    data: {{
      type: 'gpui_webmcp_ready',
      url: 'https://miniapp.acme.example/app',
      tools: [{{ name: 'fabushi_send_dharma', description: 'Send', input_schema: {{}}, read_only: false }}],
      state_summary: {{ ready: true }}
    }}
  }});
  assert.strictEqual(postedToFrame.length, 1, 'queued invoke must flush on ready');
  assert.strictEqual(postedToFrame[0].msg.type, 'gpui_webmcp_invoke');
  assert.strictEqual(postedToFrame[0].msg.request_id, 'req_1');
  assert.strictEqual(postedToFrame[0].msg.tool_name, 'fabushi_send_dharma');
  assert(bridgeEvents.some(e => e.eventType === '__gpui_webmcp_page_ready' && e.eventData.in_iframe === true));

  // 4. gpui_webmcp_response resolves promise and bridges webmcp_response
  onMsg({{
    source: mockFrameWin,
    data: {{
      type: 'gpui_webmcp_response',
      request_id: 'req_1',
      payload: {{ request_id: 'req_1', ok: true, result: {{ sent: true }}, error: null, tools: [], state_summary: {{ count: 1 }}, control_mode: 'ai_controlling' }}
    }}
  }});
  const r1 = await p1;
  assert.strictEqual(r1.ok, true);
  assert.strictEqual(r1.result.sent, true);
  assert(bridgeEvents.some(e => e.eventType === 'webmcp_response' && e.eventData.request_id === 'req_1' && e.eventData.ok === true));

  // 5. gpui_webmcp_unready resets readiness so subsequent invokes queue
  onMsg({{ source: mockFrameWin, data: {{ type: 'gpui_webmcp_unready', url: 'https://miniapp.acme.example/app' }} }});
  assert(bridgeEvents.some(e => e.eventType === '__gpui_webmcp_page_unready'));
  const pA = global.window.__GPUI_WEBMCP__.invoke('req_a', 'call_tool', 't_a', {{}});
  const pB = global.window.__GPUI_WEBMCP__.invoke('req_b', 'call_tool', 't_b', {{}});
  assert.strictEqual(postedToFrame.length, 1, 'invokes after unready must queue until next ready');

  onMsg({{ source: mockFrameWin, data: {{ type: 'gpui_webmcp_ready', url: 'https://miniapp.acme.example/app', tools: [] }} }});
  assert.strictEqual(postedToFrame.length, 3, 'both queued invokes flush on next ready');

  // 6. Concurrent out-of-order responses resolve independently by request_id
  onMsg({{ source: mockFrameWin, data: {{ type: 'gpui_webmcp_response', payload: {{ request_id: 'req_b', ok: true, result: {{ id: 'b' }} }} }} }});
  onMsg({{ source: mockFrameWin, data: {{ type: 'gpui_webmcp_response', payload: {{ request_id: 'req_a', ok: true, result: {{ id: 'a' }} }} }} }});
  const [resA, resB] = await Promise.all([pA, pB]);
  assert.strictEqual(resA.result.id, 'a');
  assert.strictEqual(resB.result.id, 'b');

  // 7. Post-load + post-unready invoke recovers cleanly via async gpui_webmcp_ping_ready
  onMsg({{ source: mockFrameWin, data: {{ type: 'gpui_webmcp_unready', url: 'https://miniapp.acme.example/app' }} }});
  for (const fn of (frameListeners.load || [])) fn();
  const prevCount = postedToFrame.length;
  const pLate = global.window.__GPUI_WEBMCP__.invoke('req_late', 'call_tool', 'fabushi_send_dharma', {{}});
  await new Promise(r => setTimeout(r, 90));
  const pingMsgs = postedToFrame.slice(prevCount).filter(x => x.msg && x.msg.type === 'gpui_webmcp_ping_ready');
  assert(pingMsgs.length >= 1, 'expected gpui_webmcp_ping_ready recovery message');
  onMsg({{ source: mockFrameWin, data: {{ type: 'gpui_webmcp_ready', url: 'https://miniapp.acme.example/app', tools: [] }} }});
  onMsg({{ source: mockFrameWin, data: {{ type: 'gpui_webmcp_response', payload: {{ request_id: 'req_late', ok: true, result: {{ late: true }} }} }} }});
  const resLate = await pLate;
  assert.strictEqual(resLate.ok, true);
  assert.strictEqual(resLate.result.late, true);

  process.exit(0);
}})().catch(err => {{ console.error(err); process.exit(1); }});
"#,
                initial_src = serde_json::to_string(external_url).unwrap(),
                wrapper_js = wrapper_js,
            );
            let out = std::process::Command::new("node")
                .arg("-e")
                .arg(&test_driver)
                .output()
                .expect("run node wrapper simulation");
            assert!(
                out.status.success(),
                "node iframe relay simulation failed:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

#[tokio::test]
async fn tier5_miniapp_html_sanitizes_background_color_and_schema_xss() {
    let (_dir, _state, router) = boot_temp_server("tier5_miniapp_xss");
    let (_alice_uid, alice_token) = register_user(&router, "alice_xss_guard").await;

    let (st_bot, bot_val) = http_json(
        &router,
        "POST",
        "/api/v1/bots",
        Some(&alice_token),
        Some(json!({
            "username": "xss_probe_bot",
            "display_name": "XSS Probe Bot",
            "description": "XSS sanitization test",
            "about": "XSS test"
        })),
    )
    .await;
    assert_eq!(st_bot, StatusCode::CREATED, "create bot: {bot_val}");
    let bot_token = bot_val["bot_token"].as_str().unwrap().to_string();

    // 1. External Mini App with malicious background_color CSS/script breakout
    let (st_ext, _) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/publishMiniApp"),
        None,
        Some(json!({
            "short_name": "ext",
            "title": "Ext Portal",
            "web_url": "https://miniapp.acme.example/app",
            "background_color": "#fff;}</style><script>window.__xss_bg=1</script><style>"
        })),
    )
    .await;
    assert_eq!(st_ext, StatusCode::OK);

    let (st_ext_html, ext_bytes) = http_raw(&router, "GET", "/miniapps/xss_probe_bot/ext", None, None).await;
    assert_eq!(st_ext_html, StatusCode::OK);
    let ext_html = String::from_utf8(ext_bytes).expect("utf8");
    assert!(
        !ext_html.contains("__xss_bg"),
        "malicious background_color payload must be stripped: {ext_html}"
    );
    assert!(
        ext_html.contains("background: #ffffff;"),
        "invalid background_color must fall back to #ffffff"
    );

    // 2. Schema Mini App with </script> breakout and <img onerror=...> payload
    let (st_sch, _) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/publishMiniApp"),
        None,
        Some(json!({
            "short_name": "sch",
            "title": "Schema App",
            "ui_schema": {
                "title": "</script><script>window.__xss_schema=1</script>",
                "subtitle": "<img src=x onerror=window.__xss_sub=1>",
                "widgets": []
            }
        })),
    )
    .await;
    assert_eq!(st_sch, StatusCode::OK);

    let (st_sch_html, sch_bytes) = http_raw(&router, "GET", "/miniapps/xss_probe_bot/sch", None, None).await;
    assert_eq!(st_sch_html, StatusCode::OK);
    let sch_html = String::from_utf8(sch_bytes).expect("utf8");
    assert!(
        !sch_html.contains("</script><script>window.__xss_schema"),
        "schema JSON must escape < and > inside inline script"
    );
    assert!(
        sch_html.contains("\\u003c/script\\u003e"),
        "schema JSON must unicode-escape script tags"
    );
    assert!(
        !sch_html.contains("hdr.innerHTML"),
        "renderApp must use textContent instead of innerHTML"
    );
}
