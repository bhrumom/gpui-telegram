use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use gpui_telegram_agent::hermes::toolsets::{HermesToolContext, HermesToolExecutor, ToolsetPreset};
use gpui_telegram_code_scene::{
    compile_and_verify, official_seed_assets, OFFICIAL_CODE_SCENE_BOTS, OFFICIAL_FABUSHI_BOT_ID,
};
use gpui_telegram_core::types::{ChatId, MsgId, UserId};
use gpui_telegram_proto::code_scene_contracts::{
    AssetKind, AssetManifest, CodeSceneAssetDto, CodeSceneCardAction, CodeSceneFeedResponse,
    ListCodeSceneAssetsResponse, PublishCodeSceneResponse, RemixSceneResponse,
    ValidationReportDto,
};
use gpui_telegram_server::db::schema::{ChatMemberRecord, ChatRecord, UserRecord};
use gpui_telegram_server::db::ServerDatabase;
use gpui_telegram_server::{build_router_with_state, config::ServerConfig, server::AppState};
use serde_json::{json, Value};
use tempfile::TempDir;
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

async fn register_user(router: &Router, username: &str) -> (i64, String) {
    let (status, val) = api(
        router,
        Method::POST,
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": username,
            "password": "Password123!",
            "first_name": username
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register failed: {val}");
    let uid = val["user_id"].as_i64().unwrap();
    let token = val["token"].as_str().unwrap().to_string();
    (uid, token)
}

fn sample_valid_cs1() -> &'static str {
    r#"
fn on_init(state) {
    state.pulse = 0.0;
}
fn on_update(dt, input, state) {
    state.pulse = state.pulse + dt * 2.0;
}
fn on_render(canvas, state, time) {
    clear(rgba(12, 14, 28, 1.0));
    circle(180, 320, 48 + sin(state.pulse) * 12, hsla(195, 0.95, 0.62, 0.95));
    text(180, 72, "CODE SCENE TEST", 18, rgba(255, 255, 255, 0.95), "center");
}
"#
}

#[tokio::test]
async fn test_code_scene_v42_migration_fk_safe_idempotent_seeding() {
    let dir = TempDir::new().unwrap();
    let (_cfg, state, _router) = boot_server_in_dir(&dir);

    let (has_v42, fk_on, table_count, bot_user_count, seed_count, ver_count): (
        bool,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = state
        .db
        .pool()
        .with_reader(|conn| {
            let v44: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM _schema_migrations WHERE version = 44)",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let fk: i64 = conn
                .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
                .unwrap();
            let tables: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (
                        'code_scenes', 'code_scene_versions', 'code_scene_interactions',
                        'code_scene_likes', 'code_scene_bookmarks', 'code_scene_comments',
                        'code_scene_follows', 'code_scene_user_profiles', 'code_scene_bot_drafts'
                    )",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let bots: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM users WHERE user_id IN (777000, 777101, 777102, 777103, 777104) AND is_bot = 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let scenes: i64 = conn
                .query_row("SELECT COUNT(*) FROM code_scenes", [], |r| r.get(0))
                .unwrap();
            let vers: i64 = conn
                .query_row("SELECT COUNT(*) FROM code_scene_versions", [], |r| r.get(0))
                .unwrap();
            Ok((v44, fk, tables, bots, scenes, vers))
        })
        .unwrap();

    assert!(has_v42, "MIGRATION_V42_CODE_SCENE must be recorded at version 44");
    assert_eq!(fk_on, 1, "PRAGMA foreign_keys must be ON");
    assert_eq!(table_count, 9, "All 9 Code Scene STRICT tables must exist");
    assert_eq!(bot_user_count, OFFICIAL_CODE_SCENE_BOTS.len() as i64);
    assert_eq!(seed_count, official_seed_assets().len() as i64);
    assert_eq!(ver_count, official_seed_assets().len() as i64);

    // Mutate a seed counter, re-run seeding -> idempotent and preserves mutated count!
    let first_seed = official_seed_assets()[0].scene_id.clone();
    let baseline = state.db.get_code_scene(&first_seed).unwrap().unwrap().likes_count;
    let toggled = state
        .db
        .toggle_code_scene_like(OFFICIAL_FABUSHI_BOT_ID, &first_seed)
        .unwrap()
        .unwrap();
    assert!(toggled.liked);
    assert_eq!(toggled.likes_count, baseline + 1);

    let inserted_second_pass = state.db.seed_official_code_scene_bots_and_assets().unwrap();
    assert_eq!(inserted_second_pass, 0);
    let after = state.db.get_code_scene(&first_seed).unwrap().unwrap().likes_count;
    assert_eq!(after, baseline + 1);
}

#[tokio::test]
async fn test_code_scene_auth_and_session_revocation_gate() {
    let dir = TempDir::new().unwrap();
    let (_cfg, _state, router) = boot_server_in_dir(&dir);

    for (method, path, body) in [
        (Method::POST, "/api/v1/code-scene/feed", Some(json!({}))),
        (Method::GET, "/api/v1/code-scene/assets", None),
        (Method::GET, "/api/v1/code-scene/assets/seed_game_flappy_bird", None),
        (Method::POST, "/api/v1/code-scene/assets/validate", Some(json!({"source_code": sample_valid_cs1()}))),
        (Method::POST, "/api/v1/code-scene/assets", Some(json!({"title": "X", "kind": "animation", "source_code": sample_valid_cs1()}))),
        (Method::POST, "/api/v1/code-scene/assets/seed_game_flappy_bird/like", None),
        (Method::POST, "/api/v1/code-scene/assets/seed_game_flappy_bird/bookmark", None),
        (Method::GET, "/api/v1/code-scene/assets/seed_game_flappy_bird/comments", None),
        (Method::POST, "/api/v1/code-scene/assets/seed_game_flappy_bird/comments", Some(json!({"content": "hi"}))),
        (Method::POST, "/api/v1/code-scene/creators/777101/follow", None),
        (Method::POST, "/api/v1/code-scene/telemetry", Some(json!({"events": []}))),
        (Method::POST, "/api/v1/code-scene/ai/generate", Some(json!({"prompt": "neon"}))),
    ] {
        let (st_none, _) = api(&router, method.clone(), path, None, body.clone()).await;
        assert_eq!(st_none, StatusCode::UNAUTHORIZED, "{path} without token");
        let (st_bad, _) = api(&router, method, path, Some("invalid.jwt.token"), body).await;
        assert_eq!(st_bad, StatusCode::UNAUTHORIZED, "{path} with bogus token");
    }

    // Register, verify valid token works, revoke active session, verify immediate 401!
    let (_uid, token) = register_user(&router, "cs_revoke_user").await;
    let (st_ok, _) = api(&router, Method::POST, "/api/v1/code-scene/feed", Some(&token), Some(json!({}))).await;
    assert_eq!(st_ok, StatusCode::OK);

    let (st_sess, sess_body) = api(&router, Method::GET, "/api/v1/auth/sessions", Some(&token), None).await;
    assert_eq!(st_sess, StatusCode::OK);
    let session_id = sess_body["sessions"][0]["session_id"].as_str().unwrap().to_string();
    let (st_del, _) = api(
        &router,
        Method::DELETE,
        &format!("/api/v1/auth/sessions/{session_id}"),
        Some(&token),
        None,
    )
    .await;
    assert!(st_del.is_success(), "session revoke failed: {st_del}");

    let (st_after, _) = api(&router, Method::POST, "/api/v1/code-scene/feed", Some(&token), Some(json!({}))).await;
    assert_eq!(st_after, StatusCode::UNAUTHORIZED, "revoked session must get 401 immediately");
}

