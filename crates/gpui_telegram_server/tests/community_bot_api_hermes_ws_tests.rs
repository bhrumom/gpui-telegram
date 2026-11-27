use axum::body::{to_bytes, Body};
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::routing::post;
use axum::Router;
use chrono::Utc;
use futures::StreamExt;
use gpui_telegram_agent::hermes::toolsets::{HermesToolExecutor, ToolsetPreset};
use gpui_telegram_agent::ApprovalMode;
use gpui_telegram_proto::community_contracts::{
    ActorKind, BotAutonomyMode, PostStatus,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use gpui_telegram_server::db::ServerDatabase;
use gpui_telegram_server::routes::community::run_community_background_tick;
use gpui_telegram_server::server::{build_router_with_state, AppState};
use gpui_telegram_server::ServerConfig;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tower::ServiceExt;

fn test_state(dir: &TempDir, db_name: &str) -> (Arc<AppState>, Router) {
    let mut cfg = ServerConfig::default();
    cfg.database_url = dir.path().join(db_name).display().to_string();
    cfg.media_storage_path = dir.path().join("media").display().to_string();
    cfg.allow_deterministic_bot_fallback = true;
    let state = Arc::new(AppState::try_new(cfg).expect("AppState::try_new"));
    let router = build_router_with_state(state.clone());
    (state, router)
}

async fn req_json(
    app: &Router,
    method: Method,
    uri: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(a) = auth {
        b = b.header("Authorization", a);
    }
    let req = if let Some(v) = body {
        b.header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&v).unwrap()))
            .unwrap()
    } else {
        b.body(Body::empty()).unwrap()
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let st = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let val = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes).to_string() }));
    (st, val)
}

