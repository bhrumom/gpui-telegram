#![allow(dead_code)]

use futures::{SinkExt, StreamExt};
use gpui_telegram_core::error::TelegramError;
pub use gpui_telegram_test_harness::tier4_e2e::{E2eSystemDriver, EphemeralTestServer};
use reqwest::Method;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

pub const CREATOR_STUDIO_BOT_UID: u64 = 777_020;
pub const DEFAULT_PASS: &str = "ApplePass!2026";

pub struct Actor {
    pub user_id: u64,
    pub username: String,
    pub token: String,
}

pub async fn boot_server() -> anyhow::Result<(EphemeralTestServer, E2eSystemDriver)> {
    let server = EphemeralTestServer::start().await?;
    let driver = E2eSystemDriver::new(server.base_url());
    Ok((server, driver))
}

/// Restarts `server` on the same SQLite WAL / CAS / Tantivy dir and returns a fresh driver
/// bound to the newly allocated ephemeral port.
pub async fn restart_server(server: &mut EphemeralTestServer) -> anyhow::Result<E2eSystemDriver> {
    server.restart().await?;
    Ok(E2eSystemDriver::new(server.base_url()))
}

pub fn unique_suffix() -> String {
    Uuid::new_v4().simple().to_string()[..10].to_string()
}

pub async fn register_actor(driver: &E2eSystemDriver, prefix: &str) -> anyhow::Result<Actor> {
    let username = format!("{prefix}_{}", unique_suffix());
    let (user_id, token) = driver.register_user(&username, DEFAULT_PASS).await?;
    Ok(Actor {
        user_id,
        username,
        token,
    })
}

pub async fn login_actor(
    driver: &E2eSystemDriver,
    username: &str,
    password: &str,
) -> anyhow::Result<Actor> {
    let (st, v) = driver
        .json_req(
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(json!({
                "username": username,
                "password": password,
                "device_name": "AppleSidebarE2e"
            })),
        )
        .await?;
    anyhow::ensure!(st == 200, "login failed with status {st}: {v}");
    let user_id = v["user_id"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("missing user_id in login response: {v}"))?;
    let token = v["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing token in login response: {v}"))?
        .to_string();
    Ok(Actor {
        user_id,
        username: username.to_string(),
        token,
    })
}

/// Elevates a registered user to server reviewer/admin (`users.is_support = 1`).
pub fn promote_user_to_admin(server: &EphemeralTestServer, user_id: u64) -> anyhow::Result<()> {
    let state = server
        .state()
        .ok_or_else(|| anyhow::anyhow!("EphemeralTestServer has no active AppState"))?;
    let updated = state.db.pool().with_writer(|conn| {
        conn.execute(
            "UPDATE users SET is_support = 1 WHERE user_id = ?1",
            [user_id as i64],
        )
        .map_err(|e| TelegramError::Database(e.to_string()))
    })?;
    anyhow::ensure!(
        updated == 1,
        "expected 1 user row updated for user_id={user_id}, updated={updated}"
    );
    Ok(())
}

/// Registers creator, consumer, and promoted admin reviewer on the given server.
pub async fn seed_creator_consumer_admin(
    server: &EphemeralTestServer,
    driver: &E2eSystemDriver,
) -> anyhow::Result<(Actor, Actor, Actor)> {
    let creator = register_actor(driver, "creator").await?;
    let consumer = register_actor(driver, "consumer").await?;
    let admin = register_actor(driver, "reviewer").await?;
    promote_user_to_admin(server, admin.user_id)?;
    Ok((creator, consumer, admin))
}