#[tokio::test]
async fn test_code_scene_feed_filtering_pagination_personalization_and_blocklist() {
    let dir = TempDir::new().unwrap();
    let (_cfg, _state, router) = boot_server_in_dir(&dir);
    let (_uid, token) = register_user(&router, "cs_feed_user").await;

    // Limit clamped <= 50, total_catalog == 18
    let (st, val) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "limit": 999 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let feed: CodeSceneFeedResponse = serde_json::from_value(val).unwrap();
    assert_eq!(feed.total_catalog, 18);
    assert!(feed.items.len() <= 50 && !feed.items.is_empty());

    // Disjoint pagination via next_cursor + invalid cursor fail-closed
    let (st1, v1) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "limit": 6 })),
    )
    .await;
    assert_eq!(st1, StatusCode::OK);
    let p1: CodeSceneFeedResponse = serde_json::from_value(v1).unwrap();
    assert_eq!(p1.items.len(), 6);
    let cursor = p1.next_cursor.expect("expected next_cursor");
    let (st2, v2) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "limit": 6, "cursor": cursor })),
    )
    .await;
    assert_eq!(st2, StatusCode::OK);
    let p2: CodeSceneFeedResponse = serde_json::from_value(v2).unwrap();
    assert_eq!(p2.items.len(), 6);
    for item in &p2.items {
        assert!(!p1.items.iter().any(|a| a.asset.scene_id == item.asset.scene_id));
    }

    let (st_bad_cur, v_bad_cur) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "cursor": "not_a_valid_cursor" })),
    )
    .await;
    assert_eq!(st_bad_cur, StatusCode::OK);
    let bad_feed: CodeSceneFeedResponse = serde_json::from_value(v_bad_cur).unwrap();
    assert!(bad_feed.items.is_empty());

    // Tab filtering: Games only
    let (st_g, v_g) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "tab": "games", "limit": 20 })),
    )
    .await;
    assert_eq!(st_g, StatusCode::OK);
    let g_feed: CodeSceneFeedResponse = serde_json::from_value(v_g).unwrap();
    assert_eq!(g_feed.items.len(), 6);
    assert!(g_feed.items.iter().all(|i| i.asset.kind == AssetKind::Game));

    // Malformed telemetry payloads rejected (non-object / empty target {}), empty events[] allowed
    let (st_arr, _) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!([1, 2, 3])),
    )
    .await;
    assert!(matches!(st_arr, StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY));

    let (st_empty_obj, _) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(st_empty_obj, StatusCode::UNPROCESSABLE_ENTITY);

    let (st_empty_batch, v_eb) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({ "events": [] })),
    )
    .await;
    assert_eq!(st_empty_batch, StatusCode::OK);
    assert_eq!(v_eb["accepted"], 0);

    // Negative blocklists: NotInterested + HideAuthor -> 0% leak in feed!
    let blocked_scene = g_feed.items[0].asset.scene_id.clone();
    let hidden_author = g_feed.items[1].asset.author_user_id;
    let (st_tel, v_tel) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({
            "events": [
                { "scene_id": blocked_scene, "kind": "not_interested" },
                { "author_user_id": hidden_author, "kind": "hide_author" }
            ]
        })),
    )
    .await;
    assert_eq!(st_tel, StatusCode::OK);
    assert_eq!(v_tel["accepted"], 2);

    let (st_after, v_after) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "limit": 50 })),
    )
    .await;
    assert_eq!(st_after, StatusCode::OK);
    let after_feed: CodeSceneFeedResponse = serde_json::from_value(v_after).unwrap();
    assert!(!after_feed.items.iter().any(|i| i.asset.scene_id == blocked_scene));
    assert!(!after_feed.items.iter().any(|i| i.asset.author_user_id == hidden_author));

    // GET /assets supports both q/query and creator_bot_id/bot_id AND hydrates viewer flags!
    let target_sid = after_feed.items[0].asset.scene_id.clone();
    let (st_like, _) = api(
        &router,
        Method::POST,
        &format!("/api/v1/code-scene/assets/{target_sid}/like"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_like, StatusCode::OK);

    let (st_list, v_list) = api(
        &router,
        Method::GET,
        "/api/v1/code-scene/assets?query=flappy&bot_id=777102",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_list, StatusCode::OK);
    let list_resp: ListCodeSceneAssetsResponse = serde_json::from_value(v_list).unwrap();
    assert!(!list_resp.assets.is_empty());

    let (st_get, v_get) = api(
        &router,
        Method::GET,
        &format!("/api/v1/code-scene/assets/{target_sid}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_get, StatusCode::OK);
    let dto: CodeSceneAssetDto = serde_json::from_value(v_get).unwrap();
    assert!(dto.viewer_liked);
}

#[tokio::test]
async fn test_code_scene_validation_publish_ai_remix_social_and_chat_card() {
    let dir = TempDir::new().unwrap();
    let (_cfg, _state, router) = boot_server_in_dir(&dir);
    let (uid, token) = register_user(&router, "cs_creator").await;

    // Create a chat for GenerativeCard readback testing
    let (st_chat, v_chat) = api(
        &router,
        Method::POST,
        "/api/v1/chats",
        Some(&token),
        Some(json!({ "title": "Code Scene Studio", "chat_type": "group", "member_ids": [uid] })),
    )
    .await;
    assert!(st_chat.is_success(), "create chat failed: {v_chat}");
    let chat_id = v_chat["chat_id"].as_i64().unwrap();

    // Validate valid & invalid scripts
    let (st_v_ok, v_ok) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/assets/validate",
        Some(&token),
        Some(json!({
            "source_code": sample_valid_cs1(),
            "manifest": AssetManifest::for_kind(AssetKind::Animation)
        })),
    )
    .await;
    assert_eq!(st_v_ok, StatusCode::OK);
    let rep_ok: ValidationReportDto = serde_json::from_value(v_ok).unwrap();
    assert!(rep_ok.ok);

    // Invalid publish returns 422 with dual-compatible ValidationReportDto envelope
    let (st_pub_bad, v_pub_bad) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/assets",
        Some(&token),
        Some(json!({
            "title": "Broken",
            "kind": "animation",
            "source_code": "const invalid = 1;"
        })),
    )
    .await;
    assert_eq!(st_pub_bad, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v_pub_bad["error"], "validation_failed");
    assert_eq!(v_pub_bad["ok"], false);
    assert_eq!(v_pub_bad["validation"]["ok"], false);

    // Valid publish with chat_id -> 201 + CreatorJustPublished fast-track + GenerativeCard in chat!
    let (st_pub, v_pub) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/assets",
        Some(&token),
        Some(json!({
            "title": "My Neon Video",
            "description": "Custom cs1 video",
            "kind": "video",
            "tags": ["neon", "video"],
            "source_code": sample_valid_cs1(),
            "chat_id": chat_id
        })),
    )
    .await;
    assert_eq!(st_pub, StatusCode::CREATED, "publish failed: {v_pub}");
    let pub_resp: PublishCodeSceneResponse = serde_json::from_value(v_pub).unwrap();
    assert!(pub_resp.posted_chat_message_id.is_some());
    assert_eq!(pub_resp.asset.kind, AssetKind::Video);
    let my_scene_id = pub_resp.asset.scene_id.clone();

    // Verify CreatorJustPublished surfaces at slot 0 in ForYou
    let (st_f, v_f) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "limit": 10 })),
    )
    .await;
    assert_eq!(st_f, StatusCode::OK);
    let feed_after: CodeSceneFeedResponse = serde_json::from_value(v_f).unwrap();
    assert_eq!(feed_after.items[0].asset.scene_id, my_scene_id);

    // Verify GenerativeCard readback in GET /api/v1/chats/{chat_id}/messages
    let (st_msgs, v_msgs) = api(
        &router,
        Method::GET,
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_msgs, StatusCode::OK);
    let msgs = v_msgs["messages"].as_array().unwrap();
    let card_msg = msgs
        .iter()
        .find(|m| m["content"]["type"] == "GenerativeCard")
        .expect("expected GenerativeCard in chat messages");
    let components = card_msg["content"]["data"]["components"].as_array().unwrap();
    let payloads: Vec<&str> = components
        .iter()
        .filter(|c| c["type"] == "Button")
        .filter_map(|c| c["action_payload"].as_str())
        .collect();
    assert_eq!(
        payloads,
        vec![
            CodeSceneCardAction::Open(my_scene_id.clone()).to_payload(),
            CodeSceneCardAction::Remix(my_scene_id.clone()).to_payload(),
            CodeSceneCardAction::Share(my_scene_id.clone()).to_payload(),
        ]
    );

    // AI generate -> iterate -> publish -> remix (draft + auto_publish)
    let (st_gen, v_gen) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/ai/generate",
        Some(&token),
        Some(json!({ "prompt": "赛博朋克霓虹弹球小游戏", "kind": "game" })),
    )
    .await;
    assert_eq!(st_gen, StatusCode::OK);
    let draft_id = v_gen["draft_id"].as_str().unwrap().to_string();
    assert_eq!(v_gen["validation"]["ok"], true);

    let (st_iter, v_iter) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/ai/iterate",
        Some(&token),
        Some(json!({ "draft_id": draft_id, "instruction": "加快速度并改成金紫色霓虹" })),
    )
    .await;
    assert_eq!(st_iter, StatusCode::OK);
    assert_eq!(v_iter["validation"]["ok"], true);

    let (st_ai_pub, v_ai_pub) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/ai/publish",
        Some(&token),
        Some(json!({ "draft_id": draft_id, "chat_id": chat_id })),
    )
    .await;
    assert_eq!(st_ai_pub, StatusCode::CREATED, "ai publish failed: {v_ai_pub}");
    let ai_pub: PublishCodeSceneResponse = serde_json::from_value(v_ai_pub).unwrap();
    let parent_id = ai_pub.asset.scene_id.clone();

    let (st_rmx, v_rmx) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/ai/remix",
        Some(&token),
        Some(json!({
            "scene_id": parent_id,
            "prompt": "变成双球模式",
            "auto_publish": true,
            "chat_id": chat_id
        })),
    )
    .await;
    assert_eq!(st_rmx, StatusCode::CREATED, "ai remix failed: {v_rmx}");
    let rmx: RemixSceneResponse = serde_json::from_value(v_rmx).unwrap();
    let rmx_pub = rmx.published.expect("auto_publish must return published");
    assert_eq!(rmx_pub.asset.remix_parent_id.as_deref(), Some(parent_id.as_str()));

    let (st_parent_get, v_parent) = api(
        &router,
        Method::GET,
        &format!("/api/v1/code-scene/assets/{parent_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_parent_get, StatusCode::OK);
    assert_eq!(v_parent["remixes_count"], 1);

    // SQLi-safe comments + validation bounds
    let (st_c_empty, _) = api(
        &router,
        Method::POST,
        &format!("/api/v1/code-scene/assets/{parent_id}/comments"),
        Some(&token),
        Some(json!({ "content": "   " })),
    )
    .await;
    assert_eq!(st_c_empty, StatusCode::UNPROCESSABLE_ENTITY);

    let sqli = "'; DROP TABLE code_scenes; -- 很酷！";
    let (st_c_ok, v_c_ok) = api(
        &router,
        Method::POST,
        &format!("/api/v1/code-scene/assets/{parent_id}/comments"),
        Some(&token),
        Some(json!({ "content": sqli })),
    )
    .await;
    assert_eq!(st_c_ok, StatusCode::CREATED);
    assert_eq!(v_c_ok["comment"]["content"], sqli);
    assert_eq!(v_c_ok["comments_count"], 1);
}

#[tokio::test]
async fn test_code_scene_full_restart_durability_drop_and_reopen() {
    let dir = TempDir::new().unwrap();
    let (cfg, state, router) = boot_server_in_dir(&dir);
    let (_uid, token) = register_user(&router, "cs_restart_user").await;

    // 1. Publish custom scene
    let (st_pub, v_pub) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/assets",
        Some(&token),
        Some(json!({
            "title": "Persistent Restart Scene",
            "description": "Survives server SIGTERM",
            "kind": "animation",
            "tags": ["durable", "shader"],
            "source_code": sample_valid_cs1()
        })),
    )
    .await;
    assert_eq!(st_pub, StatusCode::CREATED);
    let custom_id = v_pub["asset"]["scene_id"].as_str().unwrap().to_string();

    // 2. Like, bookmark, comment, follow, NotInterested, HideTag, and create AI draft
    let seeds = official_seed_assets();
    let liked_seed = seeds[0].scene_id.clone();
    let disliked_seed = seeds[1].scene_id.clone();
    let creator_to_follow = seeds[0].author_user_id;

    let (st_l, v_l) = api(
        &router,
        Method::POST,
        &format!("/api/v1/code-scene/assets/{liked_seed}/like"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_l, StatusCode::OK);
    let expected_likes = v_l["likes_count"].as_u64().unwrap();

    let (st_bm, _) = api(
        &router,
        Method::POST,
        &format!("/api/v1/code-scene/assets/{liked_seed}/bookmark"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_bm, StatusCode::OK);

    let (st_flw, _) = api(
        &router,
        Method::POST,
        &format!("/api/v1/code-scene/creators/{creator_to_follow}/follow"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_flw, StatusCode::OK);

    let (st_cmt, _) = api(
        &router,
        Method::POST,
        &format!("/api/v1/code-scene/assets/{custom_id}/comments"),
        Some(&token),
        Some(json!({ "content": "Persisted across restart!" })),
    )
    .await;
    assert_eq!(st_cmt, StatusCode::CREATED);

    let (st_tel, _) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({
            "events": [
                { "scene_id": disliked_seed, "kind": "not_interested" },
                { "kind": "hide_tag", "reason": "physics" },
                { "scene_id": liked_seed, "kind": "complete", "dwell_ms": 14000, "completion_ratio": 1.0 }
            ]
        })),
    )
    .await;
    assert_eq!(st_tel, StatusCode::OK);

    let (st_draft, v_draft) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/ai/generate",
        Some(&token),
        Some(json!({ "prompt": "星空穿梭短视频", "kind": "video" })),
    )
    .await;
    assert_eq!(st_draft, StatusCode::OK);
    let draft_id = v_draft["draft_id"].as_str().unwrap().to_string();

    // 3. Shutdown and drop entire AppState, then reopen on the EXACT same SQLite file!
    state.shutdown();
    drop(router);
    drop(state);

    let state2 = Arc::new(AppState::try_new(cfg).expect("reopen AppState"));
    let router2 = build_router_with_state(state2.clone());

    // Verify custom scene, comments, counters, viewer flags, draft, and 0% blocklist leak!
    let (st_custom, v_custom) = api(
        &router2,
        Method::GET,
        &format!("/api/v1/code-scene/assets/{custom_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_custom, StatusCode::OK);
    assert_eq!(v_custom["title"], "Persistent Restart Scene");
    assert_eq!(v_custom["comments_count"], 1);

    let (st_liked_after, v_liked_after) = api(
        &router2,
        Method::GET,
        &format!("/api/v1/code-scene/assets/{liked_seed}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_liked_after, StatusCode::OK);
    assert_eq!(v_liked_after["likes_count"], expected_likes);
    assert_eq!(v_liked_after["viewer_liked"], true);
    assert_eq!(v_liked_after["viewer_bookmarked"], true);
    assert_eq!(v_liked_after["viewer_following_author"], true);

    let (st_cmts, v_cmts) = api(
        &router2,
        Method::GET,
        &format!("/api/v1/code-scene/assets/{custom_id}/comments"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_cmts, StatusCode::OK);
    assert_eq!(v_cmts["total"], 1);

    let (st_feed2, v_feed2) = api(
        &router2,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "limit": 50 })),
    )
    .await;
    assert_eq!(st_feed2, StatusCode::OK);
    let feed2: CodeSceneFeedResponse = serde_json::from_value(v_feed2).unwrap();
    assert!(!feed2.items.iter().any(|i| i.asset.scene_id == disliked_seed));
    assert!(!feed2.items.iter().any(|i| i.asset.tags.iter().any(|t| t.eq_ignore_ascii_case("physics"))));

    // Verify saved AI draft survived restart and can be published!
    let (st_pub_d, _) = api(
        &router2,
        Method::POST,
        "/api/v1/code-scene/ai/publish",
        Some(&token),
        Some(json!({ "draft_id": draft_id })),
    )
    .await;
    assert_eq!(st_pub_d, StatusCode::CREATED);
}

fn seed_chat_with_bot(
    db: &ServerDatabase,
    chat_id: ChatId,
    title: &str,
    chat_type: u8,
    owner: UserId,
) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    db.create_chat(&ChatRecord {
        chat_id,
        chat_type,
        peer_type: chat_type,
        title: title.to_string(),
        username: None,
        about: None,
        creator_id: owner,
        is_forum: false,
        flags: 0,
        photo_file_id: None,
        slowmode_seconds: 0,
        linked_chat_id: None,
        pinned_message_id: None,
        pts: 0,
        last_message_id: MsgId(0),
        last_message_date_ms: now_ms,
        unread_count: 0,
        account_generation: 1,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    })
    .unwrap();
    for (uid, role) in [
        (owner, "creator"),
        (UserId(OFFICIAL_FABUSHI_BOT_ID as u64), "member"),
    ] {
        db.add_chat_member(&ChatMemberRecord {
            chat_id,
            user_id: uid,
            role: role.to_string(),
            permissions_bitmask: 0,
            admin_title: None,
            admin_rights: if role == "creator" { u32::MAX } else { 0 },
            banned_rights: 0,
            until_date_ms: None,
            joined_at_ms: now_ms,
            invited_by: Some(owner),
            updated_at_ms: now_ms,
        })
        .unwrap();
    }
}

#[tokio::test]
async fn test_code_scene_hermes_tools_and_fabushi_bot_dm_group_creation() {
    // 1. Direct HermesToolExecutor test via AppState::with_db (self_weak == None!)
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("cs_tools.db");
    let db = Arc::new(ServerDatabase::open(&db_path).unwrap());
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = dir.path().join("cas").to_string_lossy().into_owned();
    let state = Arc::new(AppState::with_db(cfg, db.clone()));

    let (alice, _, _) = state
        .session_manager
        .register("alice_cs_bot", "Pass123456!", "Alice", None, "CLI")
        .unwrap();
    let now_ms = chrono::Utc::now().timestamp_millis();
    db.create_user(&UserRecord {
        user_id: alice.user_id,
        username: Some("alice_cs_bot".to_string()),
        phone: None,
        first_name: "Alice".to_string(),
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
    let chat_id = ChatId(99101);
    seed_chat_with_bot(&db, chat_id, "DM Bot", 0, alice.user_id);

    let mut ctx = HermesToolContext::new(dir.path());
    ctx.user_id = Some(alice.user_id.0);
    ctx.chat_id = Some(chat_id.0);
    ctx.bot_id = Some(OFFICIAL_FABUSHI_BOT_ID);
    ctx.toolsets.write().unwrap().apply_preset(ToolsetPreset::Full);
    ctx.platform_im_bridge = state.fabushi_core().platform_im_bridge();

    // Execute all 6 tools via HermesToolExecutor!
    let r_draft = HermesToolExecutor::execute(
        "code_scene_create_draft",
        json!({ "prompt": "霓虹弹球小游戏", "kind": "game" }),
        &ctx,
    )
    .await;
    assert!(r_draft.success, "create_draft failed: {:?}", r_draft.error);
    let draft_id = r_draft.structured.as_ref().unwrap()["draft_id"]
        .as_str()
        .unwrap()
        .to_string();

    let r_iter = HermesToolExecutor::execute(
        "code_scene_iterate_draft",
        json!({ "draft_id": draft_id, "instruction": "加快弹球速度" }),
        &ctx,
    )
    .await;
    assert!(r_iter.success, "iterate_draft failed: {:?}", r_iter.error);

    let r_val = HermesToolExecutor::execute(
        "code_scene_validate",
        json!({ "source_code": sample_valid_cs1(), "kind": "animation" }),
        &ctx,
    )
    .await;
    assert!(r_val.success);
    assert_eq!(r_val.structured.as_ref().unwrap()["ok"], true);

    // Cross-chat spoof rejected!
    let r_spoof = HermesToolExecutor::execute(
        "code_scene_publish",
        json!({ "draft_id": draft_id, "chat_id": 999999 }),
        &ctx,
    )
    .await;
    assert!(!r_spoof.success, "cross-chat spoof must be rejected");

    let r_pub = HermesToolExecutor::execute(
        "code_scene_publish",
        json!({ "draft_id": draft_id, "post_card_to_chat": true }),
        &ctx,
    )
    .await;
    assert!(r_pub.success, "publish failed: {:?}", r_pub.error);
    assert_eq!(r_pub.inline_buttons.len(), 3);
    let published_id = r_pub.structured.as_ref().unwrap()["scene_id"]
        .as_str()
        .unwrap()
        .to_string();

    let r_rmx = HermesToolExecutor::execute(
        "code_scene_remix",
        json!({ "scene_id": published_id, "prompt": "加入星空粒子", "auto_publish": true }),
        &ctx,
    )
    .await;
    assert!(r_rmx.success, "remix failed: {:?}", r_rmx.error);

    let r_mine = HermesToolExecutor::execute("code_scene_list_mine", json!({}), &ctx).await;
    assert!(r_mine.success);
    assert!(r_mine.structured.as_ref().unwrap()["count"].as_u64().unwrap() >= 2);

    // 2. End-to-end DM + Group turn via @fabushi_ai_bot (handle_ai_bot_reply)
    let router = build_router_with_state(state.clone());
    let (bob_uid, bob_token) = register_user(&router, "bob_cs_chat").await;
    let dm_chat = ChatId(99202);
    seed_chat_with_bot(&db, dm_chat, "Fabushi DM", 4, UserId(bob_uid as u64));

    gpui_telegram_server::routes::bots_miniapps::handle_ai_bot_reply(
        &state,
        dm_chat,
        UserId(bob_uid as u64),
        UserId(OFFICIAL_FABUSHI_BOT_ID as u64),
        "/scene create game 霓虹弹球小游戏",
    )
    .await;

    let (st_dm_msgs, v_dm_msgs) = api(
        &router,
        Method::GET,
        &format!("/api/v1/chats/{}/messages", dm_chat.0),
        Some(&bob_token),
        None,
    )
    .await;
    assert_eq!(st_dm_msgs, StatusCode::OK);
    let dm_msgs = v_dm_msgs["messages"].as_array().unwrap();
    let dm_card = dm_msgs
        .iter()
        .find(|m| m["content"]["type"] == "GenerativeCard")
        .expect("DM /scene turn must post a GenerativeCard");
    assert_eq!(dm_card["sender_id"].as_i64(), Some(OFFICIAL_FABUSHI_BOT_ID));
    let open_payload = dm_card["content"]["data"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|c| {
            c["action_payload"]
                .as_str()
                .filter(|p| p.starts_with("code_scene:open:"))
                .map(str::to_string)
        })
        .expect("GenerativeCard must include code_scene:open:<id> button");
    let created_scene_id = open_payload.trim_start_matches("code_scene:open:");
    let created_asset = db.get_code_scene(created_scene_id).unwrap().unwrap();
    assert!(compile_and_verify(&created_asset.source_code, &created_asset.manifest).is_ok());

    // Group chat natural language mention turn
    let group_chat = ChatId(99303);
    seed_chat_with_bot(&db, group_chat, "AI Creators Group", 1, UserId(bob_uid as u64));

    gpui_telegram_server::routes::bots_miniapps::handle_ai_bot_reply(
        &state,
        group_chat,
        UserId(bob_uid as u64),
        UserId(OFFICIAL_FABUSHI_BOT_ID as u64),
        "@fabushi_ai_bot 帮我做一个赛博流体动画发到代码场景",
    )
    .await;

    let (st_grp_msgs, v_grp_msgs) = api(
        &router,
        Method::GET,
        &format!("/api/v1/chats/{}/messages", group_chat.0),
        Some(&bob_token),
        None,
    )
    .await;
    assert_eq!(st_grp_msgs, StatusCode::OK);
    assert!(
        v_grp_msgs["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"]["type"] == "GenerativeCard"),
        "Group natural-language creation turn must post GenerativeCard"
    );

    // Uniform cross-chat guard on all 6 code_scene_* Hermes tools
    for (tool_name, args) in [
        ("code_scene_create_draft", json!({ "prompt": "neon", "chat_id": 888888 })),
        ("code_scene_iterate_draft", json!({ "draft_id": draft_id, "instruction": "faster", "chat_id": 888888 })),
        ("code_scene_validate", json!({ "source_code": sample_valid_cs1(), "chat_id": 888888 })),
        ("code_scene_publish", json!({ "draft_id": draft_id, "chat_id": 888888 })),
        ("code_scene_remix", json!({ "scene_id": published_id, "prompt": "faster", "chat_id": 888888 })),
        ("code_scene_list_mine", json!({ "chat_id": 888888 })),
    ] {
        let r = HermesToolExecutor::execute(tool_name, args, &ctx).await;
        assert!(!r.success, "{tool_name} must reject cross-chat chat_id spoofing");
    }

    // Direct bot fallback Remix on missing parent replies explicitly instead of silently dropping
    let before_count = api(
        &router,
        Method::GET,
        &format!("/api/v1/chats/{}/messages", dm_chat.0),
        Some(&bob_token),
        None,
    )
    .await
    .1["messages"]
        .as_array()
        .unwrap()
        .len();
    gpui_telegram_server::routes::bots_miniapps::handle_ai_bot_reply(
        &state,
        dm_chat,
        UserId(bob_uid as u64),
        UserId(OFFICIAL_FABUSHI_BOT_ID as u64),
        "/scene remix nonexistent_scene_404 改成太空风格",
    )
    .await;
    let after_msgs = api(
        &router,
        Method::GET,
        &format!("/api/v1/chats/{}/messages", dm_chat.0),
        Some(&bob_token),
        None,
    )
    .await
    .1["messages"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        after_msgs.len() > before_count,
        "Missing-parent Remix fallback must send an explicit reply"
    );
}

#[tokio::test]
async fn test_code_scene_social_telemetry_rest_restart_parity_and_event_dedup() {
    let dir = TempDir::new().unwrap();
    let (cfg, state, router) = boot_server_in_dir(&dir);
    let (_uid, token) = register_user(&router, "cs_social_parity").await;

    // Publish fresh scene S with 0 likes/bookmarks/comments/shares
    let (st_pub, v_pub) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/assets",
        Some(&token),
        Some(json!({
            "title": "Social Parity Target",
            "kind": "animation",
            "tags": ["parity"],
            "source_code": sample_valid_cs1()
        })),
    )
    .await;
    assert_eq!(st_pub, StatusCode::CREATED);
    let sid = v_pub["asset"]["scene_id"].as_str().unwrap().to_string();

    // 1. Telemetry-only double_tap_like, bookmark, comment MUST update reward/profile signals
    //    WITHOUT creating phantom likes/bookmarks or counter drift!
    let (st_tel, v_tel) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({
            "events": [
                { "event_id": "tel_dtl_1", "scene_id": sid, "kind": "double_tap_like" },
                { "event_id": "tel_bm_1", "scene_id": sid, "kind": "bookmark" },
                { "event_id": "tel_cmt_1", "scene_id": sid, "kind": "comment" }
            ]
        })),
    )
    .await;
    assert_eq!(st_tel, StatusCode::OK);
    assert_eq!(v_tel["accepted"], 3);
    assert!(v_tel["reward"].as_f64().unwrap() > 0.0);

    let (st_get0, v_get0) = api(
        &router,
        Method::GET,
        &format!("/api/v1/code-scene/assets/{sid}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_get0, StatusCode::OK);
    assert_eq!(v_get0["viewer_liked"], false);
    assert_eq!(v_get0["likes_count"], 0);
    assert_eq!(v_get0["viewer_bookmarked"], false);
    assert_eq!(v_get0["bookmarks_count"], 0);
    assert_eq!(v_get0["comments_count"], 0);

    // 2. REST POST /like converges cleanly to liked: true, likes_count: 1
    let (st_like, v_like) = api(
        &router,
        Method::POST,
        &format!("/api/v1/code-scene/assets/{sid}/like"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_like, StatusCode::OK);
    assert_eq!(v_like["liked"], true);
    assert_eq!(v_like["likes_count"], 1);

    // Telemetry-only `unlike` MUST NOT desync relational liked=true / likes_count=1!
    let (st_unlike_tel, _) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({ "event_id": "tel_unlike_1", "scene_id": sid, "kind": "unlike" })),
    )
    .await;
    assert_eq!(st_unlike_tel, StatusCode::OK);

    let (_, v_get_live) = api(
        &router,
        Method::GET,
        &format!("/api/v1/code-scene/assets/{sid}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(v_get_live["viewer_liked"], true);
    assert_eq!(v_get_live["likes_count"], 1);

    // 3. Repeated event_id telemetry MUST NOT double-count shares_count or duplicate DB rows!
    let (st_sh1, v_sh1) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({ "event_id": "fixed_dedup_ev_001", "scene_id": sid, "kind": "share" })),
    )
    .await;
    assert_eq!(st_sh1, StatusCode::OK);
    assert_eq!(v_sh1["accepted"], 1);

    let (st_sh2, v_sh2) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({ "event_id": "fixed_dedup_ev_001", "scene_id": sid, "kind": "share" })),
    )
    .await;
    assert_eq!(st_sh2, StatusCode::OK);
    assert_eq!(v_sh2["accepted"], 0);

    let (_, v_after_sh) = api(
        &router,
        Method::GET,
        &format!("/api/v1/code-scene/assets/{sid}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(v_after_sh["shares_count"], 1);

    // 4. Drop + reopen AppState -> 100% identical pre/post-restart parity!
    state.shutdown();
    drop(router);
    drop(state);

    let state2 = Arc::new(AppState::try_new(cfg).expect("reopen AppState"));
    let router2 = build_router_with_state(state2.clone());

    let (_, v_post_restart) = api(
        &router2,
        Method::GET,
        &format!("/api/v1/code-scene/assets/{sid}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(v_post_restart["viewer_liked"], true);
    assert_eq!(v_post_restart["likes_count"], 1);
    assert_eq!(v_post_restart["viewer_bookmarked"], false);
    assert_eq!(v_post_restart["bookmarks_count"], 0);
    assert_eq!(v_post_restart["comments_count"], 0);
    assert_eq!(v_post_restart["shares_count"], 1);

    let interaction_rows: i64 = state2
        .db
        .pool()
        .with_reader(|conn| {
            Ok(conn
                .query_row(
                    "SELECT COUNT(*) FROM code_scene_interactions WHERE event_id = 'fixed_dedup_ev_001'",
                    [],
                    |r| r.get(0),
                )
                .unwrap())
        })
        .unwrap();
    assert_eq!(interaction_rows, 1);

    // Toggling like after restart cleanly unlikes to liked=false, likes_count=0!
    let (st_unlike_rest, v_unlike_rest) = api(
        &router2,
        Method::POST,
        &format!("/api/v1/code-scene/assets/{sid}/like"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_unlike_rest, StatusCode::OK);
    assert_eq!(v_unlike_rest["liked"], false);
    assert_eq!(v_unlike_rest["likes_count"], 0);
}

#[tokio::test]
async fn test_code_scene_query_pagination_bilingual_score_invariance_validate_parity_and_hide_follow() {
    let dir = TempDir::new().unwrap();
    let (cfg, state, router) = boot_server_in_dir(&dir);
    let (_uid, token) = register_user(&router, "cs_query_parity").await;

    // 1. Lossless query pagination:
    //    (a) tab="for_you", query="game", limit=2 must losslessly paginate the EXACT set returned by limit=50
    let (st_full, v_full) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "tab": "for_you", "query": "game", "limit": 50 })),
    )
    .await;
    assert_eq!(st_full, StatusCode::OK);
    let full_feed: CodeSceneFeedResponse = serde_json::from_value(v_full).unwrap();
    let full_ids: Vec<String> = full_feed.items.iter().map(|i| i.asset.scene_id.clone()).collect();
    assert!(full_ids.len() >= 6, "query='game' must match at least all 6 games");

    let mut seen_for_you_ids = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..10 {
        let mut body = json!({ "tab": "for_you", "query": "game", "limit": 2 });
        if let Some(ref c) = cursor {
            body["cursor"] = json!(c);
        }
        let (st, v) = api(&router, Method::POST, "/api/v1/code-scene/feed", Some(&token), Some(body)).await;
        assert_eq!(st, StatusCode::OK);
        let feed: CodeSceneFeedResponse = serde_json::from_value(v).unwrap();
        assert!(feed.items.len() <= 2);
        for item in &feed.items {
            assert!(
                !seen_for_you_ids.contains(&item.asset.scene_id),
                "duplicate item across query pages: {}",
                item.asset.scene_id
            );
            seen_for_you_ids.push(item.asset.scene_id.clone());
        }
        cursor = feed.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(cursor.is_none(), "query pagination must terminate with next_cursor=None");
    assert_eq!(
        seen_for_you_ids, full_ids,
        "paginated query='game' limit=2 must losslessly match full limit=50 result"
    );

    //    (b) tab="games", query="game", limit=2 paginates all 6 games cleanly in 2+2+2 -> None
    let mut seen_game_ids = Vec::new();
    let mut g_cursor: Option<String> = None;
    for page_idx in 0..5 {
        let mut body = json!({ "tab": "games", "query": "game", "limit": 2 });
        if let Some(ref c) = g_cursor {
            body["cursor"] = json!(c);
        }
        let (st, v) = api(&router, Method::POST, "/api/v1/code-scene/feed", Some(&token), Some(body)).await;
        assert_eq!(st, StatusCode::OK);
        let feed: CodeSceneFeedResponse = serde_json::from_value(v).unwrap();
        for item in &feed.items {
            assert_eq!(item.asset.kind, AssetKind::Game);
            assert!(!seen_game_ids.contains(&item.asset.scene_id));
            seen_game_ids.push(item.asset.scene_id.clone());
        }
        g_cursor = feed.next_cursor;
        if page_idx < 2 {
            assert_eq!(feed.items.len(), 2, "games page {page_idx} must return 2 games");
            assert!(g_cursor.is_some(), "games page {page_idx} must return next_cursor");
        } else {
            assert_eq!(feed.items.len(), 2, "games page 2 must return final 2 games");
            assert!(g_cursor.is_none(), "games page 2 must terminate with next_cursor=None");
            break;
        }
    }
    assert_eq!(seen_game_ids.len(), 6, "all 6 games must be reachable across 3 pages");

    // 2. Bilingual CJK query "赛博弹球" returns matching scenes; nonsense query returns 0
    let (st_zh, v_zh) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "query": "赛博弹球", "limit": 10 })),
    )
    .await;
    assert_eq!(st_zh, StatusCode::OK);
    let zh_feed: CodeSceneFeedResponse = serde_json::from_value(v_zh).unwrap();
    assert!(!zh_feed.items.is_empty(), "bilingual CJK query '赛博弹球' must match scenes");

    let (st_none, v_none) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "query": "ImpossibleQuery999888", "limit": 10 })),
    )
    .await;
    assert_eq!(st_none, StatusCode::OK);
    let none_feed: CodeSceneFeedResponse = serde_json::from_value(v_none).unwrap();
    assert!(none_feed.items.is_empty());
    assert!(none_feed.next_cursor.is_none());

    // 3. Page-invariant final_score on single-page (limit=20) vs paginated (limit=5 x 4)
    let first_vid = official_seed_assets()
        .into_iter()
        .find(|a| a.kind == AssetKind::Video)
        .unwrap()
        .scene_id;
    let _ = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({ "scene_id": first_vid, "kind": "fast_skip", "dwell_ms": 400 })),
    )
    .await;

    let (_, v_single) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "limit": 20 })),
    )
    .await;
    let single_feed: CodeSceneFeedResponse = serde_json::from_value(v_single).unwrap();
    let single_map: std::collections::HashMap<String, gpui_telegram_proto::code_scene_contracts::ScoreBreakdown> =
        single_feed
            .items
            .iter()
            .map(|i| (i.asset.scene_id.clone(), i.explanation.breakdown))
            .collect();

    let mut paged_map = std::collections::HashMap::new();
    let mut cur: Option<String> = None;
    for _ in 0..5 {
        let mut req = json!({ "limit": 5 });
        if let Some(ref c) = cur {
            req["cursor"] = json!(c);
        }
        let (_, v_p) = api(&router, Method::POST, "/api/v1/code-scene/feed", Some(&token), Some(req)).await;
        let p: CodeSceneFeedResponse = serde_json::from_value(v_p).unwrap();
        for item in &p.items {
            paged_map.insert(item.asset.scene_id.clone(), item.explanation.breakdown);
        }
        cur = p.next_cursor;
        if cur.is_none() {
            break;
        }
    }
    let b_single = single_map.get(&first_vid).expect("fast-skipped vid in single page");
    let b_paged = paged_map.get(&first_vid).expect("fast-skipped vid in paginated fetch");
    let pre_mmr_single = b_single.final_score + b_single.mmr_penalty;
    let pre_mmr_paged = b_paged.final_score + b_paged.mmr_penalty;
    assert!(
        (pre_mmr_single - pre_mmr_paged).abs() < 1e-4,
        "Pre-MMR Stage-2 score must be page-invariant: single={pre_mmr_single} paged={pre_mmr_paged}"
    );
    assert!(
        (b_single.final_score - b_paged.final_score).abs() < 0.30,
        "final_score must remain on unified scale without cursor-dependent distortion: single={} paged={}",
        b_single.final_score,
        b_paged.final_score
    );

    // 4. Comments offset=usize::MAX returns [] (no i64 wrap to -1)
    for idx in 0..3 {
        let (st_c, _) = api(
            &router,
            Method::POST,
            "/api/v1/code-scene/assets/seed_game_flappy_bird/comments",
            Some(&token),
            Some(json!({ "content": format!("comment #{idx}") })),
        )
        .await;
        assert_eq!(st_c, StatusCode::CREATED);
    }
    let (st_off, v_off) = api(
        &router,
        Method::GET,
        "/api/v1/code-scene/assets/seed_game_flappy_bird/comments?limit=10&offset=18446744073709551615",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_off, StatusCode::OK);
    assert_eq!(v_off["comments"].as_array().unwrap().len(), 0);
    assert_eq!(v_off["total"].as_u64().unwrap(), 3);

    // 5. /assets/validate vs /assets parity on all 5 invalid payloads
    let invalid_cases = [
        (
            "video duration -5.0",
            json!({ "source_code": sample_valid_cs1(), "manifest": { "kind": "video", "width": 360, "height": 640, "fps": 60, "duration_sec": -5.0, "seed": 1 } }),
            json!({ "title": "Bad", "kind": "video", "source_code": sample_valid_cs1(), "manifest": { "kind": "video", "width": 360, "height": 640, "fps": 60, "duration_sec": -5.0, "seed": 1 } }),
        ),
        (
            "video duration 0.0",
            json!({ "source_code": sample_valid_cs1(), "manifest": { "kind": "video", "width": 360, "height": 640, "fps": 60, "duration_sec": 0.0, "seed": 1 } }),
            json!({ "title": "Bad", "kind": "video", "source_code": sample_valid_cs1(), "manifest": { "kind": "video", "width": 360, "height": 640, "fps": 60, "duration_sec": 0.0, "seed": 1 } }),
        ),
        (
            "animation duration -10.0",
            json!({ "source_code": sample_valid_cs1(), "manifest": { "kind": "animation", "width": 360, "height": 640, "fps": 60, "duration_sec": -10.0, "seed": 1 } }),
            json!({ "title": "Bad", "kind": "animation", "source_code": sample_valid_cs1(), "manifest": { "kind": "animation", "width": 360, "height": 640, "fps": 60, "duration_sec": -10.0, "seed": 1 } }),
        ),
        (
            "code_format js",
            json!({ "source_code": sample_valid_cs1(), "code_format": "js", "manifest": AssetManifest::for_kind(AssetKind::Animation) }),
            json!({ "title": "Bad", "kind": "animation", "code_format": "js", "source_code": sample_valid_cs1() }),
        ),
        (
            "code_format empty",
            json!({ "source_code": sample_valid_cs1(), "code_format": "", "manifest": AssetManifest::for_kind(AssetKind::Animation) }),
            json!({ "title": "Bad", "kind": "animation", "code_format": "", "source_code": sample_valid_cs1() }),
        ),
    ];
    for (label, val_payload, pub_payload) in invalid_cases {
        let (st_v, v_v) = api(&router, Method::POST, "/api/v1/code-scene/assets/validate", Some(&token), Some(val_payload)).await;
        assert_eq!(st_v, StatusCode::OK, "validate {label}");
        assert_eq!(v_v["ok"], false, "validate must reject {label}: {v_v}");

        let (st_p, _) = api(&router, Method::POST, "/api/v1/code-scene/assets", Some(&token), Some(pub_payload)).await;
        assert_eq!(st_p, StatusCode::UNPROCESSABLE_ENTITY, "publish must reject {label} with 422");
    }

    // 6. Concurrent 5-way /assets/validate calls all succeed with 0 wall-clock timeout flakes
    let heavy_anim = r#"
