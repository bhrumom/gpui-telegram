use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use futures::{SinkExt, StreamExt};
use gpui_telegram_agent::hermes::toolsets::{
    find_tool_spec, HermesToolContext, HermesToolExecutor, PlatformImCaller, ToolsetId,
    ToolsetPreset,
};
use gpui_telegram_core::core::deep_links::{parse_apple_deep_link, AppleDeepLinkSection};
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_media::{
    decode_wav_pcm, slice_wav_from_offset_ms, synthesize_music_track_wav, SynthInstrument,
    SynthNote, SynthTrackInput, SynthWaveformKind,
};
use gpui_telegram_proto::creations::{
    validate_creation_for_review, UserCreationDto, CREATOR_STUDIO_BOT_USER_ID,
    CREATOR_STUDIO_BOT_USERNAME,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use gpui_telegram_server::{
    db::social_store::MediaMetadataDto,
    server::{build_router_with_state, AppState},
    ServerConfig,
};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message as TungsteniteMessage;
use tower::ServiceExt;

const CREATOR_BOT_TOKEN: &str = "777020:AAOfficialCreatorStudioBotV1";

fn make_temp_state() -> (TempDir, ServerConfig, Arc<AppState>, Router) {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("creator_bot_test.db");
    let media_dir = dir.path().join("media");
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    let state = Arc::new(AppState::try_new(cfg.clone()).expect("AppState::try_new"));
    let app = build_router_with_state(state.clone());
    (dir, cfg, state, app)
}

async fn http_json(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(tok) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {tok}"));
    }
    let body_bytes = match body {
        Some(v) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            serde_json::to_vec(&v).unwrap()
        }
        None => Vec::new(),
    };
    let req = builder.body(Body::from(body_bytes)).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 32 * 1024 * 1024).await.unwrap();
    let val = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, val)
}

async fn http_form(app: &Router, method: &str, uri: &str, form_body: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form_body.as_bytes().to_vec()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 16 * 1024 * 1024).await.unwrap();
    let val = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, val)
}

async fn http_raw_json_body(
    app: &Router,
    method: &str,
    uri: &str,
    raw_body: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(raw_body.as_bytes().to_vec()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 16 * 1024 * 1024).await.unwrap();
    let val = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, val)
}

async fn http_raw(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(tok) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {tok}"));
    }
    let req = builder.body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 32 * 1024 * 1024).await.unwrap();
    (status, bytes.to_vec())
}

async fn register_user(
    app: &Router,
    username: &str,
    first_name: &str,
    last_name: Option<&str>,
) -> (u64, String) {
    let mut payload = json!({
        "username": username,
        "password": "StrongPassword123!",
        "first_name": first_name,
        "device_name": "TestDevice"
    });
    if let Some(ln) = last_name {
        payload["last_name"] = json!(ln);
    }
    let (status, body) = http_json(app, "POST", "/api/v1/auth/register", None, Some(payload)).await;
    assert_eq!(status, StatusCode::CREATED, "register failed: {body}");
    (
        body["user_id"].as_u64().unwrap(),
        body["token"].as_str().unwrap().to_string(),
    )
}

fn promote_admin(state: &AppState, user_id: u64) {
    state
        .db
        .pool()
        .with_writer(|conn| {
            conn.execute(
                "UPDATE users SET is_support = 1 WHERE user_id = ?1",
                rusqlite::params![user_id as i64],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok(())
        })
        .expect("promote admin");
}

fn build_im_tool_ctx(
    state: &Arc<AppState>,
    user_id: Option<u64>,
    chat_id: i64,
    bot_id: i64,
) -> HermesToolContext {
    let uid_str = user_id.map(|u| u.to_string());
    let mut ctx = state.fabushi_core().build_tool_context_with_user(
        &format!("sess_{:?}_{chat_id}", user_id),
        Some(chat_id),
        None,
        uid_str.as_deref(),
        1,
        Some(ToolsetPreset::Full),
        0,
        Arc::new(AtomicBool::new(false)),
    );
    ctx.user_id = user_id;
    ctx.bot_id = Some(bot_id);
    ctx
}

async fn open_creator_bot_dm(app: &Router, token: &str) -> (i64, usize) {
    let (status, body) = http_json(
        app,
        "POST",
        "/api/v1/contacts/add",
        Some(token),
        Some(json!({ "username": CREATOR_STUDIO_BOT_USERNAME, "is_bot": true })),
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "contacts/add failed ({status}): {body}"
    );
    let chat_id = body["chat_id"].as_i64().expect("chat_id");
    let (ms, msgs_body) = http_json(
        app,
        "GET",
        &format!("/api/v1/chats/{chat_id}/messages?limit=100"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(ms, StatusCode::OK);
    let count = msgs_body["messages"].as_array().map_or(0, Vec::len);
    (chat_id, count)
}

async fn send_bot_cmd_and_wait(
    app: &Router,
    token: &str,
    chat_id: i64,
    text: &str,
) -> Value {
    let (st, sent) = http_json(
        app,
        "POST",
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(token),
        Some(json!({ "text": text })),
    )
    .await;
    assert!(
        st == StatusCode::OK || st == StatusCode::CREATED,
        "send message failed ({st}): {sent}"
    );
    let sent_id = sent["id"].as_i64().or_else(|| sent["message"]["id"].as_i64()).unwrap_or(0);

    for _ in 0..120 {
        let (ms, body) = http_json(
            app,
            "GET",
            &format!("/api/v1/chats/{chat_id}/messages?limit=100"),
            Some(token),
            None,
        )
        .await;
        assert_eq!(ms, StatusCode::OK);
        if let Some(arr) = body["messages"].as_array() {
            if let Some(reply) = arr.iter().rev().find(|m| {
                m["id"].as_i64().unwrap_or(0) > sent_id
                    && m["sender_id"].as_i64() == Some(CREATOR_STUDIO_BOT_USER_ID)
            }) {
                return reply.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Timed out waiting for @CreatorStudioBot reply to: {text}");
}

fn extract_inline_urls(reply: &Value) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(rows) = reply.get("inline_keyboard").and_then(Value::as_array) {
        for row in rows {
            if let Some(btns) = row.as_array() {
                for btn in btns {
                    if let Some(u) = btn.get("url").and_then(Value::as_str) {
                        urls.push(u.to_string());
                    }
                }
            }
        }
    }
    urls
}

fn extract_creation_id_from_reply(reply: &Value) -> String {
    for u in extract_inline_urls(reply) {
        if let Some(parsed) = parse_apple_deep_link(&u) {
            if let Some(cid) = parsed.creation_id {
                return cid;
            }
        }
    }
    panic!("No creation_id deep link found in bot reply: {reply}");
}

async fn submit_and_approve(
    app: &Router,
    owner_tok: &str,
    admin_tok: &str,
    creation_id: &str,
    visibility: &str,
) -> Value {
    let (st, sub) = http_json(
        app,
        "POST",
        &format!("/api/v1/creations/{creation_id}/submit-review"),
        Some(owner_tok),
        Some(json!({ "visibility": visibility })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "submit-review failed: {sub}");
    let (rst, rev) = http_json(
        app,
        "POST",
        &format!("/api/v1/creations/{creation_id}/review"),
        Some(admin_tok),
        Some(json!({ "decision": "approve", "visibility": visibility })),
    )
    .await;
    assert_eq!(rst, StatusCode::OK, "review approve failed: {rev}");
    rev
}

async fn ws_connect_and_handshake(
    addr: std::net::SocketAddr,
    token: &str,
    nonce: u64,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://{addr}/api/v1/ws?token={token}");
    let (mut ws, _) = connect_async(&url).await.expect("ws connect");
    let ping = serde_json::to_string(&WsClientFrame::Ping { nonce }).unwrap();
    ws.send(TungsteniteMessage::Text(ping.into())).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Some(Ok(TungsteniteMessage::Text(txt)))) =
            tokio::time::timeout(rem, ws.next()).await
        {
            if let Ok(WsServerFrame::Pong { nonce: n }) = serde_json::from_str(&txt) {
                if n == nonce {
                    return ws;
                }
            }
        }
    }
    panic!("WS Ping/Pong handshake timed out");
}

async fn ws_next_creation_event(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    timeout_ms: u64,
) -> Option<UpdateEvent> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    while tokio::time::Instant::now() < deadline {
        let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(rem, ws.next()).await {
            Ok(Some(Ok(TungsteniteMessage::Text(txt)))) => {
                if let Ok(WsServerFrame::Update(ev)) = serde_json::from_str(&txt) {
                    if matches!(
                        ev,
                        UpdateEvent::UserCreationChanged { .. }
                            | UpdateEvent::PublicCatalogChanged { .. }
                    ) {
                        return Some(ev);
                    }
                }
            }
            Ok(Some(Ok(_))) => continue,
            _ => break,
        }
    }
    None
}

#[tokio::test]
async fn test_01_creator_studio_bot_seeding_and_29_toolset_invariant() {
    assert_eq!(ToolsetId::all().len(), 29);
    for tool_name in [
        "create_book",
        "create_music",
        "create_game",
        "update_my_creation",
        "list_my_creations",
        "submit_creation_for_review",
        "search_public_catalog",
        "recommend_public_creations",
    ] {
        let spec = find_tool_spec(tool_name).unwrap_or_else(|| panic!("Missing tool {tool_name}"));
        assert_eq!(spec.toolset, ToolsetId::PlatformIm);
    }

    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_01", "Alice", Some("Author")).await;
    assert!(
        alice_uid >= 100_001 && alice_uid < 500_000,
        "Human UID allocation must stay in 100_001..500_000, got {alice_uid}"
    );

    // Zero regressions on empty new-user chats, miniapps, and bots/my
    let (s_chats, chats) = http_json(&app, "GET", "/api/v1/chats", Some(&alice_tok), None).await;
    assert_eq!(s_chats, StatusCode::OK);
    assert_eq!(chats["chats"].as_array().map_or(0, Vec::len), 0);

    let (s_mybots, mybots) = http_json(&app, "GET", "/api/v1/bots/my", Some(&alice_tok), None).await;
    assert_eq!(s_mybots, StatusCode::OK);
    assert_eq!(mybots["bots"].as_array().map_or(0, Vec::len), 0);

    let (s_apps, apps) = http_json(&app, "GET", "/api/v1/miniapps", Some(&alice_tok), None).await;
    assert_eq!(s_apps, StatusCode::OK);
    assert_eq!(apps["apps"].as_array().map_or(0, Vec::len), 0);

    // Discoverable via GET /api/v1/bots
    let (s_bots, bots) = http_json(&app, "GET", "/api/v1/bots", Some(&alice_tok), None).await;
    assert_eq!(s_bots, StatusCode::OK, "GET /api/v1/bots failed: {bots}");
    let creator_bot = bots["bots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["bot_user_id"].as_i64() == Some(CREATOR_STUDIO_BOT_USER_ID))
        .expect("@CreatorStudioBot must appear in GET /api/v1/bots");
    assert_eq!(creator_bot["username"], CREATOR_STUDIO_BOT_USERNAME);
    assert_eq!(creator_bot["is_official"], true);
    assert_eq!(creator_bot["is_inline"], true);
    assert_eq!(creator_bot["commands"].as_array().unwrap().len(), 13);

    // Discoverable via GET /api/v1/bots/profile/CreatorStudioBot & peers/resolve
    let (s_prof, prof) = http_json(
        &app,
        "GET",
        "/api/v1/bots/profile/CreatorStudioBot",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(s_prof, StatusCode::OK, "profile failed: {prof}");
    assert_eq!(prof["bot"]["bot_user_id"], CREATOR_STUDIO_BOT_USER_ID);
    assert_eq!(prof["bot"]["is_inline"], true);
    assert_eq!(prof["bot"]["is_official"], true);

    let (s_res, resolved) = http_json(
        &app,
        "GET",
        "/api/v1/peers/resolve?username=CreatorStudioBot",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(s_res, StatusCode::OK, "resolve failed: {resolved}");
    assert_eq!(resolved["user"]["user_id"], CREATOR_STUDIO_BOT_USER_ID);
    assert_eq!(resolved["user"]["is_bot"], true);

    // Telegram Bot API getMe works with @CreatorStudioBot token & reports inline support
    let (s_me, me) = http_json(
        &app,
        "GET",
        &format!("/bot/{CREATOR_BOT_TOKEN}/getMe"),
        None,
        None,
    )
    .await;
    assert_eq!(s_me, StatusCode::OK, "getMe failed: {me}");
    assert_eq!(me["ok"], true);
    assert_eq!(me["result"]["id"], CREATOR_STUDIO_BOT_USER_ID);
    assert_eq!(me["result"]["username"], CREATOR_STUDIO_BOT_USERNAME);
    assert_eq!(me["result"]["supports_inline_queries"], true);

    // BotFather & fabushi_ai_bot remain intact
    let (s_fb, fb) = http_json(&app, "GET", "/api/v1/fabushi-bots", Some(&alice_tok), None).await;
    assert_eq!(s_fb, StatusCode::OK);
    let fb_arr = fb["bots"].as_array().unwrap();
    assert!(fb_arr.iter().any(|b| b["bot_user_id"].as_i64() == Some(777_000)));
    assert!(fb_arr
        .iter()
        .any(|b| b["bot_user_id"].as_i64() == Some(CREATOR_STUDIO_BOT_USER_ID)));

    state.shutdown();
}

#[tokio::test]
async fn test_02_hermes_platform_im_creation_tools_lifecycle_and_validation() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_02", "Alice", Some("Maker")).await;
    let (bob_uid, bob_tok) = register_user(&app, "bob_m3_02", "Bob", Some("Reader")).await;
    let (admin_uid, admin_tok) = register_user(&app, "admin_m3_02", "Admin", None).await;
    promote_admin(&state, admin_uid);

    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), 9001, CREATOR_STUDIO_BOT_USER_ID);
    let bob_ctx = build_im_tool_ctx(&state, Some(bob_uid), 9002, CREATOR_STUDIO_BOT_USER_ID);

    // 1. create_book
    let book_res = HermesToolExecutor::execute(
        "create_book",
        json!({
            "title": "Atlas of Nebulae",
            "summary": "A vivid deep-space guide covering star clusters and cosmic voyages."
        }),
        &alice_ctx,
    )
    .await;
    assert!(book_res.success, "create_book failed: {}", book_res.output);
    let book_s = book_res.structured.unwrap();
    let book_id = book_s["creation_id"].as_str().unwrap().to_string();
    assert_eq!(book_s["deep_link"], format!("tg://apple/creations/{book_id}"));
    let book_dto: UserCreationDto = serde_json::from_value(book_s["creation"].clone()).unwrap();
    assert_eq!(book_dto.owner_user_id, alice_uid as i64);
    assert_eq!(book_dto.creator_bot_user_id, Some(CREATOR_STUDIO_BOT_USER_ID));
    assert_eq!(book_dto.creator_chat_id, Some(9001));
    assert!(validate_creation_for_review(&book_dto).is_ok());

    // 2. create_music (real WAV synthesis + CAS + media_files ACL lifecycle)
    let music_res = HermesToolExecutor::execute(
        "create_music",
        json!({
            "title": "Midnight Pulse",
            "summary": "Atmospheric synthwave track with synthesized piano, warm pad, and bass.",
            "bpm": 116
        }),
        &alice_ctx,
    )
    .await;
    assert!(music_res.success, "create_music failed: {}", music_res.output);
    let music_s = music_res.structured.unwrap();
    let music_id = music_s["creation_id"].as_str().unwrap().to_string();
    let media_ids = music_s["audio_media_file_ids"].as_array().unwrap();
    assert_eq!(media_ids.len(), 1, "Expected synthesized WAV media_id in CAS");
    let wav_media_id = media_ids[0].as_str().unwrap().to_string();
    let music_dto: UserCreationDto = serde_json::from_value(music_s["creation"].clone()).unwrap();
    assert!(validate_creation_for_review(&music_dto).is_ok());
    if let gpui_telegram_proto::creations::CreationPayload::Music(ref mp) = music_dto.payload {
        assert_eq!(mp.tracks[0].waveform_5bit.len(), 63);
        assert_eq!(mp.tracks[0].audio_media_file_id.as_deref(), Some(wav_media_id.as_str()));
    } else {
        panic!("Expected MusicPayload");
    }

    // ACL check: Owner can stream draft WAV, Bob gets 403 while draft, 200 after publish, 403 after unpublish
    let (st_owner, wav_bytes) = http_raw(&app, "GET", &format!("/api/v1/media/{wav_media_id}"), Some(&alice_tok)).await;
    assert_eq!(st_owner, StatusCode::OK);
    assert!(wav_bytes.starts_with(b"RIFF") && &wav_bytes[8..12] == b"WAVE");
    let (st_bob_draft, _) = http_raw(&app, "GET", &format!("/api/v1/media/{wav_media_id}"), Some(&bob_tok)).await;
    assert_eq!(st_bob_draft, StatusCode::FORBIDDEN);
    submit_and_approve(&app, &alice_tok, &admin_tok, &music_id, "public").await;
    let (st_bob_pub, pub_wav) = http_raw(&app, "GET", &format!("/api/v1/media/{wav_media_id}"), Some(&bob_tok)).await;
    assert_eq!(st_bob_pub, StatusCode::OK);
    assert_eq!(pub_wav, wav_bytes);
    let (st_unpub, _) = http_json(&app, "POST", &format!("/api/v1/creations/{music_id}/unpublish"), Some(&alice_tok), None).await;
    assert_eq!(st_unpub, StatusCode::OK);
    let (st_bob_after, _) = http_raw(&app, "GET", &format!("/api/v1/media/{wav_media_id}"), Some(&bob_tok)).await;
    assert_eq!(st_bob_after, StatusCode::FORBIDDEN);

    // 3. create_game across all 6 native GameEngineKind variants
    for engine in ["grid_puzzle", "trivia_quiz", "interactive_fiction", "arcade_dodge", "snake_grid", "card_match"] {
        let g_res = HermesToolExecutor::execute(
            "create_game",
            json!({
                "title": format!("Test Game {engine}"),
                "engine_kind": engine
            }),
            &alice_ctx,
        )
        .await;
        assert!(g_res.success, "create_game {engine} failed: {}", g_res.output);
        let g_dto: UserCreationDto = serde_json::from_value(g_res.structured.unwrap()["creation"].clone()).unwrap();
        assert!(validate_creation_for_review(&g_dto).is_ok(), "engine {engine} invalid");
    }

    // 4. list_my_creations scoped strictly to caller
    let alice_list = HermesToolExecutor::execute("list_my_creations", json!({ "kind": "book" }), &alice_ctx).await;
    assert!(alice_list.success);
    assert_eq!(alice_list.structured.unwrap()["count"], 1);
    let bob_list = HermesToolExecutor::execute("list_my_creations", json!({}), &bob_ctx).await;
    assert!(bob_list.success);
    assert_eq!(bob_list.structured.unwrap()["count"], 0);

    // 5. update_my_creation CAS check
    let upd_ok = HermesToolExecutor::execute(
        "update_my_creation",
        json!({
            "creation_id": book_id,
            "expected_revision": book_dto.cloud_revision,
            "title": "Atlas of Nebulae Revised"
        }),
        &alice_ctx,
    )
    .await;
    assert!(upd_ok.success, "update_my_creation failed: {}", upd_ok.output);
    let upd_stale = HermesToolExecutor::execute(
        "update_my_creation",
        json!({
            "creation_id": book_id,
            "expected_revision": book_dto.cloud_revision,
            "title": "Stale Edit"
        }),
        &alice_ctx,
    )
    .await;
    assert!(!upd_stale.success);
    assert!(upd_stale.output.contains("revision_conflict") || upd_stale.error.as_deref().unwrap_or("").contains("revision_conflict"));

    // 6. submit_creation_for_review valid + structured validation_errors on invalid draft
    let sub_ok = HermesToolExecutor::execute(
        "submit_creation_for_review",
        json!({ "creation_id": book_id, "visibility": "public" }),
        &alice_ctx,
    )
    .await;
    assert!(sub_ok.success, "submit failed: {}", sub_ok.output);
    assert_eq!(sub_ok.structured.unwrap()["review_status"], "pending_review");

    let bad_book = HermesToolExecutor::execute(
        "create_book",
        json!({ "title": "Broken Draft", "summary": "short", "chapters": [] }),
        &alice_ctx,
    )
    .await;
    assert!(bad_book.success);
    let bad_id = bad_book.structured.unwrap()["creation_id"].as_str().unwrap().to_string();
    let sub_bad = HermesToolExecutor::execute(
        "submit_creation_for_review",
        json!({ "creation_id": bad_id }),
        &alice_ctx,
    )
    .await;
    assert!(!sub_bad.success);
    let bad_struct = sub_bad.structured.expect("structured validation payload");
    assert_eq!(bad_struct["error"], "validation_failed");
    assert!(!bad_struct["validation_errors"].as_array().unwrap().is_empty());

    // Cloud REST my & sync reflect bot creations
    let (s_my, my_body) = http_json(&app, "GET", "/api/v1/creations/my", Some(&alice_tok), None).await;
    assert_eq!(s_my, StatusCode::OK);
    assert!(my_body["items"].as_array().unwrap().len() >= 9);
    let (s_sync, sync_body) = http_json(&app, "GET", "/api/v1/creations/sync?since_revision=0", Some(&alice_tok), None).await;
    assert_eq!(s_sync, StatusCode::OK);
    assert!(sync_body["latest_revision"].as_i64().unwrap() >= 9);

    state.shutdown();
}

#[tokio::test]
async fn test_03_hermes_tools_cross_user_isolation_and_caller_pinning() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, _alice_tok) = register_user(&app, "alice_m3_03", "Alice", None).await;
    let (bob_uid, _bob_tok) = register_user(&app, "bob_m3_03", "Bob", None).await;

    // Missing caller.user_id (even with bot_id present) MUST be rejected by private creation tools
    let no_user_ctx = build_im_tool_ctx(&state, None, 9100, CREATOR_STUDIO_BOT_USER_ID);
    for (tool, args) in [
        ("create_book", json!({ "title": "Spoofed", "owner_user_id": alice_uid })),
        ("list_my_creations", json!({ "owner_user_id": alice_uid })),
        ("update_my_creation", json!({ "creation_id": "cr_any", "title": "X" })),
        ("submit_creation_for_review", json!({ "creation_id": "cr_any" })),
    ] {
        let res = HermesToolExecutor::execute(tool, args, &no_user_ctx).await;
        assert!(!res.success, "{tool} must reject missing caller.user_id");
    }

    // Spoofed owner_user_id argument is strictly ignored; pinned to caller.user_id
    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), 9101, CREATOR_STUDIO_BOT_USER_ID);
    let created = HermesToolExecutor::execute(
        "create_book",
        json!({ "title": "Alice Secret Novel", "owner_user_id": bob_uid }),
        &alice_ctx,
    )
    .await;
    assert!(created.success);
    let cs = created.structured.unwrap();
    let cid = cs["creation_id"].as_str().unwrap().to_string();
    assert_eq!(cs["creation"]["owner_user_id"], alice_uid as i64);

    // Bob cannot list, edit, or submit Alice's creation
    let bob_ctx = build_im_tool_ctx(&state, Some(bob_uid), 9102, CREATOR_STUDIO_BOT_USER_ID);
    let bob_list = HermesToolExecutor::execute("list_my_creations", json!({ "owner_user_id": alice_uid }), &bob_ctx).await;
    assert!(bob_list.success);
    assert_eq!(bob_list.structured.unwrap()["count"], 0);

    let bob_edit = HermesToolExecutor::execute(
        "update_my_creation",
        json!({ "creation_id": cid, "title": "Hijacked by Bob", "owner_user_id": alice_uid }),
        &bob_ctx,
    )
    .await;
    assert!(!bob_edit.success);

    let bob_submit = HermesToolExecutor::execute(
        "submit_creation_for_review",
        json!({ "creation_id": cid, "owner_user_id": alice_uid }),
        &bob_ctx,
    )
    .await;
    assert!(!bob_submit.success);

    // Verify Alice's creation remains completely untouched
    let alice_list = HermesToolExecutor::execute("list_my_creations", json!({}), &alice_ctx).await;
    let items = alice_list.structured.unwrap()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Alice Secret Novel");
    assert_eq!(items[0]["review_status"], "draft");

    state.shutdown();
}

