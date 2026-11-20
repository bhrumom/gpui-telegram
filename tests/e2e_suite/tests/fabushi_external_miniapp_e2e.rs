//! End-to-End Integration Test for Two-Server Telegram-Parity Architecture:
//!
//! Server A (`gpui_telegram_server`) on ephemeral port A + own SQLite DB
//!   - Has ZERO `fabushi_%` tables and ZERO hardcoded fabushi logic
//!   - Issues HMAC-SHA256 signed `initData`, serves Bot HTTP API (`/bot{token}/...`),
//!     and proxies standard `allows_write_to_pm` permission gating.
//!
//! Server B (`fabushi_miniapp::web_server`) on ephemeral port B + own SQLite DB (`fabushi.db`)
//!   - Serves HTML/WASM/assets + `/js/telegram-web-app.js` proxy
//!   - Verifies `X-Telegram-Init-Data` with `bot_token` via standard Telegram HMAC-SHA256
//!   - Delivers DM receipts strictly via `POST {im_server_url}/bot{bot_token}/sendMessage`.

use fabushi_miniapp::storage;
use fabushi_miniapp::web_server::{build_router, FabushiServerConfig, FabushiServerState};
use gpui_telegram_proto::miniapp_contracts::{AllowSendMessageResponse, MiniAppLaunchContext};
use gpui_telegram_server::{build_router_with_state, config::ServerConfig, server::AppState};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

fn resolve_publish_cli_binary() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    let bin_name = if cfg!(windows) {
        "publish_fabushi_miniapp.exe"
    } else {
        "publish_fabushi_miniapp"
    };
    let candidate = exe
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join(bin_name))
        .expect("target debug/release dir");
    assert!(
        candidate.exists(),
        "publish_fabushi_miniapp binary not found at {}; run `cargo build -p fabushi_miniapp --features server --bin publish_fabushi_miniapp` first",
        candidate.display()
    );
    candidate
}

fn count_fabushi_tables_in_im_db(state: &AppState) -> i64 {
    state
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
        .expect("with_reader")
}