/// Pure-Rust RFC 3986 percent encoder for UTF-8/CJK and special query characters.
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for &b in input.as_bytes() {
        let is_unreserved =
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if is_unreserved {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Generates a unique valid 4x4 PNG whose pixels encode `tag` so SHA-256 `media_id` never collides.
pub fn make_unique_png_bytes(tag: &str) -> Vec<u8> {
    let mut img = image::RgbaImage::new(4, 4);
    let seed = format!("{tag}:{}", Uuid::new_v4().simple());
    let bytes = seed.as_bytes();
    for y in 0..4u32 {
        for x in 0..4u32 {
            let idx = ((y * 4 + x) as usize * 3) % bytes.len();
            let r = bytes[idx];
            let g = bytes[(idx + 1) % bytes.len()];
            let b = bytes[(idx + 2) % bytes.len()];
            img.put_pixel(x, y, image::Rgba([r, g, b, 255]));
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode unique test PNG");
    out.into_inner()
}

/// Generates a unique valid 44.1kHz 16-bit mono RIFF/WAVE PCM buffer (>0.1s) seeded by `tag`.
pub fn make_unique_wav_bytes(tag: &str) -> Vec<u8> {
    let sample_rate: u32 = 44_100;
    let num_samples: usize = 8_820; // 0.2s PCM audio
    let data_bytes: u32 = (num_samples * 2) as u32;
    let riff_size: u32 = 36 + data_bytes;
    let seed = format!("{tag}:{}", Uuid::new_v4().simple());
    let seed_bytes = seed.as_bytes();

    let mut wav = Vec::with_capacity(44 + data_bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());

    let freq = 220.0 + (seed_bytes[0] as f32 * 2.0);
    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let salt = if i < seed_bytes.len() {
            (seed_bytes[i] as i16) - 64
        } else {
            0
        };
        let s = ((2.0 * std::f32::consts::PI * freq * t).sin() * 12_000.0) as i16;
        wav.extend_from_slice(&s.saturating_add(salt).to_le_bytes());
    }
    wav
}

/// Uploads raw media bytes without `chat_id` so non-owners get 403 until linked creation publishes.
pub async fn upload_raw_media(
    driver: &E2eSystemDriver,
    token: &str,
    filename: &str,
    mime_type: &str,
    bytes: Vec<u8>,
) -> anyhow::Result<String> {
    let url = format!(
        "{}/api/v1/media/upload?filename={}&mime_type={}&is_voice=false",
        driver.base_url,
        percent_encode(filename),
        percent_encode(mime_type)
    );
    let resp = driver
        .http
        .post(&url)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, mime_type)
        .body(bytes)
        .send()
        .await?;
    let st = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    anyhow::ensure!(st == 201 || st == 200, "media upload failed {st}: {body}");
    let media_id = body["media_id"]
        .as_str()
        .or_else(|| body["file_id"].as_str())
        .ok_or_else(|| anyhow::anyhow!("missing media_id in upload response: {body}"))?;
    Ok(media_id.to_string())
}

pub async fn download_media(
    driver: &E2eSystemDriver,
    token: &str,
    media_id: &str,
) -> anyhow::Result<(u16, Vec<u8>)> {
    let url = format!("{}/api/v1/media/{}", driver.base_url, percent_encode(media_id));
    let resp = driver.http.get(&url).bearer_auth(token).send().await?;
    let st = resp.status().as_u16();
    let bytes = resp.bytes().await?.to_vec();
    Ok((st, bytes))
}

pub fn make_book_creation(
    title: &str,
    summary: &str,
    author: &str,
    language: &str,
    genres: &[&str],
    tags: &[&str],
    cover_media_id: Option<&str>,
) -> Value {
    json!({
        "kind": "book",
        "title": title,
        "subtitle": "Apple Books Edition",
        "summary": summary,
        "author_display_name": author,
        "language": language,
        "genres": genres,
        "tags": tags,
        "age_rating": "9+",
        "cover": {
            "gradient_start_hex": "#1B4332",
            "gradient_end_hex": "#081C15",
            "accent_hex": "#95D5B2",
            "icon_emoji": "📖",
            "media_file_id": cover_media_id
        },
        "payload": {
            "kind": "book",
            "author": author,
            "series_name": "Chronicles",
            "series_index": 1,
            "blurb": summary,
            "estimated_read_mins": 14,
            "word_count": 320,
            "chapters": [
                {
                    "chapter_id": "ch_01",
                    "chapter_number": 1,
                    "title": "Chapter 1: Departure",
                    "subtitle": "Into the quiet orbit",
                    "pages": [
                        {
                            "page_number": 1,
                            "blocks": [
                                { "type": "heading", "level": 1, "text": "Chapter 1: Departure" },
                                {
                                    "type": "paragraph",
                                    "text": "Beneath the translucent dome of Europa Station, Clara calibrated the starlight chronometer while distant ice rings caught the amber horizon of Jupiter."
                                },
                                {
                                    "type": "block_quote",
                                    "text": "Every map begins as a rumor drawn by travelers who refused to stop.",
                                    "attribution": "Navigator Log 04"
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    })
}

pub fn make_music_creation(
    title: &str,
    summary: &str,
    artist: &str,
    language: &str,
    genres: &[&str],
    tags: &[&str],
    cover_media_id: Option<&str>,
    audio_media_id: Option<&str>,
) -> Value {
    json!({
        "kind": "music",
        "title": title,
        "subtitle": "Spatial Synth EP",
        "summary": summary,
        "author_display_name": artist,
        "language": language,
        "genres": genres,
        "tags": tags,
        "age_rating": "4+",
        "cover": {
            "gradient_start_hex": "#FA243C",
            "gradient_end_hex": "#2A0A18",
            "accent_hex": "#FF8FA3",
            "icon_emoji": "🎵",
            "media_file_id": cover_media_id
        },
        "payload": {
            "kind": "music",
            "artist": artist,
            "album_title": format!("{title} Single"),
            "release_year": 2026,
            "primary_genre": genres.first().copied().unwrap_or("Electronic"),
            "mood_tags": ["Chill", "Focus"],
            "bpm": 118,
            "musical_key": "C major",
            "time_signature": "4/4",
            "total_duration_secs": 42,
            "tracks": [
                {
                    "track_id": "trk_01",
                    "track_number": 1,
                    "title": title,
                    "duration_secs": 42,
                    "bpm": 118,
                    "instruments": [
                        { "channel": 0, "name": "Warm Piano", "waveform": "piano", "gain": 0.8 },
                        { "channel": 1, "name": "Sub Bass", "waveform": "sine", "gain": 0.6 }
                    ],
                    "melody_notes": [
                        { "midi_note": 60, "start_beat": 0.0, "duration_beats": 1.0, "velocity": 96, "channel": 0 },
                        { "midi_note": 64, "start_beat": 1.0, "duration_beats": 1.0, "velocity": 92, "channel": 0 },
                        { "midi_note": 67, "start_beat": 2.0, "duration_beats": 1.0, "velocity": 98, "channel": 0 },
                        { "midi_note": 72, "start_beat": 3.0, "duration_beats": 1.0, "velocity": 100, "channel": 0 }
                    ],
                    "chord_progression": [
                        { "symbol": "Cmaj7", "start_beat": 0.0, "duration_beats": 4.0 }
                    ],
                    "lyrics": [
                        { "timestamp_ms": 0, "text": "Midnight glass reflections glow" },
                        { "timestamp_ms": 4000, "text": "Signals drifting over snow" }
                    ],
                    "audio_media_file_id": audio_media_id,
                    "waveform_5bit": []
                }
            ]
        }
    })
}

pub fn make_game_creation(
    title: &str,
    summary: &str,
    developer: &str,
    language: &str,
    genres: &[&str],
    tags: &[&str],
    cover_media_id: Option<&str>,
) -> Value {
    json!({
        "kind": "game",
        "title": title,
        "subtitle": "Apple Arcade Original",
        "summary": summary,
        "author_display_name": developer,
        "language": language,
        "genres": genres,
        "tags": tags,
        "age_rating": "4+",
        "cover": {
            "gradient_start_hex": "#007AFF",
            "gradient_end_hex": "#1B1464",
            "accent_hex": "#5AC8FA",
            "icon_emoji": "🎮",
            "media_file_id": cover_media_id
        },
        "payload": {
            "kind": "game",
            "developer": developer,
            "category": "trivia",
            "controls_hint": "Select the right option with keyboard 1-4 or click.",
            "rules_markdown": "Answer every cosmic trivia question accurately to climb the global leaderboard.",
            "engine_kind": "trivia_quiz",
            "supports_leaderboard": true,
            "estimated_session_mins": 5,
            "spec": {
                "type": "trivia_quiz",
                "time_limit_secs": 60,
                "questions": [
                    {
                        "question_id": "q_01",
                        "prompt": "Which moon of Jupiter has a subsurface liquid ocean beneath an icy crust?",
                        "choices": ["Europa", "Phobos", "Deimos", "Charon"],
                        "correct_index": 0,
                        "explanation": "Europa harbors a global salty ocean beneath its ice shell."
                    },
                    {
                        "question_id": "q_02",
                        "prompt": "What musical interval spans twelve semitones?",
                        "choices": ["Fifth", "Octave", "Third", "Seventh"],
                        "correct_index": 1,
                        "explanation": "An octave spans twelve semitones and doubles frequency."
                    }
                ]
            },
            "achievements": [
                {
                    "achievement_id": "ach_first_clear",
                    "title": "First Launch",
                    "description": "Complete your first trivia session.",
                    "icon_emoji": "🏆",
                    "target_score": 100
                }
            ]
        }
    })
}

pub fn make_app_creation(
    title: &str,
    summary: &str,
    developer: &str,
    language: &str,
    genres: &[&str],
    tags: &[&str],
    cover_media_id: Option<&str>,
    linked_miniapp: Option<Value>,
) -> Value {
    json!({
        "kind": "app",
        "title": title,
        "subtitle": "Featured Utility for macOS & Desktop",
        "summary": summary,
        "author_display_name": developer,
        "language": language,
        "genres": genres,
        "tags": tags,
        "age_rating": "4+",
        "cover": {
            "gradient_start_hex": "#0A84FF",
            "gradient_end_hex": "#003A75",
            "accent_hex": "#64D2FF",
            "icon_emoji": "📱",
            "media_file_id": cover_media_id
        },
        "payload": {
            "kind": "app",
            "developer_name": developer,
            "version_string": "1.0.0",
            "release_notes": "Initial Apple App Store release with cloud sync.",
            "category": "productivity",
            "long_description": format!("{summary} Built with clean Apple HIG ergonomics and offline-ready state synchronization."),
            "screenshots": [
                {
                    "title": "Workspace Overview",
                    "caption": "Clean glassmorphic dashboard.",
                    "media_file_id": cover_media_id,
                    "accent_hex": "#0A84FF"
                }
            ],
            "size_label": "4.2 MB",
            "price_label": "Free",
            "permissions": ["cloud_storage"],
            "linked_miniapp": linked_miniapp,
            "ui_schema_json": {
                "schema_version": 1,
                "layout": "dashboard",
                "widgets": [{ "type": "metric", "label": "Tasks", "value": "0" }]
            }
        }
    })
}

pub async fn create_creation_raw(
    driver: &E2eSystemDriver,
    token: &str,
    payload: Value,
) -> anyhow::Result<(u16, Value)> {
    driver
        .json_req(Method::POST, "/api/v1/creations", Some(token), Some(payload))
        .await
}

pub async fn create_creation_ok(
    driver: &E2eSystemDriver,
    token: &str,
    payload: Value,
) -> anyhow::Result<Value> {
    let (st, body) = create_creation_raw(driver, token, payload).await?;
    anyhow::ensure!(st == 201 || st == 200, "POST /api/v1/creations failed {st}: {body}");
    anyhow::ensure!(
        body["creation_id"].as_str().is_some_and(|s| !s.is_empty()),
        "missing creation_id: {body}"
    );
    Ok(body)
}

pub async fn get_creation_raw(
    driver: &E2eSystemDriver,
    token: Option<&str>,
    creation_id: &str,
) -> anyhow::Result<(u16, Value)> {
    driver
        .json_req(
            Method::GET,
            &format!("/api/v1/creations/{}", percent_encode(creation_id)),
            token,
            None,
        )
        .await
}

pub async fn update_creation_raw(
    driver: &E2eSystemDriver,
    token: &str,
    creation_id: &str,
    payload: Value,
) -> anyhow::Result<(u16, Value)> {
    driver
        .json_req(
            Method::PUT,
            &format!("/api/v1/creations/{}", percent_encode(creation_id)),
            Some(token),
            Some(payload),
        )
        .await
}

pub async fn delete_creation_raw(
    driver: &E2eSystemDriver,
    token: &str,
    creation_id: &str,
) -> anyhow::Result<(u16, Value)> {
    driver
        .json_req(
            Method::DELETE,
            &format!("/api/v1/creations/{}", percent_encode(creation_id)),
            Some(token),
            None,
        )
        .await
}

pub async fn submit_review_raw(
    driver: &E2eSystemDriver,
    token: &str,
    creation_id: &str,
) -> anyhow::Result<(u16, Value)> {
    driver
        .json_req(
            Method::POST,
            &format!("/api/v1/creations/{}/submit-review", percent_encode(creation_id)),
            Some(token),
            Some(json!({})),
        )
        .await
}

pub async fn withdraw_review_raw(
    driver: &E2eSystemDriver,
    token: &str,
    creation_id: &str,
) -> anyhow::Result<(u16, Value)> {
    driver
        .json_req(
            Method::POST,
            &format!("/api/v1/creations/{}/withdraw-review", percent_encode(creation_id)),
            Some(token),
            Some(json!({})),
        )
        .await
}

pub async fn unpublish_raw(
    driver: &E2eSystemDriver,
    token: &str,
    creation_id: &str,
) -> anyhow::Result<(u16, Value)> {
    driver
        .json_req(
            Method::POST,
            &format!("/api/v1/creations/{}/unpublish", percent_encode(creation_id)),
            Some(token),
            Some(json!({})),
        )
        .await
}

pub async fn review_decision_raw(
    driver: &E2eSystemDriver,
    token: &str,
    creation_id: &str,
    body: Value,
) -> anyhow::Result<(u16, Value)> {
    driver
        .json_req(
            Method::POST,
            &format!("/api/v1/creations/{}/review", percent_encode(creation_id)),
            Some(token),
            Some(body),
        )
        .await
}

pub async fn get_review_queue_raw(
    driver: &E2eSystemDriver,
    token: &str,
    query: &str,
) -> anyhow::Result<(u16, Value)> {
    let path = if query.is_empty() {
        "/api/v1/creations/review-queue".to_string()
    } else {
        format!("/api/v1/creations/review-queue?{query}")
    };
    driver.json_req(Method::GET, &path, Some(token), None).await
}

pub async fn get_audit_raw(
    driver: &E2eSystemDriver,
    token: &str,
    creation_id: &str,
) -> anyhow::Result<(u16, Value)> {
    driver
        .json_req(
            Method::GET,
            &format!("/api/v1/creations/{}/audit", percent_encode(creation_id)),
            Some(token),
            None,
        )
        .await
}

/// Unwraps `SubmitReviewResponse { creation, validation_errors }` to its `UserCreationDto`.
pub fn unwrap_creation_dto<'a>(v: &'a Value) -> &'a Value {
    v.get("creation").filter(|c| c.is_object()).unwrap_or(v)
}

/// Validates a `200 OK` `POST /api/v1/creations/{id}/submit-review` response and returns the
/// unwrapped `UserCreationDto` after asserting `validation_errors` is empty (when present).
pub fn assert_submit_review_ok<'a>(
    st: u16,
    sub_body: &'a Value,
    context: &str,
) -> anyhow::Result<&'a Value> {
    anyhow::ensure!(
        st == 200,
        "submit-review failed ({context}) with status {st}: {sub_body}"
    );
    if let Some(errs) = sub_body.get("validation_errors").and_then(|v| v.as_array()) {
        anyhow::ensure!(
            errs.is_empty(),
            "expected empty validation_errors on 200 submit-review ({context}), got: {sub_body}"
        );
    }
    let creation = unwrap_creation_dto(sub_body);
    anyhow::ensure!(
        creation["review_status"].as_str() == Some("pending_review"),
        "expected pending_review after submit ({context}), got: {sub_body}"
    );
    Ok(creation)
}

pub async fn publish_creation_with_visibility(
    driver: &E2eSystemDriver,
    owner_token: &str,
    admin_token: &str,
    creation_id: &str,
    visibility: &str,
) -> anyhow::Result<Value> {
    let (st_sub, sub_body) = submit_review_raw(driver, owner_token, creation_id).await?;
    let _ = assert_submit_review_ok(st_sub, &sub_body, creation_id)?;
    let (st_rev, rev_body) = review_decision_raw(
        driver,
        admin_token,
        creation_id,
        json!({
            "decision": "approve",
            "visibility": visibility,
            "notes": format!("Approved as {visibility}")
        }),
    )
    .await?;
    anyhow::ensure!(
        st_rev == 200,
        "approve review failed {st_rev} for {creation_id}: {rev_body}"
    );
    anyhow::ensure!(
        rev_body["review_status"].as_str() == Some("published"),
        "expected published status after approve, got: {rev_body}"
    );
    anyhow::ensure!(
        rev_body["visibility"].as_str() == Some(visibility),
        "expected visibility={visibility} after approve, got: {rev_body}"
    );
    Ok(rev_body)
}

pub async fn publish_creation_public(
    driver: &E2eSystemDriver,
    owner_token: &str,
    admin_token: &str,
    creation_id: &str,
) -> anyhow::Result<Value> {
    publish_creation_with_visibility(driver, owner_token, admin_token, creation_id, "public").await
}

pub async fn publish_creation_unlisted(
    driver: &E2eSystemDriver,
    owner_token: &str,
    admin_token: &str,
    creation_id: &str,
) -> anyhow::Result<Value> {
    publish_creation_with_visibility(driver, owner_token, admin_token, creation_id, "unlisted")
        .await
}

/// 2-step official MiniApp publish: POST /api/v1/bots + POST /api/v1/miniapps (is_published = 1).
pub async fn publish_miniapp(
    driver: &E2eSystemDriver,
    owner_token: &str,
    bot_username: &str,
    short_name: &str,
    title: &str,
) -> anyhow::Result<(String, Value)> {
    let (st_bot, bot_val) = driver
        .json_req(
            Method::POST,
            "/api/v1/bots",
            Some(owner_token),
            Some(json!({
                "display_name": format!("{title} Bot"),
                "username": bot_username
            })),
        )
        .await?;
    anyhow::ensure!(st_bot == 201 || st_bot == 200, "create bot failed {st_bot}: {bot_val}");
    let bot_token = bot_val["bot_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing bot_token: {bot_val}"))?
        .to_string();

    let (st_app, app_val) = driver
        .json_req(
            Method::POST,
            "/api/v1/miniapps",
            Some(owner_token),
            Some(json!({
                "bot_username": bot_username,
                "short_name": short_name,
                "title": title,
                "description": format!("{title} published Mini App for App Store merge verification."),
                "category": "productivity",
                "icon_emoji": "🧩"
            })),
        )
        .await?;
    anyhow::ensure!(
        st_app == 201 || st_app == 200,
        "create miniapp failed {st_app}: {app_val}"
    );
    Ok((bot_token, app_val))
}

pub fn extract_items<'a>(body: &'a Value) -> anyhow::Result<&'a Vec<Value>> {
    if let Some(arr) = body.as_array() {
        return Ok(arr);
    }
    for key in ["items", "results", "entries", "reviews", "leaderboard", "states", "audit"] {
        if let Some(arr) = body.get(key).and_then(|v| v.as_array()) {
            return Ok(arr);
        }
    }
    anyhow::bail!("expected JSON list or envelope containing items/results/entries, got: {body}");
}

/// Extracts all catalog/appstore/inline-query result items while excluding top-level metadata
/// (such as echoed `query` fields) so echo fields never trigger false-fails or false-passes.
pub fn extract_surface_entries<'a>(body: &'a Value) -> anyhow::Result<Vec<&'a Value>> {
    if let Some(arr) = body.as_array() {
        return Ok(arr.iter().collect());
    }
    if let Some(obj) = body.as_object() {
        let mut found_any_array = false;
        let mut out = Vec::new();
        for key in [
            "items",
            "results",
            "entries",
            "featured",
            "apps",
            "mini_apps",
            "cross_vertical",
        ] {
            if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                found_any_array = true;
                out.extend(arr.iter());
            }
        }
        if found_any_array {
            return Ok(out);
        }
    }
    anyhow::bail!(
        "expected catalog/appstore/inline-query response containing item arrays, got: {body}"
    );
}