#[tokio::test]
async fn test_04_creator_studio_bot_slash_commands_and_deep_link_cards() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (_alice_uid, alice_tok) = register_user(&app, "alice_m3_04", "Alice", Some("Writer")).await;
    let (_bob_uid, bob_tok) = register_user(&app, "bob_m3_04", "Bob", None).await;

    let (alice_chat, _) = open_creator_bot_dm(&app, &alice_tok).await;
    let (bob_chat, _) = open_creator_bot_dm(&app, &bob_tok).await;

    // /start and /help
    let start_rep = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, "/start").await;
    assert_eq!(start_rep["content"]["type"], "GenerativeCard");
    assert!(start_rep["text"].as_str().unwrap().contains("/newbook"));
    let help_rep = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, "/help@CreatorStudioBot").await;
    assert_eq!(help_rep["content"]["type"], "GenerativeCard");
    assert!(help_rep["text"].as_str().unwrap().contains("/recommend"));

    // /newbook, /newmusic, /newgame
    let book_rep = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        "/newbook 星海航路 | 一部关于深空探索的交互式长篇科幻作品",
    )
    .await;
    assert_eq!(book_rep["sender_id"], CREATOR_STUDIO_BOT_USER_ID);
    assert_eq!(book_rep["content"]["type"], "GenerativeCard");
    let book_id = extract_creation_id_from_reply(&book_rep);
    assert!(book_rep["text"].as_str().unwrap().contains(&book_id));
    let book_url = format!("tg://apple/creations/{book_id}");
    assert!(extract_inline_urls(&book_rep).contains(&book_url));
    assert_eq!(
        parse_apple_deep_link(&book_url).unwrap().section,
        AppleDeepLinkSection::Creations
    );

    let music_rep = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        "/newmusic Neon Drift | Ambient synthwave track for late night coding",
    )
    .await;
    let music_id = extract_creation_id_from_reply(&music_rep);
    let (ms, music_json) = http_json(&app, "GET", &format!("/api/v1/creations/{music_id}"), Some(&alice_tok), None).await;
    assert_eq!(ms, StatusCode::OK);
    let music_dto: UserCreationDto = serde_json::from_value(music_json).unwrap();
    assert!(validate_creation_for_review(&music_dto).is_ok());
    if let gpui_telegram_proto::creations::CreationPayload::Music(ref mp) = music_dto.payload {
        assert_eq!(mp.tracks[0].waveform_5bit.len(), 63);
        assert!(mp.tracks[0].audio_media_file_id.is_some());
    } else {
        panic!("Expected MusicPayload");
    }

    let game_rep = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        "/newgame Orbit Quiz | Space trivia challenge for players",
    )
    .await;
    let game_id = extract_creation_id_from_reply(&game_rep);
    let (gs, game_json) = http_json(&app, "GET", &format!("/api/v1/creations/{game_id}"), Some(&alice_tok), None).await;
    assert_eq!(gs, StatusCode::OK);
    let game_dto: UserCreationDto = serde_json::from_value(game_json).unwrap();
    assert!(validate_creation_for_review(&game_dto).is_ok());

    // /mycreations and filtered /mycreations book
    let my_all = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, "/mycreations").await;
    let my_all_txt = my_all["text"].as_str().unwrap();
    assert!(my_all_txt.contains(&book_id) && my_all_txt.contains(&music_id) && my_all_txt.contains(&game_id));

    let my_books = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, "/mycreations book").await;
    let my_books_txt = my_books["text"].as_str().unwrap();
    assert!(my_books_txt.contains(&book_id));
    assert!(!my_books_txt.contains(&music_id));

    // /editwork & /submitwork
    let edit_rep = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        &format!("/editwork {book_id} title=星海航路修订版 summary=修订后的深空探索互动科幻史诗作品"),
    )
    .await;
    assert!(edit_rep["text"].as_str().unwrap().contains("星海航路修订版"));

    let sub_rep = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        &format!("/submitwork {book_id}"),
    )
    .await;
    assert!(sub_rep["text"].as_str().unwrap().contains("pending_review"));
    let (st_check, checked_book) = http_json(&app, "GET", &format!("/api/v1/creations/{book_id}"), Some(&alice_tok), None).await;
    assert_eq!(st_check, StatusCode::OK);
    assert_eq!(checked_book["title"], "星海航路修订版");
    assert_eq!(checked_book["review_status"], "pending_review");

    // Cross-user /editwork and /submitwork by Bob MUST be denied without leaking Alice's title
    let bob_edit = send_bot_cmd_and_wait(
        &app,
        &bob_tok,
        bob_chat,
        &format!("/editwork {music_id} title=HijackedTitle"),
    )
    .await;
    assert!(bob_edit["text"].as_str().unwrap().to_ascii_lowercase().contains("denied"));
    assert!(!bob_edit["text"].as_str().unwrap().contains("Neon Drift"));

    let bob_sub = send_bot_cmd_and_wait(
        &app,
        &bob_tok,
        bob_chat,
        &format!("/submitwork {music_id}"),
    )
    .await;
    assert!(bob_sub["text"].as_str().unwrap().to_ascii_lowercase().contains("denied"));

    // /submitwork on an invalid draft surfaces validation_errors cleanly
    let (st_bad, bad_created) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(json!({
            "kind": "book",
            "title": "Empty Book",
            "summary": "short",
            "author_display_name": "Alice",
            "payload": { "kind": "book", "author": "Alice", "chapters": [] }
        })),
    )
    .await;
    assert!(st_bad == StatusCode::CREATED || st_bad == StatusCode::OK);
    let bad_id = bad_created["creation_id"].as_str().unwrap();
    let bad_sub = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        &format!("/submitwork {bad_id}"),
    )
    .await;
    assert!(bad_sub["text"].as_str().unwrap().contains("validation_errors"));

    state.shutdown();
}

