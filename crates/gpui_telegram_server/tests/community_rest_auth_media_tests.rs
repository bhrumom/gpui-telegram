use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use futures::StreamExt;
use gpui_telegram_community::bot_runtime::DeterministicBotResponder;
use gpui_telegram_proto::community_contracts::{
    ActorKind, ActorProfileResponse, AlgoPreferencesDto, BotAutonomyMode, CommunityActorDto,
    CommunityDetailResponse, CommunityIdentitiesResponse, CommunitySearchResultDto,
    CreateCommentResponse, CreatePostResponse, FeedResponseDto, ModLogsResponse, ModQueueResponse,
    NotificationsListResponse, PollDto, PostDetailResponse, SummonBotResponse, TrendingResponse,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use gpui_telegram_server::server::{build_router_with_state, AppState};
use gpui_telegram_server::ServerConfig;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tower::ServiceExt;

fn test_state(dir: &TempDir, db_name: &str) -> (Arc<AppState>, axum::Router) {
    let mut cfg = ServerConfig::default();
    cfg.database_url = dir.path().join(db_name).display().to_string();
    cfg.media_storage_path = dir.path().join("media").display().to_string();
    let state = Arc::new(AppState::try_new(cfg).expect("AppState::try_new"));
    let router = build_router_with_state(state.clone());
    (state, router)
}

async fn send_req(
    app: &axum::Router,
    method: Method,
    uri: &str,
    auth_header: Option<&str>,
    act_as: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(auth) = auth_header {
        builder = builder.header("Authorization", auth);
    }
    if let Some(actor) = act_as {
        builder = builder.header("X-Community-Actor-Id", actor);
    }
    let req = if let Some(val) = body {
        builder
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&val).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

async fn register_user(app: &axum::Router, username: &str) -> (i64, String) {
    let (st, val) = send_req(
        app,
        Method::POST,
        "/api/v1/auth/register",
        None,
        None,
        Some(json!({
            "username": username,
            "password": "Password123!",
            "first_name": username,
            "device_name": "test"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register failed: {val}");
    let uid = val["user_id"].as_i64().unwrap();
    let tok = val["token"].as_str().unwrap().to_string();
    (uid, tok)
}

#[tokio::test]
async fn test_v42_migration_idempotency_startup_seed_and_tantivy_index_path() {
    let dir = TempDir::new().expect("tempdir");
    let (state, app) = test_state(&dir, "comm_v42.db");

    // 1. V42 recorded & idempotent re-run
    let v42_exists: bool = state
        .db
        .pool()
        .with_reader(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM _schema_migrations WHERE version = 42)",
                [],
                |r| r.get(0),
            )
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert!(v42_exists, "Migration V42 must be recorded");
    state
        .db
        .pool()
        .with_writer(|conn| gpui_telegram_server::db::migrations::run_migrations(conn))
        .expect("Re-running run_migrations must be idempotent");

    // 2. Community Tantivy directory uses `<db_path>.community.idx` (zero forbidden substrings)
    let idx_dir = dir.path().join("comm_v42.db.community.idx");
    assert!(idx_dir.is_dir(), "Expected persistent community index at {}", idx_dir.display());
    assert!(
        state.community_search_indexer.doc_count() >= 20,
        "Seeded published posts must be indexed immediately on startup, got {}",
        state.community_search_indexer.doc_count()
    );

    // 3. Server status advertises "community" capability
    let (st, status_val) = send_req(&app, Method::GET, "/api/v1/status", None, None, None).await;
    assert_eq!(st, StatusCode::OK);
    let caps = status_val["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c.as_str() == Some("community")));

    // 4. Seeded feed & search work immediately for a freshly registered user
    let (_uid, tok) = register_user(&app, "alice_v42").await;
    let bearer = format!("Bearer {tok}");
    let (st, feed_val) = send_req(
        &app,
        Method::GET,
        "/api/v1/community/feed?tab=for_you&limit=20",
        Some(&bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "feed error: {feed_val}");
    let feed: FeedResponseDto = serde_json::from_value(feed_val).unwrap();
    assert!(!feed.items.is_empty(), "Starter seeded feed must be non-empty");

    let (st, search_val) = send_req(
        &app,
        Method::GET,
        "/api/v1/community/search?q=rust&tab=top",
        Some(&bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "search error: {search_val}");
    let search: CommunitySearchResultDto = serde_json::from_value(search_val).unwrap();
    assert!(search.total_hits > 0, "Seeded Rust content should be searchable immediately");
}

#[tokio::test]
async fn test_unified_actor_auth_three_modes_and_atomic_bot_registration() {
    let dir = TempDir::new().expect("tempdir");
    let (state, app) = test_state(&dir, "comm_auth.db");
    let (alice_uid, alice_tok) = register_user(&app, "alice_auth").await;
    let (_bob_uid, bob_tok) = register_user(&app, "bob_auth").await;
    let alice_bearer = format!("Bearer {alice_tok}");
    let bob_bearer = format!("Bearer {bob_tok}");

    // Unauthenticated rejected with 401
    let (st, _) = send_req(&app, Method::GET, "/api/v1/community/identities", None, None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = send_req(&app, Method::GET, "/api/v1/community/identities", Some("Bot invalid:token"), None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Atomic POST /identities/bot with handle that lacks `_bot` suffix (tests compliant derivation!)
    let (st, bot_resp) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/bot",
        Some(&alice_bearer),
        None,
        Some(json!({
            "handle": "rust_mentor",
            "display_name": "Rust Mentor AI",
            "bio": "Helps with Rust lifetimes and GPUI",
            "bot_autonomy_mode": "reactive",
            "bot_capabilities": ["rust", "gpui", "review"],
            "bot_persona": "Concise Rust systems architect"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "bot create failed: {bot_resp}");
    let bot_actor: CommunityActorDto = serde_json::from_value(bot_resp["actor"].clone()).unwrap();
    let bot_token = bot_resp["bot_token"].as_str().unwrap().to_string();
    let bot_user_id = bot_resp["bot_user_id"].as_i64().unwrap();
    assert_eq!(bot_actor.actor_id, format!("b:{bot_user_id}"));
    assert_eq!(bot_actor.handle, "rust_mentor");
    assert_eq!(bot_actor.actor_kind, ActorKind::Bot);

    // Duplicate handle -> 409 Conflict with ZERO orphan rows created in bots_registry
    let before_bots = state.db.list_bots_by_owner(alice_uid).unwrap().len();
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/bot",
        Some(&alice_bearer),
        None,
        Some(json!({
            "handle": "rust_mentor",
            "display_name": "Duplicate Bot"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(state.db.list_bots_by_owner(alice_uid).unwrap().len(), before_bots);

    // Mode 1: Human Bearer posts as u:{alice_uid}
    let (st, p1_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({ "body": "Mode 1 human post #rust" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "mode 1 failed: {p1_val}");
    let p1: CreatePostResponse = serde_json::from_value(p1_val).unwrap();
    assert_eq!(p1.post.author_actor_id, format!("u:{alice_uid}"));
    assert_eq!(p1.post.author_kind, ActorKind::Human);

    // Mode 2: Owner Act-As-Bot posts as b:{bot_user_id}
    let (st, p2_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        Some(&bot_actor.actor_id),
        Some(json!({ "body": "Mode 2 owner act-as-bot post" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "mode 2 failed: {p2_val}");
    let p2: CreatePostResponse = serde_json::from_value(p2_val).unwrap();
    assert_eq!(p2.post.author_actor_id, bot_actor.actor_id);
    assert_eq!(p2.post.author_kind, ActorKind::Bot);

    // Security: Bob attempting Act-As Alice's bot MUST fail with 403 Forbidden
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        Some(&bot_actor.actor_id),
        Some(json!({ "body": "Spoofed bot post" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Forged unknown X-Community-Actor-Id MUST fail with 403
    let (st, _) = send_req(
        &app,
        Method::GET,
        "/api/v1/community/identities",
        Some(&bob_bearer),
        Some("b:9999999"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Mode 3: Direct Bot Auth (`Authorization: Bot <bot_token>`)
    let bot_auth = format!("Bot {bot_token}");
    let (st, p3_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bot_auth),
        None,
        Some(json!({ "body": "Mode 3 direct bot auth post" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "mode 3 failed: {p3_val}");
    let p3: CreatePostResponse = serde_json::from_value(p3_val).unwrap();
    assert_eq!(p3.post.author_actor_id, bot_actor.actor_id);

    // Mode 3 cross-actor spoofing blocked with 403
    let (st, _) = send_req(
        &app,
        Method::GET,
        "/api/v1/community/identities",
        Some(&bot_auth),
        Some(&format!("u:{alice_uid}")),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // GET /identities lists both Alice's human actor and owned bot
    let (st, ids_val) = send_req(&app, Method::GET, "/api/v1/community/identities", Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let ids: CommunityIdentitiesResponse = serde_json::from_value(ids_val).unwrap();
    assert_eq!(ids.active_actor_id, format!("u:{alice_uid}"));
    assert!(ids.bot_actors.iter().any(|b| b.actor_id == bot_actor.actor_id));
}

async fn upload_media(
    app: &axum::Router,
    bearer: &str,
    filename: &str,
    bytes: &[u8],
    chat_id: Option<i64>,
) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let payload = json!({
        "filename": filename,
        "mime_type": "image/png",
        "data_base64": b64,
    });
    let uri = match chat_id {
        Some(cid) => format!("/api/v1/media/upload?chat_id={cid}"),
        None => "/api/v1/media/upload".to_string(),
    };
    let (st, val) = send_req(
        app,
        Method::POST,
        &uri,
        Some(bearer),
        None,
        Some(payload),
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
async fn test_public_community_media_acl_and_media_hijack_defense() {
    let dir = TempDir::new().expect("tempdir");
    let (_state, app) = test_state(&dir, "comm_media.db");
    let (_alice_uid, alice_tok) = register_user(&app, "alice_med").await;
    let (_bob_uid, bob_tok) = register_user(&app, "bob_med").await;
    let alice_bearer = format!("Bearer {alice_tok}");
    let bob_bearer = format!("Bearer {bob_tok}");

    // Alice creates a private group chat without Bob
    let (st, chat_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_bearer),
        None,
        Some(json!({ "title": "Alice Private", "chat_type": "group" })),
    )
    .await;
    assert!(st.is_success(), "create chat failed: {chat_val}");
    let private_chat_id = chat_val["chat_id"]
        .as_i64()
        .or_else(|| chat_val["id"].as_i64())
        .unwrap();

    let png_a = b"\x89PNG\r\n\x1a\nPUBLIC_COMMUNITY_IMG_A";
    let png_b = b"\x89PNG\r\n\x1a\nUNATTACHED_IMG_B";
    let png_c = b"\x89PNG\r\n\x1a\nPRIVATE_CHAT_IMG_C";

    let mid_a = upload_media(&app, &alice_bearer, "a.png", png_a, None).await;
    let mid_b = upload_media(&app, &alice_bearer, "b.png", png_b, None).await;
    let mid_c = upload_media(&app, &alice_bearer, "c.png", png_c, Some(private_chat_id)).await;

    // Before attaching A, Bob gets 403 on A, B, and C
    for mid in [&mid_a, &mid_b, &mid_c] {
        let (st, _) = send_req(&app, Method::GET, &format!("/api/v1/media/{mid}"), Some(&bob_bearer), None, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "Unattached/private media {mid} must be 403 for Bob");
    }

    // Media-hijack defense: Bob CANNOT attach Alice's unattached media B to Bob's post
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        None,
        Some(json!({
            "format": "media",
            "body": "Trying to hijack Alice unattached media",
            "media": [{ "media_id": mid_b, "mime_type": "image/png" }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "Media hijack of unattached upload must be 403");

    // Private chat media C CANNOT be attached to public community post even by Alice
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({
            "format": "media",
            "body": "Trying to leak private chat media",
            "media": [{ "media_id": mid_c, "mime_type": "image/png" }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "Private chat media must never be attachable to public posts");

    // Alice attaches A to a published community post -> Bob now gets 200 byte-exact for A!
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({
            "format": "media",
            "body": "Public community screenshot",
            "media": [{ "media_id": mid_a, "mime_type": "image/png" }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/media/{mid_a}"))
        .header("Authorization", &bob_bearer)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "Published community media A must be 200 for Bob");
    let got_bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(got_bytes.as_ref(), png_a);

    // Unattached B and private chat C STILL return 403 for Bob
    for mid in [&mid_b, &mid_c] {
        let (st, _) = send_req(&app, Method::GET, &format!("/api/v1/media/{mid}"), Some(&bob_bearer), None, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "Media {mid} must remain 403 for Bob");
    }
}

#[tokio::test]
async fn test_full_community_rest_crud_moderation_visibility_search_and_summon_bot() {
    let dir = TempDir::new().expect("tempdir");
    let (_state, app) = test_state(&dir, "comm_crud.db");
    let (alice_uid, alice_tok) = register_user(&app, "alice_crud").await;
    let (bob_uid, bob_tok) = register_user(&app, "bob_crud").await;
    let alice_bearer = format!("Bearer {alice_tok}");
    let bob_bearer = format!("Bearer {bob_tok}");

    // Sync Alice & Bob via /identities
    let (st, _) = send_req(&app, Method::GET, "/api/v1/community/identities", Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::GET, "/api/v1/community/identities", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);

    // PATCH /actors/{id}: owner OK, outsider 403
    let (st, patched) = send_req(
        &app,
        Method::PATCH,
        &format!("/api/v1/community/actors/u:{alice_uid}"),
        Some(&alice_bearer),
        None,
        Some(json!({ "bio": "Updated Rustacean bio" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "patch self failed: {patched}");
    let (st, _) = send_req(
        &app,
        Method::PATCH,
        &format!("/api/v1/community/actors/u:{alice_uid}"),
        Some(&bob_bearer),
        None,
        Some(json!({ "bio": "Hacked bio" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Create community + duplicate slug 409 + join + wiki + mod-queue 403 for non-mod
    let (st, comm_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/communities",
        Some(&alice_bearer),
        None,
        Some(json!({ "slug": "gpui_lab", "name": "GPUI Lab", "description": "Testing lab", "allow_bots": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create community failed: {comm_val}");
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/communities",
        Some(&bob_bearer),
        None,
        Some(json!({ "slug": "gpui_lab", "name": "Dup", "description": "dup" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/communities/gpui_lab/join", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::GET, "/api/v1/community/communities/gpui_lab/mod-queue", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "Non-mod must get 403 on mod-queue");
    let (st, mq_val) = send_req(&app, Method::GET, "/api/v1/community/communities/gpui_lab/mod-queue", Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let _mq: ModQueueResponse = serde_json::from_value(mq_val).unwrap();

    let (st, _) = send_req(
        &app,
        Method::PUT,
        "/api/v1/community/communities/gpui_lab/wiki",
        Some(&alice_bearer),
        None,
        Some(json!({ "markdown": "# Welcome to GPUI Lab", "summary": "init wiki" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, cd_val) = send_req(&app, Method::GET, "/api/v1/community/communities/gpui_lab", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let cd: CommunityDetailResponse = serde_json::from_value(cd_val).unwrap();
    assert_eq!(cd.moderators.len(), 1);
    assert!(cd.wiki_revisions.len() >= 1);
    assert_eq!(cd.wiki_revisions[0].revision, 2);

    // Draft post hidden from Bob (404) but visible to Alice (200)
    let (st, draft_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({ "community_slug": "gpui_lab", "body": "Secret draft text", "status": "draft" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let draft_id = draft_val["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(&app, Method::GET, &format!("/api/v1/community/posts/{draft_id}"), Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "Draft post must be hidden from non-authors");
    let (st, _) = send_req(&app, Method::GET, &format!("/api/v1/community/posts/{draft_id}"), Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);

    // Create Poll post, vote as Human Bob + Bot -> verify human_votes & bot_votes breakdown
    let (st, bot_create) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/bot",
        Some(&alice_bearer),
        None,
        Some(json!({
            "handle": "lab_helper_bot",
            "display_name": "Lab Helper",
            "bot_autonomy_mode": "reactive",
            "bot_capabilities": ["gpui", "rust"]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bot_actor_id = bot_create["actor"]["actor_id"].as_str().unwrap().to_string();
    let bot_token = bot_create["bot_token"].as_str().unwrap().to_string();

    let (st, poll_post_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({
            "community_slug": "gpui_lab",
            "format": "poll",
            "title": "UniqueZirconiumPoll",
            "body": "Which engine wins? #UniqueZirconiumTag",
            "poll": { "question": "Best UI?", "options": ["GPUI", "Web"], "multi_select": false, "duration_secs": 3600 }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "poll post failed: {poll_post_val}");
    let poll_post: CreatePostResponse = serde_json::from_value(poll_post_val).unwrap();
    let poll_post_id = poll_post.post.post_id.clone();
    let opt0 = poll_post.post.poll.as_ref().unwrap().options[0].option_id.clone();

    let (st, pv1) = send_req(
        &app,
        Method::POST,
        &format!("/api/v1/community/posts/{poll_post_id}/poll-vote"),
        Some(&bob_bearer),
        None,
        Some(json!({ "option_ids": [opt0] })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "human poll vote failed: {pv1}");
    let (st, pv2) = send_req(
        &app,
        Method::POST,
        &format!("/api/v1/community/posts/{poll_post_id}/poll-vote"),
        Some(&format!("Bot {bot_token}")),
        None,
        Some(json!({ "option_ids": [opt0] })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "bot poll vote failed: {pv2}");
    let poll_dto: PollDto = serde_json::from_value(pv2).unwrap();
    assert_eq!(poll_dto.options[0].all_votes, 2);
    assert_eq!(poll_dto.options[0].human_votes, 1);
    assert_eq!(poll_dto.options[0].bot_votes, 1);

    // Summon Bot (nonexistent 404, Manual bot 403, Reactive bot 200 with created_comment)
    let (st, _) = send_req(
        &app,
        Method::POST,
        &format!("/api/v1/community/posts/{poll_post_id}/summon-bot"),
        Some(&alice_bearer),
        None,
        Some(json!({ "bot_handle": "no_such_bot_xyz" })),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, summon_val) = send_req(
        &app,
        Method::POST,
        &format!("/api/v1/community/posts/{poll_post_id}/summon-bot"),
        Some(&alice_bearer),
        None,
        Some(json!({ "bot_actor_id": bot_actor_id, "prompt": "Compare GPUI frame pacing" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "summon bot failed: {summon_val}");
    let summon: SummonBotResponse = serde_json::from_value(summon_val).unwrap();
    assert!(summon.triggered);
    assert!(summon.created_comment.is_some());

    // Vote, like, repost (empty body), bookmark (empty body), award, comments CRUD
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{poll_post_id}/vote"), Some(&bob_bearer), None, Some(json!({ "direction": 1 }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{poll_post_id}/like"), Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{poll_post_id}/repost"), Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{poll_post_id}/bookmark"), Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{poll_post_id}/award"), Some(&bob_bearer), None, Some(json!({ "award_kind": "insightful" }))).await;
    assert_eq!(st, StatusCode::OK);

    let (st, c_val) = send_req(
        &app,
        Method::POST,
        &format!("/api/v1/community/posts/{poll_post_id}/comments"),
        Some(&bob_bearer),
        None,
        Some(json!({ "body": "GPUI renders at 120fps!" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let c_resp: CreateCommentResponse = serde_json::from_value(c_val).unwrap();
    let cid = c_resp.comment.comment_id.clone();
    let (st, _) = send_req(&app, Method::PATCH, &format!("/api/v1/community/comments/{cid}"), Some(&bob_bearer), None, Some(json!({ "body": "GPUI renders at 120fps cleanly!" }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/comments/{cid}/vote"), Some(&alice_bearer), None, Some(json!({ "direction": 1 }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/comments/{cid}/like"), Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/comments/{cid}/award"), Some(&alice_bearer), None, Some(json!({ "award_kind": "gold" }))).await;
    assert_eq!(st, StatusCode::OK);

    // Search finds UniqueZirconiumPoll; after mod Remove it disappears from Tantivy immediately!
    let (st, s_before) = send_req(&app, Method::GET, "/api/v1/community/search?query=UniqueZirconiumPoll&tab=top", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let sr_before: CommunitySearchResultDto = serde_json::from_value(s_before).unwrap();
    assert!(sr_before.posts.iter().any(|p| p.post_id == poll_post_id));

    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/communities/gpui_lab/mod-actions",
        Some(&alice_bearer),
        None,
        Some(json!({ "action": "remove", "target_kind": "post", "target_id": poll_post_id, "reason": "cleanup" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, s_after) = send_req(&app, Method::GET, "/api/v1/community/search?query=UniqueZirconiumPoll&tab=top", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let sr_after: CommunitySearchResultDto = serde_json::from_value(s_after).unwrap();
    assert!(!sr_after.posts.iter().any(|p| p.post_id == poll_post_id), "Removed post must vanish from Tantivy");

    let (st, logs_val) = send_req(&app, Method::GET, "/api/v1/community/communities/gpui_lab/mod-logs", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let logs: ModLogsResponse = serde_json::from_value(logs_val).unwrap();
    assert!(!logs.logs.is_empty());

    // Algo prefs, signals, lists, trending, notifications + empty-body mark-read, actor profile, follow, mutes-blocks
    let (st, ap_val) = send_req(&app, Method::GET, "/api/v1/community/algo-prefs", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let mut ap: AlgoPreferencesDto = serde_json::from_value(ap_val).unwrap();
    ap.bot_content_ratio = 0.6;
    let (st, _) = send_req(&app, Method::PUT, "/api/v1/community/algo-prefs", Some(&bob_bearer), None, Some(serde_json::to_value(&ap).unwrap())).await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/signals", Some(&bob_bearer), None, Some(json!({ "post_id": poll_post_id, "kind": "dwell", "dwell_ms": 4200 }))).await;
    assert_eq!(st, StatusCode::OK);

    let (st, list_val) = send_req(&app, Method::POST, "/api/v1/community/lists", Some(&bob_bearer), None, Some(json!({ "name": "My Devs", "kind": "mixed", "community_slugs": ["gpui_lab"] }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let lid = list_val["list_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(&app, Method::PATCH, &format!("/api/v1/community/lists/{lid}"), Some(&bob_bearer), None, Some(json!({ "name": "My Devs v2" }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::DELETE, &format!("/api/v1/community/lists/{lid}"), Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);

    let (st, tr_val) = send_req(&app, Method::GET, "/api/v1/community/trending?limit=10", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let _tr: TrendingResponse = serde_json::from_value(tr_val).unwrap();

    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/actors/u:{alice_uid}/follow"), Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, prof_val) = send_req(&app, Method::GET, &format!("/api/v1/community/actors/u:{alice_uid}"), Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let prof: ActorProfileResponse = serde_json::from_value(prof_val).unwrap();
    assert!(prof.actor.is_following);

    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/mutes-blocks", Some(&bob_bearer), None, Some(json!({ "target_kind": "keyword", "target_value": "spamword", "action": "mute" }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::DELETE, "/api/v1/community/mutes-blocks?target_kind=keyword&target_value=spamword&action=mute", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);

    let (st, notifs_val) = send_req(&app, Method::GET, "/api/v1/community/notifications", Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let _notifs: NotificationsListResponse = serde_json::from_value(notifs_val).unwrap();
    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/notifications/read", Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);

    // Ensure Manual mode bot cannot be summoned (403)
    let (st, _) = send_req(
        &app,
        Method::PATCH,
        &format!("/api/v1/community/actors/{bot_actor_id}"),
        Some(&alice_bearer),
        None,
        Some(json!({ "bot_autonomy_mode": BotAutonomyMode::Manual })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, open_post) = send_req(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bearer), None, Some(json!({ "community_slug": "gpui_lab", "body": "Summon test open post" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let open_pid = open_post["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{open_pid}/summon-bot"), Some(&alice_bearer), None, Some(json!({ "bot_actor_id": bot_actor_id }))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, det_val) = send_req(&app, Method::GET, &format!("/api/v1/community/posts/{open_pid}"), Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let _det: PostDetailResponse = serde_json::from_value(det_val).unwrap();
    let _ = bob_uid;
}

#[tokio::test]
async fn test_identity_upsert_persists_custom_handle_preserves_counters_and_enforces_409() {
    let dir = TempDir::new().expect("tempdir");
    let (_state, app) = test_state(&dir, "comm_ident_upsert.db");
    let (_alice_uid, alice_tok) = register_user(&app, "alice_orig").await;
    let (_bob_uid, bob_tok) = register_user(&app, "bob_orig").await;
    let alice_bearer = format!("Bearer {alice_tok}");
    let bob_bearer = format!("Bearer {bob_tok}");

    // Trigger ActorAuth pre-sync for Alice and Bob
    let (st, _) = send_req(&app, Method::GET, "/api/v1/community/identities", Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::GET, "/api/v1/community/identities", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);

    // Build non-zero counters for Alice: 1 published post, 1 upvote from Bob, 1 follower from Bob
    let (st, p_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({ "title": "IdentityRenameSearchToken", "body": "Testing handle reindex in Tantivy" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let alice_post_id = p_val["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{alice_post_id}/vote"), Some(&bob_bearer), None, Some(json!({ "direction": 1 }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/actors/alice_orig/follow", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);

    // POST /identities/human updates handle while preserving counters
    let (st, upd_h) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/human",
        Some(&alice_bearer),
        None,
        Some(json!({
            "handle": "alice_custom",
            "display_name": "Alice Custom",
            "bio": "Persisted custom bio"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "human upsert failed: {upd_h}");
    let alice_dto: CommunityActorDto = serde_json::from_value(upd_h).unwrap();
    assert_eq!(alice_dto.handle, "alice_custom");
    assert_eq!(alice_dto.display_name, "Alice Custom");
    assert_eq!(alice_dto.bio, "Persisted custom bio");
    assert!(alice_dto.posts_count >= 1, "posts_count must be preserved");
    assert!(alice_dto.post_karma > 0, "post_karma must be preserved");
    assert!(alice_dto.followers_count >= 1, "followers_count must be preserved");

    // GET /actors/{new_handle} -> 200, old handle -> 404, Tantivy author search reflects new handle
    let (st, prof_new) = send_req(&app, Method::GET, "/api/v1/community/actors/alice_custom", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK, "renamed human actor lookup failed: {prof_new}");
    let prof_dto: ActorProfileResponse = serde_json::from_value(prof_new).unwrap();
    assert_eq!(prof_dto.actor.handle, "alice_custom");
    assert_eq!(prof_dto.actor.posts_count, alice_dto.posts_count);
    assert_eq!(prof_dto.actor.post_karma, alice_dto.post_karma);

    let (st, _) = send_req(&app, Method::GET, "/api/v1/community/actors/alice_orig", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, s_val) = send_req(&app, Method::GET, "/api/v1/community/search?q=IdentityRenameSearchToken&author=alice_custom", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let sr: CommunitySearchResultDto = serde_json::from_value(s_val).unwrap();
    assert!(sr.posts.iter().any(|p| p.post_id == alice_post_id), "Tantivy search by renamed author handle must succeed");

    // Subsequent ActorAuth sync must NOT revert the renamed handle
    let (st, ids_val) = send_req(&app, Method::GET, "/api/v1/community/identities", Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let ids: CommunityIdentitiesResponse = serde_json::from_value(ids_val).unwrap();
    assert_eq!(ids.human_actor.unwrap().handle, "alice_custom");

    // Case-insensitive handle collision -> 409 Conflict
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/human",
        Some(&bob_bearer),
        None,
        Some(json!({ "handle": "ALICE_CUSTOM", "display_name": "Collision" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Bot upsert with existing bot_user_id: updates handle + actor_kind + preserves counters
    let (st, bot_init) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/bot",
        Some(&alice_bearer),
        None,
        Some(json!({ "handle": "orig_bot_id", "display_name": "Orig Bot" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bot_user_id = bot_init["bot_user_id"].as_i64().unwrap();
    let bot_token = bot_init["bot_token"].as_str().unwrap().to_string();
    let bot_auth = format!("Bot {bot_token}");

    let (st, bp_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bot_auth),
        None,
        Some(json!({ "title": "BotRenameSearchToken", "body": "Bot authored post before rename" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bot_post_id = bp_val["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{bot_post_id}/vote"), Some(&bob_bearer), None, Some(json!({ "direction": 1 }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/actors/orig_bot_id/follow", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);

    // Re-link existing bot_user_id with renamed handle + HybridAgent upgrade
    let (st, bot_upd) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/bot",
        Some(&alice_bearer),
        None,
        Some(json!({
            "bot_user_id": bot_user_id,
            "handle": "renamed_hybrid_bot",
            "display_name": "Renamed Hybrid",
            "actor_kind": "hybrid_agent",
            "bot_autonomy_mode": "autonomous",
            "bot_capabilities": ["rust", "audit"]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "bot upsert failed: {bot_upd}");
    let bot_dto: CommunityActorDto = serde_json::from_value(bot_upd["actor"].clone()).unwrap();
    assert_eq!(bot_dto.handle, "renamed_hybrid_bot");
    assert_eq!(bot_dto.actor_kind, ActorKind::HybridAgent);
    assert_eq!(bot_dto.bot_autonomy_mode, BotAutonomyMode::Autonomous);
    assert!(bot_dto.posts_count >= 1, "bot posts_count must be preserved");
    assert!(bot_dto.post_karma > 0, "bot post_karma must be preserved");
    assert!(bot_dto.followers_count >= 1, "bot followers_count must be preserved");

    let (st, _) = send_req(&app, Method::GET, "/api/v1/community/actors/renamed_hybrid_bot", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, bs_val) = send_req(&app, Method::GET, "/api/v1/community/search?q=BotRenameSearchToken&author=renamed_hybrid_bot", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let bsr: CommunitySearchResultDto = serde_json::from_value(bs_val).unwrap();
    assert!(bsr.posts.iter().any(|p| p.post_id == bot_post_id));

    // Case-insensitive collision with Alice's handle -> 409; Bob linking Alice's bot_user_id -> 403
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/bot",
        Some(&alice_bearer),
        None,
        Some(json!({ "bot_user_id": bot_user_id, "handle": "ALICE_CUSTOM", "display_name": "Dup" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/bot",
        Some(&bob_bearer),
        None,
        Some(json!({ "bot_user_id": bot_user_id, "handle": "hijacked_bot", "display_name": "Hijack" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_non_public_post_isolation_quote_exfil_engagements_and_ws_privacy() {
    let dir = TempDir::new().expect("tempdir");
    let (_state, app) = test_state(&dir, "comm_nonpub_iso.db");
    let (_alice_uid, alice_tok) = register_user(&app, "alice_iso").await;
    let (_bob_uid, bob_tok) = register_user(&app, "bob_iso").await;
    let alice_bearer = format!("Bearer {alice_tok}");
    let bob_bearer = format!("Bearer {bob_tok}");

    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/communities",
        Some(&alice_bearer),
        None,
        Some(json!({ "slug": "priv_lab", "name": "Privacy Lab", "allow_bots": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // 1. Draft poll post (also with secret title/body)
    let (st, draft_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({
            "community_slug": "priv_lab",
            "format": "poll",
            "title": "SECRET_DRAFT_TITLE",
            "body": "SECRET_DRAFT_BODY",
            "status": "draft",
            "poll": { "question": "SECRET_POLL_Q?", "options": ["Alpha", "Beta"], "duration_secs": 3600 }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "draft poll: {draft_val}");
    let draft_id = draft_val["post"]["post_id"].as_str().unwrap().to_string();
    let draft_opt0 = draft_val["post"]["poll"]["options"][0]["option_id"].as_str().unwrap().to_string();

    // 2. Scheduled post
    let future_ms = Utc::now().timestamp_millis() + 600_000;
    let (st, sched_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({
            "community_slug": "priv_lab",
            "title": "SECRET_SCHED_TITLE",
            "body": "SECRET_SCHED_BODY",
            "status": "scheduled",
            "scheduled_at_ms": future_ms
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let sched_id = sched_val["post"]["post_id"].as_str().unwrap().to_string();

    // 3. Deleted post (after Bob bookmarked it to test stale bookmark cleanup)
    let (st, del_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({ "community_slug": "priv_lab", "title": "SECRET_DEL_TITLE", "body": "SECRET_DEL_BODY" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let del_id = del_val["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{del_id}/bookmark"), Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::DELETE, &format!("/api/v1/community/posts/{del_id}"), Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);

    // 4. Mod-removed post (pinned BEFORE removal to verify pinned_posts filters it out)
    let (st, rem_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({ "community_slug": "priv_lab", "title": "SECRET_REM_TITLE", "body": "SECRET_REM_BODY" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let rem_id = rem_val["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/communities/priv_lab/mod-actions", Some(&alice_bearer), None, Some(json!({ "action": "pin", "target_kind": "post", "target_id": rem_id }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/communities/priv_lab/mod-actions", Some(&alice_bearer), None, Some(json!({ "action": "remove", "target_kind": "post", "target_id": rem_id }))).await;
    assert_eq!(st, StatusCode::OK);

    let (st, cd_val) = send_req(&app, Method::GET, "/api/v1/community/communities/priv_lab", Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let cd: CommunityDetailResponse = serde_json::from_value(cd_val).unwrap();
    assert!(!cd.pinned_posts.iter().any(|p| p.post_id == rem_id), "Removed pinned post must be excluded");
    assert!(!cd.community.pinned_post_ids.contains(&rem_id), "pinned_post_ids must exclude removed post");

    // Quote / Repost / Crosspost referencing Draft, Scheduled, Deleted, Removed -> ALL 404, zero SECRET_ leak
    for hidden_id in [&draft_id, &sched_id, &del_id, &rem_id] {
        for payload in [
            json!({ "format": "quote", "quoted_post_id": hidden_id, "body": "quote exfil" }),
            json!({ "format": "repost", "reposted_post_id": hidden_id, "body": "repost exfil" }),
            json!({ "format": "crosspost", "crosspost_of_id": hidden_id, "body": "xpost exfil" }),
        ] {
            let (st, v) = send_req(&app, Method::POST, "/api/v1/community/posts", Some(&bob_bearer), None, Some(payload)).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "Non-public ref {hidden_id} must return 404, got {v}");
            assert!(!v.to_string().contains("SECRET_"), "Response leaked secret content: {v}");
        }

        // Engagement mutations blocked with 404
        for (path, body) in [
            (format!("/api/v1/community/posts/{hidden_id}/vote"), Some(json!({ "direction": 1 }))),
            (format!("/api/v1/community/posts/{hidden_id}/like"), None),
            (format!("/api/v1/community/posts/{hidden_id}/repost"), None),
            (format!("/api/v1/community/posts/{hidden_id}/poll-vote"), Some(json!({ "option_ids": [draft_opt0] }))),
            (format!("/api/v1/community/posts/{hidden_id}/award"), Some(json!({ "award_kind": "gold" }))),
            (format!("/api/v1/community/posts/{hidden_id}/summon-bot"), Some(json!({}))),
            (format!("/api/v1/community/posts/{hidden_id}/comments"), Some(json!({ "body": "sneaky comment" }))),
        ] {
            let (st, v) = send_req(&app, Method::POST, &path, Some(&bob_bearer), None, body).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{path} on hidden post must return 404, got {v}");
            assert!(!v.to_string().contains("SECRET_"));
        }
        // Report signal on hidden post blocked with 404
        let (st, _) = send_req(&app, Method::POST, "/api/v1/community/signals", Some(&bob_bearer), None, Some(json!({ "post_id": hidden_id, "kind": "report" }))).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    // Bookmarking Draft/Scheduled/Removed blocked with 404, while removing existing stale bookmark on Deleted returns 200
    for hidden_id in [&draft_id, &sched_id, &rem_id] {
        let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{hidden_id}/bookmark"), Some(&bob_bearer), None, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    let (st, unbm) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{del_id}/bookmark"), Some(&bob_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK, "Removing already-existing bookmark on deleted post must succeed: {unbm}");
    assert_eq!(unbm["bookmarked"], false);

    // Even author cannot comment on own Draft (404); PATCH deleted post preserves 410 Gone
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{draft_id}/comments"), Some(&alice_bearer), None, Some(json!({ "body": "author draft comment" }))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send_req(&app, Method::PATCH, &format!("/api/v1/community/posts/{del_id}"), Some(&alice_bearer), None, Some(json!({ "body": "revive" }))).await;
    assert_eq!(st, StatusCode::GONE);

    // Community mod (Alice) CANNOT PATCH, DELETE, or mod-action Bob's private Draft (404)
    let (st, bob_draft) = send_req(&app, Method::POST, "/api/v1/community/posts", Some(&bob_bearer), None, Some(json!({ "community_slug": "priv_lab", "body": "Bob private draft", "status": "draft" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let bob_draft_id = bob_draft["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(&app, Method::PATCH, &format!("/api/v1/community/posts/{bob_draft_id}"), Some(&alice_bearer), None, Some(json!({ "body": "mod overwrite" }))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send_req(&app, Method::DELETE, &format!("/api/v1/community/posts/{bob_draft_id}"), Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/communities/priv_lab/mod-actions", Some(&alice_bearer), None, Some(json!({ "action": "pin", "target_kind": "post", "target_id": bob_draft_id }))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Tombstoned deleted comment: vote/like/award -> 404, PATCH -> 410 Gone
    let (st, pub_val) = send_req(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bearer), None, Some(json!({ "community_slug": "priv_lab", "body": "Public post for tombstone test" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let pub_id = pub_val["post"]["post_id"].as_str().unwrap().to_string();
    let (st, c1) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{pub_id}/comments"), Some(&alice_bearer), None, Some(json!({ "body": "Parent comment" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let tomb_cid = c1["comment"]["comment_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{pub_id}/comments"), Some(&bob_bearer), None, Some(json!({ "parent_comment_id": tomb_cid, "body": "Child keeping parent tombstoned" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = send_req(&app, Method::DELETE, &format!("/api/v1/community/comments/{tomb_cid}"), Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);
    for (p, b) in [
        (format!("/api/v1/community/comments/{tomb_cid}/vote"), Some(json!({ "direction": 1 }))),
        (format!("/api/v1/community/comments/{tomb_cid}/like"), None),
        (format!("/api/v1/community/comments/{tomb_cid}/award"), Some(json!({ "award_kind": "gold" }))),
    ] {
        let (st, _) = send_req(&app, Method::POST, &p, Some(&bob_bearer), None, b).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{p} on tombstoned comment must be 404");
    }
    let (st, _) = send_req(&app, Method::PATCH, &format!("/api/v1/community/comments/{tomb_cid}"), Some(&alice_bearer), None, Some(json!({ "body": "edit tombstone" }))).await;
    assert_eq!(st, StatusCode::GONE);

    // Live WS privacy: PATCH and DELETE on Draft MUST NOT broadcast to Bob, while Draft -> Published DOES
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let srv_app = app.clone();
    let srv_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, srv_app)
            .with_graceful_shutdown(async move { let _ = stop_rx.await; })
            .await;
    });
    let (mut ws_bob, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/ws?token={bob_tok}")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (st, _) = send_req(&app, Method::PATCH, &format!("/api/v1/community/posts/{draft_id}"), Some(&alice_bearer), None, Some(json!({ "body": "SECRET_DRAFT_BODY_EDITED" }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, draft2_val) = send_req(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bearer), None, Some(json!({ "body": "Ephemeral private draft", "status": "draft" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let draft2_id = draft2_val["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(&app, Method::DELETE, &format!("/api/v1/community/posts/{draft2_id}"), Some(&alice_bearer), None, None).await;
    assert_eq!(st, StatusCode::OK);

    // Publish draft_id -> Bob MUST receive ONLY this published post_updated, never the private draft edits/deletes
    let (st, _) = send_req(&app, Method::PATCH, &format!("/api/v1/community/posts/{draft_id}"), Some(&alice_bearer), None, Some(json!({ "status": "published" }))).await;
    assert_eq!(st, StatusCode::OK);

    let mut saw_published_update = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(250), ws_bob.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(txt)))) => {
                if let Ok(WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, payload_json, .. })) =
                    serde_json::from_str::<WsServerFrame>(&txt)
                {
                    assert_ne!(post_id.as_deref(), Some(draft2_id.as_str()), "Draft deletion leaked over WS!");
                    if post_id.as_deref() == Some(draft_id.as_str()) {
                        assert!(!payload_json.contains("\"draft\""), "Private draft edit leaked over WS: {payload_json}");
                        if event_kind == "post_updated" && payload_json.contains("\"published\"") {
                            saw_published_update = true;
                            break;
                        }
                    }
                }
            }
            _ => break,
        }
    }
    assert!(saw_published_update, "Publishing draft via PATCH must broadcast post_updated");
    drop(ws_bob);
    let _ = stop_tx.send(());
    let _ = srv_handle.await;
}

#[tokio::test]
async fn test_summon_bot_stage2_nested_echo_suppression_and_owner_bot_media_acl() {
    let dir = TempDir::new().expect("tempdir");
    let (state, app) = test_state(&dir, "comm_summon_echo_media.db");
    let (_alice_uid, alice_tok) = register_user(&app, "alice_echo").await;
    let (_bob_uid, bob_tok) = register_user(&app, "bob_echo").await;
    let alice_bearer = format!("Bearer {alice_tok}");
    let bob_bearer = format!("Bearer {bob_tok}");

    // 1. Register Alice's reactive bot + Bob's bot
    let (st, bot_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/bot",
        Some(&alice_bearer),
        None,
        Some(json!({
            "handle": "echo_guard_bot",
            "display_name": "Echo Guard Bot",
            "bot_autonomy_mode": "mention_only",
            "bot_capabilities": ["rust", "systems"]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let echo_bot: CommunityActorDto = serde_json::from_value(bot_val["actor"].clone()).unwrap();
    let alice_bot_auth = format!("Bot {}", bot_val["bot_token"].as_str().unwrap());

    let (st, bob_bot_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/identities/bot",
        Some(&bob_bearer),
        None,
        Some(json!({ "handle": "bob_media_bot", "display_name": "Bob Media Bot" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bob_bot_actor_id = bob_bot_val["actor"]["actor_id"].as_str().unwrap().to_string();
    let bob_bot_auth = format!("Bot {}", bob_bot_val["bot_token"].as_str().unwrap());

    // 2. Owner-bot vs attacker-bot media ACL (no public-pinning bypass, private-chat blocked)
    let (st, chat_v) = send_req(&app, Method::POST, "/api/v1/chats", Some(&alice_bearer), None, Some(json!({ "title": "Alice Secret", "chat_type": "group" }))).await;
    assert!(st.is_success());
    let priv_cid = chat_v["chat_id"].as_i64().or_else(|| chat_v["id"].as_i64()).unwrap();

    let mid_owner_ok = upload_media(&app, &alice_bearer, "ok.png", b"\x89PNG\r\n\x1a\nOWNER_OK", None).await;
    let mid_owner_unattached = upload_media(&app, &alice_bearer, "unatt.png", b"\x89PNG\r\n\x1a\nUNATT", None).await;
    let mid_owner_chat = upload_media(&app, &alice_bearer, "chat.png", b"\x89PNG\r\n\x1a\nCHAT", Some(priv_cid)).await;

    // Alice's bot (Direct Bot AND Act-As) CAN attach Alice's unattached upload, CANNOT attach private chat media
    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bot_auth), None, Some(json!({ "format": "media", "body": "Owner bot attaching owner upload", "media": [{ "media_id": mid_owner_ok, "mime_type": "image/png" }] }))).await;
    assert_eq!(st, StatusCode::CREATED, "Alice's Direct Bot must be allowed to attach Alice's unattached media");
    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bearer), Some(&echo_bot.actor_id), Some(json!({ "format": "media", "body": "Act-As attaching owner upload", "media": [{ "media_id": mid_owner_ok, "mime_type": "image/png" }] }))).await;
    assert_eq!(st, StatusCode::CREATED, "Alice Act-As own bot must be allowed to attach Alice's media");
    let (st, _) = send_req(&app, Method::POST, "/api/v1/community/posts", Some(&alice_bot_auth), None, Some(json!({ "format": "media", "body": "Private chat leak", "media": [{ "media_id": mid_owner_chat, "mime_type": "image/png" }] }))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Bob's bot (Direct Bot AND Act-As) CANNOT attach Alice's unattached OR already-published media (no public-pinning)
    for (auth_h, act_as) in [(bob_bot_auth.as_str(), None), (bob_bearer.as_str(), Some(bob_bot_actor_id.as_str()))] {
        for mid in [&mid_owner_unattached, &mid_owner_ok, &mid_owner_chat] {
            let (st, _) = send_req(&app, Method::POST, "/api/v1/community/posts", Some(auth_h), act_as, Some(json!({ "format": "media", "body": "Hijack attempt", "media": [{ "media_id": mid, "mime_type": "image/png" }] }))).await;
            assert_eq!(st, StatusCode::FORBIDDEN, "Non-owner bot must get 403 attaching Alice media {mid}");
        }
    }

    // 3. Stage-2 Outbound Review & Nested Depth-1 Echo-Dedup on summon_bot_on_post (Auditor V2)
    let (st, echo_post_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({ "title": "FramePacingBench", "body": "Discuss GPUI frame pacing and memory layout" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let echo_post_resp: CreatePostResponse = serde_json::from_value(echo_post_val).unwrap();
    let echo_pid = echo_post_resp.post.post_id.clone();

    // Seed depth-0 comment by Alice, then depth-1 nested reply by Bob containing the exact reply
    // DeterministicBotResponder will generate when flat_comments.len() == 2 with last speaker = bob_echo
    let (st, c0_val) = send_req(&app, Method::POST, &format!("/api/v1/community/posts/{echo_pid}/comments"), Some(&alice_bearer), None, Some(json!({ "body": "Initial root observation" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let c0_resp: CreateCommentResponse = serde_json::from_value(c0_val).unwrap();

    let mut sim_c1 = c0_resp.comment.clone();
    sim_c1.comment_id = "sim_c1".to_string();
    sim_c1.author_actor_id = "sim_bob".to_string();
    sim_c1.author_handle = "bob_echo".to_string();
    let trigger_prompt = "Analyze frame pacing";
    let trigger_text = format!("@{} {trigger_prompt}", echo_bot.handle);
    let expected_draft = DeterministicBotResponder::generate_reply(
        &echo_bot,
        &echo_post_resp.post,
        &[c0_resp.comment.clone(), sim_c1],
        &trigger_text,
    )
    .expect("deterministic draft");

    let (st, _) = send_req(
        &app,
        Method::POST,
        &format!("/api/v1/community/posts/{echo_pid}/comments"),
        Some(&bob_bearer),
        None,
        Some(json!({
            "parent_comment_id": c0_resp.comment.comment_id,
            "body": expected_draft
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // Summon bot: flatten_comment_tree sees depth-1 nested reply, Stage-2 echo-dedup rejects it,
    // and summon_bot_on_post MUST return 200 OK with triggered == false & created_comment == None!
    let (st, summon_val) = send_req(
        &app,
        Method::POST,
        &format!("/api/v1/community/posts/{echo_pid}/summon-bot"),
        Some(&alice_bearer),
        None,
        Some(json!({ "bot_actor_id": echo_bot.actor_id, "prompt": trigger_prompt })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "summon echo response: {summon_val}");
    let summon: SummonBotResponse = serde_json::from_value(summon_val).unwrap();
    assert!(!summon.triggered, "Stage-2 echo-dedup MUST suppress duplicate reply on summon-bot: {summon:?}");
    assert!(summon.created_comment.is_none(), "Suppressed summon-bot MUST NOT persist a comment");

    let cmt_count: i64 = state
        .db
        .pool()
        .with_reader(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM community_comments WHERE post_id = ?1",
                rusqlite::params![&echo_pid],
                |r| r.get(0),
            )
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert_eq!(cmt_count, 2, "Comment count must stay 2 after Stage-2 suppression");
}

#[tokio::test]
async fn test_patch_post_sanitizes_embedded_refs_after_origin_deleted_or_redrafted() {
    let dir = TempDir::new().expect("tempdir");
    let (_state, app) = test_state(&dir, "comm_patch_sanitize.db");
    let (_alice_uid, alice_tok) = register_user(&app, "alice_psan").await;
    let (_bob_uid, bob_tok) = register_user(&app, "bob_psan").await;
    let alice_bearer = format!("Bearer {alice_tok}");
    let bob_bearer = format!("Bearer {bob_tok}");

    // 1. Deleted origin: Alice publishes orig_del, Bob quotes/reposts/crossposts it, Alice deletes orig_del
    let (st, orig_del_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({
            "title": "DELETABLE_ORIGIN_SECRET_888",
            "body": "DELETABLE_ORIGIN_BODY_888"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let orig_del_id = orig_del_val["post"]["post_id"].as_str().unwrap().to_string();

    let (st, q_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        None,
        Some(json!({ "format": "quote", "quoted_post_id": orig_del_id, "body": "Bob quote" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bob_quote_id = q_val["post"]["post_id"].as_str().unwrap().to_string();
    assert!(q_val["post"]["quoted_post"].is_object());

    let (st, r_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        None,
        Some(json!({ "format": "repost", "reposted_post_id": orig_del_id, "body": "Bob repost" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bob_repost_id = r_val["post"]["post_id"].as_str().unwrap().to_string();

    let (st, x_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        None,
        Some(json!({ "format": "crosspost", "crosspost_of_id": orig_del_id, "body": "Bob xpost" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bob_xpost_id = x_val["post"]["post_id"].as_str().unwrap().to_string();

    let (st, _) = send_req(
        &app,
        Method::DELETE,
        &format!("/api/v1/community/posts/{orig_del_id}"),
        Some(&alice_bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // PATCH on quote, repost, and crosspost MUST strip embedded summaries of the deleted origin!
    for (pid, field) in [
        (&bob_quote_id, "quoted_post"),
        (&bob_repost_id, "reposted_post"),
        (&bob_xpost_id, "crosspost_origin"),
    ] {
        let (st, patched) = send_req(
            &app,
            Method::PATCH,
            &format!("/api/v1/community/posts/{pid}"),
            Some(&bob_bearer),
            None,
            Some(json!({ "body": "Edited after origin deleted" })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "PATCH {pid} failed: {patched}");
        assert!(
            patched[field].is_null(),
            "PATCH {pid} leaked deleted {field}: {patched}"
        );
        let serialized = patched.to_string();
        assert!(
            !serialized.contains("DELETABLE_ORIGIN_SECRET_888")
                && !serialized.contains("DELETABLE_ORIGIN_BODY_888"),
            "PATCH {pid} leaked deleted origin content: {patched}"
        );
    }

    // 2. Unpublished back to Draft with brand-new never-published secret text
    let (st, orig_pub_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({ "title": "Init Pub", "body": "Public v1" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let orig_pub_id = orig_pub_val["post"]["post_id"].as_str().unwrap().to_string();

    let (st, q2_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        None,
        Some(json!({ "format": "quote", "quoted_post_id": orig_pub_id, "body": "Bob quote 2" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bob_quote2_id = q2_val["post"]["post_id"].as_str().unwrap().to_string();

    let (st, x2_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        None,
        Some(json!({ "format": "crosspost", "crosspost_of_id": orig_pub_id, "body": "Bob xpost 2" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bob_xpost2_id = x2_val["post"]["post_id"].as_str().unwrap().to_string();

    let (st, _) = send_req(
        &app,
        Method::PATCH,
        &format!("/api/v1/community/posts/{orig_pub_id}"),
        Some(&alice_bearer),
        None,
        Some(json!({
            "status": "draft",
            "title": "NEVER_PUBLISHED_SECRET_777",
            "body": "PRIVATE_DRAFT_SECRET_BODY_777"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    for (pid, field) in [
        (&bob_quote2_id, "quoted_post"),
        (&bob_xpost2_id, "crosspost_origin"),
    ] {
        let (st, patched) = send_req(
            &app,
            Method::PATCH,
            &format!("/api/v1/community/posts/{pid}"),
            Some(&bob_bearer),
            None,
            Some(json!({ "body": "Attempt exfil redrafted secret" })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "PATCH {pid} failed: {patched}");
        assert!(
            patched[field].is_null(),
            "PATCH {pid} leaked re-drafted {field}: {patched}"
        );
        let serialized = patched.to_string();
        assert!(
            !serialized.contains("NEVER_PUBLISHED_SECRET_777")
                && !serialized.contains("PRIVATE_DRAFT_SECRET_BODY_777"),
            "PATCH {pid} leaked re-drafted secret text: {patched}"
        );
    }

    // 3. Draft -> Published transition via PATCH backfills mention notification idempotently
    let (st, dr_val) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&alice_bearer),
        None,
        Some(json!({
            "title": "Draft mentioning @bob_psan in title",
            "body": "Waiting to publish",
            "status": "draft"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let dr_id = dr_val["post"]["post_id"].as_str().unwrap().to_string();

    let (st, _) = send_req(
        &app,
        Method::PATCH,
        &format!("/api/v1/community/posts/{dr_id}"),
        Some(&alice_bearer),
        None,
        Some(json!({ "status": "published" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    // Re-patching published post must NOT duplicate the mention notification
    let (st, _) = send_req(
        &app,
        Method::PATCH,
        &format!("/api/v1/community/posts/{dr_id}"),
        Some(&alice_bearer),
        None,
        Some(json!({ "body": "Minor typo fix" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, notifs_val) = send_req(
        &app,
        Method::GET,
        "/api/v1/community/notifications?kind=mention",
        Some(&bob_bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let notifs: NotificationsListResponse = serde_json::from_value(notifs_val).unwrap();
    let dr_mentions: Vec<_> = notifs
        .notifications
        .iter()
        .filter(|n| n.target_post_id.as_deref() == Some(dr_id.as_str()))
        .collect();
    assert_eq!(
        dr_mentions.len(),
        1,
        "Draft->Published transition must insert exactly 1 mention notification: {dr_mentions:?}"
    );
}

#[tokio::test]
async fn test_mod_actions_whitespace_target_kind_enforces_guard_tantivy_sync_and_ws() {
    let dir = TempDir::new().expect("tempdir");
    let (_state, app) = test_state(&dir, "comm_mod_ws_trim.db");
    let (_alice_uid, alice_tok) = register_user(&app, "alice_modtrim").await;
    let (_bob_uid, bob_tok) = register_user(&app, "bob_modtrim").await;
    let alice_bearer = format!("Bearer {alice_tok}");
    let bob_bearer = format!("Bearer {bob_tok}");

    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/communities",
        Some(&alice_bearer),
        None,
        Some(json!({ "slug": "mod_trim_lab", "name": "Mod Trim Lab", "allow_bots": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/communities/mod_trim_lab/join",
        Some(&bob_bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Bob creates Draft, Scheduled, and Deleted posts in Alice-moderated c/mod_trim_lab
    let (st, dr_v) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        None,
        Some(json!({ "community_slug": "mod_trim_lab", "title": "SecretDraftTrim", "body": "Draft", "status": "draft" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let draft_id = dr_v["post"]["post_id"].as_str().unwrap().to_string();

    let future_ms = Utc::now().timestamp_millis() + 600_000;
    let (st, sc_v) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        None,
        Some(json!({ "community_slug": "mod_trim_lab", "title": "SecretSchedTrim", "body": "Sched", "status": "scheduled", "scheduled_at_ms": future_ms })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let sched_id = sc_v["post"]["post_id"].as_str().unwrap().to_string();

    let (st, del_v) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        None,
        Some(json!({ "community_slug": "mod_trim_lab", "title": "SecretDelTrim", "body": "Del" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let del_id = del_v["post"]["post_id"].as_str().unwrap().to_string();
    let (st, _) = send_req(
        &app,
        Method::DELETE,
        &format!("/api/v1/community/posts/{del_id}"),
        Some(&bob_bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Whitespace-padded target_kind MUST be rejected with 404 on Draft, Scheduled, and Deleted posts!
    for (action, tk, tid, extra_flair) in [
        ("pin", " post ", &draft_id, None),
        ("lock", " POST ", &sched_id, None),
        ("set_post_flair", "\tpost\n", &del_id, Some("Tampered")),
    ] {
        let mut payload = json!({
            "action": action,
            "target_kind": tk,
            "target_id": tid,
        });
        if let Some(f) = extra_flair {
            payload["flair"] = json!(f);
        }
        let (st, resp) = send_req(
            &app,
            Method::POST,
            "/api/v1/community/communities/mod_trim_lab/mod-actions",
            Some(&alice_bearer),
            None,
            Some(payload),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::NOT_FOUND,
            "Whitespace target_kind {tk:?} action={action} on hidden post must return 404, got: {resp}"
        );
    }

    // Confirm Bob's Draft and Scheduled posts were NOT mutated
    let (st, dr_after) = send_req(
        &app,
        Method::GET,
        &format!("/api/v1/community/posts/{draft_id}"),
        Some(&bob_bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(dr_after["post"]["is_pinned"], false);
    let (st, sc_after) = send_req(
        &app,
        Method::GET,
        &format!("/api/v1/community/posts/{sched_id}"),
        Some(&bob_bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sc_after["post"]["is_locked"], false);

    // Published post + live WS check: whitespace-padded " post " Remove MUST purge Tantivy AND emit WS post_id!
    let (st, pub_v) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/posts",
        Some(&bob_bearer),
        None,
        Some(json!({
            "community_slug": "mod_trim_lab",
            "title": "WhitespaceModRemoveToken42",
            "body": "Published post to remove with padded target_kind"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let pub_id = pub_v["post"]["post_id"].as_str().unwrap().to_string();

    let (st, s_pre) = send_req(
        &app,
        Method::GET,
        "/api/v1/community/search?q=WhitespaceModRemoveToken42",
        Some(&bob_bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let sr_pre: CommunitySearchResultDto = serde_json::from_value(s_pre).unwrap();
    assert!(sr_pre.posts.iter().any(|p| p.post_id == pub_id));

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
    let (mut ws_bob, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/ws?token={bob_tok}"))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (st, rem_resp) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/communities/mod_trim_lab/mod-actions",
        Some(&alice_bearer),
        None,
        Some(json!({
            "action": "remove",
            "target_kind": " post ",
            "target_id": pub_id,
            "reason": "padded remove"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "padded remove failed: {rem_resp}");
    assert_eq!(rem_resp["target_kind"], "post");

    // Verify Tantivy immediately removed it
    let (st, s_post) = send_req(
        &app,
        Method::GET,
        "/api/v1/community/search?q=WhitespaceModRemoveToken42",
        Some(&bob_bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let sr_post: CommunitySearchResultDto = serde_json::from_value(s_post).unwrap();
    assert!(
        !sr_post.posts.iter().any(|p| p.post_id == pub_id),
        "Whitespace-padded ' post ' Remove MUST delete post from Tantivy"
    );

    // Verify WS mod_action event carries exact post_id == Some(pub_id)
    let mut saw_mod_ws = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(250), ws_bob.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(txt)))) => {
                if let Ok(WsServerFrame::Update(UpdateEvent::CommunityEvent {
                    event_kind,
                    post_id,
                    ..
                })) = serde_json::from_str::<WsServerFrame>(&txt)
                {
                    if event_kind == "mod_action" && post_id.as_deref() == Some(pub_id.as_str()) {
                        saw_mod_ws = true;
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    assert!(
        saw_mod_ws,
        "Whitespace-padded ' post ' mod_action WS event must preserve post_id"
    );

    // Approve with " POST " MUST re-index into Tantivy immediately
    let (st, _) = send_req(
        &app,
        Method::POST,
        "/api/v1/community/communities/mod_trim_lab/mod-actions",
        Some(&alice_bearer),
        None,
        Some(json!({
            "action": "approve",
            "target_kind": " POST ",
            "target_id": pub_id,
            "reason": "padded approve"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, s_re) = send_req(
        &app,
        Method::GET,
        "/api/v1/community/search?q=WhitespaceModRemoveToken42",
        Some(&bob_bearer),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let sr_re: CommunitySearchResultDto = serde_json::from_value(s_re).unwrap();
    assert!(sr_re.posts.iter().any(|p| p.post_id == pub_id));

    drop(ws_bob);
    let _ = stop_tx.send(());
    let _ = srv_handle.await;
}

