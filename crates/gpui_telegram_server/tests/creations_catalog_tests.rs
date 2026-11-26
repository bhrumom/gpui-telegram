use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use futures::{SinkExt, StreamExt};
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use gpui_telegram_server::{
    db::{run_migrations, ALL_MIGRATIONS},
    server::{build_router_with_state, AppState},
    ServerConfig,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message as TungsteniteMessage;
use tower::ServiceExt;

fn make_temp_state() -> (TempDir, ServerConfig, Arc<AppState>, Router) {
    let dir = TempDir::new().expect("temp dir");
    let db_path = dir.path().join("creations_test.db");
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
    let val = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, val)
}

async fn http_raw(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Vec<u8>,
    content_type: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(tok) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {tok}"));
    }
    if let Some(ct) = content_type {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }
    let req = builder.body(Body::from(body)).unwrap();
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

fn sample_book_req(title: &str, summary: &str, genres: &[&str], cover_media: Option<&str>) -> Value {
    let mut cover = json!({
        "gradient_start_hex": "#1D4ED8",
        "gradient_end_hex": "#0F172A",
        "accent_hex": "#60A5FA",
        "icon_emoji": "📚"
    });
    if let Some(mid) = cover_media {
        cover["media_file_id"] = json!(mid);
    }
    json!({
        "kind": "book",
        "title": title,
        "subtitle": "Original Edition",
        "summary": summary,
        "author_display_name": "Alice Author",
        "language": "zh-CN",
        "genres": genres,
        "tags": ["featured", "reading"],
        "cover": cover,
        "age_rating": "4+",
        "payload": {
            "kind": "book",
            "author": "Alice Author",
            "blurb": summary,
            "estimated_read_mins": 12,
            "word_count": 1200,
            "chapters": [
                {
                    "chapter_id": "ch_1",
                    "chapter_number": 1,
                    "title": "Chapter One",
                    "pages": [
                        {
                            "page_number": 1,
                            "blocks": [
                                { "type": "heading", "level": 1, "text": "Into the Nebula" },
                                { "type": "paragraph", "text": "Across the quiet starfield, the navigator tuned the orbital compass and recorded every luminous constellation along the frontier route." }
                            ]
                        }
                    ]
                }
            ]
        }
    })
}

fn sample_music_req(title: &str, summary: &str, genres: &[&str], audio_media: Option<&str>) -> Value {
    let mut track = json!({
        "track_id": "tr_1",
        "track_number": 1,
        "title": "Midnight Pulse",
        "duration_secs": 32,
        "bpm": 118,
        "musical_key": "Am",
        "instruments": [{ "channel": 0, "name": "Synth Lead", "waveform": "sine", "gain": 0.8, "pan": 0.0 }],
        "melody_notes": [
            { "midi_note": 60, "start_beat": 0.0, "duration_beats": 1.0, "velocity": 96, "channel": 0 },
            { "midi_note": 64, "start_beat": 1.0, "duration_beats": 1.0, "velocity": 96, "channel": 0 },
            { "midi_note": 67, "start_beat": 2.0, "duration_beats": 1.0, "velocity": 96, "channel": 0 },
            { "midi_note": 72, "start_beat": 3.0, "duration_beats": 1.0, "velocity": 96, "channel": 0 }
        ],
        "chord_progression": [],
        "lyrics": [{ "timestamp_ms": 0, "text": "Stars drift across the midnight bay" }],
        "waveform_5bit": []
    });
    if let Some(mid) = audio_media {
        track["audio_media_file_id"] = json!(mid);
    }
    json!({
        "kind": "music",
        "title": title,
        "subtitle": "Hi-Res Synth",
        "summary": summary,
        "author_display_name": "Composer Nova",
        "language": "en",
        "genres": genres,
        "tags": ["synthwave", "ambient"],
        "cover": {
            "gradient_start_hex": "#EC4899",
            "gradient_end_hex": "#312E81",
            "accent_hex": "#F472B6",
            "icon_emoji": "🎵"
        },
        "payload": {
            "kind": "music",
            "artist": "Composer Nova",
            "album_title": title,
            "release_year": 2026,
            "primary_genre": "Electronic",
            "mood_tags": ["chill"],
            "bpm": 118,
            "musical_key": "Am",
            "time_signature": "4/4",
            "total_duration_secs": 32,
            "tracks": [track]
        }
    })
}

fn sample_game_req(title: &str, summary: &str, genres: &[&str]) -> Value {
    json!({
        "kind": "game",
        "title": title,
        "subtitle": "Arcade Challenge",
        "summary": summary,
        "author_display_name": "Indie Dev",
        "language": "en",
        "genres": genres,
        "tags": ["arcade", "puzzle"],
        "cover": {
            "gradient_start_hex": "#10B981",
            "gradient_end_hex": "#064E3B",
            "accent_hex": "#34D399",
            "icon_emoji": "🎮"
        },
        "payload": {
            "kind": "game",
            "developer": "Indie Dev",
            "category": "trivia",
            "controls_hint": "Pick the correct answer",
            "rules_markdown": "Answer each question to build your high score.",
            "engine_kind": "trivia_quiz",
            "supports_leaderboard": true,
            "estimated_session_mins": 5,
            "spec": {
                "type": "trivia_quiz",
                "time_limit_secs": 30,
                "questions": [
                    {
                        "question_id": "q1",
                        "prompt": "Which planet is known as the Red Planet?",
                        "choices": ["Mars", "Venus", "Saturn"],
                        "correct_index": 0,
                        "explanation": "Iron oxide on the surface gives Mars its reddish color."
                    }
                ]
            },
            "achievements": [
                {
                    "achievement_id": "ach_1",
                    "title": "First Win",
                    "description": "Score at least 100 points",
                    "target_score": 100,
                    "icon_emoji": "🏆"
                }
            ]
        }
    })
}

fn sample_app_req(title: &str, summary: &str, genres: &[&str], screenshot_media: Option<&str>) -> Value {
    let mut shot = json!({
        "title": "Main Workspace",
        "caption": "Clean Apple-style UI",
        "accent_hex": "#2563EB"
    });
    if let Some(mid) = screenshot_media {
        shot["media_file_id"] = json!(mid);
    }
    json!({
        "kind": "app",
        "title": title,
        "subtitle": "Productivity Suite",
        "summary": summary,
        "author_display_name": "Studio Labs",
        "language": "en",
        "genres": genres,
        "tags": ["productivity", "tools"],
        "cover": {
            "gradient_start_hex": "#2563EB",
            "gradient_end_hex": "#1E3A8A",
            "accent_hex": "#93C5FD",
            "icon_emoji": "🧩"
        },
        "payload": {
            "kind": "app",
            "developer_name": "Studio Labs",
            "version_string": "1.2.0",
            "release_notes": "Initial Apple Store launch",
            "category": "Productivity",
            "long_description": "A genuine interactive utility app with clean widgets and shortcuts.",
            "screenshots": [shot],
            "size_label": "4.2 MB",
            "price_label": "Free",
            "permissions": ["notifications"],
            "ui_schema_json": { "version": 1, "layout": "dashboard" }
        }
    })
}

async fn submit_and_approve(
    app: &Router,
    owner_token: &str,
    admin_token: &str,
    creation_id: &str,
    visibility: &str,
) -> Value {
    let (st, body) = http_json(
        app,
        "POST",
        &format!("/api/v1/creations/{creation_id}/submit-review"),
        Some(owner_token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "submit-review failed: {body}");
    assert_eq!(body["creation"]["review_status"], "pending_review");

    let (st, body) = http_json(
        app,
        "POST",
        &format!("/api/v1/creations/{creation_id}/review"),
        Some(admin_token),
        Some(json!({ "decision": "approve", "visibility": visibility })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "approve failed: {body}");
    assert_eq!(body["review_status"], "published");
    assert_eq!(body["visibility"], visibility);
    body
}

#[tokio::test]
async fn test_01_migration_v41_upgrade_from_v40_idempotent_and_clean_db() {
    assert_eq!(ALL_MIGRATIONS.len(), 38, "ALL_MIGRATIONS must remain length 38");

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("upgrade_v40_to_v41.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _schema_migrations (version INTEGER PRIMARY KEY, applied_at_ms INTEGER NOT NULL);",
    )
    .unwrap();

    // Simulate an existing production database already migrated through v1..=v40.
    for (idx, sql) in ALL_MIGRATIONS.iter().enumerate() {
        if let Err(e) = conn.execute_batch(sql) {
            assert!(
                e.to_string().contains("duplicate column name"),
                "unexpected migration error on v{}: {e}",
                idx + 1
            );
            for stmt in sql.split(';') {
                let clean = stmt.trim();
                if clean.is_empty() {
                    continue;
                }
                if let Err(se) = conn.execute_batch(clean) {
                    assert!(
                        se.to_string().contains("duplicate column name"),
                        "unexpected stmt error on v{}: {se}",
                        idx + 1
                    );
                }
            }
        }
        conn.execute(
            "INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (?1, ?2)",
            rusqlite::params![(idx + 1) as i64, 1_700_000_000_000i64 + idx as i64],
        )
        .unwrap();
    }
    conn.execute_batch(gpui_telegram_server::db::mcp_oauth_store::MIGRATION_V39_MCP_OAUTH)
        .unwrap();
    conn.execute(
        "INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (39, 1700000000039)",
        [],
    )
    .unwrap();
    conn.execute_batch(gpui_telegram_server::db::remote_assist_store::MIGRATION_V40_REMOTE_ASSIST)
        .unwrap();
    conn.execute(
        "INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (40, 1700000000040)",
        [],
    )
    .unwrap();

    // Upgrade to V43 (V41 = automation apps, V42 = community) via canonical run_migrations.
    run_migrations(&conn).expect("upgrade v40 -> v43 should succeed");

    // Verify v39 & v40 were NOT shifted or re-run, and v41 + v42 + v43 are recorded.
    let v39_ts: i64 = conn
        .query_row("SELECT applied_at_ms FROM _schema_migrations WHERE version = 39", [], |r| r.get(0))
        .unwrap();
    let v40_ts: i64 = conn
        .query_row("SELECT applied_at_ms FROM _schema_migrations WHERE version = 40", [], |r| r.get(0))
        .unwrap();
    let has_v41: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM _schema_migrations WHERE version = 41)", [], |r| r.get(0))
        .unwrap();
    let has_v42: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM _schema_migrations WHERE version = 42)", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v39_ts, 1_700_000_000_039);
    assert_eq!(v40_ts, 1_700_000_000_040);
    assert!(has_v41);
    let has_v43: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM _schema_migrations WHERE version = 43)", [], |r| r.get(0))
        .unwrap();
    assert!(has_v42);
    assert!(has_v43);

    // Verify all 8 V43 tables/virtual tables and all 3 FTS5 triggers exist.
    for tbl in [
        "user_creations",
        "public_catalog_entries",
        "public_catalog_fts",
        "creation_review_audit",
        "user_creation_states",
        "creation_reviews",
        "creation_game_scores",
        "creation_public_assets",
    ] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [tbl],
                |r| r.get(0),
            )
            .unwrap();
        assert!(exists, "missing V43 table {tbl}");
    }
    for trg in ["trg_public_catalog_ai", "trg_public_catalog_au", "trg_public_catalog_ad"] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'trigger' AND name = ?1)",
                [trg],
                |r| r.get(0),
            )
            .unwrap();
        assert!(exists, "missing V43 trigger {trg}");
    }

    // Zero dummy seed records on clean startup.
    let uc_cnt: i64 = conn.query_row("SELECT COUNT(*) FROM user_creations", [], |r| r.get(0)).unwrap();
    let cat_cnt: i64 = conn.query_row("SELECT COUNT(*) FROM public_catalog_entries", [], |r| r.get(0)).unwrap();
    assert_eq!(uc_cnt, 0);
    assert_eq!(cat_cnt, 0);

    // Idempotent re-run of run_migrations.
    run_migrations(&conn).expect("second run_migrations must be idempotent");
}