pub fn value_contains_creation_id(v: &Value, creation_id: &str) -> bool {
    match v {
        Value::String(s) => s == creation_id || s.contains(creation_id),
        Value::Array(arr) => arr.iter().any(|x| value_contains_creation_id(x, creation_id)),
        Value::Object(map) => map.values().any(|x| value_contains_creation_id(x, creation_id)),
        _ => false,
    }
}

pub fn value_contains_substring(v: &Value, needle: &str) -> bool {
    match v {
        Value::String(s) => s.contains(needle),
        Value::Array(arr) => arr.iter().any(|x| value_contains_substring(x, needle)),
        Value::Object(map) => map.values().any(|x| value_contains_substring(x, needle)),
        _ => false,
    }
}

/// Opens (or reuses) a direct DM chat with official `@CreatorStudioBot` (`UID 777_020`).
pub async fn open_creator_bot_dm(driver: &E2eSystemDriver, token: &str) -> anyhow::Result<i64> {
    let (st, v) = driver
        .json_req(
            Method::POST,
            "/api/v1/chats",
            Some(token),
            Some(json!({ "peer_user_id": CREATOR_STUDIO_BOT_UID })),
        )
        .await?;
    anyhow::ensure!(
        st == 201 || st == 200,
        "failed to open DM with @CreatorStudioBot ({CREATOR_STUDIO_BOT_UID}): {st} {v}"
    );
    let chat_id = v["chat_id"]
        .as_i64()
        .or_else(|| v["id"].as_i64())
        .ok_or_else(|| anyhow::anyhow!("missing chat_id opening @CreatorStudioBot DM: {v}"))?;
    Ok(chat_id)
}