#[tokio::test]
async fn test_05_creator_studio_bot_nl_creation_fallback_en_and_cjk() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_05", "Alice", None).await;
    let (chat_id, _) = open_creator_bot_dm(&app, &alice_tok).await;

    let cases = [
        ("帮我创作一本科幻图书叫《量子花园》", "book", "量子花园"),
        ("Create a chill lo-fi music track called Rainy Kyoto", "music", "Rainy Kyoto"),
        ("做一个太空益智小游戏叫星际迷宫", "game", "星际迷宫"),
        ("Create a book about Alice's journey called Starlight Odyssey", "book", "Starlight Odyssey"),
    ];

    for (prompt, expected_kind, expected_title) in cases {
        let rep = send_bot_cmd_and_wait(&app, &alice_tok, chat_id, prompt).await;
        assert_eq!(rep["content"]["type"], "GenerativeCard");
        let cid = extract_creation_id_from_reply(&rep);
        let (st, created) = http_json(&app, "GET", &format!("/api/v1/creations/{cid}"), Some(&alice_tok), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(created["owner_user_id"], alice_uid as i64);
        assert_eq!(created["kind"], expected_kind);
        assert_eq!(created["title"], expected_title);
        let dto: UserCreationDto = serde_json::from_value(created).unwrap();
        assert!(
            validate_creation_for_review(&dto).is_ok(),
            "NL creation '{expected_title}' must pass review validation"
        );
    }

    state.shutdown();
}

#[tokio::test]
async fn test_06_public_recommendations_across_bot_tools_slash_and_inline_query() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_06", "Alice", Some("Studio")).await;
    let (bob_uid, bob_tok) = register_user(&app, "bob_m3_06", "Bob", None).await;
    let (admin_uid, admin_tok) = register_user(&app, "admin_m3_06", "Admin", None).await;
    promote_admin(&state, admin_uid);

    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), 9201, CREATOR_STUDIO_BOT_USER_ID);
    let bob_ctx = build_im_tool_ctx(&state, Some(bob_uid), 9202, CREATOR_STUDIO_BOT_USER_ID);

    // Create & publish Book (CJK), Music (EN), Game (EN+CJK), and App (EN)
    let book_id = HermesToolExecutor::execute(
        "create_book",
        json!({ "title": "星海航路", "summary": "面向深空探索者的硬科幻长篇云端读物，包含星图与编年史。" }),
        &alice_ctx,
    )
    .await
    .structured
    .unwrap()["creation_id"]
        .as_str()
        .unwrap()
        .to_string();
    submit_and_approve(&app, &alice_tok, &admin_tok, &book_id, "public").await;

    let music_id = HermesToolExecutor::execute(
        "create_music",
        json!({ "title": "Neon Horizon", "summary": "Lush retro synthwave album for late-night focus and coding." }),
        &alice_ctx,
    )
    .await
    .structured
    .unwrap()["creation_id"]
        .as_str()
        .unwrap()
        .to_string();
    submit_and_approve(&app, &alice_tok, &admin_tok, &music_id, "public").await;

    let game_id = HermesToolExecutor::execute(
        "create_game",
        json!({ "title": "Cyber 星际迷宫", "summary": "Fast-paced cyber puzzle maze adventure with global leaderboard." }),
        &alice_ctx,
    )
    .await
    .structured
    .unwrap()["creation_id"]
        .as_str()
        .unwrap()
        .to_string();
    submit_and_approve(&app, &alice_tok, &admin_tok, &game_id, "public").await;

    let (st_app, app_created) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(json!({
            "kind": "app",
            "title": "Focus Timer Pro",
            "summary": "Minimalist Apple-style pomodoro and deep-work companion utility.",
            "author_display_name": "Alice Studio",
            "payload": {
                "kind": "app",
                "developer_name": "Alice Studio",
                "version_string": "1.0.0",
                "category": "Productivity",
                "long_description": "A focused minimalist workspace timer designed for deep creative sessions and daily rhythm.",
                "ui_schema_json": { "version": 1, "components": [] }
            }
        })),
    )
    .await;
    assert!(st_app == StatusCode::CREATED || st_app == StatusCode::OK);
    let app_id = app_created["creation_id"].as_str().unwrap().to_string();
    submit_and_approve(&app, &alice_tok, &admin_tok, &app_id, "public").await;

    // Add a 5-star review on Neon Horizon
    let (st_rev, _) = http_json(
        &app,
        "POST",
        &format!("/api/v1/catalog/{music_id}/reviews"),
        Some(&bob_tok),
        Some(json!({ "rating": 5, "title": "Great", "body": "Super crisp synth tones!" })),
    )
    .await;
    assert!(st_rev == StatusCode::OK || st_rev == StatusCode::CREATED);

    // 1. Hermes search_public_catalog
    let search_cjk = HermesToolExecutor::execute("search_public_catalog", json!({ "query": "星海" }), &bob_ctx).await;
    assert!(search_cjk.success);
    assert_eq!(search_cjk.structured.unwrap()["items"][0]["creation_id"], book_id);

    // 2. Hermes recommend_public_creations (browse + kind + deep_link)
    let rec_all = HermesToolExecutor::execute("recommend_public_creations", json!({ "limit": 10 }), &bob_ctx).await;
    assert!(rec_all.success);
    let rec_items = rec_all.structured.unwrap()["items"].as_array().unwrap().clone();
    assert_eq!(rec_items.len(), 4);
    for item in &rec_items {
        let dl = item["deep_link"].as_str().unwrap();
        let parsed = parse_apple_deep_link(dl).expect("valid tg://apple deep link");
        assert_eq!(parsed.creation_id.as_deref(), item["creation_id"].as_str());
    }

    // 3. @CreatorStudioBot /recommend
    let (bob_chat, _) = open_creator_bot_dm(&app, &bob_tok).await;
    let bot_rec_books = send_bot_cmd_and_wait(&app, &bob_tok, bob_chat, "/recommend books 星海").await;
    assert!(bot_rec_books["text"].as_str().unwrap().contains("星海航路"));
    assert!(extract_inline_urls(&bot_rec_books).contains(&format!("tg://apple/books/{book_id}")));
    let bot_rec_music = send_bot_cmd_and_wait(&app, &bob_tok, bob_chat, "/recommend Neon").await;
    assert!(extract_inline_urls(&bot_rec_music).contains(&format!("tg://apple/music/{music_id}")));

    // 4. GET /api/v1/bots/inline-query
    let (s_unauth, _) = http_json(&app, "GET", "/api/v1/bots/inline-query", None, None).await;
    assert_eq!(s_unauth, StatusCode::UNAUTHORIZED);

    let (s_def, def_body) = http_json(&app, "GET", "/api/v1/bots/inline-query", Some(&bob_tok), None).await;
    assert_eq!(s_def, StatusCode::OK, "inline-query failed: {def_body}");
    assert_eq!(def_body["bot_username"], CREATOR_STUDIO_BOT_USERNAME);
    assert_eq!(def_body["total"], 4);
    assert_eq!(def_body["results"].as_array().unwrap().len(), 4);
    assert_eq!(def_body["items"].as_array().unwrap().len(), 4);
    assert!(def_body["query_id"].as_str().unwrap().starts_with("iq_"));

    let (s_at, at_body) = http_json(
        &app,
        "GET",
        "/api/v1/bots/inline-query?bot_username=@CreatorStudioBot&kind=games&query=%E6%98%9F%E9%99%85",
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(s_at, StatusCode::OK);
    assert_eq!(at_body["results"].as_array().unwrap().len(), 1);
    assert_eq!(at_body["results"][0]["deep_link"], format!("tg://apple/games/{game_id}"));

    let (s_lim0, lim0) = http_json(&app, "GET", "/api/v1/bots/inline-query?limit=0", Some(&bob_tok), None).await;
    assert_eq!(s_lim0, StatusCode::OK);
    assert_eq!(lim0["total"], 0);

    let (s_lim2, lim2) = http_json(&app, "GET", "/api/v1/bots/inline-query?limit=2", Some(&bob_tok), None).await;
    assert_eq!(s_lim2, StatusCode::OK);
    assert_eq!(lim2["results"].as_array().unwrap().len(), 2);

    // Negative / boundary cases
    let (s_404, _) = http_json(&app, "GET", "/api/v1/bots/inline-query?bot_username=NoSuchBot999", Some(&bob_tok), None).await;
    assert_eq!(s_404, StatusCode::NOT_FOUND);
    let (s_dis, _) = http_json(&app, "GET", "/api/v1/bots/inline-query?bot_username=BotFather", Some(&bob_tok), None).await;
    assert_eq!(s_dis, StatusCode::BAD_REQUEST);
    let (s_badk, _) = http_json(&app, "GET", "/api/v1/bots/inline-query?kind=podcast", Some(&bob_tok), None).await;
    assert_eq!(s_badk, StatusCode::BAD_REQUEST);
    let (s_badl, _) = http_json(&app, "GET", "/api/v1/bots/inline-query?limit=abc", Some(&bob_tok), None).await;
    assert_eq!(s_badl, StatusCode::BAD_REQUEST);
    for probe in ["%00", "%25", "_", "*"] {
        let (sp, bp) = http_json(&app, "GET", &format!("/api/v1/bots/inline-query?query={probe}"), Some(&bob_tok), None).await;
        assert_eq!(sp, StatusCode::OK);
        assert_eq!(bp["total"], 0);
    }

    state.shutdown();
}

#[tokio::test]
async fn test_07_telegram_bot_api_answer_inline_query_and_recommend_content() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_07", "Alice", None).await;
    let (admin_uid, admin_tok) = register_user(&app, "admin_m3_07", "Admin", None).await;
    promote_admin(&state, admin_uid);

    // Publish 1 item so inline-query + recommendContent return real data
    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), 9301, CREATOR_STUDIO_BOT_USER_ID);
    let cid = HermesToolExecutor::execute(
        "create_book",
        json!({ "title": "Quantum Tales 量子物语", "summary": "Bilingual collection of speculative science fiction stories." }),
        &alice_ctx,
    )
    .await
    .structured
    .unwrap()["creation_id"]
        .as_str()
        .unwrap()
        .to_string();
    submit_and_approve(&app, &alice_tok, &admin_tok, &cid, "public").await;

    // Create a developer bot via POST /api/v1/bots (note: is_inline == false by default!)
    let (sb, dev_bot) = http_json(
        &app,
        "POST",
        "/api/v1/bots",
        Some(&alice_tok),
        Some(json!({
            "username": "dev_inline_test_bot",
            "display_name": "Dev Inline Bot",
            "description": "Test bot"
        })),
    )
    .await;
    assert!(sb == StatusCode::CREATED || sb == StatusCode::OK, "bot create failed: {dev_bot}");
    let dev_token = dev_bot["bot_token"].as_str().unwrap().to_string();

    // Chained query_id from /api/v1/bots/inline-query -> /bot/{token}/answerInlineQuery
    let (_, iq_resp) = http_json(&app, "GET", "/api/v1/bots/inline-query?query=Quantum", Some(&alice_tok), None).await;
    let qid = iq_resp["query_id"].as_str().unwrap();

    let (st1, r1) = http_json(
        &app,
        "POST",
        &format!("/bot/{CREATOR_BOT_TOKEN}/answerInlineQuery"),
        None,
        Some(json!({
            "inline_query_id": qid,
            "results": iq_resp["results"],
            "cache_time": 60,
            "is_personal": true,
            "next_offset": "page2",
            "button": { "text": "Open Studio", "start_parameter": "studio" }
        })),
    )
    .await;
    assert_eq!(st1, StatusCode::OK, "answerInlineQuery failed: {r1}");
    assert_eq!(r1["ok"], true);
    assert_eq!(r1["result"], true);

    // Developer bot (is_inline == false) + /bot{token} compact path + form-urlencoded + numeric inline_query_id + stringified results
    let (st2, r2) = http_form(
        &app,
        "POST",
        &format!("/bot{dev_token}/answerInlineQuery"),
        "inline_query_id=987654&next_offset=20&results=%5B%7B%22type%22%3A%22article%22%2C%22id%22%3A%22res_1%22%2C%22title%22%3A%22Quantum%22%7D%5D",
    )
    .await;
    assert_eq!(st2, StatusCode::OK, "form answerInlineQuery failed: {r2}");
    assert_eq!(r2["ok"], true);

    // Empty results array MUST succeed
    let (st3, r3) = http_json(
        &app,
        "POST",
        &format!("/bot/{CREATOR_BOT_TOKEN}/answerInlineQuery"),
        None,
        Some(json!({ "inline_query_id": "iq_empty", "results": [] })),
    )
    .await;
    assert_eq!(st3, StatusCode::OK);
    assert_eq!(r3["ok"], true);

    // Negative cases
    let (s_unauth, ru) = http_json(&app, "POST", "/bot/bad_token_xyz/answerInlineQuery", None, Some(json!({ "inline_query_id": "q1", "results": [] }))).await;
    assert_eq!(s_unauth, StatusCode::UNAUTHORIZED);
    assert_eq!(ru["ok"], false);

    let too_many: Vec<Value> = (0..51).map(|i| json!({ "id": format!("id_{i}"), "type": "article" })).collect();
    for bad_body in [
        json!({ "results": [] }),
        json!({ "inline_query_id": "   ", "results": [] }),
        json!({ "inline_query_id": "q1", "results": "not_json" }),
        json!({ "inline_query_id": "q1", "results": too_many }),
        json!({ "inline_query_id": "q1", "results": [{ "id": "dup", "type": "article" }, { "id": "dup", "type": "article" }] }),
        json!({ "inline_query_id": "q1", "results": [{ "id": "ok" }] }),
        json!({ "inline_query_id": "q1", "results": [], "cache_time": -5 }),
    ] {
        let (sb_err, rb_err) = http_json(&app, "POST", &format!("/bot/{CREATOR_BOT_TOKEN}/answerInlineQuery"), None, Some(bad_body)).await;
        assert_eq!(sb_err, StatusCode::BAD_REQUEST);
        assert_eq!(rb_err["ok"], false);
    }

    // recommendContent
    let (sr_ok, rr_ok) = http_json(&app, "POST", &format!("/bot/{CREATOR_BOT_TOKEN}/recommendContent"), None, Some(json!({ "query": "量子", "kind": "books", "limit": 5 }))).await;
    assert_eq!(sr_ok, StatusCode::OK);
    assert_eq!(rr_ok["ok"], true);
    assert_eq!(rr_ok["result"][0]["creation_id"], cid);

    let (sr_401, _) = http_json(&app, "POST", "/bot/invalid_token/recommendContent", None, Some(json!({}))).await;
    assert_eq!(sr_401, StatusCode::UNAUTHORIZED);
    let (sr_400, _) = http_json(&app, "POST", &format!("/bot/{CREATOR_BOT_TOKEN}/recommendContent"), None, Some(json!({ "kind": "unknown" }))).await;
    assert_eq!(sr_400, StatusCode::BAD_REQUEST);

    state.shutdown();
}