fn on_init(state) { state.t = 0.0; }
fn on_update(dt, input, state) { state.t = state.t + dt; }
fn on_render(canvas, state, time) {
    clear(rgba(8, 10, 22, 1.0));
    for i in 0..80 {
        let a = time * 1.4 + i * 0.25;
        let x = 180 + cos(a) * (40 + (i % 8) * 14);
        let y = 320 + sin(a * 1.2) * (60 + (i % 6) * 20);
        circle(x, y, 6, hsla((i * 11 + time * 40) % 360, 0.9, 0.6, 0.85));
    }
}
"#;
    let payload = json!({
        "source_code": heavy_anim,
        "manifest": AssetManifest::for_kind(AssetKind::Animation)
    });
    let (r1, r2, r3, r4, r5) = tokio::join!(
        api(&router, Method::POST, "/api/v1/code-scene/assets/validate", Some(&token), Some(payload.clone())),
        api(&router, Method::POST, "/api/v1/code-scene/assets/validate", Some(&token), Some(payload.clone())),
        api(&router, Method::POST, "/api/v1/code-scene/assets/validate", Some(&token), Some(payload.clone())),
        api(&router, Method::POST, "/api/v1/code-scene/assets/validate", Some(&token), Some(payload.clone())),
        api(&router, Method::POST, "/api/v1/code-scene/assets/validate", Some(&token), Some(payload)),
    );
    for (idx, (st, v)) in [r1, r2, r3, r4, r5].into_iter().enumerate() {
        assert_eq!(st, StatusCode::OK, "concurrent validate #{idx} HTTP status");
        assert_eq!(v["ok"], true, "concurrent validate #{idx} must pass: {v}");
    }

    // 7. HideAuthor followed by POST /creators/{id}/follow un-hides author & persists across restart
    let (st_hide, _) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({ "kind": "hide_author", "author_user_id": 777102 })),
    )
    .await;
    assert_eq!(st_hide, StatusCode::OK);

    let (st_flw, v_flw) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/creators/777102/follow",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_flw, StatusCode::OK);
    assert_eq!(v_flw["following"], true);

    let (_, v_flappy) = api(
        &router,
        Method::GET,
        "/api/v1/code-scene/assets/seed_game_flappy_bird",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(v_flappy["viewer_following_author"], true);

    let (_, v_flw_tab) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/feed",
        Some(&token),
        Some(json!({ "tab": "following", "limit": 20 })),
    )
    .await;
    let flw_feed: CodeSceneFeedResponse = serde_json::from_value(v_flw_tab).unwrap();
    assert!(flw_feed.items.iter().any(|i| i.asset.author_user_id == 777102));

    // Restart and confirm HideAuthor+Follow consistency survives!
    state.shutdown();
    drop(router);
    drop(state);
    let state2 = Arc::new(AppState::try_new(cfg).expect("reopen AppState"));
    let router2 = build_router_with_state(state2);

    let (_, v_flappy2) = api(
        &router2,
        Method::GET,
        "/api/v1/code-scene/assets/seed_game_flappy_bird",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(v_flappy2["viewer_following_author"], true);
}