/// Sends a message to `@CreatorStudioBot` and polls `GET /api/v1/chats/{chat_id}/messages` until
/// the bot emits a reply with `id > sent_id && sender_id == CREATOR_STUDIO_BOT_UID`.
pub async fn send_bot_command_and_poll_reply(
    driver: &E2eSystemDriver,
    token: &str,
    chat_id: i64,
    command_text: &str,
) -> anyhow::Result<Value> {
    let (st_send, send_val) = driver
        .json_req(
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(token),
            Some(json!({ "text": command_text })),
        )
        .await?;
    anyhow::ensure!(
        st_send == 201 || st_send == 200,
        "send message to @CreatorStudioBot failed {st_send}: {send_val}"
    );
    let sent_id = send_val["message_id"]
        .as_i64()
        .or_else(|| send_val["id"].as_i64())
        .ok_or_else(|| anyhow::anyhow!("missing message_id in send response: {send_val}"))?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    loop {
        let (st_list, list_val) = driver
            .json_req(
                Method::GET,
                &format!("/api/v1/chats/{chat_id}/messages?limit=100"),
                Some(token),
                None,
            )
            .await?;
        anyhow::ensure!(
            st_list == 200,
            "poll messages failed {st_list}: {list_val}"
        );
        if let Some(messages) = list_val["messages"].as_array() {
            if let Some(reply) = messages.iter().rev().find(|m| {
                let mid = m["id"]
                    .as_i64()
                    .or_else(|| m["message_id"].as_i64())
                    .unwrap_or(0);
                let sid = m["sender_id"].as_u64().unwrap_or(0);
                mid > sent_id && sid == CREATOR_STUDIO_BOT_UID
            }) {
                return Ok(reply.clone());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for @CreatorStudioBot reply after sent_id={sent_id} for command `{command_text}`"
            );
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

fn collect_apple_links_from_str(s: &str, out: &mut Vec<String>) {
    let mut rest = s;
    while let Some(idx) = rest.find("tg://apple/") {
        let tail = &rest[idx..];
        let end = tail
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '>' | ',' | ';')
            })
            .unwrap_or(tail.len());
        let link = tail[..end].to_string();
        if !out.contains(&link) {
            out.push(link);
        }
        rest = &tail[end..];
    }
}