#[tokio::test]
async fn test_08_live_ws_user_creation_changed_from_bot_and_hermes_tools() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_08", "Alice", None).await;
    let (_bob_uid, bob_tok) = register_user(&app, "bob_m3_08", "Bob", None).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, server_app).await;
    });

    let mut alice_ws = ws_connect_and_handshake(addr, &alice_tok, 1001).await;
    let mut bob_ws = ws_connect_and_handshake(addr, &bob_tok, 1002).await;

    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), 9401, CREATOR_STUDIO_BOT_USER_ID);
    let mut prev_rev = 0i64;

    // 1. Hermes create_book -> update_my_creation -> submit_creation_for_review
    let created = HermesToolExecutor::execute("create_book", json!({ "title": "WS Live Book" }), &alice_ctx).await;
    assert!(created.success);
    let cid = created.structured.unwrap()["creation_id"].as_str().unwrap().to_string();
    let ev1 = ws_next_creation_event(&mut alice_ws, 2000).await.expect("WS event 1");
    if let UpdateEvent::UserCreationChanged { creation_id, owner_user_id, cloud_revision, .. } = ev1 {
        assert_eq!(creation_id, cid);
        assert_eq!(owner_user_id, alice_uid as i64);
        assert!(cloud_revision > prev_rev);
        prev_rev = cloud_revision;
    } else {
        panic!("Expected UserCreationChanged, got {ev1:?}");
    }

    let upd = HermesToolExecutor::execute("update_my_creation", json!({ "creation_id": cid, "title": "WS Live Book v2" }), &alice_ctx).await;
    assert!(upd.success);
    let ev2 = ws_next_creation_event(&mut alice_ws, 2000).await.expect("WS event 2");
    if let UpdateEvent::UserCreationChanged { cloud_revision, .. } = ev2 {
        assert!(cloud_revision > prev_rev);
        prev_rev = cloud_revision;
    } else {
        panic!("Expected UserCreationChanged, got {ev2:?}");
    }

    let sub = HermesToolExecutor::execute("submit_creation_for_review", json!({ "creation_id": cid }), &alice_ctx).await;
    assert!(sub.success);
    let ev3 = ws_next_creation_event(&mut alice_ws, 2000).await.expect("WS event 3");
    if let UpdateEvent::UserCreationChanged { cloud_revision, .. } = ev3 {
        assert!(cloud_revision > prev_rev);
        prev_rev = cloud_revision;
    } else {
        panic!("Expected UserCreationChanged, got {ev3:?}");
    }

    // 2. @CreatorStudioBot /newmusic -> /editwork -> /submitwork
    let (alice_chat, _) = open_creator_bot_dm(&app, &alice_tok).await;
    let music_rep = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, "/newmusic WS Synth | Live websocket synth track for testing").await;
    let music_id = extract_creation_id_from_reply(&music_rep);
    let ev4 = ws_next_creation_event(&mut alice_ws, 2000).await.expect("WS event 4");
    if let UpdateEvent::UserCreationChanged { creation_id, cloud_revision, .. } = ev4 {
        assert_eq!(creation_id, music_id);
        assert!(cloud_revision > prev_rev);
        prev_rev = cloud_revision;
    } else {
        panic!("Expected UserCreationChanged, got {ev4:?}");
    }

    let _ = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &format!("/editwork {music_id} title=WS Synth v2")).await;
    let ev5 = ws_next_creation_event(&mut alice_ws, 2000).await.expect("WS event 5");
    if let UpdateEvent::UserCreationChanged { cloud_revision, .. } = ev5 {
        assert!(cloud_revision > prev_rev);
        prev_rev = cloud_revision;
    } else {
        panic!("Expected UserCreationChanged, got {ev5:?}");
    }

    let _ = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &format!("/submitwork {music_id}")).await;
    let ev6 = ws_next_creation_event(&mut alice_ws, 2000).await.expect("WS event 6");
    if let UpdateEvent::UserCreationChanged { cloud_revision, .. } = ev6 {
        assert!(cloud_revision > prev_rev);
    } else {
        panic!("Expected UserCreationChanged, got {ev6:?}");
    }

    // Non-owner Bob MUST receive ZERO UserCreationChanged or PublicCatalogChanged frames
    assert!(
        ws_next_creation_event(&mut bob_ws, 400).await.is_none(),
        "Bob must not receive Alice's private UserCreationChanged events"
    );

    server_handle.abort();
    state.shutdown();
}

#[tokio::test]
async fn test_09_adversarial_4_layer_zero_leak_matrix_all_6_states_en_cjk_id_owner_and_non_owner() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_09", "Alice", None).await;
    let (bob_uid, bob_tok) = register_user(&app, "bob_m3_09", "Bob", None).await;
    let (admin_uid, admin_tok) = register_user(&app, "admin_m3_09", "Admin", None).await;
    promote_admin(&state, admin_uid);

    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), 9501, CREATOR_STUDIO_BOT_USER_ID);
    let bob_ctx = build_im_tool_ctx(&state, Some(bob_uid), 9502, CREATOR_STUDIO_BOT_USER_ID);

    let mk_book = |title: &'static str, summary: &'static str| {
        let ctx = build_im_tool_ctx(&state, Some(alice_uid), 9501, CREATOR_STUDIO_BOT_USER_ID);
        async move {
            let res = HermesToolExecutor::execute("create_book", json!({ "title": title, "summary": summary }), &ctx).await;
            assert!(res.success, "create_book failed: {}", res.output);
            res.structured.unwrap()["creation_id"].as_str().unwrap().to_string()
        }
    };

    // 1. Draft
    let id_draft = mk_book("CanaryDraftAlpha 秘密草稿甲", "Private draft canary content 秘密草稿甲详情").await;
    // 2. PendingReview
    let id_pending = mk_book("CanaryPendingBeta 秘密待审乙", "Pending review canary content 秘密待审乙详情").await;
    let _ = http_json(&app, "POST", &format!("/api/v1/creations/{id_pending}/submit-review"), Some(&alice_tok), Some(json!({ "visibility": "public" }))).await;
    // 3. Rejected
    let id_rejected = mk_book("CanaryRejectedGamma 秘密驳回丙", "Rejected canary content 秘密驳回丙详情").await;
    let _ = http_json(&app, "POST", &format!("/api/v1/creations/{id_rejected}/submit-review"), Some(&alice_tok), Some(json!({ "visibility": "public" }))).await;
    let (s_rej, _) = http_json(&app, "POST", &format!("/api/v1/creations/{id_rejected}/review"), Some(&admin_tok), Some(json!({ "decision": "reject", "reason": "Needs revision" }))).await;
    assert_eq!(s_rej, StatusCode::OK);
    // 4. Unlisted
    let id_unlisted = mk_book("CanaryUnlistedDelta 秘密非公开丁", "Unlisted canary content 秘密非公开丁详情").await;
    submit_and_approve(&app, &alice_tok, &admin_tok, &id_unlisted, "unlisted").await;
    // 5. Unpublished (was public, then unpublished)
    let id_unpub = mk_book("CanaryUnpubEpsilon 秘密下架戊", "Unpublished canary content 秘密下架戊详情").await;
    submit_and_approve(&app, &alice_tok, &admin_tok, &id_unpub, "public").await;
    let (s_unp, _) = http_json(&app, "POST", &format!("/api/v1/creations/{id_unpub}/unpublish"), Some(&alice_tok), None).await;
    assert_eq!(s_unp, StatusCode::OK);
    // 6. Soft-deleted (was public, then deleted)
    let id_deleted = mk_book("CanaryDeletedZeta 秘密删除己", "Deleted canary content 秘密删除己详情").await;
    submit_and_approve(&app, &alice_tok, &admin_tok, &id_deleted, "public").await;
    let (s_del, _) = http_json(&app, "DELETE", &format!("/api/v1/creations/{id_deleted}"), Some(&alice_tok), None).await;
    assert!(s_del == StatusCode::OK || s_del == StatusCode::NO_CONTENT);

    // Control: Published + Public beacon
    let id_public = mk_book("PublicControlBeacon 公开灯塔", "Public published control beacon 公开灯塔导航").await;
    submit_and_approve(&app, &alice_tok, &admin_tok, &id_public, "public").await;

    let forbidden_tokens: Vec<String> = vec![
        "CanaryDraftAlpha".into(), "秘密草稿甲".into(), id_draft.clone(),
        "CanaryPendingBeta".into(), "秘密待审乙".into(), id_pending.clone(),
        "CanaryRejectedGamma".into(), "秘密驳回丙".into(), id_rejected.clone(),
        "CanaryUnlistedDelta".into(), "秘密非公开丁".into(), id_unlisted.clone(),
        "CanaryUnpubEpsilon".into(), "秘密下架戊".into(), id_unpub.clone(),
        "CanaryDeletedZeta".into(), "秘密删除己".into(), id_deleted.clone(),
    ];

    let mut probes: Vec<String> = vec!["".into(), "Canary".into(), "秘密".into()];
    probes.extend(forbidden_tokens.iter().cloned());

    let (alice_chat, _) = open_creator_bot_dm(&app, &alice_tok).await;
    let (bob_chat, _) = open_creator_bot_dm(&app, &bob_tok).await;

    let assert_zero_leak = |surface: &str, actor: &str, probe: &str, serialized: &str| {
        for secret in &forbidden_tokens {
            assert!(
                !serialized.contains(secret.as_str()),
                "ZERO-LEAK VIOLATION on {surface} (actor={actor}, probe='{probe}'): found secret '{secret}' in payload: {serialized}"
            );
        }
    };

    for (actor, ctx, tok, chat_id) in [
        ("owner_alice", &alice_ctx, alice_tok.as_str(), alice_chat),
        ("other_bob", &bob_ctx, bob_tok.as_str(), bob_chat),
    ] {
        for probe in &probes {
            // Surface 1: Hermes search_public_catalog
            let r1 = HermesToolExecutor::execute("search_public_catalog", json!({ "query": probe, "limit": 50 }), ctx).await;
            assert!(r1.success);
            let s1 = serde_json::to_string(&r1.structured).unwrap();
            assert_zero_leak("search_public_catalog", actor, probe, &s1);
            if probe.is_empty() {
                assert!(s1.contains(&id_public));
            }

            // Surface 2: Hermes recommend_public_creations
            let r2 = HermesToolExecutor::execute("recommend_public_creations", json!({ "query": probe, "limit": 50 }), ctx).await;
            assert!(r2.success);
            let s2 = serde_json::to_string(&r2.structured).unwrap();
            assert_zero_leak("recommend_public_creations", actor, probe, &s2);
            if probe.is_empty() {
                assert!(s2.contains(&id_public));
            }

            // Surface 3: GET /api/v1/bots/inline-query
            let enc: String = url::form_urlencoded::byte_serialize(probe.as_bytes()).collect();
            let (st3, b3) = http_json(&app, "GET", &format!("/api/v1/bots/inline-query?query={enc}&limit=50"), Some(tok), None).await;
            assert_eq!(st3, StatusCode::OK);
            let s3 = serde_json::to_string(&b3).unwrap();
            assert_zero_leak("inline-query", actor, probe, &s3);
            if probe.is_empty() {
                assert!(s3.contains(&id_public));
            }

            // Surface 4: POST /bot/{token}/recommendContent
            let (st4, b4) = http_json(&app, "POST", &format!("/bot/{CREATOR_BOT_TOKEN}/recommendContent"), None, Some(json!({ "query": probe, "limit": 50 }))).await;
            assert_eq!(st4, StatusCode::OK);
            let s4 = serde_json::to_string(&b4).unwrap();
            assert_zero_leak("recommendContent", actor, probe, &s4);
            if probe.is_empty() {
                assert!(s4.contains(&id_public));
            }

            // Surface 5: @CreatorStudioBot /recommend
            let cmd = if probe.is_empty() { "/recommend".to_string() } else { format!("/recommend {probe}") };
            let rep5 = send_bot_cmd_and_wait(&app, tok, chat_id, &cmd).await;
            let s5 = serde_json::to_string(&rep5).unwrap();
            assert_zero_leak("@CreatorStudioBot /recommend", actor, probe, &s5);
            if probe.is_empty() {
                assert!(s5.contains(&id_public));
            }
        }
    }

    state.shutdown();
}

#[tokio::test]
async fn test_10_cold_restart_durability_for_creator_bot_works_and_recommendations() {
    let (_dir, cfg, state1, app1) = make_temp_state();
    let (_alice_uid, alice_tok) = register_user(&app1, "alice_m3_10", "Alice", None).await;
    let (admin_uid, admin_tok) = register_user(&app1, "admin_m3_10", "Admin", None).await;
    promote_admin(&state1, admin_uid);

    let (alice_chat, _) = open_creator_bot_dm(&app1, &alice_tok).await;
    let book_rep = send_bot_cmd_and_wait(
        &app1,
        &alice_tok,
        alice_chat,
        "/newbook Durable Odyssey | Persistent cloud novel tested across cold restart",
    )
    .await;
    let book_id = extract_creation_id_from_reply(&book_rep);
    let _ = send_bot_cmd_and_wait(&app1, &alice_tok, alice_chat, &format!("/submitwork {book_id}")).await;
    let (s_app, _) = http_json(
        &app1,
        "POST",
        &format!("/api/v1/creations/{book_id}/review"),
        Some(&admin_tok),
        Some(json!({ "decision": "approve", "visibility": "public" })),
    )
    .await;
    assert_eq!(s_app, StatusCode::OK);

    // Cold shutdown and reopen on the exact same TempDir SQLite + CAS files
    state1.shutdown();
    drop(app1);
    drop(state1);

    let state2 = Arc::new(AppState::try_new(cfg).expect("reopen AppState after cold restart"));
    let app2 = build_router_with_state(state2.clone());

    // Re-login after restart to verify session/user/bot durability
    let (st_login, login_body) = http_json(
        &app2,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({
            "username": "alice_m3_10",
            "password": "StrongPassword123!",
            "device_name": "RestartDevice"
        })),
    )
    .await;
    assert_eq!(st_login, StatusCode::OK, "re-login failed: {login_body}");
    let tok2 = login_body["token"].as_str().unwrap().to_string();

    // Idempotent @CreatorStudioBot seeding preserved
    let (sp, prof) = http_json(&app2, "GET", "/api/v1/bots/profile/CreatorStudioBot", Some(&tok2), None).await;
    assert_eq!(sp, StatusCode::OK);
    assert_eq!(prof["bot"]["bot_user_id"], CREATOR_STUDIO_BOT_USER_ID);
    assert_eq!(prof["bot"]["is_inline"], true);

    // /mycreations, /recommend, and GET /api/v1/bots/inline-query return exact persisted data
    let my_rep = send_bot_cmd_and_wait(&app2, &tok2, alice_chat, "/mycreations").await;
    assert!(my_rep["text"].as_str().unwrap().contains(&book_id));
    assert!(my_rep["text"].as_str().unwrap().contains("Durable Odyssey"));

    let rec_rep = send_bot_cmd_and_wait(&app2, &tok2, alice_chat, "/recommend Odyssey").await;
    assert!(rec_rep["text"].as_str().unwrap().contains("Durable Odyssey"));
    assert!(extract_inline_urls(&rec_rep).contains(&format!("tg://apple/books/{book_id}")));

    let (si, iq) = http_json(&app2, "GET", "/api/v1/bots/inline-query?query=Odyssey", Some(&tok2), None).await;
    assert_eq!(si, StatusCode::OK);
    assert_eq!(iq["total"], 1);
    assert_eq!(iq["results"][0]["creation_id"], book_id);
    assert_eq!(iq["results"][0]["deep_link"], format!("tg://apple/books/{book_id}"));

    state2.shutdown();
}