#[tokio::test]
async fn test_code_scene_hide_author_canonical_resolution_and_restart_follow_parity() {
    let dir = TempDir::new().unwrap();
    let (cfg, state, router) = boot_server_in_dir(&dir);
    let (uid, token) = register_user(&router, "cs_hide_canon").await;

    let count_follow = |db: &ServerDatabase, follower: i64, creator: i64| -> i64 {
        db.pool()
            .with_reader(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT COUNT(*) FROM code_scene_follows WHERE follower_user_id = ?1 AND creator_user_id = ?2",
                        rusqlite::params![follower, creator],
                        |r| r.get(0),
                    )
                    .unwrap())
            })
            .unwrap()
    };

    // Variant A: HideAuthor(scene_id="seed_game_flappy_bird" [777102], author_user_id=0)
    // must delete (uid, 777102) from code_scene_follows so subsequent POST /follow returns true!
    let (st, v) = api(&router, Method::POST, "/api/v1/code-scene/creators/777102/follow", Some(&token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["following"], true);
    assert_eq!(count_follow(&state.db, uid, 777102), 1);

    let (st, _) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({
            "event_id": "iter3_var_a",
            "kind": "hide_author",
            "scene_id": "seed_game_flappy_bird",
            "author_user_id": 0
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(count_follow(&state.db, uid, 777102), 0, "Variant A leaked code_scene_follows row");
    let (_, v_a) = api(&router, Method::GET, "/api/v1/code-scene/assets/seed_game_flappy_bird", Some(&token), None).await;
    assert_eq!(v_a["viewer_following_author"], false);

    // Re-following 777102 must return following=true (not toggle off a leaked SQLite row!).
    let (st, v_reflw) = api(&router, Method::POST, "/api/v1/code-scene/creators/777102/follow", Some(&token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v_reflw["following"], true);
    assert_eq!(count_follow(&state.db, uid, 777102), 1);

    // Variant B: Follow BOTH 777101 and 777102, then HideAuthor(scene_id="seed_game_flappy_bird"[777102],
    // author_user_id=777101). Must hide & delete ONLY 777102 while preserving 777101 across restart!
    let (st, v701) = api(&router, Method::POST, "/api/v1/code-scene/creators/777101/follow", Some(&token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v701["following"], true);
    assert_eq!(count_follow(&state.db, uid, 777101), 1);
    assert_eq!(count_follow(&state.db, uid, 777102), 1);

    let (st, _) = api(
        &router,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({
            "event_id": "iter3_var_b",
            "kind": "hide_author",
            "scene_id": "seed_game_flappy_bird",
            "author_user_id": 777101
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(count_follow(&state.db, uid, 777101), 1, "Variant B must NOT delete unrelated follow 777101");
    assert_eq!(count_follow(&state.db, uid, 777102), 0, "Variant B must delete scene author 777102");
    let (_, v_701_live) = api(&router, Method::GET, "/api/v1/code-scene/assets/seed_anim_aurora_plasma", Some(&token), None).await;
    let (_, v_702_live) = api(&router, Method::GET, "/api/v1/code-scene/assets/seed_game_flappy_bird", Some(&token), None).await;
    assert_eq!(v_701_live["viewer_following_author"], true);
    assert_eq!(v_702_live["viewer_following_author"], false);

    // Restart server and verify 777101 follow survives while 777102 stays hidden/unfollowed!
    state.shutdown();
    drop(router);
    drop(state);
    let state2 = Arc::new(AppState::try_new(cfg.clone()).expect("reopen AppState after Variant B"));
    let router2 = build_router_with_state(state2.clone());

    let (_, v_701_post) = api(&router2, Method::GET, "/api/v1/code-scene/assets/seed_anim_aurora_plasma", Some(&token), None).await;
    let (_, v_702_post) = api(&router2, Method::GET, "/api/v1/code-scene/assets/seed_game_flappy_bird", Some(&token), None).await;
    assert_eq!(v_701_post["viewer_following_author"], true, "777101 follow must survive restart");
    assert_eq!(v_702_post["viewer_following_author"], false, "777102 must remain hidden/unfollowed after restart");
    assert_eq!(count_follow(&state2.db, uid, 777101), 1);
    assert_eq!(count_follow(&state2.db, uid, 777102), 0);

    // Variant C: Re-follow 777102, then HideAuthor(scene_id="nonexistent_scene_999", reason="777102")
    // must delete (uid, 777102) from code_scene_follows and remain consistent across restart!
    let (st, v_re2) = api(&router2, Method::POST, "/api/v1/code-scene/creators/777102/follow", Some(&token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v_re2["following"], true);
    assert_eq!(count_follow(&state2.db, uid, 777102), 1);

    let (st, _) = api(
        &router2,
        Method::POST,
        "/api/v1/code-scene/telemetry",
        Some(&token),
        Some(json!({
            "event_id": "iter3_var_c",
            "kind": "hide_author",
            "scene_id": "nonexistent_scene_999",
            "reason": "777102"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(count_follow(&state2.db, uid, 777102), 0, "Variant C reason fallback leaked code_scene_follows row");
    assert_eq!(count_follow(&state2.db, uid, 777101), 1, "Variant C must preserve unrelated 777101 follow");
    let (_, v_c) = api(&router2, Method::GET, "/api/v1/code-scene/assets/seed_game_flappy_bird", Some(&token), None).await;
    assert_eq!(v_c["viewer_following_author"], false);

    // Re-following 777102 after Variant C cleanly toggles to following=true!
    let (st, v_re3) = api(&router2, Method::POST, "/api/v1/code-scene/creators/777102/follow", Some(&token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v_re3["following"], true);
    assert_eq!(count_follow(&state2.db, uid, 777102), 1);
}