/// Scans `GenerativeCard` buttons (`action_payload`), `inline_keyboard` (`url`/`callback_data`),
/// and `text` for canonical `tg://apple/<section>/<creation_id>` deep-links.
pub fn extract_apple_deep_links_from_message(msg: &Value) -> Vec<String> {
    let mut links = Vec::new();
    if let Some(txt) = msg["text"].as_str() {
        collect_apple_links_from_str(txt, &mut links);
    }
    if let Some(components) = msg["content"]["data"]["components"].as_array() {
        for comp in components {
            if let Some(payload) = comp["action_payload"].as_str() {
                collect_apple_links_from_str(payload, &mut links);
            }
            if let Some(text) = comp["text"].as_str() {
                collect_apple_links_from_str(text, &mut links);
            }
        }
    }
    if let Some(rows) = msg["inline_keyboard"].as_array() {
        for row in rows {
            if let Some(btns) = row.as_array() {
                for btn in btns {
                    if let Some(url) = btn["url"].as_str() {
                        collect_apple_links_from_str(url, &mut links);
                    }
                    if let Some(cb) = btn["callback_data"].as_str() {
                        collect_apple_links_from_str(cb, &mut links);
                    }
                }
            }
        }
    }
    links
}

/// Connects an authenticated raw-JSON WebSocket to `/api/v1/ws?token=...` and completes a
/// `Ping`/`Pong` round-trip so `hub.register_session` has finished before HTTP triggers run.
pub async fn connect_raw_ws(
    server: &EphemeralTestServer,
    token: &str,
) -> anyhow::Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
    let (mut ws, _) = connect_async(server.ws_url(token)).await?;
    ws.send(Message::Text(
        json!({ "type": "Ping", "payload": { "nonce": 1 } })
            .to_string()
            .into(),
    ))
    .await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            anyhow::bail!("timed out waiting for initial WS Pong registration handshake");
        }
        match tokio::time::timeout(deadline - now, ws.next()).await {
            Ok(Some(Ok(Message::Text(txt)))) => {
                if let Ok(val) = serde_json::from_str::<Value>(&txt) {
                    if val["type"].as_str() == Some("Pong")
                        && val["payload"]["nonce"].as_u64() == Some(1)
                    {
                        break;
                    }
                }
            }
            Ok(Some(Ok(Message::Ping(p)))) => {
                let _ = ws.send(Message::Pong(p)).await;
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                anyhow::bail!("WebSocket closed during initial Ping/Pong registration handshake");
            }
            Ok(Some(Err(e))) => anyhow::bail!("WebSocket handshake read error: {e}"),
            Ok(Some(Ok(_))) => {}
            Err(_) => anyhow::bail!("timed out waiting for initial WS Pong registration handshake"),
        }
    }
    Ok(ws)
}