#[tokio::test]
async fn test_11_concurrent_music_wav_cas_acl_and_chat_media_non_clobber() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_11", "Alice", None).await;
    let (bob_uid, bob_tok) = register_user(&app, "bob_m3_11", "Bob", None).await;
    let (_charlie_uid, charlie_tok) = register_user(&app, "charlie_m3_11", "Charlie", None).await;
    let (admin_uid, admin_tok) = register_user(&app, "admin_m3_11", "Admin", None).await;
    promote_admin(&state, admin_uid);

    let (alice_chat, _) = open_creator_bot_dm(&app, &alice_tok).await;
    let (bob_chat, _) = open_creator_bot_dm(&app, &bob_tok).await;
    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), alice_chat, CREATOR_STUDIO_BOT_USER_ID);
    let bob_ctx = build_im_tool_ctx(&state, Some(bob_uid), bob_chat, CREATOR_STUDIO_BOT_USER_ID);

    let identical_tracks = json!([{
        "track_id": "trk_identical",
        "track_number": 1,
        "title": "Shared Synth Motif",
        "duration_secs": 4,
        "bpm": 120,
        "instruments": [{
            "channel": 0,
            "name": "Lead Sine",
            "waveform": "sine",
            "gain": 0.7,
            "pan": 0.0
        }],
        "melody_notes": [
            { "midi_note": 60, "start_beat": 0.0, "duration_beats": 1.0, "velocity": 100, "channel": 0 },
            { "midi_note": 64, "start_beat": 1.0, "duration_beats": 1.0, "velocity": 100, "channel": 0 },
            { "midi_note": 67, "start_beat": 2.0, "duration_beats": 1.0, "velocity": 100, "channel": 0 },
            { "midi_note": 72, "start_beat": 3.0, "duration_beats": 1.0, "velocity": 100, "channel": 0 }
        ],
        "chord_progression": [],
        "lyrics": [],
        "waveform_5bit": [],
        "audio_media_file_id": null
    }]);

    // 1. Concurrent multi-user tokio::join! rounds with byte-identical synth input
    for round in 0..4 {
        let args_a = json!({
            "title": format!("Concurrent Synth A {round}"),
            "summary": "Concurrent multi-user identical music creation ACL regression test",
            "bpm": 120,
            "tracks": identical_tracks,
            "synthesize_audio": true
        });
        let args_b = json!({
            "title": format!("Concurrent Synth B {round}"),
            "summary": "Concurrent multi-user identical music creation ACL regression test",
            "bpm": 120,
            "tracks": identical_tracks,
            "synthesize_audio": true
        });
        let (res_a, res_b) = tokio::join!(
            HermesToolExecutor::execute("create_music", args_a, &alice_ctx),
            HermesToolExecutor::execute("create_music", args_b, &bob_ctx),
        );
        assert!(res_a.success, "round {round} Alice create_music failed: {}", res_a.output);
        assert!(res_b.success, "round {round} Bob create_music failed: {}", res_b.output);

        let val_a = res_a.structured.unwrap();
        let val_b = res_b.structured.unwrap();
        let cid_a = val_a["creation_id"].as_str().unwrap().to_string();
        let cid_b = val_b["creation_id"].as_str().unwrap().to_string();
        let mid_a = val_a["audio_media_file_ids"][0].as_str().unwrap().to_string();
        let mid_b = val_b["audio_media_file_ids"][0].as_str().unwrap().to_string();
        assert_ne!(
            mid_a, mid_b,
            "round {round}: concurrent identical music synthesis MUST produce distinct owner-tagged CAS IDs"
        );

        // Draft ACL: each owner can stream own track (200), non-owner blocked (403)
        let (sa_a, wav_a) = http_raw(&app, "GET", &format!("/api/v1/media/{mid_a}"), Some(&alice_tok)).await;
        let (sb_a, _) = http_raw(&app, "GET", &format!("/api/v1/media/{mid_a}"), Some(&bob_tok)).await;
        let (sb_b, wav_b) = http_raw(&app, "GET", &format!("/api/v1/media/{mid_b}"), Some(&bob_tok)).await;
        let (sa_b, _) = http_raw(&app, "GET", &format!("/api/v1/media/{mid_b}"), Some(&alice_tok)).await;
        let (sc_a, _) = http_raw(&app, "GET", &format!("/api/v1/media/{mid_a}"), Some(&charlie_tok)).await;
        let (sc_b, _) = http_raw(&app, "GET", &format!("/api/v1/media/{mid_b}"), Some(&charlie_tok)).await;
        assert_eq!(sa_a, StatusCode::OK, "round {round}: Alice draft stream must be 200");
        assert_eq!(sb_a, StatusCode::FORBIDDEN, "round {round}: Bob must be 403 on Alice draft");
        assert_eq!(sb_b, StatusCode::OK, "round {round}: Bob draft stream must be 200");
        assert_eq!(sa_b, StatusCode::FORBIDDEN, "round {round}: Alice must be 403 on Bob draft");
        assert_eq!(sc_a, StatusCode::FORBIDDEN);
        assert_eq!(sc_b, StatusCode::FORBIDDEN);

        // Verify valid RIFF header size, even JUNK alignment, decode_wav_pcm & slice_wav_from_offset_ms
        for wav in [&wav_a, &wav_b] {
            assert!(wav.len() > 44 && &wav[0..4] == b"RIFF" && &wav[8..12] == b"WAVE");
            assert_eq!(wav.len() % 2, 0, "WAV length must stay even-aligned after JUNK chunk");
            let riff_sz = u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize;
            assert_eq!(riff_sz, wav.len() - 8, "RIFF chunk size must match file len - 8");
            let pcm = decode_wav_pcm(wav).expect("decode_wav_pcm with JUNK chunk");
            assert_eq!(pcm.sample_rate, 44_100);
            assert!(pcm.samples.iter().any(|&s| s != 0));
            let sliced = slice_wav_from_offset_ms(wav, 500).expect("slice_wav_from_offset_ms");
            assert!(sliced.len() > 44 && &sliced[0..4] == b"RIFF");
        }

        // Submit + approve both -> Charlie (public listener) gets 200 on BOTH
        for (ctx, cid) in [(&alice_ctx, &cid_a), (&bob_ctx, &cid_b)] {
            let sub = HermesToolExecutor::execute(
                "submit_creation_for_review",
                json!({ "creation_id": cid, "visibility": "public" }),
                ctx,
            )
            .await;
            assert!(sub.success, "submit {cid} failed: {}", sub.output);
            let (sr, rb) = http_json(
                &app,
                "POST",
                &format!("/api/v1/creations/{cid}/review"),
                Some(&admin_tok),
                Some(json!({ "decision": "approve", "visibility": "public" })),
            )
            .await;
            assert_eq!(sr, StatusCode::OK, "approve {cid} failed: {rb}");
        }
        let (pub_a, _) = http_raw(&app, "GET", &format!("/api/v1/media/{mid_a}"), Some(&charlie_tok)).await;
        let (pub_b, _) = http_raw(&app, "GET", &format!("/api/v1/media/{mid_b}"), Some(&charlie_tok)).await;
        assert_eq!(pub_a, StatusCode::OK, "round {round}: Charlie must stream Alice published track");
        assert_eq!(pub_b, StatusCode::OK, "round {round}: Charlie must stream Bob published track");
    }

    // 2. Sequential same-owner chat media non-clobber check
    let (sc, dm_body) = http_json(
        &app,
        "POST",
        "/api/v1/contacts/add",
        Some(&alice_tok),
        Some(json!({ "username": "bob_m3_11" })),
    )
    .await;
    assert!(sc == StatusCode::OK || sc == StatusCode::CREATED, "create DM failed: {dm_body}");
    let dm_chat_id = dm_body["chat_id"].as_i64().unwrap();

    let raw_synth = synthesize_music_track_wav(&SynthTrackInput {
        track_id: "trk_identical".into(),
        title: "Shared Synth Motif".into(),
        bpm: 120,
        sample_rate: 44_100,
        channels: 1,
        duration_secs_hint: Some(4.0),
        instruments: vec![SynthInstrument {
            channel: 0,
            name: "Lead Sine".into(),
            waveform: SynthWaveformKind::Sine,
            gain: 0.7,
            pan: 0.0,
            adsr: None,
        }],
        melody_notes: vec![
            SynthNote { midi_note: 60, start_beat: 0.0, duration_beats: 1.0, velocity: 100, channel: 0 },
            SynthNote { midi_note: 64, start_beat: 1.0, duration_beats: 1.0, velocity: 100, channel: 0 },
            SynthNote { midi_note: 67, start_beat: 2.0, duration_beats: 1.0, velocity: 100, channel: 0 },
            SynthNote { midi_note: 72, start_beat: 3.0, duration_beats: 1.0, velocity: 100, channel: 0 },
        ],
        chord_progression: vec![],
        lyrics: vec![],
    })
    .expect("raw synth");
    let chat_media_id = state.cas.store_blob(&raw_synth.wav_bytes).await.expect("store chat blob");
    let chat_meta = MediaMetadataDto {
        media_id: chat_media_id.clone(),
        filename: "chat_voice.wav".into(),
        mime_type: "audio/wav".into(),
        size_bytes: raw_synth.wav_bytes.len() as u64,
        sha256: chat_media_id.clone(),
        width: None,
        height: None,
        duration_seconds: Some(4),
        waveform: Some(raw_synth.waveform_5bit),
        thumbnail_data_url: None,
        is_voice: false,
        is_video_note: false,
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    state
        .db
        .save_media_file(&chat_meta, UserId(alice_uid), Some(ChatId(dm_chat_id)))
        .expect("save chat media");

    let (bob_before, _) = http_raw(&app, "GET", &format!("/api/v1/media/{chat_media_id}"), Some(&bob_tok)).await;
    assert_eq!(bob_before, StatusCode::OK, "Bob must access shared DM audio before Alice creates music");

    let alice_music = HermesToolExecutor::execute(
        "create_music",
        json!({
            "title": "Same Owner Non-Clobber Track",
            "summary": "Ensures music synthesis does not clobber existing chat media_files.chat_id",
            "bpm": 120,
            "tracks": identical_tracks,
            "synthesize_audio": true
        }),
        &alice_ctx,
    )
    .await;
    assert!(alice_music.success, "create_music failed: {}", alice_music.output);
    let creation_media_id = alice_music.structured.unwrap()["audio_media_file_ids"][0]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(creation_media_id, chat_media_id);

    let (bob_after, _) = http_raw(&app, "GET", &format!("/api/v1/media/{chat_media_id}"), Some(&bob_tok)).await;
    assert_eq!(bob_after, StatusCode::OK, "Bob's access to DM chat audio MUST remain 200 after Alice creates music");
    let (bob_draft, _) = http_raw(&app, "GET", &format!("/api/v1/media/{creation_media_id}"), Some(&bob_tok)).await;
    assert_eq!(bob_draft, StatusCode::FORBIDDEN, "Bob must still get 403 on Alice's private draft track");

    state.shutdown();
}

#[tokio::test]
async fn test_12_direct_bridge_and_executor_u64_max_and_expected_revision_cas_rejection() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_12", "Alice", None).await;
    let (alice_chat, _) = open_creator_bot_dm(&app, &alice_tok).await;
    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), alice_chat, CREATOR_STUDIO_BOT_USER_ID);
    let bridge = alice_ctx.platform_im_bridge.clone().expect("ServerPlatformImBridge attached");

    let created = HermesToolExecutor::execute(
        "create_book",
        json!({
            "title": "CAS Protected Atlas",
            "summary": "Original locked summary for optimistic concurrency verification."
        }),
        &alice_ctx,
    )
    .await;
    assert!(created.success, "create_book failed: {}", created.output);
    let creation_id = created.structured.unwrap()["creation_id"].as_str().unwrap().to_string();

    // 1. Through HermesToolExecutor: u64::MAX / negative / string / float expected_revision MUST be rejected
    for bad_rev in [
        json!(u64::MAX),
        json!((i64::MAX as u64) + 1),
        json!(-1),
        json!("wrong_rev_string"),
        json!(1.5),
    ] {
        let res = HermesToolExecutor::execute(
            "update_my_creation",
            json!({
                "creation_id": creation_id,
                "expected_revision": bad_rev,
                "title": "CAS_BYPASSED_EXECUTOR"
            }),
            &alice_ctx,
        )
        .await;
        assert!(!res.success, "Executor must reject expected_revision={bad_rev}");
    }

    // Also verify executor rejects u64::MAX on recommend_public_creations / search_public_catalog / create_music
    assert!(!HermesToolExecutor::execute("recommend_public_creations", json!({ "limit": u64::MAX }), &alice_ctx).await.success);
    assert!(!HermesToolExecutor::execute("search_public_catalog", json!({ "limit": u64::MAX, "offset": u64::MAX }), &alice_ctx).await.success);
    assert!(!HermesToolExecutor::execute("create_music", json!({ "title": "Bad", "bpm": u64::MAX }), &alice_ctx).await.success);

    // 2. Direct ServerPlatformImBridge::execute_im_tool (bypassing executor): MUST also fail closed!
    let caller = PlatformImCaller {
        user_id: Some(alice_uid),
        chat_id: Some(alice_chat),
        bot_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        bound_agent_id: None,
        session_id: "direct_bridge_cas".into(),
    };
    for bad_rev in [
        json!(u64::MAX),
        json!("wrong_rev_string"),
        json!(-1),
        json!(1.5),
        json!(true),
    ] {
        let res = bridge
            .execute_im_tool(
                "update_my_creation",
                &json!({
                    "creation_id": creation_id,
                    "expected_revision": bad_rev,
                    "title": "DIRECT_BRIDGE_CAS_BYPASSED"
                }),
                caller.clone(),
            )
            .await;
        assert!(res.is_err(), "Direct bridge must reject expected_revision={bad_rev}, got: {res:?}");
    }
    assert!(bridge.execute_im_tool("recommend_public_creations", &json!({ "limit": u64::MAX }), caller.clone()).await.is_err());
    assert!(bridge.execute_im_tool("recommend_public_creations", &json!({ "limit": 51 }), caller.clone()).await.is_err());
    assert!(bridge.execute_im_tool("search_public_catalog", &json!({ "limit": u64::MAX }), caller.clone()).await.is_err());
    assert!(bridge.execute_im_tool("search_public_catalog", &json!({ "offset": u64::MAX }), caller.clone()).await.is_err());
    assert!(bridge.execute_im_tool("search_public_catalog", &json!({ "kind": 123 }), caller.clone()).await.is_err());
    assert!(bridge.execute_im_tool("create_book", &json!({ "title": "Bad Series", "series_index": u64::MAX }), caller.clone()).await.is_err());
    assert!(bridge.execute_im_tool("create_game", &json!({ "title": "Bad Mins", "estimated_session_mins": u64::MAX }), caller.clone()).await.is_err());

    // Confirm creation was NEVER overwritten and revision stayed at 1
    let (sg, got) = http_json(&app, "GET", &format!("/api/v1/creations/{creation_id}"), Some(&alice_tok), None).await;
    assert_eq!(sg, StatusCode::OK);
    assert_eq!(got["title"], "CAS Protected Atlas");
    assert_eq!(got["cloud_revision"], 1);

    // Valid expected_revision=1 succeeds and bumps cloud_revision to 2
    let ok_upd = HermesToolExecutor::execute(
        "update_my_creation",
        json!({
            "creation_id": creation_id,
            "expected_revision": 1,
            "title": "CAS Updated Legitimately"
        }),
        &alice_ctx,
    )
    .await;
    assert!(ok_upd.success, "valid CAS update failed: {}", ok_upd.output);
    assert_eq!(ok_upd.structured.unwrap()["cloud_revision"], 2);

    state.shutdown();
}