async fn register_human(app: &Router, username: &str) -> (i64, String) {
    let (st, v) = req_json(
        app,
        Method::POST,
        "/api/v1/auth/register",
        None,
        Some(json!({ "username": username, "password": "Password123!", "first_name": username, "device_name": "test" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register failed: {v}");
    (v["user_id"].as_i64().unwrap(), v["token"].as_str().unwrap().to_string())
}

async fn register_bot(
    app: &Router,
    owner_bearer: &str,
    handle: &str,
    autonomy: &str,
) -> (i64, String, String) {
    let (st, v) = req_json(
        app,
        Method::POST,
        "/api/v1/community/identities/bot",
        Some(owner_bearer),
        Some(json!({
            "handle": handle,
            "display_name": format!("Bot {handle}"),
            "bot_autonomy_mode": autonomy,
            "bot_capabilities": ["rust", "moderation", "analysis"]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "bot create failed: {v}");
    (
        v["bot_user_id"].as_i64().unwrap(),
        v["bot_token"].as_str().unwrap().to_string(),
        v["actor"]["actor_id"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn test_bot_api_all_11_community_methods_both_route_shapes_and_5_hop_anti_loop() {
    let dir = TempDir::new().expect("tempdir");
    let (_state, app) = test_state(&dir, "bot_api_comm.db");
    let (_uid, tok) = register_human(&app, "alice_botapi").await;
    let bearer = format!("Bearer {tok}");

    let (_b1_id, b1_tok, b1_actor) = register_bot(&app, &bearer, "alpha_citizen_bot", "manual").await;
    let (_b2_id, b2_tok, _b2_actor) = register_bot(&app, &bearer, "beta_citizen_bot", "manual").await;

    // Create community owned by Alice and promote alpha_citizen_bot to bot_mod
    let (st, _) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/communities",
        Some(&bearer),
        Some(json!({ "slug": "bot_api_arena", "name": "Bot Arena", "allow_bots": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/communities/bot_api_arena/mod-actions",
        Some(&bearer),
        Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": b1_actor })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 401 invalid token & 404 unknown method & 400 bad params
    let (st, v) = req_json(&app, Method::POST, "/bot/bad_token/community.getFeed", None, Some(json!({}))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(v["ok"], false);
    let (st, v) = req_json(&app, Method::POST, &format!("/bot/{b1_tok}/community.unknownOp"), None, Some(json!({}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(v["ok"], false);
    let (st, v) = req_json(&app, Method::POST, &format!("/bot{b1_tok}/community.getPost"), None, Some(json!({}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["ok"], false);

    // Test both `/bot/{token}/community.*` AND `/bot{token}/community.*` across all 11+ methods
    // 1. community.createPost (split route)
    let (st, v) = req_json(
        &app,
        Method::POST,
        &format!("/bot/{b1_tok}/community.createPost"),
        None,
        Some(json!({ "community_slug": "bot_api_arena", "title": "BotApiQuasarPost", "body": "Hello @beta_citizen_bot from Bot API!" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "createPost failed: {v}");
    assert_eq!(v["ok"], true);
    let post_id = v["result"]["post"]["post_id"].as_str().unwrap().to_string();

    // 2. community.getFeed (concat route `/bot{token}/...`)
    let (st, v) = req_json(&app, Method::POST, &format!("/bot{b1_tok}/community.getFeed"), None, Some(json!({ "community": "bot_api_arena", "limit": 10 }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["ok"], true);
    assert!(!v["result"]["items"].as_array().unwrap().is_empty());

    // 3. community.getPost
    let (st, v) = req_json(&app, Method::POST, &format!("/bot/{b1_tok}/community.getPost"), None, Some(json!({ "post_id": post_id }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["result"]["post"]["post_id"], post_id);

    // 4. community.vote, 5. community.like, 6. community.repost, 7. community.quotePost
    let (st, v) = req_json(&app, Method::POST, &format!("/bot{b2_tok}/community.vote"), None, Some(json!({ "post_id": post_id, "direction": 1 }))).await;
    assert_eq!(st, StatusCode::OK, "vote: {v}");
    assert_eq!(v["result"]["score"], 2);

    let (st, v) = req_json(&app, Method::POST, &format!("/bot/{b2_tok}/community.like"), None, Some(json!({ "post_id": post_id }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["result"]["active"], true);

    let (st, v) = req_json(&app, Method::POST, &format!("/bot{b2_tok}/community.repost"), None, Some(json!({ "post_id": post_id }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["result"]["reposted"], true);

    let (st, v) = req_json(&app, Method::POST, &format!("/bot/{b2_tok}/community.quotePost"), None, Some(json!({ "quoted_post_id": post_id, "body": "Quoting @alpha_citizen_bot!" }))).await;
    assert_eq!(st, StatusCode::OK, "quotePost: {v}");
    assert_eq!(v["result"]["post"]["format"], "quote");

    // 8. community.search
    let (st, v) = req_json(&app, Method::POST, &format!("/bot{b2_tok}/community.search"), None, Some(json!({ "query": "BotApiQuasarPost" }))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(v["result"]["posts"].as_array().unwrap().iter().any(|p| p["post_id"] == post_id));

    // 9. community.getMentions (beta was mentioned in post_id!)
    let (st, v) = req_json(&app, Method::POST, &format!("/bot/{b2_tok}/community.getMentions"), None, Some(json!({ "limit": 20 }))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!v["result"]["notifications"].as_array().unwrap().is_empty());

    // 10. community.createComment + STRICT 5-HOP ANTI-LOOP CEILING
    let (st, hp_val) = req_json(&app, Method::POST, "/api/v1/community/posts", Some(&bearer), Some(json!({ "community_slug": "bot_api_arena", "body": "Human root post for 5-hop test" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let hop_post_id = hp_val["post"]["post_id"].as_str().unwrap().to_string();

    let mut parent_cid: Option<String> = None;
    for hop in 1..=5 {
        let tok_cur = if hop % 2 == 1 { &b1_tok } else { &b2_tok };
        let mut body = json!({ "post_id": hop_post_id, "body": format!("Bot hop {hop} analysis") });
        if let Some(ref p) = parent_cid {
            body["parent_comment_id"] = json!(p);
        }
        let (st, v) = req_json(&app, Method::POST, &format!("/bot/{tok_cur}/community.createComment"), None, Some(body)).await;
        assert_eq!(st, StatusCode::OK, "Hop {hop} must succeed, got: {v}");
        parent_cid = Some(v["result"]["comment"]["comment_id"].as_str().unwrap().to_string());
    }
    // Hop 6 MUST BE STRICTLY SUPPRESSED!
    let (st, v) = req_json(
        &app,
        Method::POST,
        &format!("/bot/{b2_tok}/community.createComment"),
        None,
        Some(json!({ "post_id": hop_post_id, "parent_comment_id": parent_cid, "body": "Hop 6 forbidden!" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "Hop 6 must be rejected: {v}");
    assert_eq!(v["ok"], false);
    assert!(v["description"].as_str().unwrap_or("").contains("bot_loop_budget_exceeded"));

    // 11. community.modAction + alias community.moderate
    let (st, v) = req_json(
        &app,
        Method::POST,
        &format!("/bot/{b1_tok}/community.modAction"),
        None,
        Some(json!({ "community_slug": "bot_api_arena", "action": "lock", "target_kind": "post", "target_id": post_id, "reason": "bot_mod lock" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "modAction failed: {v}");
    let (st, v) = req_json(
        &app,
        Method::POST,
        &format!("/bot{b1_tok}/community.moderate"),
        None,
        Some(json!({ "community_slug": "bot_api_arena", "action": "unlock", "target_kind": "post", "target_id": post_id, "reason": "bot_mod unlock" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "moderate alias failed: {v}");
}

#[tokio::test]
async fn test_bot_api_get_updates_live_webhook_secret_header_and_unreachable_nonblocking() {
    let dir = TempDir::new().expect("tempdir");
    let (_state, app) = test_state(&dir, "bot_webhook.db");
    let (_uid, tok) = register_human(&app, "alice_hook").await;
    let bearer = format!("Bearer {tok}");
    let (_bid, b_tok, _b_actor) = register_bot(&app, &bearer, "hook_watch_bot", "manual").await;

    // Spin up ephemeral webhook receiver on 127.0.0.1:0
    let (tx, mut rx) = mpsc::unbounded_channel::<(Option<String>, Value)>();
    let hook_router = Router::new()
        .route(
            "/hook",
            post(
                |AxumState(tx): AxumState<mpsc::UnboundedSender<(Option<String>, Value)>>,
                 headers: HeaderMap,
                 axum::Json(body): axum::Json<Value>| async move {
                    let sec = headers
                        .get("X-Telegram-Bot-Api-Secret-Token")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    let _ = tx.send((sec, body));
                    StatusCode::OK
                },
            ),
        )
        .with_state(tx);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hook_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, hook_router).await;
    });

    // Register webhook with secret token
    let (st, v) = req_json(
        &app,
        Method::POST,
        &format!("/bot/{b_tok}/setWebhook"),
        None,
        Some(json!({ "url": format!("http://{addr}/hook"), "secret_token": "comm_secret_42" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "setWebhook: {v}");

    // Human mentions @hook_watch_bot in a post
    let (st, _) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bearer),
        Some(json!({ "body": "Paging @hook_watch_bot for community architecture review!" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // 1. Verify getUpdates has community_mention
    let (st, upd) = req_json(&app, Method::GET, &format!("/bot/{b_tok}/getUpdates"), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let updates = upd["result"].as_array().unwrap();
    assert!(
        updates.iter().any(|u| u.get("community_mention").is_some()),
        "Expected community_mention in getUpdates: {upd}"
    );

    // 2. Verify live webhook received POST with exact secret token header
    let (got_secret, got_body) = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
        .await
        .expect("webhook timeout")
        .expect("webhook channel closed");
    assert_eq!(got_secret.as_deref(), Some("comm_secret_42"));
    assert!(got_body.get("community_mention").is_some(), "webhook payload: {got_body}");
    assert!(got_body.get("update_id").is_some());

    // 3. Unreachable webhook URL must NOT block HTTP response latency
    let (st, _) = req_json(
        &app,
        Method::POST,
        &format!("/bot/{b_tok}/setWebhook"),
        None,
        Some(json!({ "url": "http://127.0.0.1:1/unreachable_hook" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let t0 = std::time::Instant::now();
    let (st, _) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bearer),
        Some(json!({ "body": "Mention @hook_watch_bot with unreachable webhook" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert!(
        t0.elapsed() < std::time::Duration::from_millis(1500),
        "Unreachable webhook must not slow down HTTP response: {:?}",
        t0.elapsed()
    );
    hook_handle.abort();
}

#[tokio::test]
async fn test_live_ws_community_event_fanout_scheduler_tick_and_weak_appstate_drain() {
    let dir = TempDir::new().expect("tempdir");
    let (state, app) = test_state(&dir, "ws_sched.db");
    let weak_state = Arc::downgrade(&state);
    let weak_idx = Arc::downgrade(&state.community_search_indexer);

    let (_alice_uid, alice_tok) = register_human(&app, "alice_ws").await;
    let (_bob_uid, bob_tok) = register_human(&app, "bob_ws").await;
    let alice_bearer = format!("Bearer {alice_tok}");

    // Register an autonomous bot with rust/gpui capabilities
    let (_bid, _btok, _b_actor) = register_bot(&app, &alice_bearer, "auto_rust_bot", "autonomous").await;

    // Bind live TCP listener for real WS connections
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let srv_app = app.clone();
    let srv_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, srv_app)
            .with_graceful_shutdown(async move {
                let _ = stop_rx.await;
            })
            .await;
    });

    let ws_alice_url = format!("ws://{addr}/api/v1/ws?token={alice_tok}");
    let ws_bob_url = format!("ws://{addr}/api/v1/ws?token={bob_tok}");
    let (mut ws_alice, _) = tokio_tungstenite::connect_async(&ws_alice_url).await.unwrap();
    let (ws_bob, _) = tokio_tungstenite::connect_async(&ws_bob_url).await.unwrap();
    // Drop Bob's socket immediately to verify dead subscribers never block broadcast_all
    drop(ws_bob);
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;

    // Create a published post -> Alice must receive CommunityEvent("post_created") over WS
    let (st, _) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        Some(json!({ "title": "WS Live Post", "body": "Testing real-time rust gpui analysis fanout!" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let mut saw_post_created = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(400), ws_alice.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(txt)))) => {
                if let Ok(WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, .. })) =
                    serde_json::from_str::<WsServerFrame>(&txt)
                {
                    if event_kind == "post_created" {
                        saw_post_created = true;
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    assert!(saw_post_created, "Live WS client must receive UpdateEvent::CommunityEvent(post_created)");

    // Schedule a post & backdate its scheduled_at_ms, plus create a poll & backdate its closes_at_ms
    let future_ms = Utc::now().timestamp_millis() + 120_000;
    let (st, sched_val) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        Some(json!({
            "title": "ScheduledCobaltPost",
            "body": "Scheduled post ready to publish via 5s tick",
            "status": "scheduled",
            "scheduled_at_ms": future_ms
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "scheduled post: {sched_val}");
    let sched_pid = sched_val["post"]["post_id"].as_str().unwrap().to_string();

    let (st, poll_val) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        Some(json!({
            "format": "poll",
            "body": "Expiring poll for scheduler",
            "poll": { "question": "Ready?", "options": ["Yes", "No"], "duration_secs": 3600 }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let poll_pid = poll_val["post"]["post_id"].as_str().unwrap().to_string();

    let past_ms = Utc::now().timestamp_millis() - 5_000;
    state
        .db
        .pool()
        .with_writer(|conn| {
            conn.execute(
                "UPDATE community_posts SET scheduled_at_ms = ?1 WHERE post_id = ?2",
                rusqlite::params![past_ms, sched_pid],
            )
            .unwrap();
            conn.execute(
                "UPDATE community_posts SET poll_closes_at_ms = ?1 WHERE post_id = ?2",
                rusqlite::params![past_ms, poll_pid],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

    // Run background tick deterministically
    run_community_background_tick(&state).await;

    // Verify scheduled post is now Published + indexed in Tantivy, and poll is closed
    let (sched_status, poll_closed) = state
        .db
        .pool()
        .with_reader(|conn| {
            let p1 = gpui_telegram_community::store::ops::get_post_conn(conn, &sched_pid, None)?.unwrap();
            let p2 = gpui_telegram_community::store::ops::get_post_conn(conn, &poll_pid, None)?.unwrap();
            Ok((p1.status, p2.poll.unwrap().is_closed))
        })
        .unwrap();
    assert_eq!(sched_status, PostStatus::Published);
    assert!(poll_closed, "Expired poll must be closed by scheduler tick");

    let (st, search_v) = req_json(&app, Method::GET, "/api/v1/community/search?q=ScheduledCobaltPost", Some(&alice_bearer), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(search_v["posts"].as_array().unwrap().iter().any(|p| p["post_id"] == sched_pid));

    // Verify Weak<AppState> drains cleanly with ZERO Arc leaks!
    drop(ws_alice);
    state.shutdown();
    let _ = stop_tx.send(());
    let _ = srv_handle.await;
    drop(app);
    drop(state);

    let drain_deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    while (weak_state.strong_count() > 0 || weak_idx.strong_count() > 0)
        && std::time::Instant::now() < drain_deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(weak_state.strong_count(), 0, "AppState leaked strong Arc reference after shutdown!");
    assert_eq!(weak_idx.strong_count(), 0, "CommunitySearchIndexer leaked strong Arc reference!");
}

#[tokio::test]
async fn test_hermes_tool_executor_server_platform_im_bridge_e2e_bot_and_human() {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("hermes_comm_bridge.db");
    let db = Arc::new(ServerDatabase::open(&db_path).expect("open db"));
    let mut cfg = ServerConfig::default();
    cfg.database_url = db_path.display().to_string();
    cfg.media_storage_path = dir.path().join("media").display().to_string();
    // Deliberately construct AppState::with_db WITHOUT build_router_with_state (self_weak unset!)
    let state = Arc::new(AppState::with_db(cfg, db.clone()));

    let now_ms = Utc::now().timestamp_millis();
    let alice_uid = gpui_telegram_core::types::UserId(100_501);
    db.create_user(&gpui_telegram_server::db::schema::UserRecord {
        user_id: alice_uid,
        username: Some("alice_hermes".to_string()),
        phone: None,
        first_name: "Alice".to_string(),
        last_name: None,
        bio: Some("Human engineer".to_string()),
        password_hash: None,
        is_bot: false,
        is_verified: false,
        is_support: false,
        is_scam: false,
        is_fake: false,
        profile_photo_file_id: None,
        account_generation: 1,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    })
    .unwrap();

    let bot = db
        .create_registered_bot(
            alice_uid.0 as i64,
            "hermes_citizen_bot",
            "Hermes Citizen Bot",
            "First-class community AI citizen",
            "Community Bot",
            "Helpful systems thinker",
            "",
            "",
            "[]",
            false,
            false,
        )
        .expect("create registered bot");

    // Create community owned by Alice and promote bot to bot_mod
    let _ = gpui_telegram_server::routes::community::sync_telegram_identities_for_user(
        &db, alice_uid, now_ms,
    )
    .unwrap();
    let bot_actor_id = gpui_telegram_community::bot_actor_id(bot.bot_user_id);
    db.pool()
        .with_transaction(|tx| {
            gpui_telegram_community::store::ops::create_community_tx(
                tx,
                &gpui_telegram_community::human_actor_id(alice_uid.0 as i64),
                &gpui_telegram_proto::community_contracts::CreateCommunityRequest {
                    slug: "hermes_hq".to_string(),
                    name: "Hermes HQ".to_string(),
                    description: "Agent community".to_string(),
                    category: "tech".to_string(),
                    banner_media_id: None,
                    icon_media_id: None,
                    icon_emoji: None,
                    accent_color: None,
                    rules: vec![],
                    post_flairs: vec![],
                    user_flairs: vec![],
                    wiki_markdown: String::new(),
                    automod_rules: vec![],
                    allow_bots: true,
                    require_bot_disclosure: true,
                    is_nsfw: false,
                },
                now_ms,
            )?;
            gpui_telegram_community::store::ops::apply_mod_action_tx(
                tx,
                "hermes_hq",
                &gpui_telegram_community::human_actor_id(alice_uid.0 as i64),
                &gpui_telegram_proto::community_contracts::ModActionRequest {
                    action: gpui_telegram_proto::community_contracts::ModActionKind::PromoteMod,
                    target_kind: "actor".to_string(),
                    target_id: bot_actor_id.clone(),
                    reason: Some("bot mod".to_string()),
                    flair: None,
                    queue_item_id: None,
                },
                now_ms,
            )?;
            Ok(())
        })
        .unwrap();

    // Pure Bot context: user_id = None, bot_id = Some(bot.bot_user_id)
    let bot_ctx = state
        .fabushi_core()
        .build_tool_context_with_user(
            "sess_pure_bot",
            None,
            None,
            None,
            1,
            Some(ToolsetPreset::HermesTelegram),
            0,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .with_chat_and_bot(None, Some(bot.bot_user_id))
        .with_user_id(None)
        .with_approval_mode(ApprovalMode::Prompt);

    // 1. Bot creates post -> authored as b:{bot_user_id} (ActorKind::Bot) & indexed in Tantivy!
    let r_post = HermesToolExecutor::execute(
        "community_create_post",
        json!({
            "community_slug": "hermes_hq",
            "title": "HermesNebulaPost",
            "body": "Published natively via Hermes PlatformIm pure Bot context!"
        }),
        &bot_ctx,
    )
    .await;
    assert!(r_post.success, "community_create_post failed: {:?}", r_post.error);
    let post_val = r_post.structured.unwrap();
    let post_id = post_val["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(post_val["post"]["author_actor_id"], bot_actor_id);
    assert_eq!(post_val["post"]["author_kind"], ActorKind::Bot.as_str());

    // 2. Search finds the bot's post immediately via Weak<CommunitySearchIndexer> (even with self_weak=None!)
    let r_search = HermesToolExecutor::execute(
        "community_search",
        json!({ "query": "HermesNebulaPost", "tab": "top" }),
        &bot_ctx,
    )
    .await;
    assert!(r_search.success, "community_search failed: {:?}", r_search.error);
    assert!(
        r_search.structured.unwrap()["posts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["post_id"] == post_id)
    );

    // 3. Browse feed & get post
    let r_feed = HermesToolExecutor::execute(
        "community_browse_feed",
        json!({ "community_slug": "hermes_hq", "sort": "new", "limit": 10 }),
        &bot_ctx,
    )
    .await;
    assert!(r_feed.success);
    let r_get = HermesToolExecutor::execute(
        "community_get_post",
        json!({ "post_id": post_id }),
        &bot_ctx,
    )
    .await;
    assert!(r_get.success);

    // 4. Human context: user_id = Some(alice_uid) -> authors as u:{alice_uid}
    let human_ctx = state
        .fabushi_core()
        .build_tool_context_with_user(
            "sess_human_comm",
            None,
            None,
            Some(&alice_uid.0.to_string()),
            1,
            Some(ToolsetPreset::HermesTelegram),
            0,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
    let r_cmt = HermesToolExecutor::execute(
        "community_comment",
        json!({ "post_id": post_id, "body": "Human replying via Hermes bridge!" }),
        &human_ctx,
    )
    .await;
    assert!(r_cmt.success, "community_comment failed: {:?}", r_cmt.error);
    assert_eq!(
        r_cmt.structured.unwrap()["comment"]["author_kind"],
        ActorKind::Human.as_str()
    );

    // 5. Vote, quote post, follow
    let r_vote = HermesToolExecutor::execute(
        "community_vote",
        json!({ "post_id": post_id, "direction": 1 }),
        &human_ctx,
    )
    .await;
    assert!(r_vote.success);
    assert_eq!(r_vote.structured.unwrap()["score"], 2);

    let r_quote = HermesToolExecutor::execute(
        "community_quote_post",
        json!({ "quoted_post_id": post_id, "body": "Bot quoting its own post!" }),
        &bot_ctx,
    )
    .await;
    assert!(r_quote.success, "community_quote_post failed: {:?}", r_quote.error);

    let r_follow = HermesToolExecutor::execute(
        "community_follow",
        json!({ "actor_id": gpui_telegram_community::human_actor_id(alice_uid.0 as i64), "follow": true }),
        &bot_ctx,
    )
    .await;
    assert!(r_follow.success, "community_follow failed: {:?}", r_follow.error);

    // 6. ApprovalMode::Deny blocks community_mod_action with ZERO DB mutation!
    let bot_deny_ctx = bot_ctx
        .clone()
        .with_session("sess_pure_bot_deny", 1)
        .with_approval_mode(ApprovalMode::Deny);
    let r_mod_deny = HermesToolExecutor::execute(
        "community_mod_action",
        json!({
            "community_slug": "hermes_hq",
            "action": "lock",
            "target_kind": "post",
            "target_id": post_id,
            "reason": "should be denied"
        }),
        &bot_deny_ctx,
    )
    .await;
    assert!(!r_mod_deny.success, "ApprovalMode::Deny must block community_mod_action");
    let post_still_unlocked = db
        .pool()
        .with_reader(|conn| gpui_telegram_community::store::ops::get_post_conn(conn, &post_id, None))
        .unwrap()
        .unwrap();
    assert!(!post_still_unlocked.is_locked, "Denied mod action must leave post unlocked");

    // 7. Under Prompt approval, bot_mod executes community_mod_action cleanly!
    let r_mod_ok = HermesToolExecutor::execute(
        "community_mod_action",
        json!({
            "community_slug": "hermes_hq",
            "action": "lock",
            "target_kind": "post",
            "target_id": post_id,
            "reason": "approved bot_mod lock"
        }),
        &bot_ctx,
    )
    .await;
    assert!(r_mod_ok.success, "bot_mod community_mod_action failed: {:?}", r_mod_ok.error);
    let post_now_locked = db
        .pool()
        .with_reader(|conn| gpui_telegram_community::store::ops::get_post_conn(conn, &post_id, None))
        .unwrap()
        .unwrap();
    assert!(post_now_locked.is_locked);
    let _ = BotAutonomyMode::Manual;
}

async fn upload_media_b64(
    app: &Router,
    bearer: &str,
    filename: &str,
    bytes: &[u8],
    chat_id: Option<i64>,
) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let uri = match chat_id {
        Some(cid) => format!("/api/v1/media/upload?chat_id={cid}"),
        None => "/api/v1/media/upload".to_string(),
    };
    let (st, val) = req_json(
        app,
        Method::POST,
        &uri,
        Some(bearer),
        Some(json!({ "filename": filename, "mime_type": "image/png", "data_base64": b64 })),
    )
    .await;
    assert!(st.is_success(), "media upload failed ({st}): {val}");
    val["media_id"]
        .as_str()
        .or_else(|| val["file_id"].as_str())
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn test_bot_api_and_hermes_media_hijack_blocked_and_draft_quote_vote_blocked() {
    let dir = TempDir::new().expect("tempdir");
    let (state, app) = test_state(&dir, "bot_hermes_acl.db");
    let (_alice_uid, alice_tok) = register_human(&app, "alice_acl").await;
    let (bob_uid, bob_tok) = register_human(&app, "bob_acl").await;
    let alice_bearer = format!("Bearer {alice_tok}");
    let bob_bearer = format!("Bearer {bob_tok}");

    let (alice_bot_id, alice_bot_tok, _alice_bot_actor) =
        register_bot(&app, &alice_bearer, "alice_owner_bot", "manual").await;
    let (bob_bot_id, bob_bot_tok, _bob_bot_actor) =
        register_bot(&app, &bob_bearer, "bob_attacker_bot", "manual").await;

    // Alice creates private chat + uploads 4 files: unattached1, unattached2, draft_media, chat_media
    let (st, chat_v) = req_json(&app, Method::POST, "/api/v1/chats", Some(&alice_bearer), Some(json!({ "title": "Alice Priv", "chat_type": "group" }))).await;
    assert!(st.is_success());
    let priv_chat_id = chat_v["chat_id"].as_i64().or_else(|| chat_v["id"].as_i64()).unwrap();

    let mid_unatt1 = upload_media_b64(&app, &alice_bearer, "u1.png", b"\x89PNG\r\n\x1a\nU1", None).await;
    let mid_unatt2 = upload_media_b64(&app, &alice_bearer, "u2.png", b"\x89PNG\r\n\x1a\nU2", None).await;
    let mid_draft = upload_media_b64(&app, &alice_bearer, "dr.png", b"\x89PNG\r\n\x1a\nDR", None).await;
    let mid_chat = upload_media_b64(&app, &alice_bearer, "ch.png", b"\x89PNG\r\n\x1a\nCH", Some(priv_chat_id)).await;

    // Alice creates a Draft post holding mid_draft, a Published post to quote/delete, and a Deleted post
    let (st, draft_v) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        Some(json!({
            "format": "media",
            "title": "SECRET_BOTAPI_DRAFT",
            "body": "SECRET_DRAFT_CONTENT",
            "status": "draft",
            "media": [{ "media_id": mid_draft, "mime_type": "image/png" }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let draft_pid = draft_v["post"]["post_id"].as_str().unwrap().to_string();

    let (st, pub_v) = req_json(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bearer), Some(json!({ "body": "Public anchor post" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let pub_pid = pub_v["post"]["post_id"].as_str().unwrap().to_string();

    let (st, del_v) = req_json(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bearer), Some(json!({ "title": "SECRET_DELETED_TITLE", "body": "SECRET_DELETED_BODY" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let del_pid = del_v["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = req_json(&app, Method::DELETE, &format!("/api/v1/community/posts/{del_pid}"), Some(&alice_bearer), None).await;
    assert_eq!(st, StatusCode::OK);

    // 1. Bot API media-hijack attempts by Bob's bot (media, thread_parts[*].media, draft, chat, quotePost) -> ALL 403!
    let hijack_cases = vec![
        ("community.createPost", json!({ "format": "media", "body": "hijack u1", "media": [{ "media_id": mid_unatt1, "mime_type": "image/png" }] })),
        ("community.createPost", json!({ "format": "micro", "body": "root", "thread_parts": [{ "body": "part2", "media": [{ "media_id": mid_unatt1, "mime_type": "image/png" }] }] })),
        ("community.createPost", json!({ "format": "media", "body": "hijack draft", "media": [{ "media_id": mid_draft, "mime_type": "image/png" }] })),
        ("community.createPost", json!({ "format": "media", "body": "hijack chat", "media": [{ "media_id": mid_chat, "mime_type": "image/png" }] })),
        ("community.quotePost", json!({ "quoted_post_id": pub_pid, "body": "quote hijack", "media": [{ "media_id": mid_unatt1, "mime_type": "image/png" }] })),
    ];
    for (method_name, payload) in hijack_cases {
        let (st, v) = req_json(&app, Method::POST, &format!("/bot/{bob_bot_tok}/{method_name}"), None, Some(payload)).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "Bot API {method_name} hijack must be 403, got: {v}");
        assert_eq!(v["ok"], false);
    }

    // Positive & private-chat check for Alice's own bot via Bot API
    let (st, v) = req_json(&app, Method::POST, &format!("/bot/{alice_bot_tok}/community.createPost"), None, Some(json!({ "format": "media", "body": "owner bot ok", "media": [{ "media_id": mid_unatt1, "mime_type": "image/png" }] }))).await;
    assert_eq!(st, StatusCode::OK, "Owner's bot must be allowed to attach owner's unattached media: {v}");
    let (st, _) = req_json(&app, Method::POST, &format!("/bot/{alice_bot_tok}/community.createPost"), None, Some(json!({ "format": "media", "body": "owner chat block", "media": [{ "media_id": mid_chat, "mime_type": "image/png" }] }))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // 2. Hermes media-hijack attempts by Bob's Bot & Human contexts -> ALL success == false with Forbidden!
    let bob_bot_ctx = state.fabushi_core().build_tool_context_with_user("s_bob_bot", None, None, None, 1, Some(ToolsetPreset::HermesTelegram), 0, Arc::new(std::sync::atomic::AtomicBool::new(false)))
        .with_chat_and_bot(None, Some(bob_bot_id))
        .with_user_id(None);
    let bob_human_ctx = state.fabushi_core().build_tool_context_with_user("s_bob_hum", None, None, Some(&bob_uid.to_string()), 1, Some(ToolsetPreset::HermesTelegram), 0, Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let alice_bot_ctx = state.fabushi_core().build_tool_context_with_user("s_alice_bot", None, None, None, 1, Some(ToolsetPreset::HermesTelegram), 0, Arc::new(std::sync::atomic::AtomicBool::new(false)))
        .with_chat_and_bot(None, Some(alice_bot_id))
        .with_user_id(None);

    for ctx in [&bob_bot_ctx, &bob_human_ctx] {
        for (tool, args) in [
            ("community_create_post", json!({ "format": "media", "body": "h1", "media": [{ "media_id": mid_unatt2, "mime_type": "image/png" }] })),
            ("community_create_post", json!({ "format": "micro", "body": "h2", "thread_parts": [{ "body": "tp", "media": [{ "media_id": mid_unatt2, "mime_type": "image/png" }] }] })),
            ("community_create_post", json!({ "format": "media", "body": "h3", "media": [{ "media_id": mid_draft, "mime_type": "image/png" }] })),
            ("community_quote_post", json!({ "quoted_post_id": pub_pid, "body": "hq", "media": [{ "media_id": mid_chat, "mime_type": "image/png" }] })),
        ] {
            let r = HermesToolExecutor::execute(tool, args, ctx).await;
            assert!(!r.success, "Hermes {tool} media hijack MUST fail: {:?}", r.structured);
            let err_msg = r.error.unwrap_or_default();
            assert!(err_msg.contains("Forbidden"), "Hermes {tool} media hijack must fail with Forbidden, got: {err_msg}");
        }
    }
    // Victim's unattached2, draft, and private-chat media MUST remain 403 Forbidden for Bob!
    for mid in [&mid_unatt2, &mid_draft, &mid_chat] {
        let (st, _) = req_json(&app, Method::GET, &format!("/api/v1/media/{mid}"), Some(&bob_bearer), None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "Victim media {mid} must remain 403 for Bob");
    }
    // Alice's own Hermes pure bot CAN attach Alice's unattached2, CANNOT attach private-chat media
    let r_ok = HermesToolExecutor::execute("community_create_post", json!({ "format": "media", "body": "owner hermes ok", "media": [{ "media_id": mid_unatt2, "mime_type": "image/png" }] }), &alice_bot_ctx).await;
    assert!(r_ok.success, "Owner's pure bot must be allowed to attach owner's unattached media: {:?}", r_ok.error);
    let r_chat = HermesToolExecutor::execute("community_create_post", json!({ "format": "media", "body": "owner hermes chat block", "media": [{ "media_id": mid_chat, "mime_type": "image/png" }] }), &alice_bot_ctx).await;
    assert!(!r_chat.success);

    // 3. Bot API & Hermes Draft / Deleted post quote/comment/vote/like/repost -> ALL 404 / failure!
    for hidden_pid in [&draft_pid, &del_pid] {
        for (m, payload) in [
            ("community.quotePost", json!({ "quoted_post_id": hidden_pid, "body": "quote leak" })),
            ("community.createPost", json!({ "format": "quote", "quoted_post_id": hidden_pid, "body": "create quote leak" })),
            ("community.createComment", json!({ "post_id": hidden_pid, "body": "hidden comment" })),
            ("community.vote", json!({ "post_id": hidden_pid, "direction": 1 })),
            ("community.like", json!({ "post_id": hidden_pid })),
            ("community.repost", json!({ "post_id": hidden_pid })),
        ] {
            let (st, v) = req_json(&app, Method::POST, &format!("/bot/{bob_bot_tok}/{m}"), None, Some(payload)).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "Bot API {m} on hidden post must be 404: {v}");
            assert_eq!(v["ok"], false);
            assert!(!v.to_string().contains("SECRET_"));
        }
        for (tool, args) in [
            ("community_quote_post", json!({ "quoted_post_id": hidden_pid, "body": "hermes quote leak" })),
            ("community_comment", json!({ "post_id": hidden_pid, "body": "hermes hidden comment" })),
            ("community_vote", json!({ "post_id": hidden_pid, "direction": 1 })),
        ] {
            let r = HermesToolExecutor::execute(tool, args, &bob_bot_ctx).await;
            assert!(!r.success, "Hermes {tool} on hidden post MUST fail");
        }
    }
}

#[tokio::test]
async fn test_concurrent_5_hop_toctou_race_banned_bot_filter_and_title_mentions() {
    let dir = TempDir::new().expect("tempdir");
    let (state, app) = test_state(&dir, "bot_race_ban_title.db");
    let (_alice_uid, alice_tok) = register_human(&app, "alice_race").await;
    let alice_bearer = format!("Bearer {alice_tok}");

    let (_race_bid, race_bot_tok, _race_bot_actor) =
        register_bot(&app, &alice_bearer, "race_flood_bot", "manual").await;

    // 1. 12-way concurrent Bot API community.createComment flood synchronized on Barrier(12)
    let (st, p_val) = req_json(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bearer), Some(json!({ "body": "Concurrent 5-hop race target post" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let race_pid = p_val["post"]["post_id"].as_str().unwrap().to_string();

    let barrier = Arc::new(tokio::sync::Barrier::new(12));
    let mut handles = Vec::with_capacity(12);
    for idx in 0..12 {
        let app_cl = app.clone();
        let tok_cl = race_bot_tok.clone();
        let pid_cl = race_pid.clone();
        let bar_cl = barrier.clone();
        handles.push(tokio::spawn(async move {
            bar_cl.wait().await;
            req_json(
                &app_cl,
                Method::POST,
                &format!("/bot/{tok_cl}/community.createComment"),
                None,
                Some(json!({ "post_id": pid_cl, "body": format!("Concurrent bot turn {idx}") })),
            )
            .await
        }));
    }
    let mut ok_count = 0usize;
    let mut rejected_400_count = 0usize;
    for h in handles {
        let (st, v) = h.await.unwrap();
        if st == StatusCode::OK {
            ok_count += 1;
        } else {
            assert_eq!(st, StatusCode::BAD_REQUEST, "Unexpected rejection status: {st} {v}");
            assert!(v["description"].as_str().unwrap_or("").contains("bot_loop_budget_exceeded"));
            rejected_400_count += 1;
        }
    }
    assert_eq!(ok_count, 5, "Exactly 5 concurrent bot comments may succeed");
    assert_eq!(rejected_400_count, 7, "Exactly 7 excess bot comments must be rejected");
    let db_cnt: i64 = state
        .db
        .pool()
        .with_reader(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM community_comments WHERE post_id = ?1",
                rusqlite::params![&race_pid],
                |r| r.get(0),
            )
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert_eq!(db_cnt, 5, "SQLite must persist strictly <= 5 consecutive bot comments");

    // 2. Banned bot pre-filter before BotWakeSelector (no starvation of eligible bot & zero cooldown burn)
    let (st, _) = req_json(&app, Method::POST, "/api/v1/community/communities", Some(&alice_bearer), Some(json!({ "slug": "ban_filter_lab", "name": "Ban Filter Lab", "allow_bots": true }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (_b_ban_id, _b_ban_tok, banned_actor_id) =
        register_bot(&app, &alice_bearer, "banned_alpha_bot", "reactive").await;
    let (_b_ok_id, _b_ok_tok, eligible_actor_id) =
        register_bot(&app, &alice_bearer, "eligible_beta_bot", "reactive").await;

    let (st, _) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/communities/ban_filter_lab/mod-actions",
        Some(&alice_bearer),
        Some(json!({ "action": "ban_actor", "target_kind": "actor", "target_id": banned_actor_id, "reason": "test ban" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, bp_val) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        Some(json!({
            "community_slug": "ban_filter_lab",
            "title": "Architecture check",
            "body": "Paging @banned_alpha_bot and @eligible_beta_bot for rust systems review!"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let ban_pid = bp_val["post"]["post_id"].as_str().unwrap().to_string();

    // Poll up to 2s for async reactive bot turn
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut eligible_replied = false;
    while tokio::time::Instant::now() < deadline {
        let cnt: i64 = state
            .db
            .pool()
            .with_reader(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM community_comments WHERE post_id = ?1 AND author_actor_id = ?2",
                    rusqlite::params![&ban_pid, &eligible_actor_id],
                    |r| r.get(0),
                )
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
            })
            .unwrap();
        if cnt >= 1 {
            eligible_replied = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    assert!(eligible_replied, "Eligible bot MUST NOT be starved by banned bot ahead of it");
    let (banned_cmts, banned_cds): (i64, i64) = state
        .db
        .pool()
        .with_reader(|conn| {
            let c: i64 = conn.query_row(
                "SELECT COUNT(*) FROM community_comments WHERE post_id = ?1 AND author_actor_id = ?2",
                rusqlite::params![&ban_pid, &banned_actor_id],
                |r| r.get(0),
            ).map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
            let cd: i64 = conn.query_row(
                "SELECT COUNT(*) FROM community_bot_cooldowns WHERE bot_actor_id = ?1",
                rusqlite::params![&banned_actor_id],
                |r| r.get(0),
            ).map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
            Ok((c, cd))
        })
        .unwrap();
    assert_eq!(banned_cmts, 0, "Banned bot must create 0 comments");
    assert_eq!(banned_cds, 0, "Banned bot must NOT burn any cooldown rows");

    // 3. Title-only @mention on Scheduled post publish tick AND Hermes fallback (self_weak == None)
    let (_ts_bid, ts_bot_tok, _ts_actor) =
        register_bot(&app, &alice_bearer, "title_sched_bot", "manual").await;
    let future_ms = Utc::now().timestamp_millis() + 120_000;
    let (st, sch_val) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        Some(json!({
            "title": "Urgent alert for @title_sched_bot only in title",
            "body": "Body deliberately has zero mentions",
            "status": "scheduled",
            "scheduled_at_ms": future_ms
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let sch_pid = sch_val["post"]["post_id"].as_str().unwrap().to_string();
    let past_ms = Utc::now().timestamp_millis() - 5_000;
    state
        .db
        .pool()
        .with_writer(|conn| {
            conn.execute(
                "UPDATE community_posts SET scheduled_at_ms = ?1 WHERE post_id = ?2",
                rusqlite::params![past_ms, &sch_pid],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    run_community_background_tick(&state).await;

    let (st, upd) = req_json(&app, Method::GET, &format!("/bot/{ts_bot_tok}/getUpdates"), None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        upd["result"].as_array().unwrap().iter().any(|u| {
            u.get("community_mention").and_then(|m| m["post_id"].as_str()) == Some(sch_pid.as_str())
        }),
        "Scheduled post publish tick must scan title for @mentions: {upd}"
    );

    // Hermes fallback (self_weak == None) scanning title-only @mention
    let dir2 = TempDir::new().expect("tempdir2");
    let db2_path = dir2.path().join("hermes_title_fb.db");
    let db2 = Arc::new(ServerDatabase::open(&db2_path).unwrap());
    let mut cfg2 = ServerConfig::default();
    cfg2.database_url = db2_path.display().to_string();
    cfg2.media_storage_path = dir2.path().join("media").display().to_string();
    let st2 = Arc::new(AppState::with_db(cfg2, db2.clone()));
    let now_ms = Utc::now().timestamp_millis();
    let u2 = gpui_telegram_core::types::UserId(200_777);
    db2.create_user(&gpui_telegram_server::db::schema::UserRecord {
        user_id: u2,
        username: Some("human_fb".into()),
        phone: None,
        first_name: "Human".into(),
        last_name: None,
        bio: None,
        password_hash: None,
        is_bot: false,
        is_verified: false,
        is_support: false,
        is_scam: false,
        is_fake: false,
        profile_photo_file_id: None,
        account_generation: 1,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    })
    .unwrap();
    let b2 = db2
        .create_registered_bot(u2.0 as i64, "title_hermes_bot", "Title Hermes Bot", "", "", "", "", "", "[]", false, false)
        .unwrap();
    let _ = gpui_telegram_server::routes::community::sync_telegram_identities_for_user(&db2, u2, now_ms).unwrap();
    let hum_ctx = st2.fabushi_core().build_tool_context_with_user("s_fb", None, None, Some(&u2.0.to_string()), 1, Some(ToolsetPreset::HermesTelegram), 0, Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let r_fb = HermesToolExecutor::execute("community_create_post", json!({ "title": "Calling @title_hermes_bot in title", "body": "No mention in body" }), &hum_ctx).await;
    assert!(r_fb.success, "Hermes fallback create_post failed: {:?}", r_fb.error);
    let fb_upds = db2.get_bot_api_updates(b2.bot_user_id, None, 20).unwrap();
    assert!(
        fb_upds.iter().any(|u| u.payload_json.contains("community_mention")),
        "Hermes fallback (self_weak == None) must enqueue title @mention update: {fb_upds:?}"
    );
}

#[tokio::test]
async fn test_get_mentions_sql_prefilter_anti_starvation_and_hermes_mod_action_conditional_tantivy() {
    let dir = TempDir::new().expect("tempdir");
    let (state, app) = test_state(&dir, "mentions_tantivy_mod.db");
    let (_alice_uid, alice_tok) = register_human(&app, "alice_mt").await;
    let (_bob_uid, bob_tok) = register_human(&app, "bob_mt").await;
    let alice_bearer = format!("Bearer {alice_tok}");
    let bob_bearer = format!("Bearer {bob_tok}");

    let (beta_bid, beta_tok, beta_actor_id) =
        register_bot(&app, &alice_bearer, "beta_mentions_bot", "manual").await;
    let beta_bot_auth = format!("Bot {beta_tok}");

    // 1. Seed 2 mentions + 1 reply to beta_mentions_bot, then bury them under 15 newer non-mention notifications
    let (st, m1_v) = req_json(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bearer), Some(json!({ "body": "Older mention @beta_mentions_bot #1" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let m1_pid = m1_v["post"]["post_id"].as_str().unwrap().to_string();

    let (st, bc_v) = req_json(&app, Method::POST, &format!("/bot/{beta_tok}/community.createComment"), None, Some(json!({ "post_id": m1_pid, "body": "Bot reply anchor" }))).await;
    assert_eq!(st, StatusCode::OK);
    let bot_cid = bc_v["result"]["comment"]["comment_id"].as_str().unwrap().to_string();

    // Human replies to bot's comment (creates 'reply' notification)
    let (st, _) = req_json(&app, Method::POST, &format!("/api/v1/community/posts/{m1_pid}/comments"), Some(&bob_bearer), Some(json!({ "parent_comment_id": bot_cid, "body": "Human reply to bot comment" }))).await;
    assert_eq!(st, StatusCode::CREATED);

    // Newer mention #2
    let (st, _) = req_json(&app, Method::POST, "/api/v1/community/posts", Some(&bob_bearer), Some(json!({ "body": "Newer mention @beta_mentions_bot #2" }))).await;
    assert_eq!(st, StatusCode::CREATED);

    // Bury under 15 newer non-mention notifications (follow, likes, reposts, awards on bot's posts)
    let (st, _) = req_json(&app, Method::POST, &format!("/api/v1/community/actors/{beta_actor_id}/follow"), Some(&bob_bearer), None).await;
    assert_eq!(st, StatusCode::OK);
    for i in 0..7 {
        let (st, bp) = req_json(&app, Method::POST, &format!("/bot/{beta_tok}/community.createPost"), None, Some(json!({ "body": format!("Bot noise post {i}") }))).await;
        assert_eq!(st, StatusCode::OK);
        let bpid = bp["result"]["post"]["post_id"].as_str().unwrap().to_string();
        let (st, _) = req_json(&app, Method::POST, &format!("/api/v1/community/posts/{bpid}/like"), Some(&bob_bearer), None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = req_json(&app, Method::POST, &format!("/api/v1/community/posts/{bpid}/repost"), Some(&bob_bearer), None).await;
        assert_eq!(st, StatusCode::OK);
    }

    // community.getMentions with limit=5, include_replies=true MUST NOT be starved by the 15 non-mentions!
    let (st, gm1) = req_json(&app, Method::POST, &format!("/bot/{beta_tok}/community.getMentions"), None, Some(json!({ "limit": 5, "include_replies": true }))).await;
    assert_eq!(st, StatusCode::OK, "getMentions failed: {gm1}");
    let notifs1 = gm1["result"]["notifications"].as_array().unwrap();
    assert!(notifs1.len() >= 3, "Expected >=3 mention+reply notifications despite 15 newer non-mentions, got: {gm1}");
    assert!(notifs1.iter().any(|n| n["kind"] == "mention"));
    assert!(notifs1.iter().any(|n| n["kind"] == "reply"));
    for n in notifs1 {
        let k = n["kind"].as_str().unwrap();
        assert!(matches!(k, "mention" | "bot_summon" | "reply" | "bot_reply"), "Unexpected kind {k} in getMentions");
    }

    // include_replies=false returns mentions only (no replies, no likes/reposts)
    let (st, gm2) = req_json(&app, Method::POST, &format!("/bot/{beta_tok}/community.getMentions"), None, Some(json!({ "limit": 5, "include_replies": false }))).await;
    assert_eq!(st, StatusCode::OK);
    let notifs2 = gm2["result"]["notifications"].as_array().unwrap();
    assert_eq!(notifs2.len(), 2, "Expected 2 mentions when include_replies=false: {gm2}");
    assert!(notifs2.iter().all(|n| matches!(n["kind"].as_str().unwrap(), "mention" | "bot_summon")));

    // Mark newest mention read, then verify unread_only=true + limit=1 still returns the older unread mention!
    let newest_mention_id = notifs2[0]["notif_id"].as_str().unwrap().to_string();
    let older_mention_id = notifs2[1]["notif_id"].as_str().unwrap().to_string();
    let (st, _) = req_json(&app, Method::POST, "/api/v1/community/notifications/read", Some(&beta_bot_auth), Some(json!({ "notif_ids": [newest_mention_id] }))).await;
    assert_eq!(st, StatusCode::OK);

    let (st, gm3) = req_json(&app, Method::POST, &format!("/bot/{beta_tok}/community.getMentions"), None, Some(json!({ "limit": 1, "include_replies": false, "unread_only": true }))).await;
    assert_eq!(st, StatusCode::OK);
    let notifs3 = gm3["result"]["notifications"].as_array().unwrap();
    assert_eq!(notifs3.len(), 1, "unread_only=true + limit=1 must not be starved by newer read mention: {gm3}");
    assert_eq!(notifs3[0]["notif_id"].as_str().unwrap(), older_mention_id);
    assert_eq!(notifs3[0]["is_read"], false);

    // 2. Hermes community_mod_action conditional Tantivy indexing (R2 #3)
    let (st, _) = req_json(&app, Method::POST, "/api/v1/community/communities", Some(&alice_bearer), Some(json!({ "slug": "hermes_tantivy_hq", "name": "Hermes Tantivy HQ", "allow_bots": true }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = req_json(&app, Method::POST, "/api/v1/community/communities/hermes_tantivy_hq/mod-actions", Some(&alice_bearer), Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": beta_actor_id }))).await;
    assert_eq!(st, StatusCode::OK);

    let (st, draft_v) = req_json(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bearer), Some(json!({ "community_slug": "hermes_tantivy_hq", "title": "HermesSecretDraftZebra", "body": "Never index draft", "status": "draft" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let draft_pid = draft_v["post"]["post_id"].as_str().unwrap().to_string();

    let (st, rem_v) = req_json(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bearer), Some(json!({ "community_slug": "hermes_tantivy_hq", "title": "HermesRemovedZebra", "body": "Removed post flair test" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let rem_pid = rem_v["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = req_json(&app, Method::POST, "/api/v1/community/communities/hermes_tantivy_hq/mod-actions", Some(&alice_bearer), Some(json!({ "action": "remove", "target_kind": "post", "target_id": rem_pid }))).await;
    assert_eq!(st, StatusCode::OK);

    let mod_bot_ctx = state
        .fabushi_core()
        .build_tool_context_with_user("s_mod_tantivy", None, None, None, 1, Some(ToolsetPreset::HermesTelegram), 0, Arc::new(std::sync::atomic::AtomicBool::new(false)))
        .with_chat_and_bot(None, Some(beta_bid))
        .with_user_id(None)
        .with_approval_mode(ApprovalMode::Prompt);

    // Mod actions on Draft post MUST fail and NEVER index Draft
    let r_draft_flair = HermesToolExecutor::execute("community_mod_action", json!({ "community_slug": "hermes_tantivy_hq", "action": "set_post_flair", "target_kind": "post", "target_id": draft_pid, "flair": "DraftFlair" }), &mod_bot_ctx).await;
    assert!(!r_draft_flair.success, "Mod action on Draft post must fail");
    let r_draft_approve = HermesToolExecutor::execute("community_mod_action", json!({ "community_slug": "hermes_tantivy_hq", "action": "approve", "target_kind": "post", "target_id": draft_pid }), &mod_bot_ctx).await;
    assert!(!r_draft_approve.success, "Approving Draft post must fail");

    // SetPostFlair on Removed Published post succeeds, but post MUST STAY OUT of Tantivy!
    let r_rem_flair = HermesToolExecutor::execute("community_mod_action", json!({ "community_slug": "hermes_tantivy_hq", "action": "set_post_flair", "target_kind": "post", "target_id": rem_pid, "flair": "Reviewed" }), &mod_bot_ctx).await;
    assert!(r_rem_flair.success, "SetPostFlair on removed post failed: {:?}", r_rem_flair.error);

    for needle in ["HermesSecretDraftZebra", "HermesRemovedZebra"] {
        let req_dto = gpui_telegram_proto::community_contracts::CommunitySearchRequestDto {
            query: needle.to_string(),
            ..Default::default()
        };
        let hits = state.community_search_indexer.search_hits(&req_dto).unwrap();
        assert!(hits.is_empty(), "Non-public post '{needle}' MUST have 0 Tantivy hits, got {hits:?}");
    }

    // Approving the previously-Removed Published post flips is_removed=0 -> MUST NOW be indexed in Tantivy!
    let r_rem_approve = HermesToolExecutor::execute("community_mod_action", json!({ "community_slug": "hermes_tantivy_hq", "action": "approve", "target_kind": "post", "target_id": rem_pid }), &mod_bot_ctx).await;
    assert!(r_rem_approve.success, "Approving removed post failed: {:?}", r_rem_approve.error);
    let hits_after = state
        .community_search_indexer
        .search_hits(&gpui_telegram_proto::community_contracts::CommunitySearchRequestDto {
            query: "HermesRemovedZebra".to_string(),
            ..Default::default()
        })
        .unwrap();
    assert!(
        hits_after.iter().any(|h| h.post_id == rem_pid),
        "Approved Published post MUST be indexed in Tantivy: {hits_after:?}"
    );
}

#[tokio::test]
async fn test_scheduled_post_title_mention_wakes_reactive_bot_and_populates_get_mentions() {
    let dir = TempDir::new().expect("tempdir");
    let (state, app) = test_state(&dir, "sched_reactive_wake.db");
    let (_alice_uid, alice_tok) = register_human(&app, "alice_sched_wake").await;
    let alice_bearer = format!("Bearer {alice_tok}");

    let (st, _) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/communities",
        Some(&alice_bearer),
        Some(json!({ "slug": "sched_wake_lab", "name": "Sched Wake Lab", "allow_bots": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (_b_ban_id, _b_ban_tok, banned_actor_id) =
        register_bot(&app, &alice_bearer, "sched_banned_bot", "reactive").await;
    let (_b_ok_id, healthy_tok, healthy_actor_id) =
        register_bot(&app, &alice_bearer, "sched_reactive_bot", "reactive").await;
    let (_b_mo_id, mention_only_tok, mention_only_actor_id) =
        register_bot(&app, &alice_bearer, "sched_mention_only_bot", "mention_only").await;

    // Ban sched_banned_bot in c/sched_wake_lab
    let (st, _) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/communities/sched_wake_lab/mod-actions",
        Some(&alice_bearer),
        Some(json!({
            "action": "ban_actor",
            "target_kind": "actor",
            "target_id": banned_actor_id,
            "reason": "test scheduled wake ban filter"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 1. Schedule a post with title-only @sched_reactive_bot (and @sched_banned_bot ahead of it)
    let future_ms = Utc::now().timestamp_millis() + 120_000;
    let (st, sched_val) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        Some(json!({
            "community_slug": "sched_wake_lab",
            "title": "UniqueSchedWake9944 alerting @sched_banned_bot and @sched_reactive_bot",
            "body": "Body deliberately contains zero mentions for rust gpui review",
            "status": "scheduled",
            "scheduled_at_ms": future_ms
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "scheduled post failed: {sched_val}");
    let sched_pid = sched_val["post"]["post_id"].as_str().unwrap().to_string();

    // Before tick: 0 mentions in community.getMentions and 0 comments
    let (st, pre_gm) = req_json(
        &app,
        Method::POST,
        &format!("/bot/{healthy_tok}/community.getMentions"),
        None,
        Some(json!({ "limit": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        pre_gm["result"]["notifications"].as_array().unwrap().is_empty(),
        "Unpublished scheduled post must not emit mention notifications early: {pre_gm}"
    );

    // Backdate scheduled_at_ms and run background tick
    let past_ms = Utc::now().timestamp_millis() - 5_000;
    state
        .db
        .pool()
        .with_writer(|conn| {
            conn.execute(
                "UPDATE community_posts SET scheduled_at_ms = ?1 WHERE post_id = ?2",
                rusqlite::params![past_ms, &sched_pid],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    run_community_background_tick(&state).await;

    // Poll up to 2s for async reactive bot reply comment
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut reactive_replied = false;
    while tokio::time::Instant::now() < deadline {
        let cnt: i64 = state
            .db
            .pool()
            .with_reader(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM community_comments WHERE post_id = ?1 AND author_actor_id = ?2",
                    rusqlite::params![&sched_pid, &healthy_actor_id],
                    |r| r.get(0),
                )
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
            })
            .unwrap();
        if cnt >= 1 {
            reactive_replied = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    assert!(
        reactive_replied,
        "Scheduled post publish tick MUST wake title-mentioned @sched_reactive_bot to post a real reply comment"
    );

    // Verify banned bot produced 0 comments and burned 0 cooldowns
    let (banned_cmts, banned_cds): (i64, i64) = state
        .db
        .pool()
        .with_reader(|conn| {
            let c: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM community_comments WHERE post_id = ?1 AND author_actor_id = ?2",
                    rusqlite::params![&sched_pid, &banned_actor_id],
                    |r| r.get(0),
                )
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
            let cd: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM community_bot_cooldowns WHERE bot_actor_id = ?1",
                    rusqlite::params![&banned_actor_id],
                    |r| r.get(0),
                )
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
            Ok((c, cd))
        })
        .unwrap();
    assert_eq!(banned_cmts, 0);
    assert_eq!(banned_cds, 0);

    // Verify community.getMentions is populated with kind=='mention' targeting sched_pid
    let (st, post_gm) = req_json(
        &app,
        Method::POST,
        &format!("/bot/{healthy_tok}/community.getMentions"),
        None,
        Some(json!({ "limit": 10, "include_replies": false })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "getMentions failed: {post_gm}");
    let notifs = post_gm["result"]["notifications"].as_array().unwrap();
    assert!(
        notifs.iter().any(|n| {
            n["kind"] == "mention" && n["target_post_id"].as_str() == Some(sched_pid.as_str())
        }),
        "Scheduled post publish must populate community.getMentions: {post_gm}"
    );

    // Verify getUpdates also has community_mention
    let (st, upd) = req_json(
        &app,
        Method::GET,
        &format!("/bot/{healthy_tok}/getUpdates"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        upd["result"].as_array().unwrap().iter().any(|u| {
            u.get("community_mention")
                .and_then(|m| m["post_id"].as_str())
                == Some(sched_pid.as_str())
        }),
        "Expected community_mention in getUpdates: {upd}"
    );

    // 2. Also verify MentionOnly bot wakes up + gets idempotent community.getMentions on Scheduled publish
    let (st, sched_mo) = req_json(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        Some(json!({
            "community_slug": "sched_wake_lab",
            "title": "MentionOnly check for @sched_mention_only_bot",
            "body": "Body with no mentions",
            "status": "scheduled",
            "scheduled_at_ms": future_ms
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let mo_pid = sched_mo["post"]["post_id"].as_str().unwrap().to_string();
    state
        .db
        .pool()
        .with_writer(|conn| {
            conn.execute(
                "UPDATE community_posts SET scheduled_at_ms = ?1 WHERE post_id = ?2",
                rusqlite::params![past_ms, &mo_pid],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    run_community_background_tick(&state).await;

    let deadline2 = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut mo_replied = false;
    while tokio::time::Instant::now() < deadline2 {
        let cnt: i64 = state
            .db
            .pool()
            .with_reader(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM community_comments WHERE post_id = ?1 AND author_actor_id = ?2",
                    rusqlite::params![&mo_pid, &mention_only_actor_id],
                    |r| r.get(0),
                )
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
            })
            .unwrap();
        if cnt >= 1 {
            mo_replied = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    assert!(
        mo_replied,
        "Scheduled post publish tick MUST wake title-mentioned MentionOnly bot"
    );

    let (st, mo_gm) = req_json(
        &app,
        Method::POST,
        &format!("/bot/{mention_only_tok}/community.getMentions"),
        None,
        Some(json!({ "limit": 10, "include_replies": false })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let mo_notifs = mo_gm["result"]["notifications"].as_array().unwrap();
    assert_eq!(
        mo_notifs
            .iter()
            .filter(|n| n["target_post_id"].as_str() == Some(mo_pid.as_str()))
            .count(),
        1,
        "MentionOnly bot must receive exactly 1 mention notification on scheduled publish: {mo_gm}"
    );
}