fn unwrap_ws_update_event(frame: &Value) -> Option<(String, Value)> {
    match frame["type"].as_str()? {
        "Update" => {
            let payload = &frame["payload"];
            let variant = payload["type"].as_str()?.to_string();
            Some((variant, payload["data"].clone()))
        }
        "PtsPush" => {
            let ev = &frame["payload"]["event"];
            let variant = ev["type"].as_str()?.to_string();
            Some((variant, ev["data"].clone()))
        }
        _ => None,
    }
}

/// Waits for a WebSocket `Update` or `PtsPush` event whose `(variant_name, data)` satisfies `pred`.
pub async fn recv_ws_event_matching<F>(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
    mut pred: F,
) -> anyhow::Result<(String, Value)>
where
    F: FnMut(&str, &Value) -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            anyhow::bail!("timed out waiting for matching WS UpdateEvent");
        }
        match tokio::time::timeout(deadline - now, ws.next()).await {
            Ok(Some(Ok(Message::Text(txt)))) => {
                if let Ok(val) = serde_json::from_str::<Value>(&txt) {
                    if let Some((variant, data)) = unwrap_ws_update_event(&val) {
                        if pred(&variant, &data) {
                            return Ok((variant, data));
                        }
                    }
                }
            }
            Ok(Some(Ok(Message::Ping(p)))) => {
                let _ = ws.send(Message::Pong(p)).await;
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                anyhow::bail!("WebSocket closed before matching UpdateEvent arrived");
            }
            Ok(Some(Err(e))) => anyhow::bail!("WebSocket read error: {e}"),
            Ok(Some(Ok(_))) => {}
            Err(_) => anyhow::bail!("timed out waiting for matching WS UpdateEvent"),
        }
    }
}