#[tokio::test]
async fn test_13_creator_bot_nl_contractions_quoted_editwork_and_other_bot_mention() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (_alice_uid, alice_tok) = register_user(&app, "alice_m3_13", "Alice", None).await;
    let (alice_chat, _) = open_creator_bot_dm(&app, &alice_tok).await;

    // 1. Finding 5: NL single-quote contraction & leading apostrophe boundary extraction
    let cases = [
        (
            "Create a book called 'Alice's Starlight Journey'",
            "Alice's Starlight Journey",
        ),
        (
            "Compose a song 'Don't Stop Believing'",
            "Don't Stop Believing",
        ),
        (
            "Write a book about '90s rock'n'roll titled Starlight",
            "Starlight",
        ),
        (
            "Create a book called Alice's Cosmic Atlas",
            "Alice's Cosmic Atlas",
        ),
    ];

    let mut first_id = String::new();
    for (prompt, expected_title) in cases {
        let rep = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, prompt).await;
        let cid = extract_creation_id_from_reply(&rep);
        if first_id.is_empty() {
            first_id = cid.clone();
        }
        let (st, got) = http_json(&app, "GET", &format!("/api/v1/creations/{cid}"), Some(&alice_tok), None).await;
        assert_eq!(st, StatusCode::OK, "fetch {cid} failed: {got}");
        assert_eq!(
            got["title"], expected_title,
            "NL prompt `{prompt}` extracted wrong title"
        );
    }

    // 2. Finding 6: /editwork quote-aware splitting (summary= inside double/single quotes)
    let _ = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        &format!(
            "/editwork {first_id} title=\"Chapter 1 summary=intro\" summary=\"Real clean summary text here\""
        ),
    )
    .await;
    let (st, got) = http_json(&app, "GET", &format!("/api/v1/creations/{first_id}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["title"], "Chapter 1 summary=intro");
    assert_eq!(got["summary"], "Real clean summary text here");

    let _ = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        &format!(
            "/editwork {first_id} title='Alice's summary=part2' summary='Updated clean summary text here'"
        ),
    )
    .await;
    let (st2, got2) = http_json(&app, "GET", &format!("/api/v1/creations/{first_id}"), Some(&alice_tok), None).await;
    assert_eq!(st2, StatusCode::OK);
    assert_eq!(got2["title"], "Alice's summary=part2");
    assert_eq!(got2["summary"], "Updated clean summary text here");

    // 3. Finding 7: /cmd@OtherBot MUST NOT execute mutations; /cmd@CreatorStudioBot still works
    let (sl_before, list_before) = http_json(&app, "GET", "/api/v1/creations/my", Some(&alice_tok), None).await;
    assert_eq!(sl_before, StatusCode::OK);
    let count_before = list_before["items"].as_array().unwrap().len();

    let _ = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        "/newbook@SomeOtherBot Should Not Create | Valid summary here",
    )
    .await;
    let _ = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        &format!("/editwork@SomeOtherBot {first_id} title=\"Hijacked Title\""),
    )
    .await;
    let _ = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        &format!("/submitwork@SomeOtherBot {first_id}"),
    )
    .await;

    let (sl_after, list_after) = http_json(&app, "GET", "/api/v1/creations/my", Some(&alice_tok), None).await;
    assert_eq!(sl_after, StatusCode::OK);
    assert_eq!(
        list_after["items"].as_array().unwrap().len(),
        count_before,
        "/newbook@SomeOtherBot must NOT create any work"
    );
    let (st3, got3) = http_json(&app, "GET", &format!("/api/v1/creations/{first_id}"), Some(&alice_tok), None).await;
    assert_eq!(st3, StatusCode::OK);
    assert_eq!(got3["title"], "Alice's summary=part2");
    assert_eq!(got3["review_status"], "draft");

    // Targeted @CreatorStudioBot (case-insensitive) continues to work
    let own_rep = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        "/newbook@creatorstudiobot Targeted Own Bot | Valid summary for CreatorStudioBot mention",
    )
    .await;
    let own_id = extract_creation_id_from_reply(&own_rep);
    let (st4, got4) = http_json(&app, "GET", &format!("/api/v1/creations/{own_id}"), Some(&alice_tok), None).await;
    assert_eq!(st4, StatusCode::OK);
    assert_eq!(got4["title"], "Targeted Own Bot");

    state.shutdown();
}

#[tokio::test]
async fn test_14_telegram_bot_api_recommend_content_and_answer_inline_query_hardening() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_14", "Alice", None).await;
    let (admin_uid, admin_tok) = register_user(&app, "admin_m3_14", "Admin", None).await;
    promote_admin(&state, admin_uid);

    let (alice_chat, _) = open_creator_bot_dm(&app, &alice_tok).await;
    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), alice_chat, CREATOR_STUDIO_BOT_USER_ID);

    // Seed & publish 2 distinct public items (only 1 contains "2049")
    let b1 = HermesToolExecutor::execute(
        "create_book",
        json!({
            "title": "PublicBeacon 2049",
            "summary": "Futuristic orbital beacon chronicle set in the year 2049.",
            "genres": ["SciFi"]
        }),
        &alice_ctx,
    )
    .await;
    assert!(b1.success, "b1 failed: {}", b1.output);
    let id1 = b1.structured.unwrap()["creation_id"].as_str().unwrap().to_string();

    let m1 = HermesToolExecutor::execute(
        "create_music",
        json!({
            "title": "Aurora Nocturne",
            "summary": "Gentle ambient waltz composed under winter skies.",
            "genres": ["Classical"],
            "primary_genre": "Classical",
            "bpm": 96
        }),
        &alice_ctx,
    )
    .await;
    assert!(m1.success, "m1 failed: {}", m1.output);
    let id2 = m1.structured.unwrap()["creation_id"].as_str().unwrap().to_string();

    for cid in [&id1, &id2] {
        let sub = HermesToolExecutor::execute(
            "submit_creation_for_review",
            json!({ "creation_id": cid, "visibility": "public" }),
            &alice_ctx,
        )
        .await;
        assert!(sub.success, "submit {cid} failed: {}", sub.output);
        let (sr, rb) = http_json(
            &app,
            "POST",
            &format!("/api/v1/creations/{cid}/review"),
            Some(&admin_tok),
            Some(json!({ "decision": "approve", "visibility": "public" })),
        )
        .await;
        assert_eq!(sr, StatusCode::OK, "approve {cid} failed: {rb}");
    }

    let rec_url = format!("/bot/{CREATOR_BOT_TOKEN}/recommendContent");
    let ans_url = format!("/bot/{CREATOR_BOT_TOKEN}/answerInlineQuery");

    // 1. Finding 3: recommendContent raw query/form numeric/bool/null queries MUST filter accurately
    let (s, v) = http_json(&app, "GET", &format!("{rec_url}?query=2049"), None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["result"].as_array().unwrap().len(), 1, "GET ?query=2049 must match 1 item: {v}");
    assert_eq!(v["result"][0]["creation_id"], id1);

    let (s, v) = http_form(&app, "POST", &rec_url, "query=2049").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["result"].as_array().unwrap().len(), 1, "POST form query=2049 must match 1 item: {v}");

    for empty_match_uri in [
        format!("{rec_url}?query=999888777"),
        format!("{rec_url}?query=true"),
        format!("{rec_url}?query=false"),
        format!("{rec_url}?query=null"),
        format!("{rec_url}?genre=123"),
        format!("{rec_url}?genre=true"),
        format!("{rec_url}?genre=null"),
    ] {
        let (st, body) = http_json(&app, "GET", &empty_match_uri, None, None).await;
        assert_eq!(st, StatusCode::OK, "{empty_match_uri} failed: {body}");
        assert_eq!(
            body["result"].as_array().unwrap().len(),
            0,
            "{empty_match_uri} must NOT drop scalar filter to unfiltered browse: {body}"
        );
    }
    for empty_match_form in ["query=999888777", "query=false", "query=null", "genre=true", "genre=123"] {
        let (st, body) = http_form(&app, "POST", &rec_url, empty_match_form).await;
        assert_eq!(st, StatusCode::OK, "form {empty_match_form} failed: {body}");
        assert_eq!(
            body["result"].as_array().unwrap().len(),
            0,
            "form {empty_match_form} must filter to 0 items: {body}"
        );
    }

    // Invalid kind via GET query, POST form, and POST JSON MUST all return 400
    for bad_kind_uri in [
        format!("{rec_url}?kind=123"),
        format!("{rec_url}?kind=true"),
        format!("{rec_url}?kind=null"),
    ] {
        let (st, body) = http_json(&app, "GET", &bad_kind_uri, None, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad_kind_uri} must be 400: {body}");
    }
    let (st, body) = http_form(&app, "POST", &rec_url, "kind=123").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "form kind=123 must be 400: {body}");

    for bad_json in [
        json!({ "kind": 123 }),
        json!({ "kind": true }),
        json!({ "kind": ["book"] }),
        json!({ "query": 2049 }),
        json!({ "query": true }),
        json!({ "q": 123 }),
        json!({ "genre": 123 }),
        json!({ "genre": true }),
    ] {
        let (st, body) = http_json(&app, "POST", &rec_url, None, Some(bad_json.clone())).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "JSON {bad_json} must be 400: {body}");
    }

    // 2. Finding 8: Malformed / non-object JSON body MUST return 400 (never silently swallowed)
    let (st, body) = http_raw_json_body(&app, "POST", &rec_url, "{\"kind\": \"invalid\"").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "truncated JSON must return 400: {body}");
    let (st, body) = http_raw_json_body(&app, "POST", &rec_url, "[1, 2, 3]").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "JSON array body must return 400: {body}");
    let (st, body) = http_raw_json_body(&app, "POST", &ans_url, "{\"inline_query_id\": \"q1\"").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "malformed answerInlineQuery JSON must return 400: {body}");

    // 3. Finding 4: answerInlineQuery strict is_personal boolean validation
    for bad_personal in [json!(42), json!(-5), json!("42"), json!([]), json!({})] {
        let (st, body) = http_json(
            &app,
            "POST",
            &ans_url,
            None,
            Some(json!({ "inline_query_id": "q1", "results": [], "is_personal": bad_personal })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "is_personal={bad_personal} must be 400: {body}");
    }
    let (st, body) = http_form(&app, "POST", &ans_url, "inline_query_id=q1&results=%5B%5D&is_personal=42").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "form is_personal=42 must be 400: {body}");
    for ok_personal in [json!(true), json!(false), json!(1), json!(0), json!("true"), json!("0")] {
        let (st, body) = http_json(
            &app,
            "POST",
            &ans_url,
            None,
            Some(json!({ "inline_query_id": "q1", "results": [], "is_personal": ok_personal })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "is_personal={ok_personal} must be 200: {body}");
    }

    // 4. Finding 9: Whitespace-padded results[0].id and next_offset > 64 raw bytes MUST be 400
    let padded_id = format!("  {}  ", "a".repeat(64));
    assert_eq!(padded_id.len(), 68);
    let (st, body) = http_json(
        &app,
        "POST",
        &ans_url,
        None,
        Some(json!({
            "inline_query_id": "q1",
            "results": [{ "id": padded_id, "type": "article" }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "68-byte padded results[0].id must be 400: {body}");

    let padded_offset = format!("  {}  ", "b".repeat(64));
    let (st, body) = http_form(
        &app,
        "POST",
        &ans_url,
        &format!("inline_query_id=q1&results=%5B%5D&next_offset={}", padded_offset.replace(' ', "+")),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "68-byte padded form next_offset must be 400: {body}");

    // 5. Finding 10: Form-urlencoded "true"/"false" accepted as strings for inline_query_id & next_offset,
    //    while JSON Bool values for inline_query_id & next_offset remain 400!
    let (st, body) = http_form(&app, "POST", &ans_url, "inline_query_id=true&results=%5B%5D&next_offset=true").await;
    assert_eq!(st, StatusCode::OK, "form inline_query_id=true&next_offset=true must be 200: {body}");
    let (st, body) = http_form(&app, "POST", &ans_url, "inline_query_id=false&results=%5B%5D&next_offset=false").await;
    assert_eq!(st, StatusCode::OK, "form inline_query_id=false&next_offset=false must be 200: {body}");

    let (st, body) = http_json(
        &app,
        "POST",
        &ans_url,
        None,
        Some(json!({ "inline_query_id": true, "results": [] })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "JSON inline_query_id=true must remain 400: {body}");
    let (st, body) = http_json(
        &app,
        "POST",
        &ans_url,
        None,
        Some(json!({ "inline_query_id": "q1", "results": [], "next_offset": true })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "JSON next_offset=true must remain 400: {body}");

    state.shutdown();
}

#[tokio::test]
async fn test_15_iter3_overflow_query_null_alias_editwork_and_nl_title_regressions() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_15", "Alice", None).await;
    let (admin_uid, admin_tok) = register_user(&app, "admin_m3_15", "Admin", None).await;
    promote_admin(&state, admin_uid);

    let (alice_chat, _) = open_creator_bot_dm(&app, &alice_tok).await;
    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), alice_chat, CREATOR_STUDIO_BOT_USER_ID);
    let bridge = alice_ctx.platform_im_bridge.clone().expect("ServerPlatformImBridge attached");
    let caller = PlatformImCaller {
        user_id: Some(alice_uid),
        chat_id: Some(alice_chat),
        bot_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        bound_agent_id: None,
        session_id: "iter3_regressions".into(),
    };

    // 1. N1: create_music with two 3_000_000_000s tracks MUST saturate cleanly (zero Tokio panic),
    //    both with synthesize_audio=false (hermes_im_bridge.rs) and synthesize_audio=true (creator_bot.rs).
    let huge_tracks = json!([
        {
            "track_id": "trk_huge_1",
            "track_number": 1,
            "title": "Long Track 1",
            "duration_secs": 3_000_000_000u32,
            "bpm": 120,
            "musical_key": "C major",
            "instruments": [{ "channel": 0, "name": "Lead", "waveform": "sine", "gain": 0.7, "pan": 0.0 }],
            "melody_notes": [
                { "midi_note": 60, "start_beat": 0.0, "duration_beats": 1.0, "velocity": 100, "channel": 0 },
                { "midi_note": 67, "start_beat": 1.0, "duration_beats": 1.0, "velocity": 100, "channel": 0 }
            ],
            "chord_progression": [],
            "lyrics": [],
            "audio_media_file_id": null,
            "waveform_5bit": []
        },
        {
            "track_id": "trk_huge_2",
            "track_number": 2,
            "title": "Long Track 2",
            "duration_secs": 3_000_000_000u32,
            "bpm": 120,
            "musical_key": "C major",
            "instruments": [{ "channel": 0, "name": "Lead", "waveform": "sine", "gain": 0.7, "pan": 0.0 }],
            "melody_notes": [
                { "midi_note": 64, "start_beat": 0.0, "duration_beats": 1.0, "velocity": 100, "channel": 0 },
                { "midi_note": 72, "start_beat": 1.0, "duration_beats": 1.0, "velocity": 100, "channel": 0 }
            ],
            "chord_progression": [],
            "lyrics": [],
            "audio_media_file_id": null,
            "waveform_5bit": []
        }
    ]);
    for synth_flag in [false, true] {
        let res = HermesToolExecutor::execute(
            "create_music",
            json!({
                "title": format!("Saturating Duration Suite synth={synth_flag}"),
                "summary": "Verifies saturating u32 duration accumulation without Tokio worker panic.",
                "bpm": 120,
                "synthesize_audio": synth_flag,
                "tracks": huge_tracks
            }),
            &alice_ctx,
        )
        .await;
        assert!(res.success, "create_music synth={synth_flag} failed: {}", res.output);
        let structured = res.structured.unwrap();
        let total = structured["creation"]["payload"]["total_duration_secs"].as_u64().unwrap();
        assert_eq!(total, u32::MAX as u64, "total_duration_secs must saturate at u32::MAX");
        if synth_flag {
            assert_eq!(structured["audio_media_file_ids"].as_array().unwrap().len(), 2);
        }
    }

    // 2. N2: {"query": null, "q": "Starlight"} across all 3 query tools + direct-bridge non-string rejection
    let b_star = HermesToolExecutor::execute(
        "create_book",
        json!({
            "title": "Alpha Starlight Beacon",
            "summary": "A luminous starlight chronicle published to the public catalog."
        }),
        &alice_ctx,
    )
    .await;
    assert!(b_star.success, "b_star failed: {}", b_star.output);
    let id_star = b_star.structured.unwrap()["creation_id"].as_str().unwrap().to_string();

    let b_ocean = HermesToolExecutor::execute(
        "create_book",
        json!({
            "title": "Beta Ocean Whispers",
            "summary": "Deep Pacific voyages published alongside unrelated public works."
        }),
        &alice_ctx,
    )
    .await;
    assert!(b_ocean.success, "b_ocean failed: {}", b_ocean.output);
    let id_ocean = b_ocean.structured.unwrap()["creation_id"].as_str().unwrap().to_string();

    for cid in [&id_star, &id_ocean] {
        let sub = HermesToolExecutor::execute(
            "submit_creation_for_review",
            json!({ "creation_id": cid, "visibility": "public" }),
            &alice_ctx,
        )
        .await;
        assert!(sub.success, "submit {cid} failed: {}", sub.output);
        let (sr, rb) = http_json(
            &app,
            "POST",
            &format!("/api/v1/creations/{cid}/review"),
            Some(&admin_tok),
            Some(json!({ "decision": "approve", "visibility": "public" })),
        )
        .await;
        assert_eq!(sr, StatusCode::OK, "approve {cid} failed: {rb}");
    }

    for tool_name in ["recommend_public_creations", "search_public_catalog", "list_my_creations"] {
        for q_args in [
            json!({ "query": null, "q": "Starlight" }),
            json!({ "query": "", "q": "Starlight" }),
        ] {
            let res = HermesToolExecutor::execute(tool_name, q_args.clone(), &alice_ctx).await;
            assert!(res.success, "{tool_name}({q_args}) failed: {}", res.output);
            let s = res.structured.unwrap();
            assert_eq!(s["count"], 1, "{tool_name}({q_args}) must filter to 1 item: {s}");
            assert_eq!(s["items"][0]["creation_id"], id_star);
        }
    }

    // Direct bridge MUST fail closed on non-string optional args across all 8 creation/catalog tools
    let bad_bridge_calls: Vec<(&str, Value)> = vec![
        ("recommend_public_creations", json!({ "query": 2049 })),
        ("recommend_public_creations", json!({ "q": true })),
        ("recommend_public_creations", json!({ "genre": 123 })),
        ("search_public_catalog", json!({ "query": 2049 })),
        ("search_public_catalog", json!({ "genre": 123 })),
        ("list_my_creations", json!({ "query": 2049 })),
        ("list_my_creations", json!({ "q": ["Starlight"] })),
        ("update_my_creation", json!({ "creation_id": id_star, "title": 12345 })),
        ("update_my_creation", json!({ "creation_id": id_star, "subtitle": false })),
        ("update_my_creation", json!({ "creation_id": id_star, "summary": 999 })),
        ("update_my_creation", json!({ "creation_id": id_star, "author_display_name": 123 })),
        ("update_my_creation", json!({ "creation_id": id_star, "language": 42 })),
        ("update_my_creation", json!({ "creation_id": id_star, "age_rating": 18 })),
        ("create_book", json!({ "title": "Typed Book", "author": 123 })),
        ("create_book", json!({ "title": "Typed Book", "series_name": 123 })),
        ("create_book", json!({ "title": "Typed Book", "blurb": 123 })),
        ("create_music", json!({ "title": "Typed Music", "artist": 123 })),
        ("create_music", json!({ "title": "Typed Music", "album_title": 123 })),
        ("create_music", json!({ "title": "Typed Music", "musical_key": 123 })),
        ("create_music", json!({ "title": "Typed Music", "primary_genre": 123 })),
        ("create_music", json!({ "title": "Typed Music", "time_signature": 44 })),
        ("create_game", json!({ "title": "Typed Game", "developer": 123 })),
        ("create_game", json!({ "title": "Typed Game", "controls_hint": 123 })),
        ("create_game", json!({ "title": "Typed Game", "rules_markdown": 123 })),
        ("submit_creation_for_review", json!({ "creation_id": id_star, "notes": 123 })),
    ];
    for (tool_name, bad_args) in bad_bridge_calls {
        let r = bridge.execute_im_tool(tool_name, &bad_args, caller.clone()).await;
        assert!(r.is_err(), "Direct bridge must reject {tool_name}({bad_args}), got: {r:?}");
    }

    // 3. N3: Live DM /editwork quote/apostrophe regression matrix
    let seed_rep = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, "/newbook Seed Work | Initial clean summary text").await;
    let edit_id = extract_creation_id_from_reply(&seed_rep);
    let edit_cases = [
        (
            format!("/editwork {edit_id} title=Best '90s Hits summary=Classic nostalgic tracks from the decade"),
            "Best '90s Hits",
            "Classic nostalgic tracks from the decade",
        ),
        (
            format!("/editwork {edit_id} title=\"Part 1\" and \"Part 2\" summary=\"Two quoted segments clean summary\""),
            "\"Part 1\" and \"Part 2\"",
            "Two quoted segments clean summary",
        ),
        (
            format!("/editwork {edit_id} title='Users' Guide summary=x' summary='Plural possessive clean summary here'"),
            "Users' Guide summary=x",
            "Plural possessive clean summary here",
        ),
        (
            format!("/editwork {edit_id} title=\"Unclosed Title summary=Clean summary text here"),
            "\"Unclosed Title",
            "Clean summary text here",
        ),
        (
            format!("/editwork {edit_id} summary=\"Reversed Order Clean Summary Text\" title=\"Reversed Order Title\""),
            "Reversed Order Title",
            "Reversed Order Clean Summary Text",
        ),
        (
            format!("/editwork {edit_id} title=《星海诗篇》 summary=《一段优美的云端图书简介文本》"),
            "星海诗篇",
            "一段优美的云端图书简介文本",
        ),
        (
            format!("/editwork {edit_id} title=“晨曦乐章” summary=“一段充满层次感的合成器作品简介”"),
            "晨曦乐章",
            "一段充满层次感的合成器作品简介",
        ),
        (
            format!("/editwork {edit_id} title='Part 1' and 'Part 2' summary='Two single quoted segments summary'"),
            "'Part 1' and 'Part 2'",
            "Two single quoted segments summary",
        ),
    ];
    for (cmd, exp_title, exp_summary) in edit_cases {
        let _ = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &cmd).await;
        let (st, got) = http_json(&app, "GET", &format!("/api/v1/creations/{edit_id}"), Some(&alice_tok), None).await;
        assert_eq!(st, StatusCode::OK, "fetch after `{cmd}` failed: {got}");
        assert_eq!(got["title"], exp_title, "wrong title for `{cmd}`");
        assert_eq!(got["summary"], exp_summary, "wrong summary for `{cmd}`");
    }

    // 4. N4: Live DM NL extract_nl_title regression matrix (all 9 cases)
    let nl_cases = [
        ("Create a book called 'Users' Guide to Rust'", "Users' Guide to Rust"),
        ("Compose a song 'Rock 'n' Roll Stars'", "Rock 'n' Roll Stars"),
        ("Write a book about '90s rock 'n' roll titled Starlight", "Starlight"),
        ("Write a book about '90s kids' lives titled Starlight", "Starlight"),
        ("Create a book called \"Alice's \\\"Secret\\\" Atlas\"", "Alice's \"Secret\" Atlas"),
        ("Create a book called 'Alice's Starlight Journey'", "Alice's Starlight Journey"),
        ("Compose a song 'Don't Stop Believing'", "Don't Stop Believing"),
        ("Write a book about '90s rock'n'roll titled Starlight", "Starlight"),
        ("Create a book called Alice's Cosmic Atlas", "Alice's Cosmic Atlas"),
    ];
    for (prompt, exp_title) in nl_cases {
        let rep = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, prompt).await;
        let cid = extract_creation_id_from_reply(&rep);
        let (st, got) = http_json(&app, "GET", &format!("/api/v1/creations/{cid}"), Some(&alice_tok), None).await;
        assert_eq!(st, StatusCode::OK, "fetch after NL `{prompt}` failed: {got}");
        assert_eq!(got["title"], exp_title, "NL prompt `{prompt}` extracted wrong title");
    }

    state.shutdown();
}