#[tokio::test]
async fn test_02_cloud_crud_all_four_kinds_sync_cas_isolation_and_restart_durability() {
    let (_dir, cfg, state, app) = make_temp_state();
    let (_alice_id, alice_tok) = register_user(&app, "alice_crud", "Alice", Some("Maker")).await;
    let (_bob_id, bob_tok) = register_user(&app, "bob_crud", "Bob", Some("Other")).await;

    // Unauthenticated rejected with 401.
    let (st, _) = http_json(&app, "GET", "/api/v1/creations/my", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Kind mismatch rejected with 400.
    let mut bad = sample_book_req("Mismatch", "Valid summary with enough chars.", &["Sci-Fi"], None);
    bad["kind"] = json!("music");
    let (st, _) = http_json(&app, "POST", "/api/v1/creations", Some(&alice_tok), Some(bad)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Create all 4 kinds.
    let (st, book) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_book_req("星际远征", "一部关于星际航行与未知文明的长篇科幻小说。", &["科幻"], None)),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(book["review_status"], "draft");
    assert_eq!(book["visibility"], "private");
    assert_eq!(book["cloud_revision"], 1);
    let book_id = book["creation_id"].as_str().unwrap().to_string();

    let (st, music) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_music_req("Neon Horizon", "Atmospheric synthwave soundscape for night coding.", &["Electronic"], None)),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(music["cloud_revision"], 2);
    let music_id = music["creation_id"].as_str().unwrap().to_string();

    let (st, game) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_game_req("Orbit Trivia", "Fast-paced astronomy trivia game for curious minds.", &["Trivia"])),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(game["cloud_revision"], 3);

    let (st, app_cr) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_app_req("Focus Timer", "Minimalist Apple-style deep work timer utility.", &["Productivity"], None)),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(app_cr["cloud_revision"], 4);

    // Filter /my by kind, status, and CJK query.
    let (st, my_all) = http_json(&app, "GET", "/api/v1/creations/my", Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(my_all["items"].as_array().unwrap().len(), 4);
    assert_eq!(my_all["latest_revision"], 4);

    let (st, my_cjk) = http_json(&app, "GET", "/api/v1/creations/my?kind=book&status=draft&q=%E6%98%9F%E9%99%85", Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(my_cjk["items"].as_array().unwrap().len(), 1);
    assert_eq!(my_cjk["items"][0]["creation_id"], book_id);

    // CAS update: valid expected_revision succeeds, stale fails 409 revision_conflict.
    let (st, upd) = http_json(
        &app,
        "PUT",
        &format!("/api/v1/creations/{book_id}"),
        Some(&alice_tok),
        Some(json!({ "expected_revision": 1, "title": "星际远征 修订版" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(upd["version"], 2);
    assert_eq!(upd["cloud_revision"], 5);

    let (st, conflict) = http_json(
        &app,
        "PUT",
        &format!("/api/v1/creations/{book_id}"),
        Some(&alice_tok),
        Some(json!({ "expected_revision": 1, "title": "Stale Write" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(conflict["error"], "revision_conflict");
    assert_eq!(conflict["expected_revision"], 1);
    assert_eq!(conflict["current_revision"], 5);

    // Strict cross-user isolation for Bob.
    for (m, path, body) in [
        ("GET", format!("/api/v1/creations/{book_id}"), None),
        ("PUT", format!("/api/v1/creations/{book_id}"), Some(json!({"title": "Hack"}))),
        ("DELETE", format!("/api/v1/creations/{book_id}"), None),
        ("POST", format!("/api/v1/creations/{book_id}/submit-review"), None),
        ("GET", format!("/api/v1/creations/{book_id}/audit"), None),
    ] {
        let (st, _) = http_json(&app, m, &path, Some(&bob_tok), body).await;
        assert!(
            st == StatusCode::FORBIDDEN || st == StatusCode::NOT_FOUND,
            "Bob must be denied {m} {path}, got {st}"
        );
    }

    // Soft-delete music_id: hidden from /my, delivered as tombstone in /sync.
    let (st, del) = http_json(&app, "DELETE", &format!("/api/v1/creations/{music_id}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(del["deleted_at_ms"].as_i64().is_some());
    assert_eq!(del["cloud_revision"], 6);

    let (st, my_after) = http_json(&app, "GET", "/api/v1/creations/my", Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(my_after["items"].as_array().unwrap().len(), 3);
    assert_eq!(my_after["latest_revision"], 6);

    let (st, sync_delta) = http_json(&app, "GET", "/api/v1/creations/sync?since_revision=4", Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sync_delta["latest_revision"], 6);
    let delta_items = sync_delta["items"].as_array().unwrap();
    assert_eq!(delta_items.len(), 2);
    assert_eq!(delta_items[0]["cloud_revision"], 5);
    assert_eq!(delta_items[1]["creation_id"], music_id);
    assert!(delta_items[1]["deleted_at_ms"].as_i64().is_some());

    let (st, sync_empty) = http_json(&app, "GET", "/api/v1/creations/sync?since_revision=6", Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(sync_empty["items"].as_array().unwrap().is_empty());
    assert_eq!(sync_empty["latest_revision"], 6);

    // Cold restart durability on the same TempDir SQLite file.
    state.shutdown();
    drop(app);
    drop(state);

    let state2 = Arc::new(AppState::try_new(cfg).expect("restart AppState"));
    let app2 = build_router_with_state(state2.clone());
    let (st, login) = http_json(
        &app2,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "username": "alice_crud", "password": "StrongPassword123!", "device_name": "Restart" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let alice_tok2 = login["token"].as_str().unwrap();

    let (st, sync_restarted) = http_json(&app2, "GET", "/api/v1/creations/sync?since_revision=0", Some(alice_tok2), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sync_restarted["latest_revision"], 6);
    assert_eq!(sync_restarted["items"].as_array().unwrap().len(), 4);
    state2.shutdown();
}

#[tokio::test]
async fn test_03_review_fsm_validation_422_oversized_xss_transitions_and_audit_after_delete() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (_author_id, author_tok) = register_user(&app, "author_fsm", "Author", None).await;
    let (admin_id, admin_tok) = register_user(&app, "admin_fsm", "Reviewer", None).await;
    let (_other_id, other_tok) = register_user(&app, "other_fsm", "Reader", None).await;
    promote_admin(&state, admin_id);

    // 1. Invalid draft (<50 readable chars) can be saved as draft, but submit-review -> 422.
    let mut short_book = sample_book_req("Too Short", "Valid summary length here.", &["Fiction"], None);
    short_book["payload"]["chapters"][0]["pages"][0]["blocks"] = json!([{ "type": "paragraph", "text": "Tiny" }]);
    let (st, created_short) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(short_book)).await;
    assert_eq!(st, StatusCode::CREATED);
    let short_id = created_short["creation_id"].as_str().unwrap();

    let (st, val_err) = http_json(&app, "POST", &format!("/api/v1/creations/{short_id}/submit-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(val_err["error"], "validation_failed");
    assert!(!val_err["validation_errors"].as_array().unwrap().is_empty());

    // 2. Oversized payload (> 4 MiB) passes 16 MiB route body limit on POST and fails 422 on submit-review.
    let huge_text = "A".repeat(4 * 1024 * 1024 + 8192);
    let mut huge_book = sample_book_req("Huge Book", "Valid summary for oversized payload test.", &["Sci-Fi"], None);
    huge_book["payload"]["chapters"][0]["pages"][0]["blocks"] = json!([{ "type": "paragraph", "text": huge_text }]);
    let (st, created_huge) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(huge_book)).await;
    assert_eq!(st, StatusCode::CREATED, "POST > 4 MiB draft must succeed under 16 MiB limit");
    let huge_id = created_huge["creation_id"].as_str().unwrap();
    let (st, huge_err) = http_json(&app, "POST", &format!("/api/v1/creations/{huge_id}/submit-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(huge_err["error"], "validation_failed");

    // 3. Valid book + XSS inside SubmitReviewRequest.notes -> 422!
    let (st, created) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&author_tok),
        Some(sample_book_req("Clean Novel", "A clean sci-fi novel ready for moderation review.", &["Sci-Fi"], None)),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let cid = created["creation_id"].as_str().unwrap().to_string();

    let (st, xss_err) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{cid}/submit-review"),
        Some(&author_tok),
        Some(json!({ "notes": "<script>alert(1)</script>" })),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(xss_err["error"], "validation_failed");

    // Invalid FSM transitions from Draft: direct approve -> 409, withdraw -> 409, unpublish -> 409.
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/review"), Some(&admin_tok), Some(json!({"decision":"approve"}))).await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/withdraw-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/unpublish"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Non-admin cannot inspect review-queue or review.
    let (st, _) = http_json(&app, "GET", "/api/v1/creations/review-queue", Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/review"), Some(&author_tok), Some(json!({"decision":"approve"}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Draft -> PendingReview (empty body) -> edit blocked (409) -> withdraw -> Draft -> resubmit.
    let (st, sub) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/submit-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sub["creation"]["review_status"], "pending_review");

    let (st, _) = http_json(&app, "PUT", &format!("/api/v1/creations/{cid}"), Some(&author_tok), Some(json!({"title":"TOCTOU"}))).await;
    assert_eq!(st, StatusCode::CONFLICT);

    let (st, wd) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/withdraw-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(wd["review_status"], "draft");

    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/submit-review"), Some(&author_tok), Some(json!({"notes":"Ready for review"}))).await;
    assert_eq!(st, StatusCode::OK);

    // Admin queue lists pending submission.
    let (st, q) = http_json(&app, "GET", "/api/v1/creations/review-queue?status=pending_review&kind=book", Some(&admin_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(q["items"].as_array().unwrap().len(), 1);
    assert_eq!(q["items"][0]["creation_id"], cid);

    // Reject requires non-empty reason; Approve rejects visibility == "private".
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/review"), Some(&admin_tok), Some(json!({"decision":"reject","reason":"   "}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/review"), Some(&admin_tok), Some(json!({"decision":"approve","visibility":"private"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Reject with reason -> Rejected -> Edit returns to Draft & clears rejection_reason -> Resubmit -> Approve Public.
    let (st, rej) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{cid}/review"),
        Some(&admin_tok),
        Some(json!({"decision":"reject","reason":"Expand chapter 1","notes":"Almost ready"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rej["review_status"], "rejected");
    assert_eq!(rej["rejection_reason"], "Expand chapter 1");

    let (st, edited) = http_json(&app, "PUT", &format!("/api/v1/creations/{cid}"), Some(&author_tok), Some(json!({"subtitle":"Expanded Edition"}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(edited["review_status"], "draft");
    assert!(edited["rejection_reason"].is_null());

    submit_and_approve(&app, &author_tok, &admin_tok, &cid, "public").await;

    // Non-owner non-admin cannot read audit even while published!
    let (st, _) = http_json(&app, "GET", &format!("/api/v1/creations/{cid}/audit"), Some(&other_tok), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Unpublish -> Delete -> Submit deleted (404) -> Audit STILL readable after soft-delete!
    let (st, unpub) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/unpublish"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(unpub["review_status"], "unpublished");
    assert_eq!(unpub["visibility"], "private");

    let (st, _) = http_json(&app, "DELETE", &format!("/api/v1/creations/{cid}"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/submit-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, audit) = http_json(&app, "GET", &format!("/api/v1/creations/{cid}/audit"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let actions: Vec<&str> = audit["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        vec!["submit_review", "withdraw_review", "submit_review", "reject", "submit_review", "approve", "unpublish", "delete"]
    );
    state.shutdown();
}

#[tokio::test]
async fn test_04_live_edit_frozen_published_snapshot_isolation() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (_author_id, author_tok) = register_user(&app, "author_live", "Author", None).await;
    let (admin_id, admin_tok) = register_user(&app, "admin_live", "Admin", None).await;
    let (_reader_id, reader_tok) = register_user(&app, "reader_live", "Reader", None).await;
    promote_admin(&state, admin_id);

    let (st, c1) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&author_tok),
        Some(sample_book_req("Galaxy v1", "Initial published edition of the galaxy atlas.", &["Sci-Fi"], None)),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let cid = c1["creation_id"].as_str().unwrap().to_string();
    submit_and_approve(&app, &author_tok, &admin_tok, &cid, "public").await;

    // Owner live-edits v2 draft while v1 stays frozen for readers & catalog.
    let (st, v2_edit) = http_json(
        &app,
        "PUT",
        &format!("/api/v1/creations/{cid}"),
        Some(&author_tok),
        Some(json!({ "title": "Galaxy v2 Draft", "summary": "Unapproved v2 draft that must NOT leak yet." })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v2_edit["title"], "Galaxy v2 Draft");
    assert_eq!(v2_edit["version"], 2);
    assert_eq!(v2_edit["review_status"], "published");
    assert_eq!(v2_edit["has_live_published_version"], true);
    assert_eq!(v2_edit["published_version"], 1);

    let (st, reader_v1) = http_json(&app, "GET", &format!("/api/v1/creations/{cid}"), Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(reader_v1["title"], "Galaxy v1");
    assert_eq!(reader_v1["published_version"], 1);

    let (st, cat_v1) = http_json(&app, "GET", "/api/v1/catalog/books", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(cat_v1["items"][0]["title"], "Galaxy v1");

    // Submit v2 -> Admin rejects v2 -> v1 MUST STILL stay live & frozen!
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/submit-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, rej) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{cid}/review"),
        Some(&admin_tok),
        Some(json!({ "decision": "reject", "reason": "Polish v2 summary" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rej["review_status"], "rejected");
    assert_eq!(rej["has_live_published_version"], true);

    let (st, reader_still_v1) = http_json(&app, "GET", &format!("/api/v1/creations/{cid}"), Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(reader_still_v1["title"], "Galaxy v1");
    assert!(reader_still_v1["rejection_reason"].is_null());

    // Author revises to v3, submits, and admin approves -> catalog & readers now see v3 with 0 duplicate FTS rows!
    let (st, _) = http_json(
        &app,
        "PUT",
        &format!("/api/v1/creations/{cid}"),
        Some(&author_tok),
        Some(json!({ "title": "Galaxy v3 Final", "summary": "Polished third edition approved for public catalog." })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    submit_and_approve(&app, &author_tok, &admin_tok, &cid, "public").await;

    let (st, reader_v3) = http_json(&app, "GET", &format!("/api/v1/creations/{cid}"), Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(reader_v3["title"], "Galaxy v3 Final");
    assert_eq!(reader_v3["published_version"], 3);

    let fts_count: i64 = state
        .db
        .pool()
        .with_reader(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM public_catalog_fts WHERE creation_id = ?1",
                rusqlite::params![cid],
                |r| r.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert_eq!(fts_count, 1, "public_catalog_fts must have exact 1 row after live-edit re-approval");
    state.shutdown();
}

#[tokio::test]
async fn test_05_media_acl_public_assets_chat_uploads_dedup_and_exfiltration_guard() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (_alice_id, alice_tok) = register_user(&app, "alice_med", "Alice", None).await;
    let (_bob_id, bob_tok) = register_user(&app, "bob_med", "Bob", None).await;
    let (charlie_id, charlie_tok) = register_user(&app, "charlie_med", "Charlie", None).await;
    let (admin_id, admin_tok) = register_user(&app, "admin_med", "Admin", None).await;
    promote_admin(&state, admin_id);

    // 1. Alice uploads a direct cover image (no chat_id).
    let (st, up_cover) = http_raw(
        &app,
        "POST",
        "/api/v1/media/upload?filename=cover.png&mime_type=image/png&is_voice=false",
        Some(&alice_tok),
        b"PNG_UNIQUE_COVER_BYTES_ALICE_001".to_vec(),
        Some("image/png"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let cover_json: Value = serde_json::from_slice(&up_cover).unwrap();
    let cover_id = cover_json["media_id"].as_str().unwrap().to_string();

    // 2. Alice creates a private group chat (Alice-only) and uploads audio inside that chat (chat_id != NULL).
    let (st, chat_res) = http_json(
        &app,
        "POST",
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "title": "Alice Studio Scratchpad", "chat_type": "group", "member_ids": [] })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create chat failed: {chat_res}");
    let chat_id = chat_res["chat_id"].as_i64().unwrap();

    let (st, up_wav) = http_raw(
        &app,
        "POST",
        &format!("/api/v1/media/upload?filename=track.wav&mime_type=audio/wav&is_voice=false&chat_id={chat_id}"),
        Some(&alice_tok),
        b"RIFF_UNIQUE_CHAT_AUDIO_BYTES_ALICE_002".to_vec(),
        Some("audio/wav"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let wav_json: Value = serde_json::from_slice(&up_wav).unwrap();
    let wav_id = wav_json["media_id"].as_str().unwrap().to_string();

    // 3. Bob uploads a private secret file that Alice does NOT own.
    let (st, up_secret) = http_raw(
        &app,
        "POST",
        "/api/v1/media/upload?filename=secret.png&mime_type=image/png&is_voice=false",
        Some(&bob_tok),
        b"BOB_PRIVATE_SECRET_MEDIA_999".to_vec(),
        Some("image/png"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bob_secret_id = serde_json::from_slice::<Value>(&up_secret).unwrap()["media_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Before publish: Bob is denied (403) both Alice's direct cover & chat-uploaded wav.
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{cover_id}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{wav_id}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Alice publishes Book1 & Book2 sharing the same cover_id, plus Music1 with chat-uploaded wav_id.
    let (_, b1) = http_json(&app, "POST", "/api/v1/creations", Some(&alice_tok), Some(sample_book_req("Book One", "First public book sharing cover artwork.", &["Sci-Fi"], Some(&cover_id)))).await;
    let (_, b2) = http_json(&app, "POST", "/api/v1/creations", Some(&alice_tok), Some(sample_book_req("Book Two", "Second public book sharing the same CAS cover.", &["Sci-Fi"], Some(&cover_id)))).await;
    let (_, m1) = http_json(&app, "POST", "/api/v1/creations", Some(&alice_tok), Some(sample_music_req("Studio Track", "Chat-uploaded audio track published publicly.", &["Electronic"], Some(&wav_id)))).await;
    let b1_id = b1["creation_id"].as_str().unwrap();
    let b2_id = b2["creation_id"].as_str().unwrap();
    let m1_id = m1["creation_id"].as_str().unwrap();

    submit_and_approve(&app, &alice_tok, &admin_tok, b1_id, "public").await;
    submit_and_approve(&app, &alice_tok, &admin_tok, b2_id, "public").await;
    submit_and_approve(&app, &alice_tok, &admin_tok, m1_id, "public").await;

    // After publish: Bob gets 200 OK on BOTH direct cover AND chat-uploaded WAV!
    let (st, bytes) = http_raw(&app, "GET", &format!("/api/v1/media/{cover_id}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(bytes, b"PNG_UNIQUE_COVER_BYTES_ALICE_001");
    let (st, wav_bytes) = http_raw(&app, "GET", &format!("/api/v1/media/{wav_id}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(wav_bytes, b"RIFF_UNIQUE_CHAT_AUDIO_BYTES_ALICE_002");

    // Multi-creation CAS dedup: unpublishing Book1 leaves cover accessible via Book2; deleting Book2 revokes access!
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{b1_id}/unpublish"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{cover_id}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::OK, "Book2 still references cover_id publicly");

    let (st, _) = http_json(&app, "DELETE", &format!("/api/v1/creations/{b2_id}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{cover_id}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "Deleting last referencing public creation must revoke cover ACL");

    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{m1_id}/unpublish"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{wav_id}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "Unpublishing music must revoke chat-uploaded WAV ACL");

    // Verify app_screenshot media ACL also works on Published + Public and revokes on DELETE.
    let (st, up_shot) = http_raw(
        &app,
        "POST",
        "/api/v1/media/upload?filename=shot.png&mime_type=image/png&is_voice=false",
        Some(&alice_tok),
        b"PNG_UNIQUE_APP_SCREENSHOT_ALICE_003".to_vec(),
        Some("image/png"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let shot_id = serde_json::from_slice::<Value>(&up_shot).unwrap()["media_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, a_shot) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_app_req("Shot App", "App testing screenshot CAS asset allowlist.", &["Productivity"], Some(&shot_id))),
    )
    .await;
    let a_shot_id = a_shot["creation_id"].as_str().unwrap();
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{shot_id}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    submit_and_approve(&app, &alice_tok, &admin_tok, a_shot_id, "public").await;
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{shot_id}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = http_json(&app, "DELETE", &format!("/api/v1/creations/{a_shot_id}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{shot_id}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Exfiltration guard: Alice attaches Bob's private secret media ID and gets approved Public -> Charlie & Alice MUST STILL get 403!
    let (_, exfil_book) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_book_req("Exfil Attempt", "Attempting to attach another user's private CAS media.", &["Sci-Fi"], Some(&bob_secret_id))),
    )
    .await;
    let exfil_id = exfil_book["creation_id"].as_str().unwrap();
    submit_and_approve(&app, &alice_tok, &admin_tok, exfil_id, "public").await;

    let _ = charlie_id;
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{bob_secret_id}"), Some(&charlie_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "Cross-user media exfiltration MUST be blocked");
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{bob_secret_id}"), Some(&alice_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "Attacker Alice MUST NOT gain access to Bob's private media");
    state.shutdown();
}

#[tokio::test]
async fn test_06_bilingual_search_fts5_cjk_sqli_sorting_pagination_appstore_merge_and_recommend() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (author_id, author_tok) = register_user(&app, "author_cat", "Creator", None).await;
    let (admin_id, admin_tok) = register_user(&app, "admin_cat", "Admin", None).await;
    let (_reader_id, reader_tok) = register_user(&app, "reader_cat", "Reader", None).await;
    promote_admin(&state, admin_id);

    // Publish EN + CJK items across all 4 verticals.
    let (_, b_en) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_book_req("Quantum Horizon", "Deep space quantum physics thriller across distant worlds.", &["Sci-Fi"], None))).await;
    let (_, b_zh) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_book_req("星际迷航手记", "探索银河边缘与星际探险文明的科幻作品集。", &["科幻"], None))).await;
    let (_, m_zh) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_music_req("深空旋律", "融合合成器与极简钢琴的深夜电子专辑。", &["电子"], None))).await;
    let (_, g_en) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_game_req("Nebula Quiz", "Interactive astronomy trivia game with quantum bonus rounds.", &["Trivia"]))).await;
    let (_, a_en) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_app_req("Star Notes", "Apple-style spatial notes app for creators and readers.", &["Productivity"], None))).await;

    let b_en_id = b_en["creation_id"].as_str().unwrap().to_string();
    let b_zh_id = b_zh["creation_id"].as_str().unwrap().to_string();
    let m_zh_id = m_zh["creation_id"].as_str().unwrap().to_string();
    let g_en_id = g_en["creation_id"].as_str().unwrap().to_string();
    let a_en_id = a_en["creation_id"].as_str().unwrap().to_string();

    for cid in [&b_en_id, &b_zh_id, &m_zh_id, &g_en_id, &a_en_id] {
        submit_and_approve(&app, &author_tok, &admin_tok, cid, "public").await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Seed 1 published mini_app AND 1 ad-hoc __bot_inline_webapp__ mini_app (must be excluded!).
    let now_ms = chrono::Utc::now().timestamp_millis();
    state.db.pool().with_writer(|conn| {
        conn.execute(
            "INSERT INTO mini_apps (app_id, bot_user_id, bot_username, short_name, title, description, category, web_url, is_published, created_at, updated_at) VALUES ('ma_pub_1', ?1, 'UtilityBot', 'focus', 'Focus MiniApp', 'Timer mini app', 'Productivity', 'https://example.com/focus', 1, ?2, ?2)",
            rusqlite::params![author_id as i64, now_ms],
        ).map_err(|e| TelegramError::Database(e.to_string()))?;
        conn.execute(
            "INSERT INTO mini_apps (app_id, bot_user_id, bot_username, short_name, title, description, category, web_url, is_published, created_at, updated_at) VALUES ('ma_adhoc_1', ?1, 'UtilityBot', 'adhoc', 'Hidden Adhoc', 'Must be filtered', '__bot_inline_webapp__', 'https://example.com/adhoc', 1, ?2, ?2)",
            rusqlite::params![author_id as i64, now_ms],
        ).map_err(|e| TelegramError::Database(e.to_string()))?;
        Ok(())
    }).unwrap();

    // English FTS5 bm25 ranking: title match "Quantum Horizon" outranks summary-only "Nebula Quiz".
    let (st, en_res) = http_json(&app, "GET", "/api/v1/catalog?q=Quantum", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let en_items = en_res["items"].as_array().unwrap();
    assert_eq!(en_items.len(), 2);
    assert_eq!(en_items[0]["creation_id"], b_en_id);
    assert_eq!(en_items[0]["deep_link"], format!("tg://apple/books/{b_en_id}"));

    // Unspaced CJK substring & bigram search ("星际探险").
    let (st, cjk_res) = http_json(&app, "GET", "/api/v1/catalog?q=%E6%98%9F%E9%99%85%E6%8E%A2%E9%99%A9", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(cjk_res["items"][0]["creation_id"], b_zh_id);

    // Special chars / SQL LIKE wildcards / SQLi -> 200 OK with empty results (zero wildcard expansion).
    for bad_q in [
        "*",
        "%25",
        "_",
        "%5C",
        "%22%22",
        "%27%3B%20DROP%20TABLE%20public_catalog_entries%3B%20--",
    ] {
        let (st, res) = http_json(&app, "GET", &format!("/api/v1/catalog?q={bad_q}"), Some(&reader_tok), None).await;
        assert_eq!(st, StatusCode::OK, "special query '{bad_q}' must return 200 OK, got {res}");
        assert!(res["items"].as_array().unwrap().is_empty(), "literal wildcard '{bad_q}' must not match all rows");
    }
    // FTS5 boolean operator neutralization: standalone "OR" returns 200 OK; "NO_SUCH_XYZ OR Quantum" is treated literally (0 hits).
    let (st, _) = http_json(&app, "GET", "/api/v1/catalog?q=OR", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, op_res) = http_json(&app, "GET", "/api/v1/catalog?q=NO_SUCH_XYZ%20OR%20Quantum", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(op_res["items"].as_array().unwrap().is_empty(), "FTS5 OR operator must be neutralized");

    // Ratings + engagement differentiate top_rated and popular sorting; pagination honors limit=0 & total.
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{b_zh_id}/reviews"), Some(&reader_tok), Some(json!({"rating": 5, "body": "Masterpiece"}))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{b_en_id}/reviews"), Some(&reader_tok), Some(json!({"rating": 3, "body": "Good"}))).await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, top_rated) = http_json(&app, "GET", "/api/v1/catalog/books?sort=top_rated", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(top_rated["items"][0]["creation_id"], b_zh_id);

    let (st, newest) = http_json(&app, "GET", "/api/v1/catalog?sort=newest&limit=2&offset=0", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(newest["total"], 5);
    assert_eq!(newest["items"].as_array().unwrap().len(), 2);
    assert_eq!(newest["items"][0]["creation_id"], a_en_id);

    let (st, zero_lim) = http_json(&app, "GET", "/api/v1/catalog?limit=0", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(zero_lim["total"], 5);
    assert!(zero_lim["items"].as_array().unwrap().is_empty());

    // App Store merge: apps + published mini_apps (no __bot_inline_webapp__) + cross_vertical + featured.
    let (st, appstore) = http_json(&app, "GET", "/api/v1/catalog/appstore", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(appstore["apps"].as_array().unwrap().len(), 1);
    assert_eq!(appstore["apps"][0]["creation_id"], a_en_id);
    let mini_apps = appstore["mini_apps"].as_array().unwrap();
    assert_eq!(mini_apps.len(), 1);
    assert_eq!(mini_apps[0]["short_name"], "focus");
    assert_eq!(mini_apps[0]["deep_link"], "tg://resolve?domain=UtilityBot&appname=focus");
    assert_eq!(appstore["cross_vertical"].as_array().unwrap().len(), 4);
    assert!(!appstore["featured"].as_array().unwrap().is_empty());

    // Recommendations return canonical tg://apple/<section>/<id> deep links and reasons.
    let (st, recs) = http_json(&app, "GET", "/api/v1/catalog/recommend?q=%E6%B7%B1%E7%A9%BA%E6%97%8B%E5%BE%8B&limit=5", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let rec_items = recs["items"].as_array().unwrap();
    assert_eq!(rec_items[0]["creation_id"], m_zh_id);
    assert_eq!(rec_items[0]["deep_link"], format!("tg://apple/music/{m_zh_id}"));
    assert!(!rec_items[0]["reason"].as_str().unwrap().is_empty());
    state.shutdown();
}

#[tokio::test]
async fn test_07_strict_zero_leak_matrix_across_all_surfaces_and_fts_parity() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (_author_id, author_tok) = register_user(&app, "author_zl", "Author", None).await;
    let (admin_id, admin_tok) = register_user(&app, "admin_zl", "Admin", None).await;
    let (_reader_id, reader_tok) = register_user(&app, "reader_zl", "Reader", None).await;
    promote_admin(&state, admin_id);

    // 1. Draft
    let (_, c_draft) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_book_req("SecretDraftCanary 秘密草稿", "Private draft canary content must never leak.", &["Sci-Fi"], None))).await;
    // 2. PendingReview
    let (_, c_pending) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_music_req("SecretPendingCanary 秘密待审", "Pending review canary content must never leak.", &["Electronic"], None))).await;
    let pending_id = c_pending["creation_id"].as_str().unwrap();
    http_json(&app, "POST", &format!("/api/v1/creations/{pending_id}/submit-review"), Some(&author_tok), None).await;
    // 3. Rejected
    let (_, c_rej) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_game_req("SecretRejectedCanary 秘密驳回", "Rejected canary content must never leak.", &["Trivia"]))).await;
    let rej_id = c_rej["creation_id"].as_str().unwrap();
    http_json(&app, "POST", &format!("/api/v1/creations/{rej_id}/submit-review"), Some(&author_tok), None).await;
    http_json(&app, "POST", &format!("/api/v1/creations/{rej_id}/review"), Some(&admin_tok), Some(json!({"decision":"reject","reason":"Private"}))).await;
    // 4. Unlisted (direct GET allowed, public catalog forbidden!)
    let (_, c_unlisted) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_book_req("SecretUnlistedCanary 秘密非公开", "Unlisted canary allowed via direct link only.", &["Sci-Fi"], None))).await;
    let unlisted_id = c_unlisted["creation_id"].as_str().unwrap();
    submit_and_approve(&app, &author_tok, &admin_tok, unlisted_id, "unlisted").await;
    // 5. Unpublished (was public, then unpublished)
    let (_, c_unpub) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_music_req("SecretUnpubCanary 秘密下架", "Unpublished canary removed from catalog immediately.", &["Electronic"], None))).await;
    let unpub_id = c_unpub["creation_id"].as_str().unwrap();
    submit_and_approve(&app, &author_tok, &admin_tok, unpub_id, "public").await;
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{unpub_id}/unpublish"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    // 6. Soft-deleted after public publish
    let (_, c_del) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_app_req("SecretDeletedCanary 秘密删除", "Deleted canary purged immediately from catalog.", &["Productivity"], None))).await;
    let del_id = c_del["creation_id"].as_str().unwrap();
    submit_and_approve(&app, &author_tok, &admin_tok, del_id, "public").await;
    let (st, _) = http_json(&app, "DELETE", &format!("/api/v1/creations/{del_id}"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    // 7. One genuine Public item
    let (_, c_pub) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_book_req("PublicAllowedBook", "Only this published public book should appear.", &["Sci-Fi"], None))).await;
    let pub_id = c_pub["creation_id"].as_str().unwrap().to_string();
    submit_and_approve(&app, &author_tok, &admin_tok, &pub_id, "public").await;

    let _ = c_draft;

    // Unlisted IS directly readable by reader via GET /api/v1/creations/{id}.
    let (st, unlisted_direct) = http_json(&app, "GET", &format!("/api/v1/creations/{unlisted_id}"), Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(unlisted_direct["visibility"], "unlisted");

    // Every discovery surface returns ONLY pub_id (zero leaks!).
    for uri in [
        "/api/v1/catalog",
        "/api/v1/catalog/books",
        "/api/v1/catalog/music",
        "/api/v1/catalog/games",
        "/api/v1/catalog/appstore",
        "/api/v1/catalog/recommend",
        "/api/v1/catalog?q=Secret",
        "/api/v1/catalog?q=%E7%A7%98%E5%AF%86",
        "/api/v1/catalog/recommend?q=SecretDraftCanary",
        "/api/v1/catalog/recommend?q=%E7%A7%98%E5%AF%86%E9%9D%9E%E5%85%AC%E5%BC%80",
    ] {
        let (st, body) = http_json(&app, "GET", uri, Some(&reader_tok), None).await;
        assert_eq!(st, StatusCode::OK, "{uri} failed: {body}");
        let raw = body.to_string();
        for forbidden in ["SecretDraftCanary", "SecretPendingCanary", "SecretRejectedCanary", "SecretUnlistedCanary", "SecretUnpubCanary", "SecretDeletedCanary", "秘密"] {
            assert!(!raw.contains(forbidden), "LEAK on {uri}: found '{forbidden}' in {raw}");
        }
    }

    // Physical table & FTS5 1:1 parity check.
    let (pub_cnt, fts_cnt): (i64, i64) = state
        .db
        .pool()
        .with_reader(|conn| {
            let p: i64 = conn.query_row("SELECT COUNT(*) FROM public_catalog_entries", [], |r| r.get(0)).map_err(|e| TelegramError::Database(e.to_string()))?;
            let f: i64 = conn.query_row("SELECT COUNT(*) FROM public_catalog_fts", [], |r| r.get(0)).map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok((p, f))
        })
        .unwrap();
    assert_eq!(pub_cnt, 1);
    assert_eq!(fts_cnt, 1);
    state.shutdown();
}

#[tokio::test]
async fn test_08_user_states_ratings_reviews_and_game_leaderboards() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (_author_id, author_tok) = register_user(&app, "author_eng", "Author", None).await;
    let (admin_id, admin_tok) = register_user(&app, "admin_eng", "Admin", None).await;
    let (p1_id, p1_tok) = register_user(&app, "player_one", "Grace", Some("Hopper")).await;
    let (p2_id, p2_tok) = register_user(&app, "player_two", "Alan", Some("Turing")).await;
    promote_admin(&state, admin_id);

    let (_, draft_game) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_game_req("Draft Game", "Not published yet game.", &["Trivia"]))).await;
    let draft_game_id = draft_game["creation_id"].as_str().unwrap();

    let (_, pub_book) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_book_req("State Book", "Book for testing states and ratings.", &["Sci-Fi"], None))).await;
    let book_id = pub_book["creation_id"].as_str().unwrap();
    submit_and_approve(&app, &author_tok, &admin_tok, book_id, "public").await;

    let (_, pub_game) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_game_req("Score Game", "Game for testing high score leaderboards.", &["Trivia"]))).await;
    let game_id = pub_game["creation_id"].as_str().unwrap();
    submit_and_approve(&app, &author_tok, &admin_tok, game_id, "public").await;

    // 1. Per-user states: bounds check, partial merge, save_or_install_count sync, /my-states filter.
    let (st, _) = http_json(&app, "PUT", &format!("/api/v1/catalog/{book_id}/state"), Some(&p1_tok), Some(json!({"progress_percent": 150.0}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = http_json(&app, "PUT", &format!("/api/v1/catalog/{book_id}/state"), Some(&p1_tok), Some(json!({"progress_percent": -5.0}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, st1) = http_json(&app, "PUT", &format!("/api/v1/catalog/{book_id}/state"), Some(&p1_tok), Some(json!({"in_library": true, "favorite": true, "progress_percent": 42.5, "state_json": {"chapter": 1}}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(st1["in_library"], true);
    assert_eq!(st1["progress_percent"], 42.5);

    // Partial merge keeps in_library=true and favorite=true while updating progress_percent.
    let (st, st2) = http_json(&app, "PUT", &format!("/api/v1/catalog/{book_id}/state"), Some(&p1_tok), Some(json!({"progress_percent": 80.0}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(st2["in_library"], true);
    assert_eq!(st2["favorite"], true);
    assert_eq!(st2["progress_percent"], 80.0);

    let (_, cat_b) = http_json(&app, "GET", "/api/v1/catalog/books", Some(&p1_tok), None).await;
    assert_eq!(cat_b["items"][0]["save_or_install_count"], 1);

    // Toggle in_library -> false decrements save_or_install_count accurately.
    let (st, _) = http_json(&app, "PUT", &format!("/api/v1/catalog/{book_id}/state"), Some(&p1_tok), Some(json!({"in_library": false}))).await;
    assert_eq!(st, StatusCode::OK);
    let (_, cat_b2) = http_json(&app, "GET", "/api/v1/catalog/books", Some(&p1_tok), None).await;
    assert_eq!(cat_b2["items"][0]["save_or_install_count"], 0);

    let (st, my_st) = http_json(&app, "GET", "/api/v1/catalog/my-states?kind=book", Some(&p1_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(my_st["items"].as_array().unwrap().len(), 1);

    // 2. Ratings & reviews: 1..=5 bounds, draft rejected, upsert dedup, display name & avg sync.
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{book_id}/reviews"), Some(&p1_tok), Some(json!({"rating": 0, "body": "bad"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{book_id}/reviews"), Some(&p1_tok), Some(json!({"rating": 6, "body": "bad"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{draft_game_id}/reviews"), Some(&p1_tok), Some(json!({"rating": 5, "body": "draft"}))).await;
    assert_eq!(st, StatusCode::CONFLICT);

    let (st, rev1) = http_json(&app, "POST", &format!("/api/v1/catalog/{book_id}/reviews"), Some(&p1_tok), Some(json!({"rating": 3, "title": "Nice", "body": "First pass"}))).await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(rev1["author_display_name"], "Grace Hopper");
    // Upsert same user -> rating_count stays 1, avg updates to 5.0.
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{book_id}/reviews"), Some(&p1_tok), Some(json!({"rating": 5, "title": "Loved", "body": "Revised review"}))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{book_id}/reviews"), Some(&p2_tok), Some(json!({"rating": 3, "body": "Solid"}))).await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, revs) = http_json(&app, "GET", &format!("/api/v1/catalog/{book_id}/reviews"), Some(&p1_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(revs["rating_count"], 2);
    assert!((revs["rating_avg"].as_f64().unwrap() - 4.0).abs() < 0.01);

    // 3. Game scores & leaderboards: reject non-game/draft, retain higher score, tie-break order.
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{book_id}/scores"), Some(&p1_tok), Some(json!({"score": 500}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{draft_game_id}/scores"), Some(&p1_tok), Some(json!({"score": 500}))).await;
    assert_eq!(st, StatusCode::CONFLICT);

    let (st, s1) = http_json(&app, "POST", &format!("/api/v1/catalog/{game_id}/scores"), Some(&p1_tok), Some(json!({"score": 900, "metadata": {"level": 3}}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s1["score"], 900);
    assert_eq!(s1["rank"], 1);
    assert_eq!(s1["display_name"], "Grace Hopper");

    // Lower score MUST NOT overwrite higher best_score!
    let (st, s1_low) = http_json(&app, "POST", &format!("/api/v1/catalog/{game_id}/scores"), Some(&p1_tok), Some(json!({"score": 250}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s1_low["score"], 900);

    tokio::time::sleep(Duration::from_millis(5)).await;
    let (st, s2) = http_json(&app, "POST", &format!("/api/v1/catalog/{game_id}/scores"), Some(&p2_tok), Some(json!({"score": 900}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s2["rank"], 2, "Earlier achieved_at_ms must win tie-break");

    let (st, lb) = http_json(&app, "GET", &format!("/api/v1/catalog/{game_id}/leaderboard"), Some(&p1_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let entries = lb["items"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["user_id"], p1_id as i64);
    assert_eq!(entries[0]["rank"], 1);
    assert_eq!(entries[1]["user_id"], p2_id as i64);
    assert_eq!(entries[1]["rank"], 2);
    state.shutdown();
}

async fn ws_connect_and_handshake(
    addr: std::net::SocketAddr,
    token: &str,
    nonce: u64,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let ws_url = format!("ws://{addr}/api/v1/ws?token={token}");
    let (mut ws, _) = connect_async(&ws_url).await.expect("WS connect");
    let ping = serde_json::to_string(&WsClientFrame::Ping { nonce }).unwrap();
    ws.send(TungsteniteMessage::Text(ping.into())).await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = tokio::time::timeout(remaining, ws.next())
            .await
            .expect("timed out waiting for Pong")
            .expect("ws closed")
            .expect("ws err");
        if let TungsteniteMessage::Text(txt) = msg {
            if let Ok(WsServerFrame::Pong { nonce: n }) = serde_json::from_str(&txt) {
                if n == nonce {
                    return ws;
                }
            }
        }
    }
}

async fn ws_next_creation_event(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    timeout_ms: u64,
) -> Option<UpdateEvent> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, ws.next()).await {
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
            _ => return None,
        }
    }
}

#[tokio::test]
async fn test_09_live_websocket_owner_only_and_public_catalog_events() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (alice_id, alice_tok) = register_user(&app, "alice_ws", "Alice", None).await;
    let (_bob_id, bob_tok) = register_user(&app, "bob_ws", "Bob", None).await;
    let (admin_id, admin_tok) = register_user(&app, "admin_ws", "Admin", None).await;
    promote_admin(&state, admin_id);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_app = app.clone();
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, serve_app).await;
    });

    let mut alice_ws = ws_connect_and_handshake(addr, &alice_tok, 1001).await;
    let mut bob_ws = ws_connect_and_handshake(addr, &bob_tok, 2002).await;

    // 1. Create draft -> Alice gets UserCreationChanged; Bob gets ZERO creation/catalog events.
    let (st, created) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_book_req("WS Live Book", "Testing realtime WebSocket broadcasts end to end.", &["Sci-Fi"], None)),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let cid = created["creation_id"].as_str().unwrap().to_string();

    let ev = ws_next_creation_event(&mut alice_ws, 2000).await.expect("Alice draft WS event");
    assert!(matches!(ev, UpdateEvent::UserCreationChanged { owner_user_id, .. } if owner_user_id == alice_id as i64));
    assert!(ws_next_creation_event(&mut bob_ws, 250).await.is_none(), "Bob must NOT receive owner draft event");

    // 2. Submit & Approve as UNLISTED -> Alice gets UserCreationChanged; Bob gets ZERO PublicCatalogChanged!
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/submit-review"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let _ = ws_next_creation_event(&mut alice_ws, 2000).await.expect("Alice submit event");
    assert!(ws_next_creation_event(&mut bob_ws, 200).await.is_none());

    let (st, _) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{cid}/review"),
        Some(&admin_tok),
        Some(json!({"decision":"approve","visibility":"unlisted"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let _ = ws_next_creation_event(&mut alice_ws, 2000).await.expect("Alice unlisted approve event");
    assert!(ws_next_creation_event(&mut bob_ws, 250).await.is_none(), "Unlisted approval must NOT emit PublicCatalogChanged");

    // 3. Re-submit and Approve as PUBLIC -> Bob receives PublicCatalogChanged { action: "published" }!
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/submit-review"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let _ = ws_next_creation_event(&mut alice_ws, 2000).await;

    let (st, _) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{cid}/review"),
        Some(&admin_tok),
        Some(json!({"decision":"approve","visibility":"public"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let bob_pub = ws_next_creation_event(&mut bob_ws, 2000).await.expect("Bob published catalog event");
    assert!(
        matches!(bob_pub, UpdateEvent::PublicCatalogChanged { ref creation_id, ref action, .. } if creation_id == &cid && action == "published"),
        "expected published PublicCatalogChanged, got {bob_pub:?}"
    );
    // Drain Alice's 2 events (UserCreationChanged + PublicCatalogChanged).
    let _ = ws_next_creation_event(&mut alice_ws, 1000).await;
    let _ = ws_next_creation_event(&mut alice_ws, 1000).await;

    // 4. Live-edit + Re-submit + Re-approve Public -> Bob receives PublicCatalogChanged { action: "updated" }!
    let (st, _) = http_json(&app, "PUT", &format!("/api/v1/creations/{cid}"), Some(&alice_tok), Some(json!({"title":"WS Live Book v2"}))).await;
    assert_eq!(st, StatusCode::OK);
    let _ = ws_next_creation_event(&mut alice_ws, 1000).await;
    assert!(ws_next_creation_event(&mut bob_ws, 200).await.is_none(), "Live-edit PUT must NOT emit PublicCatalogChanged");

    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/submit-review"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let _ = ws_next_creation_event(&mut alice_ws, 1000).await;

    let (st, _) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{cid}/review"),
        Some(&admin_tok),
        Some(json!({"decision":"approve","visibility":"public"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let bob_upd = ws_next_creation_event(&mut bob_ws, 2000).await.expect("Bob updated catalog event");
    assert!(
        matches!(bob_upd, UpdateEvent::PublicCatalogChanged { ref creation_id, ref action, .. } if creation_id == &cid && action == "updated"),
        "expected updated PublicCatalogChanged, got {bob_upd:?}"
    );

    // 5. Unpublish -> Bob receives PublicCatalogChanged { action: "removed" }!
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/unpublish"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let bob_rem = ws_next_creation_event(&mut bob_ws, 2000).await.expect("Bob removed catalog event");
    assert!(
        matches!(bob_rem, UpdateEvent::PublicCatalogChanged { ref creation_id, ref action, .. } if creation_id == &cid && action == "removed"),
        "expected removed PublicCatalogChanged, got {bob_rem:?}"
    );

    // 6. Deleting an unpublished/draft item emits NO PublicCatalogChanged; deleting a public item emits "removed".
    let (st, _) = http_json(&app, "DELETE", &format!("/api/v1/creations/{cid}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ws_next_creation_event(&mut bob_ws, 200).await.is_none(), "Deleting non-public item must NOT emit PublicCatalogChanged");

    let (_, c2) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_music_req("WS Delete Track", "Published track that will be deleted.", &["Electronic"], None)),
    )
    .await;
    let cid2 = c2["creation_id"].as_str().unwrap().to_string();
    submit_and_approve(&app, &alice_tok, &admin_tok, &cid2, "public").await;
    let bob_pub2 = ws_next_creation_event(&mut bob_ws, 2000).await.expect("Bob published cid2");
    assert!(matches!(bob_pub2, UpdateEvent::PublicCatalogChanged { ref creation_id, ref action, .. } if creation_id == &cid2 && action == "published"));

    let (st, _) = http_json(&app, "DELETE", &format!("/api/v1/creations/{cid2}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let bob_del2 = ws_next_creation_event(&mut bob_ws, 2000).await.expect("Bob deleted public cid2");
    assert!(
        matches!(bob_del2, UpdateEvent::PublicCatalogChanged { ref creation_id, ref action, .. } if creation_id == &cid2 && action == "removed"),
        "expected removed PublicCatalogChanged on delete, got {bob_del2:?}"
    );

    server_handle.abort();
    state.shutdown();
}

#[tokio::test]
async fn test_10_unlisted_live_edit_preservation_and_cas_media_hardening() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (_alice_id, alice_tok) = register_user(&app, "alice_h10", "Alice", None).await;
    let (_bob_id, bob_tok) = register_user(&app, "bob_h10", "Bob", None).await;
    let (_charlie_id, charlie_tok) = register_user(&app, "charlie_h10", "Charlie", None).await;
    let (admin_id, admin_tok) = register_user(&app, "admin_h10", "Admin", None).await;
    promote_admin(&state, admin_id);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_app = app.clone();
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, serve_app).await;
    });
    let mut bob_ws = ws_connect_and_handshake(addr, &bob_tok, 9101).await;

    // 1. Unlisted creation live-edit re-approval without explicit visibility MUST stay unlisted.
    let mut unlisted_req = sample_book_req(
        "Unlisted Manual v1",
        "Private unlisted manual that must never escalate to public on edit.",
        &["Manual"],
        None,
    );
    unlisted_req["visibility"] = json!("unlisted");
    let (st, created) = http_json(&app, "POST", "/api/v1/creations", Some(&alice_tok), Some(unlisted_req)).await;
    assert_eq!(st, StatusCode::CREATED);
    let cid = created["creation_id"].as_str().unwrap().to_string();

    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/submit-review"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, app1) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{cid}/review"),
        Some(&admin_tok),
        Some(json!({ "decision": "approve" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(app1["visibility"], "unlisted");

    // Live-edit v2 omitting visibility, empty-body submit, admin approves omitting visibility -> STILL unlisted!
    let (st, _) = http_json(
        &app,
        "PUT",
        &format!("/api/v1/creations/{cid}"),
        Some(&alice_tok),
        Some(json!({ "title": "Unlisted Manual v2" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/submit-review"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, app2) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{cid}/review"),
        Some(&admin_tok),
        Some(json!({ "decision": "approve" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(app2["visibility"], "unlisted", "live-edit re-approval must preserve unlisted");
    assert_eq!(app2["published_version"], 2);

    let (cat_cnt, fts_cnt): (i64, i64) = state
        .db
        .pool()
        .with_reader(|conn| {
            let c: i64 = conn.query_row("SELECT COUNT(*) FROM public_catalog_entries", [], |r| r.get(0)).map_err(|e| TelegramError::Database(e.to_string()))?;
            let f: i64 = conn.query_row("SELECT COUNT(*) FROM public_catalog_fts", [], |r| r.get(0)).map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok((c, f))
        })
        .unwrap();
    assert_eq!(cat_cnt, 0);
    assert_eq!(fts_cnt, 0);
    assert!(
        ws_next_creation_event(&mut bob_ws, 250).await.is_none(),
        "Unlisted live-edit re-approval must emit ZERO PublicCatalogChanged WS events"
    );

    // Explicit upgrade to public still works cleanly.
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/submit-review"), Some(&alice_tok), Some(json!({"visibility":"public"}))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, app3) = http_json(&app, "POST", &format!("/api/v1/creations/{cid}/review"), Some(&admin_tok), Some(json!({"decision":"approve"}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(app3["visibility"], "public");
    let pub_ev = ws_next_creation_event(&mut bob_ws, 2000).await.expect("explicit upgrade publishes");
    assert!(matches!(pub_ev, UpdateEvent::PublicCatalogChanged { ref action, .. } if action == "published"));

    // 2. Pre-upload SHA-256 CAS ACL squatting MUST fail closed (403 on /media/{id} & /meta after victim uploads).
    let future_secret = b"TOP_SECRET_VICTIM_PAYROLL_BYTES_2026_UNIQUE";
    let digest = Sha256::digest(future_secret);
    let future_hash: String = digest.iter().map(|b| format!("{b:02x}")).collect();

    let (_, squat_book) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_book_req("Squatter Book", "References future victim sha256 before upload.", &["Sci-Fi"], Some(&future_hash))),
    )
    .await;
    let squat_id = squat_book["creation_id"].as_str().unwrap();
    submit_and_approve(&app, &alice_tok, &admin_tok, squat_id, "public").await;

    let squat_asset_rows: i64 = state
        .db
        .pool()
        .with_reader(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM creation_public_assets WHERE LOWER(media_id) = LOWER(?1)",
                rusqlite::params![future_hash],
                |r| r.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert_eq!(squat_asset_rows, 0, "nonexistent future SHA-256 must NOT enter creation_public_assets");

    // Victim Bob now uploads the private secret matching future_hash.
    let (st, up_res) = http_raw(
        &app,
        "POST",
        "/api/v1/media/upload?filename=payroll.pdf&mime_type=application/pdf&is_voice=false",
        Some(&bob_tok),
        future_secret.to_vec(),
        Some("application/pdf"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let uploaded_id = serde_json::from_slice::<Value>(&up_res).unwrap()["media_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(uploaded_id, future_hash);

    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{future_hash}"), Some(&alice_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "squatter Alice must get 403 on victim blob");
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{future_hash}/meta"), Some(&alice_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "squatter Alice must get 403 on victim meta");
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{future_hash}"), Some(&charlie_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "Charlie must get 403 on victim blob");

    // 3. Uppercase hex media_id exfiltration guard: attaching Bob's hash in UPPERCASE inserts 0 rows and returns 403.
    let upper_hash = future_hash.to_ascii_uppercase();
    let (_, upper_book) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_book_req("Upper Exfil Book", "Attempts uppercase hex bypass on Bob's uploaded blob.", &["Sci-Fi"], Some(&upper_hash))),
    )
    .await;
    let upper_id = upper_book["creation_id"].as_str().unwrap();
    submit_and_approve(&app, &alice_tok, &admin_tok, upper_id, "public").await;

    let upper_asset_rows: i64 = state
        .db
        .pool()
        .with_reader(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM creation_public_assets WHERE LOWER(media_id) = LOWER(?1)",
                rusqlite::params![future_hash],
                |r| r.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert_eq!(upper_asset_rows, 0, "uppercase hex exfil must insert 0 creation_public_assets rows");
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{future_hash}"), Some(&alice_tok), vec![], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Legitimate owner referencing own uploaded blob in UPPERCASE normalizes to lowercase and works.
    let (st, alice_up) = http_raw(
        &app,
        "POST",
        "/api/v1/media/upload?filename=own.png&mime_type=image/png&is_voice=false",
        Some(&alice_tok),
        b"ALICE_LEGIT_UPPERCASE_COVER_BYTES".to_vec(),
        Some("image/png"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let alice_mid = serde_json::from_slice::<Value>(&alice_up).unwrap()["media_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, own_upper_book) = http_json(
        &app,
        "POST",
        "/api/v1/creations",
        Some(&alice_tok),
        Some(sample_book_req("Own Upper Cover", "Owner passes own SHA-256 in uppercase.", &["Sci-Fi"], Some(&alice_mid.to_ascii_uppercase()))),
    )
    .await;
    let own_upper_id = own_upper_book["creation_id"].as_str().unwrap();
    submit_and_approve(&app, &alice_tok, &admin_tok, own_upper_id, "public").await;
    let (st, _) = http_raw(&app, "GET", &format!("/api/v1/media/{alice_mid}"), Some(&bob_tok), vec![], None).await;
    assert_eq!(st, StatusCode::OK, "owner uppercase SHA-256 should normalize and authorize lowercase download");

    server_handle.abort();
    state.shutdown();
}

#[tokio::test]
async fn test_11_review_xss_bounds_malformed_action_json_stale_notes_and_kind_oracle() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (_author_id, author_tok) = register_user(&app, "author_h11", "Author", None).await;
    let (admin_id, admin_tok) = register_user(&app, "admin_h11", "Admin", None).await;
    let (_reader_id, reader_tok) = register_user(&app, "reader_h11", "Reader", None).await;
    promote_admin(&state, admin_id);

    // Create draft book & draft game for oracle + FSM tests.
    let (_, draft_book) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_book_req("Draft Oracle Book", "Private draft book for oracle and FSM tests.", &["Sci-Fi"], None))).await;
    let draft_book_id = draft_book["creation_id"].as_str().unwrap().to_string();
    let (_, draft_game) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(sample_game_req("Draft Oracle Game", "Private draft game for oracle tests.", &["Trivia"]))).await;
    let draft_game_id = draft_game["creation_id"].as_str().unwrap().to_string();

    // 1. Private creation kind oracle closed on leaderboard & score endpoints.
    let (st, _) = http_json(&app, "GET", &format!("/api/v1/catalog/{draft_book_id}/leaderboard"), Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "private non-game leaderboard must return 404");
    let (st, _) = http_json(&app, "GET", &format!("/api/v1/catalog/{draft_game_id}/leaderboard"), Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "private game leaderboard must return 404");
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{draft_book_id}/scores"), Some(&reader_tok), Some(json!({"score": 100}))).await;
    assert_eq!(st, StatusCode::CONFLICT, "private non-game score submit must return 409 matching draft game");
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/catalog/{draft_game_id}/scores"), Some(&reader_tok), Some(json!({"score": 100}))).await;
    assert_eq!(st, StatusCode::CONFLICT);

    // 2. submit-review rejects visibility == "private" with 400.
    let (st, priv_err) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{draft_book_id}/submit-review"),
        Some(&author_tok),
        Some(json!({ "visibility": "private" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(priv_err["error"], "invalid_visibility");

    // 3. Stale moderator rejection_reason & review_notes containing <script>/javascript: MUST NOT brick resubmission.
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{draft_book_id}/submit-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{draft_book_id}/review"),
        Some(&admin_tok),
        Some(json!({
            "decision": "reject",
            "reason": "Remove <script> tag mentions from appendix",
            "notes": "Flagged javascript: string during moderation"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Direct resubmit (without PUT) succeeds with empty body and clears stale moderation fields.
    let (st, direct_resub) = http_json(&app, "POST", &format!("/api/v1/creations/{draft_book_id}/submit-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK, "direct resubmit after XSS admin reason must succeed: {direct_resub}");
    assert!(direct_resub["creation"]["rejection_reason"].is_null());
    assert!(direct_resub["creation"]["review_notes"].is_null());

    // Reject again with XSS notes -> PUT clears BOTH rejection_reason & review_notes -> empty-body resubmit succeeds.
    let (st, _) = http_json(
        &app,
        "POST",
        &format!("/api/v1/creations/{draft_book_id}/review"),
        Some(&admin_tok),
        Some(json!({
            "decision": "reject",
            "reason": "Check <script> in chapter 2",
            "notes": "Contains <script> note"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, fixed_put) = http_json(
        &app,
        "PUT",
        &format!("/api/v1/creations/{draft_book_id}"),
        Some(&author_tok),
        Some(json!({ "subtitle": "Cleaned Edition" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(fixed_put["review_status"], "draft");
    assert!(fixed_put["rejection_reason"].is_null());
    assert!(fixed_put["review_notes"].is_null());
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{draft_book_id}/submit-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);

    // 4. withdraw-review & unpublish reject malformed JSON `{bad` (400) without mutating state, accept empty & `{}`.
    let (st, _) = http_raw(&app, "POST", &format!("/api/v1/creations/{draft_book_id}/withdraw-review"), Some(&author_tok), b"{bad".to_vec(), Some("application/json")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = http_raw(&app, "POST", &format!("/api/v1/creations/{draft_book_id}/withdraw-review"), Some(&author_tok), b"{bad".to_vec(), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, still_pending) = http_json(&app, "GET", &format!("/api/v1/creations/{draft_book_id}"), Some(&author_tok), None).await;
    assert_eq!(still_pending["review_status"], "pending_review", "malformed withdraw-review must not mutate state");

    let (st, _) = http_raw(&app, "POST", &format!("/api/v1/creations/{draft_book_id}/withdraw-review"), Some(&author_tok), b"{}".to_vec(), Some("application/json")).await;
    assert_eq!(st, StatusCode::OK);
    // Resubmit & approve Public so we can test published non-game oracle, public reviews XSS/bounds, and unpublish `{bad`.
    submit_and_approve(&app, &author_tok, &admin_tok, &draft_book_id, "public").await;

    // Published non-game leaderboard & scores return 400 not_a_game.
    let (st, err_lb) = http_json(&app, "GET", &format!("/api/v1/catalog/{draft_book_id}/leaderboard"), Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err_lb["error"], "not_a_game");
    let (st, err_sc) = http_json(&app, "POST", &format!("/api/v1/catalog/{draft_book_id}/scores"), Some(&reader_tok), Some(json!({"score": 100}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err_sc["error"], "not_a_game");

    // 5. Public reviews reject blank body, oversized fields, and raw/obfuscated XSS in title & body.
    for bad_review in [
        json!({ "rating": 4, "body": "   \t\n " }),
        json!({ "rating": 5, "title": "<script>alert(1)</script>", "body": "Safe body" }),
        json!({ "rating": 5, "title": "Safe title", "body": "Click javascript:alert(1)" }),
        json!({ "rating": 5, "title": "Obfuscated", "body": "Try ja\u{200B}va\tscript:evil() or vb\u{202E}script:x" }),
        json!({ "rating": 5, "title": "T".repeat(121), "body": "Safe body" }),
        json!({ "rating": 5, "body": "B".repeat(4001) }),
    ] {
        let (st, resp) = http_json(&app, "POST", &format!("/api/v1/catalog/{draft_book_id}/reviews"), Some(&reader_tok), Some(bad_review)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "unsafe/invalid review must fail 400, got {resp}");
    }

    // Malformed `{bad` on unpublish returns 400 without mutating state; `{}` succeeds.
    let (st, _) = http_raw(&app, "POST", &format!("/api/v1/creations/{draft_book_id}/unpublish"), Some(&author_tok), b"{bad".to_vec(), Some("application/json")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, still_pub) = http_json(&app, "GET", &format!("/api/v1/creations/{draft_book_id}"), Some(&author_tok), None).await;
    assert_eq!(still_pub["review_status"], "published", "malformed unpublish must not mutate state");
    let (st, _) = http_raw(&app, "POST", &format!("/api/v1/creations/{draft_book_id}/unpublish"), Some(&author_tok), b"{}".to_vec(), Some("application/json")).await;
    assert_eq!(st, StatusCode::OK);

    // 6. Live-edited v2 withdraw-review keeps v1 frozen & live; double-DELETE returns 404 without bumping revision.
    submit_and_approve(&app, &author_tok, &admin_tok, &draft_book_id, "public").await;
    let (st, _) = http_json(&app, "PUT", &format!("/api/v1/creations/{draft_book_id}"), Some(&author_tok), Some(json!({"title": "Unapproved Live V2"}))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = http_json(&app, "POST", &format!("/api/v1/creations/{draft_book_id}/submit-review"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, wd_live) = http_raw(&app, "POST", &format!("/api/v1/creations/{draft_book_id}/withdraw-review"), Some(&author_tok), vec![], None).await;
    assert_eq!(st, StatusCode::OK);
    let wd_json: Value = serde_json::from_slice(&wd_live).unwrap();
    assert_eq!(wd_json["review_status"], "draft");
    assert_eq!(wd_json["has_live_published_version"], true);
    let (st, reader_frozen) = http_json(&app, "GET", &format!("/api/v1/creations/{draft_book_id}"), Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(reader_frozen["title"], "Draft Oracle Book", "v1 must stay frozen after v2 withdraw-review");

    let (st, del1) = http_json(&app, "DELETE", &format!("/api/v1/creations/{draft_game_id}"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let rev_after_del1 = del1["cloud_revision"].as_i64().unwrap();
    let (st, _) = http_json(&app, "DELETE", &format!("/api/v1/creations/{draft_game_id}"), Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "double DELETE must return 404");
    let (st, sync_check) = http_json(&app, "GET", "/api/v1/creations/sync?since_revision=0", Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sync_check["latest_revision"], rev_after_del1, "double DELETE must not bump cloud_revision");

    state.shutdown();
}

#[tokio::test]
async fn test_12_exact_genre_nul_byte_and_search_precision_hardening() {
    let (_dir, _cfg, state, app) = make_temp_state();
    let (author_id, author_tok) = register_user(&app, "author_h12", "Author", None).await;
    let (admin_id, admin_tok) = register_user(&app, "admin_h12", "Admin", None).await;
    let (_reader_id, reader_tok) = register_user(&app, "reader_h12", "Reader", None).await;
    promote_admin(&state, admin_id);

    let items_spec = vec![
        sample_music_req("Pure Acoustic Pop", "Warm acoustic pop album recorded live in studio.", &["Pop"], None),
        sample_music_req("Cyber赛博交响曲", "Futuristic synthwave symphony with neon melodies.", &["K-Pop", "Synthwave"], None),
        sample_book_req("Starlight Odyssey", "Exploring 100 galaxies across the outer rim.", &["Sci-Fi", "Fantasy"], None),
        sample_book_req("Starlight% Special", "Guaranteed 100% pure sci-fi anthology with literal percent.", &["科幻"], None),
        sample_book_req("硬科幻纪元", "关于轨道工程与深空物理的硬科幻作品。", &["硬科幻"], None),
        sample_book_req("Cyber 星际", "Cross-language sci-fi tale of orbital voyages.", &["Sci-Fi"], None),
        sample_book_req("Cyber Space", "Pure English cyber exploration without Chinese characters.", &["Sci-Fi"], None),
        sample_book_req("普通故事", "一个赛博世界中的普通日常生活纪事。", &["小说"], None),
        sample_app_req("Starlight Studio", "Apple-style productivity studio for writers.", &["Productivity"], None),
    ];

    let mut ids = Vec::new();
    for spec in items_spec {
        let (st, created) = http_json(&app, "POST", "/api/v1/creations", Some(&author_tok), Some(spec)).await;
        assert_eq!(st, StatusCode::CREATED);
        let cid = created["creation_id"].as_str().unwrap().to_string();
        submit_and_approve(&app, &author_tok, &admin_tok, &cid, "public").await;
        ids.push(cid);
    }
    let pop_id = &ids[0];
    let cyber_symphony_id = &ids[1];
    let starlight_pct_id = &ids[3];
    let cyber_star_id = &ids[5];
    let app_id = &ids[8];

    let now_ms = chrono::Utc::now().timestamp_millis();
    state.db.pool().with_writer(|conn| {
        conn.execute(
            "INSERT INTO mini_apps (app_id, bot_user_id, bot_username, short_name, title, description, category, web_url, is_published, created_at, updated_at) VALUES ('ma_prod_h12', ?1, 'ProdBot', 'planner', 'Planner MiniApp', 'Productivity planner', 'Productivity', 'https://example.com/planner', 1, ?2, ?2)",
            rusqlite::params![author_id as i64, now_ms],
        ).map_err(|e| TelegramError::Database(e.to_string()))?;
        Ok(())
    }).unwrap();

    // 1. Exact genre matching (no substring or cross-genre-boundary false positives).
    let (st, pop_res) = http_json(&app, "GET", "/api/v1/catalog?genre=Pop", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let pop_items = pop_res["items"].as_array().unwrap();
    assert_eq!(pop_items.len(), 1, "genre=Pop must exclude K-Pop, got {pop_res}");
    assert_eq!(pop_items[0]["creation_id"], *pop_id);

    let (st, boundary_res) = http_json(&app, "GET", "/api/v1/catalog?genre=Fi%20Fan", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(boundary_res["total"], 0, "genre='Fi Fan' must NOT match ['Sci-Fi', 'Fantasy']");

    let (st, zh_genre_res) = http_json(&app, "GET", "/api/v1/catalog?genre=%E7%A7%91%E5%B9%BB", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let zh_items = zh_genre_res["items"].as_array().unwrap();
    assert_eq!(zh_items.len(), 1, "genre=科幻 must exclude 硬科幻");
    assert_eq!(zh_items[0]["creation_id"], *starlight_pct_id);

    // App Store genre parity across apps & mini_apps.
    let (st, appstore_prod_prefix) = http_json(&app, "GET", "/api/v1/catalog/appstore?genre=Prod", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(appstore_prod_prefix["apps"].as_array().unwrap().is_empty());
    assert!(appstore_prod_prefix["mini_apps"].as_array().unwrap().is_empty());

    let (st, appstore_prod_exact) = http_json(&app, "GET", "/api/v1/catalog/appstore?genre=Productivity", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(appstore_prod_exact["apps"].as_array().unwrap().len(), 1);
    assert_eq!(appstore_prod_exact["apps"][0]["creation_id"], *app_id);
    assert_eq!(appstore_prod_exact["mini_apps"].as_array().unwrap().len(), 1);

    // 2. NUL-byte search query short-circuits cleanly across catalog, recommend, appstore, and /creations/my.
    for uri in [
        "/api/v1/catalog?q=%00",
        "/api/v1/catalog?q=%00nonexistent_xyz",
    ] {
        let (st, res) = http_json(&app, "GET", uri, Some(&reader_tok), None).await;
        assert_eq!(st, StatusCode::OK, "{uri} failed: {res}");
        assert_eq!(res["total"], 0, "NUL-byte query {uri} must return total=0, got {res}");
        assert!(res["items"].as_array().unwrap().is_empty());
    }
    let (st, rec_nul) = http_json(&app, "GET", "/api/v1/catalog/recommend?q=%00", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(rec_nul["items"].as_array().unwrap().is_empty());

    let (st, appstore_nul) = http_json(&app, "GET", "/api/v1/catalog/appstore?q=%00", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(appstore_nul["apps"].as_array().unwrap().is_empty());
    assert!(appstore_nul["mini_apps"].as_array().unwrap().is_empty());
    assert!(appstore_nul["cross_vertical"].as_array().unwrap().is_empty());

    let (st, my_nul) = http_json(&app, "GET", "/api/v1/creations/my?q=%00", Some(&author_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(my_nul["items"].as_array().unwrap().is_empty());
    assert!(my_nul["latest_revision"].as_i64().unwrap() >= 9, "true latest_revision must be preserved");

    // 3. Literal wildcard precision ("Starlight%" & "100%" must NOT match "Starlight Odyssey" / "100 galaxies").
    let (st, pct_title) = http_json(&app, "GET", "/api/v1/catalog?q=Starlight%25", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let pct_items = pct_title["items"].as_array().unwrap();
    assert_eq!(pct_items.len(), 1, "Starlight% must match ONLY literal Starlight%, got {pct_title}");
    assert_eq!(pct_items[0]["creation_id"], *starlight_pct_id);

    let (st, pct_num) = http_json(&app, "GET", "/api/v1/catalog?q=100%25", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let pct_num_items = pct_num["items"].as_array().unwrap();
    assert_eq!(pct_num_items.len(), 1, "100% must match ONLY literal 100%, got {pct_num}");
    assert_eq!(pct_num_items[0]["creation_id"], *starlight_pct_id);

    // 4. Mixed EN+CJK precision ("Cyber 星" excludes "Cyber Space"; "Cyber赛博" excludes "普通故事").
    let (st, mix_space) = http_json(&app, "GET", "/api/v1/catalog?q=Cyber%20%E6%98%9F", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let mix_space_ids: Vec<&str> = mix_space["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["creation_id"].as_str().unwrap())
        .collect();
    assert_eq!(mix_space_ids, vec![cyber_star_id.as_str()], "'Cyber 星' must match only Cyber 星际");

    let (st, mix_joined) = http_json(&app, "GET", "/api/v1/catalog?q=Cyber%E8%B5%9B%E5%8D%9A", Some(&reader_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let mix_joined_ids: Vec<&str> = mix_joined["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["creation_id"].as_str().unwrap())
        .collect();
    assert_eq!(mix_joined_ids, vec![cyber_symphony_id.as_str()], "'Cyber赛博' must match only Cyber赛博交响曲");

    // 5. Scoped trg_public_catalog_au: metric update keeps FTS intact & exact 1:1 parity.
    let (st, _) = http_json(
        &app,
        "POST",
        &format!("/api/v1/catalog/{pop_id}/reviews"),
        Some(&reader_tok),
        Some(json!({ "rating": 5, "body": "Great acoustic sound!" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (pub_cnt, fts_cnt): (i64, i64) = state
        .db
        .pool()
        .with_reader(|conn| {
            let p: i64 = conn.query_row("SELECT COUNT(*) FROM public_catalog_entries", [], |r| r.get(0)).map_err(|e| TelegramError::Database(e.to_string()))?;
            let f: i64 = conn.query_row("SELECT COUNT(*) FROM public_catalog_fts", [], |r| r.get(0)).map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok((p, f))
        })
        .unwrap();
    assert_eq!(pub_cnt, 9);
    assert_eq!(fts_cnt, 9);

    state.shutdown();
}