/// Drains WS frames for `window` and fails if ANY `Update` / `PtsPush` satisfies `pred`.
pub async fn assert_no_ws_event_matching<F>(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    window: Duration,
    mut pred: F,
    context: &str,
) -> anyhow::Result<()>
where
    F: FnMut(&str, &Value) -> bool,
{
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(());
        }
        match tokio::time::timeout(deadline - now, ws.next()).await {
            Ok(Some(Ok(Message::Text(txt)))) => {
                if let Ok(val) = serde_json::from_str::<Value>(&txt) {
                    if let Some((variant, data)) = unwrap_ws_update_event(&val) {
                        if pred(&variant, &data) {
                            anyhow::bail!(
                                "WS ISOLATION LEAK ({context}): received forbidden {variant}: {data}"
                            );
                        }
                    }
                }
            }
            Ok(Some(Ok(Message::Ping(p)))) => {
                let _ = ws.send(Message::Pong(p)).await;
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => return Ok(()),
            Ok(Some(Err(e))) => anyhow::bail!("WebSocket read error ({context}): {e}"),
            Ok(Some(Ok(_))) => {}
            Err(_) => return Ok(()),
        }
    }
}

fn assert_entries_zero_leak(
    entries: &[&Value],
    path: &str,
    creation_id: &str,
    forbidden_markers: &[&str],
) -> anyhow::Result<()> {
    for entry in entries {
        anyhow::ensure!(
            !value_contains_creation_id(entry, creation_id),
            "LEAK DETECTED: creation_id `{creation_id}` appeared in `{path}` entry: {entry}"
        );
        for &marker in forbidden_markers {
            anyhow::ensure!(
                !value_contains_substring(entry, marker),
                "LEAK DETECTED: secret marker `{marker}` appeared in `{path}` entry: {entry}"
            );
        }
    }
    Ok(())
}