#[tokio::test]
async fn test_16_iter4_utf8_editwork_empty_sq_escapes_and_inline_query_q_fallback() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_16", "Alice", None).await;
    let (admin_uid, admin_tok) = register_user(&app, "admin_m3_16", "Admin", None).await;
    promote_admin(&state, admin_uid);

    let (alice_chat, _) = open_creator_bot_dm(&app, &alice_tok).await;
    let alice_ctx = build_im_tool_ctx(&state, Some(alice_uid), alice_chat, CREATOR_STUDIO_BOT_USER_ID);

    // 1. I3-1 / R1: All 9 everyday UTF-8 / CJK / emoji / accented /editwork reproducers (zero panics)
    let seed_rep = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        "/newbook UTF8 Seed | Initial clean summary text",
    )
    .await;
    let edit_id = extract_creation_id_from_reply(&seed_rep);

    let utf8_valid_cases = [
        (
            format!("/editwork {edit_id} title=星海 奇遇记 summary=这是中文 简介内容详情"),
            "星海 奇遇记",
            "这是中文 简介内容详情",
        ),
        (
            format!("/editwork {edit_id} title=🎵 🎵🎵 summary=🎶 🎶🎶🎶 音乐作品"),
            "🎵 🎵🎵",
            "🎶 🎶🎶🎶 音乐作品",
        ),
        (
            format!("/editwork {edit_id} title=《星海》 和《航路》 summary=双书名中文简介"),
            "《星海》 和《航路》",
            "双书名中文简介",
        ),
        (
            format!("/editwork {edit_id} title=\"星海\" 奇遇记 summary=\"双引号后中文简介\""),
            "\"星海\" 奇遇记",
            "双引号后中文简介",
        ),
        (
            format!("/editwork {edit_id} title=星海\u{3000}奇遇记 summary=全角空格简介文本"),
            "星海\u{3000}奇遇记",
            "全角空格简介文本",
        ),
        (
            format!("/editwork {edit_id} title=A星海 B星海 summary=C星海 D星海简介"),
            "A星海 B星海",
            "C星海 D星海简介",
        ),
    ];
    for (cmd, exp_title, exp_summary) in utf8_valid_cases {
        let _ = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &cmd).await;
        let (st, got) =
            http_json(&app, "GET", &format!("/api/v1/creations/{edit_id}"), Some(&alice_tok), None)
                .await;
        assert_eq!(st, StatusCode::OK, "fetch after `{cmd}` failed: {got}");
        assert_eq!(got["title"], exp_title, "wrong UTF-8 title for `{cmd}`");
        assert_eq!(got["summary"], exp_summary, "wrong UTF-8 summary for `{cmd}`");
    }

    for malformed in [
        format!("/editwork {edit_id} 你好世界"),
        format!("/editwork {edit_id} 🎵🎵"),
        format!("/editwork {edit_id} Caféééé"),
    ] {
        let rep = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &malformed).await;
        let rep_str = rep.to_string();
        assert!(
            rep_str.contains("title=") || rep_str.contains("summary="),
            "malformed UTF-8 /editwork `{malformed}` must return clean usage card without panicking: {rep_str}"
        );
    }

    // 2. I3-2: Empty single quotes '' before real single-quoted title
    let empty_sq_cases = [
        ("Create a book called '' and 'Starlight'", "Starlight"),
        ("Create a book called '' and 'Alice's Starlight'", "Alice's Starlight"),
        ("Create a book called '''' and 'Starlight'", "Starlight"),
    ];
    for (prompt, exp_title) in empty_sq_cases {
        let rep = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, prompt).await;
        let cid = extract_creation_id_from_reply(&rep);
        let (st, got) =
            http_json(&app, "GET", &format!("/api/v1/creations/{cid}"), Some(&alice_tok), None)
                .await;
        assert_eq!(st, StatusCode::OK, "fetch after `{prompt}` failed: {got}");
        assert_eq!(got["title"], exp_title, "empty-sq NL `{prompt}` extracted wrong title");
    }

    // 3. I3-3: Single-quoted escapes & rightmost non-split field boundary
    let sq_edit_cases = [
        (
            format!("/editwork {edit_id} title='Part 1\\' and \\'Part 2' summary='Escaped single quotes summary'"),
            "Part 1' and 'Part 2",
            "Escaped single quotes summary",
        ),
        (
            format!("/editwork {edit_id} title='Users' summary=x' summary='Plural possessive before summary='"),
            "Users' summary=x",
            "Plural possessive before summary=",
        ),
        (
            format!("/editwork {edit_id} title='One' summary='Two'"),
            "One",
            "Two",
        ),
        (
            format!("/editwork {edit_id} title='One' summary=Unquoted clean summary text here"),
            "One",
            "Unquoted clean summary text here",
        ),
        (
            format!("/editwork {edit_id} title='Part 1' and 'Part 2' summary='Two single quoted segments'"),
            "'Part 1' and 'Part 2'",
            "Two single quoted segments",
        ),
        (
            format!("/editwork {edit_id} title='One' summary='Users' Guide'"),
            "One",
            "Users' Guide",
        ),
        (
            format!("/editwork {edit_id} title='One' summary='90s Hits'"),
            "One",
            "90s Hits",
        ),
        (
            format!("/editwork {edit_id} title='One' summary='🎵 Music'"),
            "One",
            "🎵 Music",
        ),
        (
            format!("/editwork {edit_id} title='Users' summary=x' summary=Unquoted clean summary text here"),
            "Users' summary=x",
            "Unquoted clean summary text here",
        ),
    ];
    for (cmd, exp_title, exp_summary) in sq_edit_cases {
        let _ = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &cmd).await;
        let (st, got) =
            http_json(&app, "GET", &format!("/api/v1/creations/{edit_id}"), Some(&alice_tok), None)
                .await;
        assert_eq!(st, StatusCode::OK, "fetch after `{cmd}` failed: {got}");
        assert_eq!(got["title"], exp_title, "wrong single-quoted title for `{cmd}`");
        assert_eq!(got["summary"], exp_summary, "wrong single-quoted summary for `{cmd}`");
    }

    // 4. I3-4: Blank query= + q= fallback alignment in bots_inline_query & recommendContent
    let b_star = HermesToolExecutor::execute(
        "create_book",
        json!({
            "title": "Starlight 2049 Chronicle",
            "summary": "A luminous starlight chronicle set in 2049."
        }),
        &alice_ctx,
    )
    .await;
    assert!(b_star.success, "b_star failed: {}", b_star.output);
    let id_star = b_star.structured.unwrap()["creation_id"].as_str().unwrap().to_string();

    let b_ocean = HermesToolExecutor::execute(
        "create_book",
        json!({
            "title": "Deep Ocean Almanac",
            "summary": "Unrelated marine voyages in the Pacific."
        }),
        &alice_ctx,
    )
    .await;
    assert!(b_ocean.success, "b_ocean failed: {}", b_ocean.output);
    let id_ocean = b_ocean.structured.unwrap()["creation_id"].as_str().unwrap().to_string();

    for cid in [&id_star, &id_ocean] {
        let sub = HermesToolExecutor::execute(
            "submit_creation_for_review",
            json!({ "creation_id": cid, "visibility": "public" }),
            &alice_ctx,
        )
        .await;
        assert!(sub.success, "submit {cid} failed: {}", sub.output);
        let (sr, rb) = http_json(
            &app,
            "POST",
            &format!("/api/v1/creations/{cid}/review"),
            Some(&admin_tok),
            Some(json!({ "decision": "approve", "visibility": "public" })),
        )
        .await;
        assert_eq!(sr, StatusCode::OK, "approve {cid} failed: {rb}");
    }

    for url in [
        "/api/v1/bots/inline-query?query=&q=Starlight",
        "/api/v1/bots/inline-query?query=%20%20&q=Starlight",
    ] {
        let (st, body) = http_json(&app, "GET", url, Some(&alice_tok), None).await;
        assert_eq!(st, StatusCode::OK, "{url} failed: {body}");
        assert_eq!(body["total"], 1, "{url} must filter to 1 matching item: {body}");
        assert_eq!(body["items"][0]["creation_id"], id_star);
    }

    let (st_nul, body_nul) = http_json(
        &app,
        "GET",
        "/api/v1/bots/inline-query?query=%00&q=Starlight",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_nul, StatusCode::OK, "NUL inline-query failed: {body_nul}");
    assert_eq!(body_nul["total"], 0, "NUL inline-query must short-circuit to 0: {body_nul}");

    let rec_url = format!("/bot/{CREATOR_BOT_TOKEN}/recommendContent");
    for payload in [
        json!({ "query": "   ", "q": "2049" }),
        json!({ "query": "", "q": "2049" }),
        json!({ "query": null, "q": "2049" }),
    ] {
        let (st_rec, body_rec) = http_json(&app, "POST", &rec_url, None, Some(payload.clone())).await;
        assert_eq!(st_rec, StatusCode::OK, "recommendContent({payload}) failed: {body_rec}");
        assert_eq!(body_rec["ok"], true);
        let items = body_rec["result"].as_array().expect("result must be array");
        assert_eq!(items.len(), 1, "expected 1 matching item for {payload}: {body_rec}");
        assert_eq!(items[0]["creation_id"], id_star);
    }

    let (st_rnul, body_rnul) = http_json(
        &app,
        "POST",
        &rec_url,
        None,
        Some(json!({ "query": "\0", "q": "2049" })),
    )
    .await;
    assert_eq!(st_rnul, StatusCode::OK, "recommendContent NUL query failed: {body_rnul}");
    assert_eq!(body_rnul["ok"], true);
    assert_eq!(
        body_rnul["result"].as_array().expect("result array").len(),
        0,
        "NUL recommendContent must return 0: {body_rnul}"
    );

    state.shutdown();
}