async fn fetch_chat_messages(
    client: &reqwest::Client,
    im_base_url: &str,
    token: &str,
    chat_id: i64,
) -> Vec<Value> {
    let resp = client
        .get(format!("{im_base_url}/api/v1/chats/{chat_id}/messages?limit=100"))
        .bearer_auth(token)
        .send()
        .await
        .expect("GET chat messages");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let val: Value = resp.json().await.expect("parse chat messages json");
    val["messages"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn external_fabushi_miniapp_e2e_with_two_servers_and_bot_api_receipts() {
    // 1. Boot clean IM server (Server A) on ephemeral TCP port
    let im_dir = TempDir::new().expect("create IM temp dir");
    let im_db_path = im_dir.path().join("im_server.db");
    let media_dir = im_dir.path().join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");

    let mut im_cfg = ServerConfig::default();
    im_cfg.database_url = format!("sqlite://{}", im_db_path.display());
    im_cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    im_cfg.host = "127.0.0.1".to_string();
    im_cfg.port = 0;

    let im_state = Arc::new(AppState::try_new(im_cfg).expect("boot AppState"));
    assert_eq!(
        count_fabushi_tables_in_im_db(&im_state),
        0,
        "clean IM server SQLite DB must contain zero fabushi_* tables"
    );

    let im_router = build_router_with_state(im_state.clone());
    let im_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind IM listener");
    let im_addr = im_listener.local_addr().expect("IM local_addr");
    let im_base_url = format!("http://{im_addr}");
    let _im_handle = tokio::spawn(async move {
        axum::serve(im_listener, im_router)
            .await
            .expect("serve IM router");
    });

    // 2. Bind external Mini App server port (Server B) so publisher CLI knows exact URL
    let ext_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind external miniapp listener");
    let ext_addr = ext_listener.local_addr().expect("ext local_addr");
    let ext_base_url = format!("http://{ext_addr}");

    // 3. Invoke `publish_fabushi_miniapp` CLI (--skip-miniapp-health-check initially) to register bot & extract bot_token
    let ext_dir = TempDir::new().expect("create external fabushi temp dir");
    let token_path = ext_dir.path().join("fabushi_bot_token.txt");
    let cli_bin = resolve_publish_cli_binary();

    let pub1 = tokio::process::Command::new(&cli_bin)
        .arg("--server-url")
        .arg(&im_base_url)
        .arg("--miniapp-url")
        .arg(&ext_base_url)
        .arg("--write-bot-token")
        .arg(&token_path)
        .arg("--skip-miniapp-health-check")
        .output()
        .await
        .expect("spawn publish_fabushi_miniapp (pass 1)");
    assert!(
        pub1.status.success(),
        "publish_fabushi_miniapp pass 1 failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&pub1.stdout),
        String::from_utf8_lossy(&pub1.stderr)
    );

    let bot_token = std::fs::read_to_string(&token_path)
        .expect("read written bot_token")
        .trim()
        .to_string();
    assert!(
        bot_token.contains(':'),
        "expected standard Bot API token format, got: {bot_token}"
    );

    // 4. Boot standalone external `fabushi_miniapp_server` (Server B) with its own SQLite DB
    let ext_db_path = ext_dir.path().join("fabushi.db");
    let ext_state = Arc::new(
        FabushiServerState::open(FabushiServerConfig {
            im_server_url: im_base_url.clone(),
            bot_token: bot_token.clone(),
            bot_username: "fabushi_bot".to_string(),
            short_name: "fabushi".to_string(),
            db_path: ext_db_path,
            ..Default::default()
        })
        .expect("open FabushiServerState"),
    );
    let ext_router = build_router(ext_state.clone());
    let _ext_handle = tokio::spawn(async move {
        axum::serve(ext_listener, ext_router)
            .await
            .expect("serve external fabushi miniapp");
    });

    // 5. Re-run `publish_fabushi_miniapp` WITHOUT `--skip-miniapp-health-check` to test idempotency + live health checks
    let pub2 = tokio::process::Command::new(&cli_bin)
        .arg("--server-url")
        .arg(&im_base_url)
        .arg("--miniapp-url")
        .arg(&ext_base_url)
        .output()
        .await
        .expect("spawn publish_fabushi_miniapp (pass 2)");
    assert!(
        pub2.status.success(),
        "publish_fabushi_miniapp pass 2 failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&pub2.stdout),
        String::from_utf8_lossy(&pub2.stderr)
    );

    // 6. Verify static & proxy endpoints on external Mini App server
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("build reqwest client");

    let health_resp = client
        .get(format!("{ext_base_url}/health"))
        .send()
        .await
        .expect("GET /health");
    assert_eq!(health_resp.status(), reqwest::StatusCode::OK);
    let health_val: Value = health_resp.json().await.unwrap();
    assert_eq!(health_val["ok"], true);
    assert_eq!(health_val["service"], "fabushi_miniapp_server");

    let index_resp = client
        .get(format!("{ext_base_url}/"))
        .send()
        .await
        .expect("GET /");
    assert_eq!(index_resp.status(), reqwest::StatusCode::OK);
    let index_html = index_resp.text().await.unwrap();
    assert!(index_html.contains("navigator.modelContext"));

    let sdk_resp = client
        .get(format!("{ext_base_url}/js/telegram-web-app.js"))
        .send()
        .await
        .expect("GET /js/telegram-web-app.js");
    assert_eq!(sdk_resp.status(), reqwest::StatusCode::OK);
    let sdk_js = sdk_resp.text().await.unwrap();
    assert!(sdk_js.contains("Telegram.WebApp"));
    assert!(sdk_js.contains("navigator.modelContext"));

    let wasm_resp = client
        .get(format!("{ext_base_url}/api/v1/fabushi-miniapp/app.wasm"))
        .send()
        .await
        .expect("GET app.wasm");
    assert_eq!(wasm_resp.status(), reqwest::StatusCode::OK);
    let wasm_bytes = wasm_resp.bytes().await.unwrap();
    assert!(wasm_bytes.starts_with(b"\0asm"));

    let asset_resp = client
        .get(format!(
            "{ext_base_url}/api/v1/fabushi-miniapp/assets/earth_texture.jpg"
        ))
        .send()
        .await
        .expect("GET earth_texture.jpg");
    assert_eq!(asset_resp.status(), reqwest::StatusCode::OK);
    assert!(!asset_resp.bytes().await.unwrap().is_empty());

    // 7. Register end user on IM server, discover `@fabushi_bot/fabushi`, and launch to get signed initData
    let reg_resp = client
        .post(format!("{im_base_url}/api/v1/auth/register"))
        .json(&json!({
            "username": "dharma_seeker",
            "password": "SeekerStrongPassword!2026",
            "first_name": "Dharma",
            "last_name": "Seeker",
            "device_name": "ExternalE2ETest"
        }))
        .send()
        .await
        .expect("register end user");
    assert_eq!(reg_resp.status(), reqwest::StatusCode::CREATED);
    let reg_val: Value = reg_resp.json().await.unwrap();
    let end_uid = reg_val["user_id"].as_u64().expect("end_uid");
    let end_token = reg_val["token"].as_str().expect("end_token").to_string();

    let catalog_resp = client
        .get(format!("{im_base_url}/api/v1/miniapps?q=fabushi"))
        .bearer_auth(&end_token)
        .send()
        .await
        .expect("GET /api/v1/miniapps");
    assert_eq!(catalog_resp.status(), reqwest::StatusCode::OK);
    let catalog_val: Value = catalog_resp.json().await.unwrap();
    let apps = catalog_val["apps"].as_array().expect("apps array");
    let fabushi_entry = apps
        .iter()
        .find(|a| {
            a["bot_username"].as_str() == Some("fabushi_bot")
                && a["short_name"].as_str() == Some("fabushi")
        })
        .expect("@fabushi_bot/fabushi in catalog");
    assert_eq!(
        fabushi_entry["web_url"].as_str(),
        Some(ext_base_url.as_str()),
        "IM catalog must point to external fabushi_miniapp_server URL"
    );

    let launch_resp = client
        .post(format!(
            "{im_base_url}/api/v1/miniapps/fabushi_bot/fabushi/launch"
        ))
        .bearer_auth(&end_token)
        .json(&json!({}))
        .send()
        .await
        .expect("launch miniapp");
    assert_eq!(launch_resp.status(), reqwest::StatusCode::OK);
    let launch_ctx: MiniAppLaunchContext = launch_resp.json().await.expect("parse launch context");
    let init_data = launch_ctx.init_data.raw_query_string.clone();
    let bot_id = launch_ctx.companion_bot.bot_user_id;
    assert_eq!(launch_ctx.app.web_url, ext_base_url);
    assert!(!init_data.is_empty());

    // 8. External server rejects missing or tampered X-Telegram-Init-Data with 401
    let unauth_resp = client
        .get(format!("{ext_base_url}/api/v1/fabushi-miniapp/bootstrap"))
        .send()
        .await
        .expect("bootstrap without header");
    assert_eq!(unauth_resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    let tampered_init = format!("{init_data}&tamper=1");
    let bad_auth_resp = client
        .get(format!("{ext_base_url}/api/v1/fabushi-miniapp/bootstrap"))
        .header("X-Telegram-Init-Data", &tampered_init)
        .send()
        .await
        .expect("bootstrap with tampered initData");
    assert_eq!(bad_auth_resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 9. Authenticated calls with valid signed X-Telegram-Init-Data
    let boot_resp = client
        .get(format!("{ext_base_url}/api/v1/fabushi-miniapp/bootstrap"))
        .header("X-Telegram-Init-Data", &init_data)
        .send()
        .await
        .expect("GET bootstrap");
    assert_eq!(boot_resp.status(), reqwest::StatusCode::OK);
    let boot_val: Value = boot_resp.json().await.unwrap();
    assert_eq!(boot_val["type"], "bootstrap");
    assert_eq!(boot_val["profile"]["user_id"], end_uid);

    // Grant DM write permission via IM server
    let allow_resp_raw = client
        .post(format!(
            "{im_base_url}/api/v1/bots/{bot_id}/allow-send-message"
        ))
        .bearer_auth(&end_token)
        .json(&json!({}))
        .send()
        .await
        .expect("POST allow-send-message");
    assert_eq!(allow_resp_raw.status(), reqwest::StatusCode::OK);
    let allow_resp: AllowSendMessageResponse = allow_resp_raw.json().await.unwrap();
    let dm_chat_id = allow_resp.chat_id;
    let baseline_msgs = fetch_chat_messages(&client, &im_base_url, &end_token, dm_chat_id).await;
    let baseline_count = baseline_msgs.len();

    // 9a. Silent transfer (notify_receipt: false) -> updates DB totals, sends ZERO receipts
    let silent_tx_resp = client
        .post(format!("{ext_base_url}/api/v1/fabushi-miniapp/transfer"))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({
            "content_title": "心经",
            "content_kind": "sutra",
            "target_countries": ["CN", "JP"],
            "bytes_sent": 1024,
            "beam_count": 2,
            "notify_receipt": false
        }))
        .send()
        .await
        .expect("POST silent transfer");
    assert_eq!(silent_tx_resp.status(), reqwest::StatusCode::OK);
    let silent_tx_val: Value = silent_tx_resp.json().await.unwrap();
    assert_eq!(silent_tx_val["type"], "transfer_recorded");
    assert_eq!(silent_tx_val["total_transfer_count"], 1);
    assert_eq!(silent_tx_val["total_transfer_bytes"], 1024);

    let after_silent_msgs =
        fetch_chat_messages(&client, &im_base_url, &end_token, dm_chat_id).await;
    assert_eq!(
        after_silent_msgs.len(),
        baseline_count,
        "silent transfer must NOT send Bot API DM receipt"
    );

    // 9b. Final receipt-only transfer -> preserves count (no double count) & sends 1 Bot API DM receipt
    let final_tx_resp = client
        .post(format!("{ext_base_url}/api/v1/fabushi-miniapp/transfer"))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({
            "content_title": "心经",
            "content_kind": "sutra",
            "target_countries": ["CN", "JP"],
            "bytes_sent": 1024,
            "beam_count": 2,
            "notify_receipt": true,
            "receipt_only": true,
            "session_bytes": 2048,
            "session_beams": 4
        }))
        .send()
        .await
        .expect("POST final receipt transfer");
    assert_eq!(final_tx_resp.status(), reqwest::StatusCode::OK);
    let final_tx_val: Value = final_tx_resp.json().await.unwrap();
    assert_eq!(
        final_tx_val["total_transfer_count"], 1,
        "receipt_only must not double-count transfers"
    );

    let after_final_msgs =
        fetch_chat_messages(&client, &im_base_url, &end_token, dm_chat_id).await;
    assert_eq!(
        after_final_msgs.len(),
        baseline_count + 1,
        "final transfer receipt must deliver exactly 1 Bot API sendMessage receipt"
    );
    let tx_receipt_msg = after_final_msgs.last().expect("transfer receipt message");
    let tx_receipt_text = tx_receipt_msg["text"].as_str().unwrap_or("");
    assert!(tx_receipt_text.contains("全球法布施功德回执"));
    assert!(tx_receipt_text.contains("《心经》"));
    assert!(tx_receipt_text.contains("4 束法光"));
    assert_eq!(
        tx_receipt_msg["inline_keyboard"][0][0]["web_app_url"].as_str(),
        Some("gpui-miniapp://fabushi_bot/fabushi")
    );

    // 9c. Zen practice session -> delivers 1 Bot API DM receipt
    let session_resp = client
        .post(format!(
            "{ext_base_url}/api/v1/fabushi-miniapp/practice/session"
        ))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({
            "sutra": "金刚经",
            "duration_minutes": 15,
            "chant_count": 108
        }))
        .send()
        .await
        .expect("POST practice/session");
    assert_eq!(session_resp.status(), reqwest::StatusCode::OK);
    let session_val: Value = session_resp.json().await.unwrap();
    assert_eq!(session_val["type"], "practice_session_saved");

    let after_zen_msgs = fetch_chat_messages(&client, &im_base_url, &end_token, dm_chat_id).await;
    assert_eq!(after_zen_msgs.len(), baseline_count + 2);
    let zen_receipt_text = after_zen_msgs
        .last()
        .and_then(|m| m["text"].as_str())
        .unwrap_or("");
    assert!(zen_receipt_text.contains("禅室共修功德回执"));
    assert!(zen_receipt_text.contains("《金刚经》"));

    // 9d. Practice record CRUD (POST / GET / PUT / DELETE)
    let create_rec_resp = client
        .post(format!(
            "{ext_base_url}/api/v1/fabushi-miniapp/practice/record"
        ))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({
            "sutra_name": "楞严咒",
            "chant_count": 7,
            "duration_minutes": 20,
            "record_date": "2026-11-20",
            "local_time": "09:30",
            "notes": "Morning practice"
        }))
        .send()
        .await
        .expect("POST practice/record");
    assert_eq!(create_rec_resp.status(), reqwest::StatusCode::OK);
    let create_rec_val: Value = create_rec_resp.json().await.unwrap();
    assert_eq!(create_rec_val["type"], "practice_records_refreshed");
    let rec_id = create_rec_val["practice_records"]
        .as_array()
        .expect("records array")
        .iter()
        .find(|r| r["sutra_name"].as_str() == Some("楞严咒"))
        .and_then(|r| r["id"].as_i64())
        .expect("created record id");

    let get_rec_resp = client
        .get(format!(
            "{ext_base_url}/api/v1/fabushi-miniapp/practice/record"
        ))
        .header("X-Telegram-Init-Data", &init_data)
        .send()
        .await
        .expect("GET practice/record");
    assert_eq!(get_rec_resp.status(), reqwest::StatusCode::OK);
    let get_rec_val: Value = get_rec_resp.json().await.unwrap();
    assert!(get_rec_val["practice_records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"].as_i64() == Some(rec_id)));

    let upd_rec_resp = client
        .put(format!(
            "{ext_base_url}/api/v1/fabushi-miniapp/practice/record/{rec_id}"
        ))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({
            "sutra_name": "楞严咒",
            "chant_count": 21,
            "duration_minutes": 35,
            "record_date": "2026-11-20",
            "local_time": "09:30",
            "notes": "Updated morning practice"
        }))
        .send()
        .await
        .expect("PUT practice/record/{id}");
    assert_eq!(upd_rec_resp.status(), reqwest::StatusCode::OK);
    let upd_rec_val: Value = upd_rec_resp.json().await.unwrap();
    let updated_row = upd_rec_val["practice_records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"].as_i64() == Some(rec_id))
        .expect("updated record row");
    assert_eq!(updated_row["chant_count"], 21);

    let del_rec_resp = client
        .delete(format!(
            "{ext_base_url}/api/v1/fabushi-miniapp/practice/record/{rec_id}"
        ))
        .header("X-Telegram-Init-Data", &init_data)
        .send()
        .await
        .expect("DELETE practice/record/{id}");
    assert_eq!(del_rec_resp.status(), reqwest::StatusCode::OK);
    let del_rec_val: Value = del_rec_resp.json().await.unwrap();
    assert!(!del_rec_val["practice_records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"].as_i64() == Some(rec_id)));

    // 9e. Profile primary practice locking immutability
    let lock1_resp = client
        .put(format!("{ext_base_url}/api/v1/fabushi-miniapp/profile"))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({
            "display_name": "Dharma Seeker One",
            "locked_practice": {
                "title": "心经",
                "category": "sutra",
                "file_path": "",
                "selected_at_ms": 1700000000000i64,
                "is_locked": true
            }
        }))
        .send()
        .await
        .expect("PUT profile lock 1");
    assert_eq!(lock1_resp.status(), reqwest::StatusCode::OK);
    let lock1_val: Value = lock1_resp.json().await.unwrap();
    assert_eq!(lock1_val["type"], "profile_updated");
    assert_eq!(lock1_val["locked_practice"]["title"], "心经");

    let lock2_resp = client
        .put(format!("{ext_base_url}/api/v1/fabushi-miniapp/profile"))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({
            "locked_practice": {
                "title": "大悲咒",
                "category": "mantra",
                "file_path": "",
                "selected_at_ms": 1700000001000i64,
                "is_locked": true
            }
        }))
        .send()
        .await
        .expect("PUT profile lock 2");
    assert_eq!(lock2_resp.status(), reqwest::StatusCode::OK);
    let lock2_val: Value = lock2_resp.json().await.unwrap();
    assert_eq!(
        lock2_val["locked_practice"]["title"], "心经",
        "locked_practice must remain immutable once locked"
    );

    // 9f. Membership, follow, leaderboard, heartbeat, and AI chat fallback
    let vip_resp = client
        .post(format!("{ext_base_url}/api/v1/fabushi-miniapp/membership"))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({ "plan": "yearly" }))
        .send()
        .await
        .expect("POST membership");
    assert_eq!(vip_resp.status(), reqwest::StatusCode::OK);
    let vip_val: Value = vip_resp.json().await.unwrap();
    assert_eq!(vip_val["type"], "membership_activated");
    assert_eq!(vip_val["profile"]["is_vip"], true);

    let follow_resp = client
        .post(format!("{ext_base_url}/api/v1/fabushi-miniapp/follow"))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({ "target_username": "fabushi_official_publisher" }))
        .send()
        .await
        .expect("POST follow");
    assert_eq!(follow_resp.status(), reqwest::StatusCode::OK);
    let follow_val: Value = follow_resp.json().await.unwrap();
    assert_eq!(follow_val["type"], "follow_toggled");
    assert_eq!(follow_val["is_following"], true);

    let lb_resp = client
        .get(format!(
            "{ext_base_url}/api/v1/fabushi-miniapp/leaderboard?kind=global"
        ))
        .header("X-Telegram-Init-Data", &init_data)
        .send()
        .await
        .expect("GET leaderboard");
    assert_eq!(lb_resp.status(), reqwest::StatusCode::OK);
    let lb_val: Value = lb_resp.json().await.unwrap();
    assert_eq!(lb_val["type"], "leaderboard_fetched");
    assert_eq!(lb_val["kind"], "global");

    let hb_resp = client
        .post(format!("{ext_base_url}/api/v1/fabushi-miniapp/heartbeat"))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({ "activity_type": "global_sending" }))
        .send()
        .await
        .expect("POST heartbeat");
    assert_eq!(hb_resp.status(), reqwest::StatusCode::OK);
    let hb_val: Value = hb_resp.json().await.unwrap();
    assert_eq!(hb_val["type"], "heartbeat_ack");

    let ai_resp = client
        .post(format!("{ext_base_url}/api/v1/fabushi-miniapp/ai-chat"))
        .header("X-Telegram-Init-Data", &init_data)
        .json(&json!({ "message": "请问如何修持金刚经？" }))
        .send()
        .await
        .expect("POST ai-chat");
    assert_eq!(ai_resp.status(), reqwest::StatusCode::OK);
    let ai_val: Value = ai_resp.json().await.unwrap();
    assert_eq!(ai_val["type"], "ai_chat_reply");
    assert!(ai_val["reply"].as_str().unwrap_or("").contains("金刚经"));

    // 10. Confirm IM server DB still has 0 fabushi_* tables while external fabushi.db stores user data
    assert_eq!(count_fabushi_tables_in_im_db(&im_state), 0);
    let stored_profile = {
        let conn = ext_state.db.lock();
        storage::get_profile(&conn, end_uid as i64).expect("query external fabushi profile")
    };
    assert_eq!(stored_profile.total_transfer_count, 1);
    assert_eq!(stored_profile.total_practice_sessions, 1);
}