/// Verifies ZERO leakage of `(creation_id, forbidden_markers)` across ALL 7 HTTP surfaces
/// (BOTH unfiltered AND queried for each EN/CJK query) PLUS BOTH bare `@CreatorStudioBot /recommend`
/// AND `@CreatorStudioBot /recommend <q>`.
///
/// When `expected_control_id` is `Some(ctrl_id)`, also positively verifies that the published
/// public control item matching the same `vertical` & `queries` IS discovered (eliminating
/// empty-stub false passes). `forbidden_markers` MUST be disjoint from `queries` so query-echoing
/// empty replies never cause false failures.
#[allow(clippy::too_many_arguments)]
pub async fn assert_zero_leak_across_all_surfaces(
    driver: &E2eSystemDriver,
    consumer_token: &str,
    consumer_bot_chat_id: i64,
    vertical: &str,
    creation_id: &str,
    queries: &[&str],
    forbidden_markers: &[&str],
    expected_control_id: Option<&str>,
) -> anyhow::Result<()> {
    // 1. Unfiltered (no `q` / empty `query`) checks across ALL 7 HTTP surfaces + bare /recommend
    let unfiltered_endpoints = [
        "/api/v1/catalog?kind=all&limit=50",
        "/api/v1/catalog/books?limit=50",
        "/api/v1/catalog/music?limit=50",
        "/api/v1/catalog/games?limit=50",
        "/api/v1/catalog/appstore?limit=50",
        "/api/v1/catalog/recommend?kind=all&limit=20",
        "/api/v1/bots/inline-query?bot_username=CreatorStudioBot&query=&limit=20",
    ];
    let target_vertical_prefix = format!("/api/v1/catalog/{vertical}");
    for path in unfiltered_endpoints {
        let (st, body) = driver
            .json_req(Method::GET, path, Some(consumer_token), None)
            .await?;
        anyhow::ensure!(
            st == 200,
            "unfiltered public surface `{path}` must return 200 (not {st}): {body}"
        );
        let entries = extract_surface_entries(&body)?;
        assert_entries_zero_leak(&entries, path, creation_id, forbidden_markers)?;
        if let Some(ctrl_id) = expected_control_id {
            if path.starts_with("/api/v1/catalog?")
                || path.starts_with(&target_vertical_prefix)
                || path.starts_with("/api/v1/catalog/recommend")
            {
                anyhow::ensure!(
                    entries.iter().any(|e| value_contains_creation_id(e, ctrl_id)),
                    "public control item `{ctrl_id}` missing from unfiltered `{path}`: {body}"
                );
            }
        }
    }

    let bare_reply = send_bot_command_and_poll_reply(
        driver,
        consumer_token,
        consumer_bot_chat_id,
        "/recommend",
    )
    .await?;
    anyhow::ensure!(
        !value_contains_creation_id(&bare_reply, creation_id),
        "LEAK DETECTED: creation_id `{creation_id}` in bare @CreatorStudioBot /recommend: {bare_reply}"
    );
    for &marker in forbidden_markers {
        anyhow::ensure!(
            !value_contains_substring(&bare_reply, marker),
            "LEAK DETECTED: secret marker `{marker}` in bare @CreatorStudioBot /recommend: {bare_reply}"
        );
    }
    if let Some(ctrl_id) = expected_control_id {
        anyhow::ensure!(
            value_contains_creation_id(&bare_reply, ctrl_id)
                || !extract_apple_deep_links_from_message(&bare_reply).is_empty(),
            "public control item `{ctrl_id}` missing from bare @CreatorStudioBot /recommend: {bare_reply}"
        );
    }

    // 2. Queried checks across ALL 7 HTTP surfaces + `@CreatorStudioBot /recommend {q}`
    for &q in queries {
        for &marker in forbidden_markers {
            anyhow::ensure!(
                !q.contains(marker),
                "test-design guard: forbidden marker `{marker}` must NOT be a substring of query `{q}`"
            );
        }
        let enc_q = percent_encode(q);
        let endpoints = [
            format!("/api/v1/catalog?kind=all&q={enc_q}&limit=50"),
            format!("/api/v1/catalog/books?q={enc_q}&limit=50"),
            format!("/api/v1/catalog/music?q={enc_q}&limit=50"),
            format!("/api/v1/catalog/games?q={enc_q}&limit=50"),
            format!("/api/v1/catalog/appstore?q={enc_q}&limit=50"),
            format!("/api/v1/catalog/recommend?q={enc_q}&kind=all&limit=20"),
            format!(
                "/api/v1/bots/inline-query?bot_username=CreatorStudioBot&query={enc_q}&limit=20"
            ),
        ];
        for path in endpoints {
            let (st, body) = driver
                .json_req(Method::GET, &path, Some(consumer_token), None)
                .await?;
            anyhow::ensure!(
                st == 200,
                "public surface `{path}` must return 200 (not {st}) during zero-leak audit: {body}"
            );
            let entries = extract_surface_entries(&body)?;
            assert_entries_zero_leak(&entries, &path, creation_id, forbidden_markers)?;
            if let Some(ctrl_id) = expected_control_id {
                if path.starts_with("/api/v1/catalog?")
                    || path.starts_with(&target_vertical_prefix)
                    || path.starts_with("/api/v1/catalog/recommend")
                    || path.starts_with("/api/v1/bots/inline-query")
                {
                    anyhow::ensure!(
                        entries.iter().any(|e| value_contains_creation_id(e, ctrl_id)),
                        "public control item `{ctrl_id}` missing from `{path}`: {body}"
                    );
                }
            }
        }
        let reply = send_bot_command_and_poll_reply(
            driver,
            consumer_token,
            consumer_bot_chat_id,
            &format!("/recommend {q}"),
        )
        .await?;
        anyhow::ensure!(
            !value_contains_creation_id(&reply, creation_id),
            "LEAK DETECTED: creation_id `{creation_id}` appeared in @CreatorStudioBot /recommend `{q}`: {reply}"
        );
        for &marker in forbidden_markers {
            anyhow::ensure!(
                !value_contains_substring(&reply, marker),
                "LEAK DETECTED: secret marker `{marker}` appeared in @CreatorStudioBot /recommend `{q}`: {reply}"
            );
        }
        if let Some(ctrl_id) = expected_control_id {
            anyhow::ensure!(
                value_contains_creation_id(&reply, ctrl_id),
                "public control item `{ctrl_id}` missing from @CreatorStudioBot /recommend `{q}`: {reply}"
            );
        }
    }
    Ok(())
}