#[tokio::test]
async fn test_17_in_message_bot_publishing_lifecycle_search_aliases_and_zero_fake_catalog() {
    let (_dir, cfg, state, app) = make_temp_state();
    let (alice_uid, alice_tok) = register_user(&app, "alice_m3_17", "Alice", None).await;
    let (admin_uid, admin_tok) = register_user(&app, "admin_m3_17", "Admin", None).await;
    promote_admin(&state, admin_uid);

    // 1. Fresh server has ZERO legacy fake stickers/GIFs and ZERO legacy demo bots (777010..777012)
    let (st_stk, stk_body) = http_json(&app, "GET", "/api/v1/stickers/sets", Some(&alice_tok), None).await;
    assert_eq!(st_stk, StatusCode::OK, "stickers/sets failed: {stk_body}");
    assert_eq!(
        stk_body["sets"].as_array().map_or(0, Vec::len),
        0,
        "fresh server must have zero hardcoded fake sticker sets: {stk_body}"
    );
    let (st_gif, gif_body) = http_json(&app, "GET", "/api/v1/gifs/search?q=", Some(&alice_tok), None).await;
    assert_eq!(st_gif, StatusCode::OK, "gifs/search failed: {gif_body}");
    assert_eq!(
        gif_body["gifs"].as_array().map_or(0, Vec::len),
        0,
        "fresh server must have zero hardcoded fake GIFs: {gif_body}"
    );

    // 2. Global search in Messages finds @CreatorStudioBot via Chinese aliases ("创作", "发布", "图书")
    //    and never exposes legacy demo bots 777010/777011/777012.
    for kw in ["创作", "发布", "图书"] {
        let enc: String = url::form_urlencoded::byte_serialize(kw.as_bytes()).collect();
        let (st_s, s_body) = http_json(
            &app,
            "GET",
            &format!("/api/v1/search?q={enc}&category=all"),
            Some(&alice_tok),
            None,
        )
        .await;
        assert_eq!(st_s, StatusCode::OK, "search '{kw}' failed: {s_body}");
        let results = s_body["results"].as_array().expect("results array");
        assert!(
            results.iter().any(|r| r["bot_user_id"].as_i64() == Some(CREATOR_STUDIO_BOT_USER_ID)),
            "global search for '{kw}' must discover @CreatorStudioBot in Messages: {s_body}"
        );
        for legacy_demo_bot in [777_010i64, 777_011, 777_012] {
            assert!(
                !results.iter().any(|r| r["bot_user_id"].as_i64() == Some(legacy_demo_bot)),
                "legacy demo bot {legacy_demo_bot} must not appear in global search"
            );
        }
    }

    // 3. Opening DM with @CreatorStudioBot sends welcome card directly in Messages chat
    //    and omits any standalone "tg://apple/creations" panel button.
    let (alice_chat, initial_msg_count) = open_creator_bot_dm(&app, &alice_tok).await;
    assert!(
        initial_msg_count >= 1,
        "opening @CreatorStudioBot DM must deliver welcome card directly inside Messages"
    );
    let (_, dm_msgs) = http_json(
        &app,
        "GET",
        &format!("/api/v1/chats/{alice_chat}/messages?limit=20"),
        Some(&alice_tok),
        None,
    )
    .await;
    let dm_str = dm_msgs.to_string();
    assert!(
        !dm_str.contains("tg://apple/creations"),
        "welcome card must not point to removed standalone creations panel: {dm_str}"
    );

    // 4. Full in-message publishing lifecycle for an App:
    //    /newapp -> /submitwork -> /withdraw -> /submitwork -> /reviewqueue -> /reject -> /submitwork -> /approve -> /unpublish -> /deletework
    let app_rep = send_bot_cmd_and_wait(
        &app,
        &alice_tok,
        alice_chat,
        "/newapp Cloud Notes | Fast minimalist markdown workspace app for daily notes",
    )
    .await;
    let app_id = extract_creation_id_from_reply(&app_rep);
    let (st_get, app_dto) = http_json(&app, "GET", &format!("/api/v1/creations/{app_id}"), Some(&alice_tok), None).await;
    assert_eq!(st_get, StatusCode::OK, "fetch created app failed: {app_dto}");
    assert_eq!(app_dto["kind"], "app");
    assert_eq!(app_dto["owner_user_id"], alice_uid as i64);

    // Submit -> pending_review
    let sub1 = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &format!("/submitwork {app_id}")).await;
    assert!(sub1.to_string().contains("pending_review"), "submit1 failed: {sub1}");

    // Withdraw -> draft
    let wd = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &format!("/withdraw {app_id}")).await;
    assert!(wd.to_string().contains("draft"), "withdraw failed: {wd}");
    let (_, after_wd) = http_json(&app, "GET", &format!("/api/v1/creations/{app_id}"), Some(&alice_tok), None).await;
    assert_eq!(after_wd["review_status"], "draft");

    // Re-submit -> pending_review
    let sub2 = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &format!("/submitwork {app_id}")).await;
    assert!(sub2.to_string().contains("pending_review"), "submit2 failed: {sub2}");

    // Non-admin /reviewqueue and /approve must be rejected cleanly
    let non_admin_rq = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, "/reviewqueue").await;
    assert!(non_admin_rq.to_string().contains("Forbidden"), "non-admin /reviewqueue must be forbidden: {non_admin_rq}");
    let non_admin_ap = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &format!("/approve {app_id}")).await;
    assert!(non_admin_ap.to_string().contains("Forbidden"), "non-admin /approve must be forbidden: {non_admin_ap}");

    // Admin reviews via in-message @CreatorStudioBot DM: /reviewqueue -> /reject -> re-submit -> /approve
    let (admin_chat, _) = open_creator_bot_dm(&app, &admin_tok).await;
    let rq = send_bot_cmd_and_wait(&app, &admin_tok, admin_chat, "/reviewqueue").await;
    assert!(rq.to_string().contains(&app_id), "admin /reviewqueue must list {app_id}: {rq}");

    let rej = send_bot_cmd_and_wait(
        &app,
        &admin_tok,
        admin_chat,
        &format!("/reject {app_id} | Please expand description"),
    )
    .await;
    assert!(rej.to_string().contains("rejected"), "admin /reject failed: {rej}");
    let (_, after_rej) = http_json(&app, "GET", &format!("/api/v1/creations/{app_id}"), Some(&alice_tok), None).await;
    assert_eq!(after_rej["review_status"], "rejected");

    // Re-submit and approve in-message
    let _ = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &format!("/submitwork {app_id}")).await;
    let ap = send_bot_cmd_and_wait(&app, &admin_tok, admin_chat, &format!("/approve {app_id}")).await;
    assert!(ap.to_string().contains("published"), "admin /approve failed: {ap}");

    // Public catalog now contains the approved app
    let (st_cat, cat_body) = http_json(&app, "GET", "/api/v1/catalog/appstore", Some(&alice_tok), None).await;
    assert_eq!(st_cat, StatusCode::OK, "catalog/apps failed: {cat_body}");
    assert!(
        cat_body.to_string().contains(&app_id),
        "approved app must appear in public catalog/apps: {cat_body}"
    );

    // Owner /unpublish removes it from public catalog
    let unp = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &format!("/unpublish {app_id}")).await;
    assert!(unp.to_string().contains("unpublished"), "/unpublish failed: {unp}");
    let (_, cat_after_unp) = http_json(&app, "GET", "/api/v1/catalog/appstore", Some(&alice_tok), None).await;
    assert!(
        !cat_after_unp.to_string().contains(&app_id),
        "unpublished app must disappear from public catalog/apps: {cat_after_unp}"
    );

    // Owner /deletework soft-deletes the work
    let del = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, &format!("/deletework {app_id}")).await;
    assert!(
        del.to_string().contains("已删除") || del.to_string().contains("deleted"),
        "/deletework failed: {del}"
    );
    let my_after_del = send_bot_cmd_and_wait(&app, &alice_tok, alice_chat, "/mycreations").await;
    assert!(
        !my_after_del.to_string().contains(&app_id),
        "deleted work must not appear in /mycreations: {my_after_del}"
    );

    // 5. Upgrade purge test: inject legacy fake sticker pack + GIF + real user sticker pack,
    //    cold-restart AppState, verify fake rows are purged while real user pack is preserved.
    state
        .db
        .pool()
        .with_writer(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO sticker_sets(set_id, short_name, title, is_animated, is_video, is_official, created_at_ms)
                 VALUES ('pack_durov', 'durov_classic', 'Durov Classic', 0, 0, 1, 1000)",
                [],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
            conn.execute(
                "INSERT OR REPLACE INTO stickers(sticker_id, set_id, emoji, mime_type, width, height, file_id, lottie_json, sort_order)
                 VALUES ('builtin_stk_1', 'pack_durov', '😎', 'image/webp', 512, 512, NULL, NULL, 0)",
                [],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
            conn.execute(
                "INSERT OR REPLACE INTO sticker_sets(set_id, short_name, title, is_animated, is_video, is_official, created_at_ms)
                 VALUES ('pack_real_user', 'real_pack', 'Real Pack', 0, 0, 0, 2000)",
                [],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
            conn.execute(
                "INSERT OR REPLACE INTO gifs_catalog(gif_id, title, tags, width, height, duration_secs, mime_type, url, preview_url, created_at_ms)
                 VALUES ('gif_celebrate', 'Celebrate', 'party', 320, 240, 3, 'image/gif', 'https://example.invalid/c.gif', 'https://example.invalid/c.png', 1000)",
                [],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
            conn.execute(
                "INSERT OR REPLACE INTO gifs_catalog(gif_id, title, tags, width, height, duration_secs, mime_type, url, preview_url, created_at_ms)
                 VALUES ('gif_real_upload', 'Real Upload', 'real', 320, 240, 3, 'image/gif', 'https://cdn.example/real.gif', 'https://cdn.example/real.png', 2000)",
                [],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok(())
        })
        .expect("inject legacy + real sticker/gif rows");

    state.shutdown();
    drop(app);
    drop(state);

    let state2 = Arc::new(AppState::try_new(cfg).expect("reopen AppState for purge check"));
    let app2 = build_router_with_state(state2.clone());
    let (_, login_b) = http_json(
        &app2,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({
            "username": "alice_m3_17",
            "password": "StrongPassword123!",
            "device_name": "PurgeCheck"
        })),
    )
    .await;
    let tok2 = login_b["token"].as_str().unwrap().to_string();

    let (_, stk2) = http_json(&app2, "GET", "/api/v1/stickers/sets", Some(&tok2), None).await;
    let sets2 = stk2["sets"].as_array().expect("sets array");
    assert_eq!(sets2.len(), 1, "only real user sticker pack should survive restart purge: {stk2}");
    assert_eq!(sets2[0]["set_id"], "pack_real_user");

    let (_, gif2) = http_json(&app2, "GET", "/api/v1/gifs/search?q=", Some(&tok2), None).await;
    let gifs2 = gif2["gifs"].as_array().expect("gifs array");
    assert_eq!(gifs2.len(), 1, "only real user GIF should survive restart purge: {gif2}");
    assert_eq!(gifs2[0]["id"], "gif_real_upload");

    state2.shutdown();
}




