use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui_telegram_client::{
    AiGenerateSceneRequest, AiIterateSceneRequest, AiPublishSceneRequest, AssetKind, AssetManifest,
    AuthResponse, ClientConfig, CodeSceneAssetDto, CodeSceneCommentDto, CodeSceneCommentSort,
    CodeSceneFeedRequest, CodeSceneFeedTab, CodeSceneHttpClient, CreateCodeSceneCommentRequest,
    HttpClientError, ListCodeSceneAssetsQuery, ListCodeSceneCommentsQuery, LocalCodeSceneService,
    PublishCodeSceneRequest, RemixSceneRequest, SceneBehaviorEventDto, SceneBehaviorEventKind,
    TelegramClient, TelemetrySubmitPayload, ValidateCodeSceneRequest, CODE_SCENE_FORMAT_CS1,
};
use gpui_telegram_code_scene::{
    official_seed_assets, validate_to_report, CodeSceneRecommender,
    DeterministicProceduralSynthesizer, SynthRequest,
};
use gpui_telegram_core::error::TelegramError;
use parking_lot::Mutex;
use rusqlite::Connection;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
struct CapturedReq {
    method: String,
    target: String,
    auth: Option<String>,
    body: String,
}

struct MockCodeSceneState {
    recommender: CodeSceneRecommender,
    comments: BTreeMap<String, Vec<CodeSceneCommentDto>>,
    drafts: BTreeMap<String, gpui_telegram_proto::code_scene_contracts::AiGenerateSceneResponse>,
    like_calls: usize,
    bookmark_calls: usize,
    follow_calls: usize,
    telemetry_events: Vec<SceneBehaviorEventDto>,
}

struct MockCodeSceneServer {
    base_url: String,
    captured: Arc<Mutex<Vec<CapturedReq>>>,
    state: Arc<Mutex<MockCodeSceneState>>,
    fail_telemetry_503: Arc<AtomicBool>,
    fail_all_503: Arc<AtomicBool>,
    fail_comment_after_n: Arc<AtomicU64>,
    comment_post_count: Arc<AtomicU64>,
    force_remap_scene_ids: Arc<AtomicBool>,
    fail_publish_after_n: Arc<AtomicU64>,
    publish_post_count: Arc<AtomicU64>,
    delay_ms: Arc<AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockCodeSceneServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockCodeSceneServer {
    async fn start() -> Self {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(Mutex::new(MockCodeSceneState {
            recommender: CodeSceneRecommender::new_with_seeds(),
            comments: BTreeMap::new(),
            drafts: BTreeMap::new(),
            like_calls: 0,
            bookmark_calls: 0,
            follow_calls: 0,
            telemetry_events: Vec::new(),
        }));
        let fail_telemetry_503 = Arc::new(AtomicBool::new(false));
        let fail_all_503 = Arc::new(AtomicBool::new(false));
        let fail_comment_after_n = Arc::new(AtomicU64::new(0));
        let comment_post_count = Arc::new(AtomicU64::new(0));
        let force_remap_scene_ids = Arc::new(AtomicBool::new(false));
        let fail_publish_after_n = Arc::new(AtomicU64::new(0));
        let publish_post_count = Arc::new(AtomicU64::new(0));
        let delay_ms = Arc::new(AtomicU64::new(0));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cap_c = Arc::clone(&captured);
        let st_c = Arc::clone(&state);
        let fail_c = Arc::clone(&fail_telemetry_503);
        let fail_all_c = Arc::clone(&fail_all_503);
        let fail_cmt_c = Arc::clone(&fail_comment_after_n);
        let cmt_cnt_c = Arc::clone(&comment_post_count);
        let remap_c = Arc::clone(&force_remap_scene_ids);
        let fail_pub_c = Arc::clone(&fail_publish_after_n);
        let pub_cnt_c = Arc::clone(&publish_post_count);
        let delay_c = Arc::clone(&delay_ms);

        let task = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let cap = Arc::clone(&cap_c);
                let st = Arc::clone(&st_c);
                let fail_503 = Arc::clone(&fail_c);
                let fail_all = Arc::clone(&fail_all_c);
                let fail_cmt = Arc::clone(&fail_cmt_c);
                let cmt_cnt = Arc::clone(&cmt_cnt_c);
                let remap_ids = Arc::clone(&remap_c);
                let fail_pub = Arc::clone(&fail_pub_c);
                let pub_cnt = Arc::clone(&pub_cnt_c);
                let delay = Arc::clone(&delay_c);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 262144];
                    let mut total = 0usize;
                    let mut content_length = 0usize;
                    let mut header_end = None;
                    loop {
                        let n = match sock.read(&mut buf[total..]).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        total += n;
                        if header_end.is_none() {
                            if let Some(pos) =
                                buf[..total].windows(4).position(|w| w == b"\r\n\r\n")
                            {
                                header_end = Some(pos + 4);
                                let head = String::from_utf8_lossy(&buf[..pos]);
                                for line in head.lines() {
                                    if let Some((k, v)) = line.split_once(':') {
                                        if k.eq_ignore_ascii_case("content-length") {
                                            content_length = v.trim().parse().unwrap_or(0);
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(he) = header_end {
                            if total >= he + content_length {
                                break;
                            }
                        }
                    }
                    let he = header_end.unwrap_or(total);
                    let head = String::from_utf8_lossy(&buf[..he]).to_string();
                    let body = String::from_utf8_lossy(&buf[he..total]).to_string();
                    let mut lines = head.lines();
                    let req_line = lines.next().unwrap_or("");
                    let mut parts = req_line.split_whitespace();
                    let method = parts.next().unwrap_or("GET").to_string();
                    let target = parts.next().unwrap_or("/").to_string();
                    let mut auth = None;
                    for line in lines {
                        if let Some((k, v)) = line.split_once(':') {
                            if k.eq_ignore_ascii_case("authorization") {
                                auth = Some(v.trim().to_string());
                            }
                        }
                    }

                    cap.lock().push(CapturedReq {
                        method: method.clone(),
                        target: target.clone(),
                        auth: auth.clone(),
                        body: body.clone(),
                    });

                    let d = delay.load(Ordering::SeqCst);
                    if d > 0 {
                        tokio::time::sleep(Duration::from_millis(d)).await;
                    }

                    let (status_line, resp_json) = if fail_all.load(Ordering::SeqCst) {
                        (
                            "503 Service Unavailable",
                            json!({ "error": "unavailable", "message": "Mock 503 outage" }),
                        )
                    } else {
                        handle_mock_request(
                            &st,
                            &fail_503,
                            &fail_cmt,
                            &cmt_cnt,
                            &remap_ids,
                            &fail_pub,
                            &pub_cnt,
                            &method,
                            &target,
                            auth.as_deref(),
                            &body,
                        )
                    };
                    let resp_body = resp_json.to_string();
                    let raw = format!(
                        "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp_body.len(),
                        resp_body
                    );
                    let _ = sock.write_all(raw.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });

        Self {
            base_url: format!("http://{addr}"),
            captured,
            state,
            fail_telemetry_503,
            fail_all_503,
            fail_comment_after_n,
            comment_post_count,
            force_remap_scene_ids,
            fail_publish_after_n,
            publish_post_count,
            delay_ms,
            task,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_mock_request(
    st: &Arc<Mutex<MockCodeSceneState>>,
    fail_503: &Arc<AtomicBool>,
    fail_comment_after_n: &Arc<AtomicU64>,
    comment_post_count: &Arc<AtomicU64>,
    force_remap_scene_ids: &Arc<AtomicBool>,
    fail_publish_after_n: &Arc<AtomicU64>,
    publish_post_count: &Arc<AtomicU64>,
    method: &str,
    target: &str,
    auth: Option<&str>,
    body: &str,
) -> (&'static str, Value) {
    if auth != Some("Bearer valid-token") {
        return (
            "401 Unauthorized",
            json!({ "error": "invalid_token", "message": "Unauthorized" }),
        );
    }
    let path = target.split('?').next().unwrap_or(target);
    let mut g = st.lock();
    let uid = 1001i64;

    if method == "POST" && path == "/api/v1/code-scene/feed" {
        let mut req: CodeSceneFeedRequest = serde_json::from_str(body).unwrap_or_default();
        req.user_id = uid;
        let resp = g.recommender.recommend_feed(&req);
        return ("200 OK", serde_json::to_value(resp).unwrap());
    }
    if method == "GET" && path == "/api/v1/code-scene/assets" {
        let mut resp = g.recommender.list_assets(&ListCodeSceneAssetsQuery {
            limit: Some(200),
            ..Default::default()
        });
        if let Some(p) = g.recommender.export_user_profile(uid) {
            for a in &mut resp.assets {
                a.viewer_liked = p.liked_scene_ids.contains(&a.scene_id);
                a.viewer_bookmarked = p.bookmarked_scene_ids.contains(&a.scene_id);
                a.viewer_following_author = p.followed_creators.contains(&a.author_user_id);
            }
        }
        return ("200 OK", serde_json::to_value(resp).unwrap());
    }
    if method == "POST" && path == "/api/v1/code-scene/assets/validate" {
        let req: ValidateCodeSceneRequest = serde_json::from_str(body).unwrap();
        let rep = validate_to_report(&req.source_code, &req.manifest);
        return ("200 OK", serde_json::to_value(rep).unwrap());
    }
    if method == "POST" && path == "/api/v1/code-scene/assets" {
        let n = publish_post_count.fetch_add(1, Ordering::SeqCst) + 1;
        let limit_fail = fail_publish_after_n.load(Ordering::SeqCst);
        if limit_fail > 0 && n > limit_fail {
            return (
                "503 Service Unavailable",
                json!({ "error": "unavailable", "message": "Transient publish 503" }),
            );
        }
        let req: PublishCodeSceneRequest = serde_json::from_str(body).unwrap();
        let mut man = req.manifest.clone();
        man.kind = req.kind;
        man.clamp_in_place();
        let rep = validate_to_report(&req.source_code, &man);
        if !rep.ok {
            return (
                "422 Unprocessable Entity",
                json!({ "error": "validation_failed", "message": "Invalid cs1", "validation": rep }),
            );
        }
        if let Some(ref pid) = req.remix_parent_id {
            if g.recommender.get_asset(pid).is_none() {
                return (
                    "404 Not Found",
                    json!({ "error": "parent_scene_not_found", "message": format!("Remix parent scene '{pid}' was not found") }),
                );
            }
        }
        let sid = if force_remap_scene_ids.load(Ordering::SeqCst) {
            format!("srv_scene_{}", g.recommender.catalog_len() + 1)
        } else {
            req.scene_id
                .clone()
                .unwrap_or_else(|| format!("srv_scene_{}", g.recommender.catalog_len() + 1))
        };
        let asset = CodeSceneAssetDto {
            scene_id: sid,
            title: req.title,
            description: req.description,
            kind: req.kind,
            tags: req.tags,
            author_user_id: uid,
            author_username: "alice".to_string(),
            source_code: req.source_code,
            code_format: CODE_SCENE_FORMAT_CS1.to_string(),
            manifest: man,
            controls_hint: req.controls_hint,
            remix_parent_id: req.remix_parent_id,
            ..Default::default()
        };
        g.recommender.register_or_update_asset(asset.clone(), Some(uid));
        return (
            "201 Created",
            json!({ "asset": asset, "validation": rep, "posted_chat_message_id": Value::Null }),
        );
    }
    if method == "POST" && path == "/api/v1/code-scene/telemetry" {
        if fail_503.load(Ordering::SeqCst) {
            return (
                "503 Service Unavailable",
                json!({ "error": "unavailable", "message": "Server temporarily unavailable" }),
            );
        }
        let payload: TelemetrySubmitPayload = serde_json::from_str(body).unwrap();
        let events = payload.into_events();
        let accepted = events.len();
        for ev in events {
            g.recommender.record_behavior(&ev);
            g.telemetry_events.push(ev);
        }
        return (
            "200 OK",
            json!({ "accepted": accepted, "reward": 1.0, "profile_updated": accepted > 0 }),
        );
    }
    if method == "POST" && path == "/api/v1/code-scene/ai/generate" {
        let req: AiGenerateSceneRequest = serde_json::from_str(body).unwrap();
        let synth = DeterministicProceduralSynthesizer::new().synthesize(&SynthRequest::from(&req));
        let resp = synth.into_ai_generate_response("srv_draft_1", Some(777_000), Some("fabushi_ai_bot".into()), 0);
        g.drafts.insert(resp.draft_id.clone(), resp.clone());
        return ("200 OK", serde_json::to_value(resp).unwrap());
    }
    if method == "POST" && path == "/api/v1/code-scene/ai/iterate" {
        let req: AiIterateSceneRequest = serde_json::from_str(body).unwrap();
        let draft = g.drafts.get(req.draft_id.as_deref().unwrap_or("srv_draft_1")).cloned().unwrap();
        let synth = DeterministicProceduralSynthesizer::new().remix_source(&draft.source_code, &draft.manifest, &req.instruction, Some(&draft.title), &draft.tags);
        let resp = synth.into_ai_generate_response(draft.draft_id, draft.bot_id, draft.bot_username, 0);
        g.drafts.insert(resp.draft_id.clone(), resp.clone());
        return ("200 OK", serde_json::to_value(resp).unwrap());
    }
    if method == "POST" && path == "/api/v1/code-scene/ai/publish" {
        let req: AiPublishSceneRequest = serde_json::from_str(body).unwrap();
        let draft = g.drafts.get(req.draft_id.as_deref().unwrap_or("srv_draft_1")).cloned().unwrap();
        let asset = CodeSceneAssetDto {
            scene_id: "srv_ai_pub_1".to_string(),
            title: draft.title,
            description: draft.description,
            kind: draft.kind,
            tags: draft.tags,
            author_user_id: uid,
            author_username: "alice".to_string(),
            source_code: draft.source_code,
            code_format: CODE_SCENE_FORMAT_CS1.to_string(),
            manifest: draft.manifest,
            ..Default::default()
        };
        g.recommender.register_or_update_asset(asset.clone(), Some(uid));
        return ("201 Created", json!({ "asset": asset, "validation": draft.validation, "posted_chat_message_id": Value::Null }));
    }
    if method == "POST" && path == "/api/v1/code-scene/ai/remix" {
        let req: RemixSceneRequest = serde_json::from_str(body).unwrap();
        let parent = g.recommender.get_asset(&req.parent_scene_id).unwrap();
        let synth = DeterministicProceduralSynthesizer::new().remix_source(&parent.source_code, &parent.manifest, &req.prompt, Some(&parent.title), &parent.tags);
        let draft = synth.into_ai_generate_response("srv_remix_draft_1", Some(777_000), Some("fabushi_ai_bot".into()), 0);
        return ("200 OK", json!({ "draft": draft, "published": Value::Null }));
    }
    if let Some(rest) = path.strip_prefix("/api/v1/code-scene/creators/") {
        if let Some(id_s) = rest.strip_suffix("/follow") {
            let cid: i64 = id_s.parse().unwrap();
            g.follow_calls += 1;
            // Match real server: explicit follow un-hides a previously hidden author BEFORE toggling!
            if let Some(mut p) = g.recommender.export_user_profile(uid) {
                if !p.followed_creators.contains(&cid) && p.hidden_author_ids.remove(&cid) {
                    p.creator_weights.remove(&cid);
                    p.sanitize();
                    g.recommender.import_user_profile(p);
                }
            }
            let resp = g.recommender.toggle_follow(uid, cid);
            return ("200 OK", serde_json::to_value(resp).unwrap());
        }
    }
    if let Some(rest) = path.strip_prefix("/api/v1/code-scene/assets/") {
        if let Some(sid) = rest.strip_suffix("/like") {
            g.like_calls += 1;
            let Some(cur) = g.recommender.get_asset_for_user(sid, uid) else {
                return ("404 Not Found", json!({ "error": "scene_not_found", "message": "Not found" }));
            };
            let target_liked = !cur.viewer_liked;
            let target_count = if target_liked {
                cur.likes_count.saturating_add(1)
            } else {
                cur.likes_count.saturating_sub(1)
            };
            let _ = g.recommender.toggle_like(uid, sid);
            // Match real server (`routes/code_scene.rs:1308-1320`): authoritatively reconcile
            // counter & profile.liked_scene_ids while preserving disliked_scene_ids / hidden_tags.
            g.recommender
                .update_social_counters(sid, Some(target_count), None, None, None, None, None);
            let mut p = g
                .recommender
                .export_user_profile(uid)
                .unwrap_or_else(|| gpui_telegram_code_scene::UserInterestProfile::new(uid));
            let changed = if target_liked {
                p.liked_scene_ids.insert(sid.to_string())
            } else {
                p.liked_scene_ids.remove(sid)
            };
            if changed {
                p.sanitize();
                g.recommender.import_user_profile(p);
            }
            return (
                "200 OK",
                serde_json::to_value(
                    gpui_telegram_proto::code_scene_contracts::ToggleLikeResponse {
                        liked: target_liked,
                        likes_count: target_count,
                    },
                )
                .unwrap(),
            );
        }
        if let Some(sid) = rest.strip_suffix("/bookmark") {
            g.bookmark_calls += 1;
            let Some(cur) = g.recommender.get_asset_for_user(sid, uid) else {
                return ("404 Not Found", json!({ "error": "scene_not_found", "message": "Not found" }));
            };
            let target_bm = !cur.viewer_bookmarked;
            let target_count = if target_bm {
                cur.bookmarks_count.saturating_add(1)
            } else {
                cur.bookmarks_count.saturating_sub(1)
            };
            let _ = g.recommender.toggle_bookmark(uid, sid);
            // Match real server (`routes/code_scene.rs:1370-1391`): authoritatively reconcile
            // counter & profile.bookmarked_scene_ids while preserving disliked_scene_ids / hidden_tags.
            g.recommender
                .update_social_counters(sid, None, Some(target_count), None, None, None, None);
            let mut p = g
                .recommender
                .export_user_profile(uid)
                .unwrap_or_else(|| gpui_telegram_code_scene::UserInterestProfile::new(uid));
            let changed = if target_bm {
                p.bookmarked_scene_ids.insert(sid.to_string())
            } else {
                p.bookmarked_scene_ids.remove(sid)
            };
            if changed {
                p.sanitize();
                g.recommender.import_user_profile(p);
            }
            return (
                "200 OK",
                serde_json::to_value(
                    gpui_telegram_proto::code_scene_contracts::ToggleBookmarkResponse {
                        bookmarked: target_bm,
                        bookmarks_count: target_count,
                    },
                )
                .unwrap(),
            );
        }
        if let Some(sid) = rest.strip_suffix("/comments") {
            if method == "GET" {
                let list = g.comments.get(sid).cloned().unwrap_or_default();
                return ("200 OK", json!({ "scene_id": sid, "comments": list, "total": list.len() }));
            }
            if method == "POST" {
                let n = comment_post_count.fetch_add(1, Ordering::SeqCst) + 1;
                let limit_fail = fail_comment_after_n.load(Ordering::SeqCst);
                if limit_fail > 0 && n > limit_fail {
                    return (
                        "503 Service Unavailable",
                        json!({ "error": "unavailable", "message": "Transient comment 503" }),
                    );
                }
                let req: CreateCodeSceneCommentRequest = serde_json::from_str(body).unwrap();
                // Match real server: validate parent_comment_id exists on this Code Scene!
                if let Some(ref pid) = req.parent_comment_id {
                    let parent_exists = g
                        .comments
                        .get(sid)
                        .is_some_and(|list| list.iter().any(|c| c.comment_id == *pid));
                    if !parent_exists {
                        return (
                            "404 Not Found",
                            json!({
                                "error": "parent_comment_not_found",
                                "message": "Parent comment not found in this Code Scene"
                            }),
                        );
                    }
                }
                let cmt = CodeSceneCommentDto {
                    comment_id: format!("srv_cmt_{}", g.comments.get(sid).map(|v| v.len()).unwrap_or(0) + 1),
                    scene_id: sid.to_string(),
                    author_user_id: uid,
                    author_username: "alice".to_string(),
                    content: req.content,
                    parent_comment_id: req.parent_comment_id,
                    likes_count: 0,
                    viewer_liked: false,
                    created_at_ms: 1700000000000,
                };
                let entry = g.comments.entry(sid.to_string()).or_default();
                entry.push(cmt.clone());
                let cnt = entry.len() as u64;
                return ("201 Created", json!({ "comment": cmt, "comments_count": cnt }));
            }
        }
        if method == "GET" {
            return match g.recommender.get_asset_for_user(rest, uid) {
                Some(a) => ("200 OK", serde_json::to_value(a).unwrap()),
                None => ("404 Not Found", json!({ "error": "scene_not_found", "message": "Not found" })),
            };
        }
    }
    ("404 Not Found", json!({ "error": "not_found", "message": "Route not found" }))
}

fn valid_cs1_source() -> String {
    r#"
fn on_init(state) { state.t = 0.0; }
fn on_update(dt, input, state) { state.t = state.t + dt; }
fn on_render(canvas, state, time) {
    clear(rgba(8, 12, 24, 1.0));
    circle(180, 320, 44, hsla(185, 0.95, 0.62, 0.95));
}
"#
    .to_string()
}

fn mock_auth_session(user_id: u64, username: &str, token: &str) -> AuthResponse {
    AuthResponse {
        user_id,
        username: username.to_string(),
        session_id: format!("sess_{user_id}"),
        token: token.to_string(),
        first_name: Some(username.to_string()),
        created_at: None,
        expires_at: None,
    }
}

// Group 1: All 15 endpoints, Bearer auth, RFC 3986 query encoding, and HTTP error mapping
#[tokio::test]
async fn test_code_scene_http_client_all_15_endpoints_auth_and_query_encoding() {
    let srv = MockCodeSceneServer::start().await;
    let client = CodeSceneHttpClient::from_base_url(&srv.base_url);
    let tok = "valid-token";
    let seed_id = official_seed_assets()[0].scene_id.clone();

    // Missing token fails closed locally without hitting the wire
    let err_empty = client
        .fetch_feed("   ", &CodeSceneFeedRequest::default())
        .await
        .unwrap_err();
    assert!(matches!(err_empty, HttpClientError::Unauthorized(_)));
    assert!(srv.captured.lock().is_empty());

    // 1. feed
    let feed = client
        .fetch_feed(tok, &CodeSceneFeedRequest::default())
        .await
        .unwrap();
    assert_eq!(feed.total_catalog, 18);

    // 2. list_assets with RFC 3986 query encoding
    let _list = client
        .list_assets(
            tok,
            &ListCodeSceneAssetsQuery {
                q: Some("neon & retro".into()),
                kind: Some(AssetKind::Game),
                tag: Some("cyber/punk".into()),
                author_user_id: Some(777_102),
                creator_bot_id: Some(777_102),
                limit: Some(10),
                offset: Some(0),
            },
        )
        .await
        .unwrap();
    let list_req = srv.captured.lock().last().unwrap().clone();
    assert_eq!(list_req.method, "GET");
    assert!(list_req.body.is_empty());
    assert_eq!(list_req.auth.as_deref(), Some("Bearer valid-token"));
    assert!(list_req.target.contains("q=neon%20%26%20retro"));
    assert!(list_req.target.contains("kind=game"));
    assert!(list_req.target.contains("tag=cyber%2Fpunk"));
    assert!(list_req.target.contains("creator_bot_id=777102"));

    // 3. get_asset
    let asset = client.get_asset(tok, &seed_id).await.unwrap();
    assert_eq!(asset.scene_id, seed_id);

    // 4 & 5. validate & publish
    let val = client
        .validate_asset(
            tok,
            &ValidateCodeSceneRequest {
                source_code: valid_cs1_source(),
                manifest: AssetManifest::for_kind(AssetKind::Animation),
            },
        )
        .await
        .unwrap();
    assert!(val.ok);

    let pub_resp = client
        .publish_asset(
            tok,
            &PublishCodeSceneRequest {
                title: "Client HTTP Pub".into(),
                kind: AssetKind::Animation,
                source_code: valid_cs1_source(),
                manifest: AssetManifest::for_kind(AssetKind::Animation),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!pub_resp.asset.scene_id.is_empty());

    // 6-10. like, bookmark, comments (hot & newest), follow
    assert!(client.toggle_like(tok, &seed_id).await.unwrap().liked);
    assert!(client.toggle_bookmark(tok, &seed_id).await.unwrap().bookmarked);
    let c_add = client
        .add_comment(
            tok,
            &seed_id,
            &CreateCodeSceneCommentRequest {
                content: "Great scene!".into(),
                parent_comment_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(c_add.comments_count, 1);
    let c_list = client
        .list_comments(
            tok,
            &seed_id,
            &ListCodeSceneCommentsQuery {
                sort: Some(CodeSceneCommentSort::Newest),
                limit: Some(10),
                offset: Some(0),
            },
        )
        .await
        .unwrap();
    assert_eq!(c_list.total, 1);
    assert!(srv.captured.lock().last().unwrap().target.contains("sort=newest"));
    assert!(client.toggle_follow(tok, 777_101).await.unwrap().following);

    // 11. telemetry single, batch, and TelemetrySubmitPayload
    let ev = SceneBehaviorEventDto {
        scene_id: seed_id.clone(),
        kind: SceneBehaviorEventKind::Dwell,
        dwell_ms: 5000,
        ..Default::default()
    };
    assert_eq!(
        client.submit_telemetry_event(tok, &ev).await.unwrap().accepted,
        1
    );
    assert_eq!(
        client
            .submit_telemetry_batch(tok, vec![ev.clone()])
            .await
            .unwrap()
            .accepted,
        1
    );
    assert_eq!(
        client
            .submit_telemetry(tok, &TelemetrySubmitPayload::Single(Box::new(ev)))
            .await
            .unwrap()
            .accepted,
        1
    );

    // 12-15. AI generate, iterate, publish, remix
    let gen_resp = client
        .ai_generate(
            tok,
            &AiGenerateSceneRequest {
                prompt: "neon arcade".into(),
                kind: Some(AssetKind::Game),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let iter_resp = client
        .ai_iterate(
            tok,
            &AiIterateSceneRequest {
                draft_id: Some(gen_resp.draft_id.clone()),
                instruction: "faster".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let _ai_pub = client
        .ai_publish(
            tok,
            &AiPublishSceneRequest {
                draft_id: Some(iter_resp.draft_id),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let rmx = client
        .ai_remix(
            tok,
            &RemixSceneRequest {
                parent_scene_id: seed_id.clone(),
                prompt: "cyberpunk".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(rmx.draft.validation.ok);

    // 401, 404, 422 error mapping
    let e401 = client
        .fetch_feed("wrong-token", &CodeSceneFeedRequest::default())
        .await
        .unwrap_err();
    assert!(matches!(TelegramError::from(e401), TelegramError::Auth(_)));
    let e404 = client.get_asset(tok, "nonexistent_scene_id").await.unwrap_err();
    assert!(matches!(TelegramError::from(e404), TelegramError::NotFound(_)));
    let e422 = client
        .publish_asset(
            tok,
            &PublishCodeSceneRequest {
                title: "Bad".into(),
                kind: AssetKind::Animation,
                source_code: "const broken = 1;".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(TelegramError::from(e422), TelegramError::Validation(_)));
}

// Group 2: Local scene_id & creator_user_id validation guards (0 HTTP requests sent!)
#[tokio::test]
async fn test_code_scene_http_client_scene_id_and_creator_validation_guards() {
    let srv = MockCodeSceneServer::start().await;
    let client = CodeSceneHttpClient::from_base_url(&srv.base_url);
    let long_id = "a".repeat(129);
    for bad_id in [
        "",
        "   ",
        "..",
        "../users",
        "a/b",
        "a?q=1",
        "a#frag",
        "validate",
        "VALIDATE",
        "  VaLiDaTe  ",
        long_id.as_str(),
    ] {
        let err = client.get_asset("valid-token", bad_id).await.unwrap_err();
        assert!(
            matches!(err, HttpClientError::Api { status: 400, .. }),
            "Expected 400 local guard for '{bad_id}', got {err:?}"
        );
    }
    for bad_uid in [0i64, -1, -999] {
        let err = client.toggle_follow("valid-token", bad_uid).await.unwrap_err();
        assert!(matches!(err, HttpClientError::Api { status: 400, .. }));
    }
    assert!(srv.captured.lock().is_empty(), "Local guards must send 0 HTTP requests");
}

// Group 3: Non-Tokio thread safety (GPUI foreground thread compatibility)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_code_scene_http_client_non_tokio_thread_safe() {
    let srv = MockCodeSceneServer::start().await;
    let cfg = ClientConfig {
        server_url: srv.base_url.clone(),
        ..ClientConfig::default()
    };
    let tg = TelegramClient::new(cfg);
    tg.set_auth_session(mock_auth_session(1001, "alice", "valid-token"));
    let code_scene_http = tg.code_scene().clone();

    let feed = std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        futures::executor::block_on(
            code_scene_http.fetch_feed("valid-token", &CodeSceneFeedRequest::default()),
        )
        .unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(feed.total_catalog, 18);

    tokio::task::spawn_blocking(move || drop(tg)).await.unwrap();
}

// Group 4 & 5: Offline startup seeds, feed session, social, comments, publish, AI studio
#[tokio::test]
async fn test_local_code_scene_service_offline_seeds_feed_social_publish_and_ai_studio() {
    let svc = LocalCodeSceneService::new_offline(1001, Some("alice".into()));
    assert_eq!(svc.catalog_len(), 18);
    assert_eq!(svc.pending_telemetry_count(), 0);

    // Load feed, preload window [i-1,i,i+1], tabs, search, swipe, dwell/flush telemetry
    let loaded = svc.load_feed();
    assert!(loaded > 0);
    assert!(svc.preload_window_indices().contains(&0));
    svc.on_card_dwell_tick(5500, 1);
    let next_item = svc.swipe_next();
    assert!(next_item.is_some());
    assert!(svc.pending_telemetry_count() >= 1);
    let _ = svc.swipe_prev();

    assert!(svc.switch_tab(CodeSceneFeedTab::Games) >= 6);
    assert!(svc
        .items()
        .iter()
        .all(|i| i.asset.kind == AssetKind::Game));
    let opened = svc.open_scene_by_id("seed_game_flappy_bird").unwrap();
    assert_eq!(opened.asset.scene_id, "seed_game_flappy_bird");

    // Offline social + comments
    let like = svc.toggle_like_local("seed_game_flappy_bird").unwrap();
    assert!(like.liked);
    let bm = svc.toggle_bookmark_local("seed_game_flappy_bird").unwrap();
    assert!(bm.bookmarked);
    let flw = svc.toggle_follow_local(opened.asset.author_user_id);
    assert!(flw.following);

    assert!(svc.add_comment_local("seed_game_flappy_bird", "   ", None).is_err());
    assert!(svc
        .add_comment_local("seed_game_flappy_bird", &"x".repeat(2001), None)
        .is_err());
    let c1 = svc
        .add_comment_local("seed_game_flappy_bird", "Awesome game!", None)
        .unwrap();
    assert_eq!(c1.comment.content, "Awesome game!");
    let listed = svc.list_comments_local("seed_game_flappy_bird", CodeSceneCommentSort::Newest);
    assert_eq!(listed.total, 1);

    // Invalid publish rejected; valid publish surfaces at top of ForYou & MyWorks
    assert!(svc
        .publish_scene_local(&PublishCodeSceneRequest {
            title: "Bad".into(),
            kind: AssetKind::Animation,
            source_code: "const bad = 1;".into(),
            ..Default::default()
        })
        .is_err());
    let pub_ok = svc
        .publish_scene_local(&PublishCodeSceneRequest {
            title: "Offline Masterpiece".into(),
            kind: AssetKind::Animation,
            tags: vec!["offline".into()],
            source_code: valid_cs1_source(),
            manifest: AssetManifest::for_kind(AssetKind::Animation),
            ..Default::default()
        })
        .unwrap();
    assert!(pub_ok.validation.ok);
    svc.switch_tab(CodeSceneFeedTab::ForYou);
    assert_eq!(
        svc.current_item().unwrap().asset.scene_id,
        pub_ok.asset.scene_id
    );
    svc.switch_tab(CodeSceneFeedTab::MyWorks);
    assert!(svc
        .items()
        .iter()
        .any(|i| i.asset.scene_id == pub_ok.asset.scene_id));

    // Offline AI Studio: generate -> iterate -> remix (auto_publish) -> draft CRUD
    let d1 = svc.ai_generate_local(&AiGenerateSceneRequest {
        prompt: "赛博霓虹粒子".into(),
        kind: Some(AssetKind::Animation),
        ..Default::default()
    });
    assert!(d1.draft.validation.ok);
    assert_eq!(svc.list_drafts().len(), 1);
    let d2 = svc
        .ai_iterate_local(&AiIterateSceneRequest {
            draft_id: Some(d1.draft_id.clone()),
            instruction: "增加旋转光环".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(d2.draft.validation.ok);

    let parent_before = svc.get_asset(&pub_ok.asset.scene_id).unwrap().remixes_count;
    let rmx = svc
        .ai_remix_local(&RemixSceneRequest {
            parent_scene_id: pub_ok.asset.scene_id.clone(),
            prompt: "改成蓝色火焰".into(),
            auto_publish: true,
            ..Default::default()
        })
        .unwrap();
    let rmx_pub = rmx.published.unwrap();
    assert_eq!(
        rmx_pub.asset.remix_parent_id.as_deref(),
        Some(pub_ok.asset.scene_id.as_str())
    );
    assert_eq!(
        svc.get_asset(&pub_ok.asset.scene_id).unwrap().remixes_count,
        parent_before + 1
    );
}

// Group 6: SQLite persistence survives drop & reopen for same account (all 6 tables + 0% blocklist leak)
#[tokio::test]
async fn test_sqlite_persistence_survives_drop_and_reopen_same_account() {
    let dir = TempDir::new().unwrap();
    let srv_url = "http://127.0.0.1:18080";
    let seeds = official_seed_assets();
    let liked_sid = seeds[0].scene_id.clone();
    let disliked_sid = seeds[1].scene_id.clone();
    let hidden_author = seeds[2].author_user_id;

    let (pub_id, draft_id) = {
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            srv_url,
            1001,
            Some("alice".into()),
            None,
        )
        .unwrap();
        svc.load_feed();
        svc.toggle_like_local(&liked_sid).unwrap();
        svc.toggle_bookmark_local(&liked_sid).unwrap();
        svc.toggle_follow_local(seeds[0].author_user_id);
        svc.add_comment_local(&liked_sid, "Persisted offline comment", None)
            .unwrap();

        // Trigger NotInterested, HideAuthor, HideTag via feed session
        let _ = svc.open_scene_by_id(&disliked_sid);
        svc.record_interaction(SceneBehaviorEventKind::NotInterested, None);
        let _ = svc.open_scene_by_id(&seeds[2].scene_id);
        svc.record_interaction(SceneBehaviorEventKind::HideAuthor, None);
        svc.record_interaction(SceneBehaviorEventKind::HideTag, Some("physics".into()));
        svc.on_card_dwell_tick(6000, 1);
        let _ = svc.flush_active_card_telemetry();

        let published = svc
            .publish_scene_local(&PublishCodeSceneRequest {
                title: "Durable Local Scene".into(),
                kind: AssetKind::Animation,
                tags: vec!["durable".into()],
                source_code: valid_cs1_source(),
                manifest: AssetManifest::for_kind(AssetKind::Animation),
                ..Default::default()
            })
            .unwrap();
        let draft = svc.ai_generate_local(&AiGenerateSceneRequest {
            prompt: "银河粒子".into(),
            kind: Some(AssetKind::Animation),
            ..Default::default()
        });
        assert!(svc.pending_telemetry_count() > 0);
        (published.asset.scene_id, draft.draft_id)
    };

    // Reopen exact same TempDir & account scope
    let svc2 = LocalCodeSceneService::open_with_dir(
        dir.path(),
        srv_url,
        1001,
        Some("alice".into()),
        None,
    )
    .unwrap();
    let liked_asset = svc2.get_asset(&liked_sid).unwrap();
    assert!(liked_asset.viewer_liked);
    assert!(liked_asset.viewer_bookmarked);
    assert!(liked_asset.viewer_following_author);
    assert!(svc2.get_asset(&pub_id).is_some());
    assert!(svc2.get_draft(&draft_id).is_some());
    assert_eq!(
        svc2.list_comments_local(&liked_sid, CodeSceneCommentSort::Newest)
            .total,
        1
    );
    assert!(svc2.pending_telemetry_count() > 0);

    // 0% blocklist leak after restart
    svc2.load_feed();
    let feed_items = svc2.items();
    assert!(!feed_items.iter().any(|i| i.asset.scene_id == disliked_sid));
    assert!(!feed_items
        .iter()
        .any(|i| i.asset.author_user_id == hidden_author));
    assert!(!feed_items
        .iter()
        .any(|i| i.asset.tags.iter().any(|t| t.eq_ignore_ascii_case("physics"))));
}

fn count_all_code_scene_tables(db_path: &std::path::Path) -> Vec<(&'static str, i64)> {
    let conn = Connection::open(db_path).unwrap();
    let tables = [
        "code_scenes_cache",
        "code_scene_local_profile",
        "code_scene_telemetry_outbox",
        "code_scene_local_drafts",
        "code_scene_local_social",
        "code_scene_local_comments",
    ];
    tables
        .into_iter()
        .map(|t| {
            let cnt: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap();
            (t, cnt)
        })
        .collect()
}

// Group 7 & 8: Account switch isolation (live, TelegramClient::set_auth_session, restart, different server_url, and guest->auth)
#[tokio::test]
async fn test_account_switch_and_guest_isolation_zero_leak_across_all_6_tables() {
    let dir = TempDir::new().unwrap();
    let srv_a = "http://127.0.0.1:18081";
    let srv_b = "http://127.0.0.1:18082";
    let seeds = official_seed_assets();
    let sid0 = seeds[0].scene_id.clone();
    let sid1 = seeds[1].scene_id.clone();

    let cfg = ClientConfig {
        server_url: srv_a.to_string(),
        ..ClientConfig::default()
    };
    let tg = TelegramClient::open_with_dir(cfg, dir.path()).unwrap();
    tg.set_auth_session(mock_auth_session(1001, "alice", "tok_a"));
    let svc = tg.build_code_scene_service();
    svc.load_feed();
    svc.toggle_like_local(&sid0).unwrap();
    svc.toggle_bookmark_local(&sid0).unwrap();
    svc.toggle_follow_local(seeds[0].author_user_id);
    let _ = svc.open_scene_by_id(&sid1);
    svc.record_interaction(SceneBehaviorEventKind::NotInterested, None);
    svc.record_interaction(SceneBehaviorEventKind::HideTag, Some("retro".into()));
    svc.add_comment_local(&sid0, "Alice private comment", None).unwrap();
    let _ = svc.ai_generate_local(&AiGenerateSceneRequest {
        prompt: "Alice draft".into(),
        ..Default::default()
    });
    let _ = svc
        .publish_scene_local(&PublishCodeSceneRequest {
            title: "Alice Private Local Scene".into(),
            kind: AssetKind::Animation,
            source_code: valid_cs1_source(),
            manifest: AssetManifest::for_kind(AssetKind::Animation),
            ..Default::default()
        })
        .unwrap();

    // Verify all 6 tables populated for Account A
    let db_path = dir.path().join("client_cache.db");
    for (tbl, cnt) in count_all_code_scene_tables(&db_path) {
        assert!(cnt > 0, "Expected rows in {tbl} for Account A");
    }

    // (a) Switch via TelegramClient::set_auth_session to Account B -> fires LocalStore::clear_all + clear_hook!
    tg.set_auth_session(mock_auth_session(2002, "bob", "tok_b"));
    for (tbl, cnt) in count_all_code_scene_tables(&db_path) {
        assert_eq!(cnt, 0, "Table {tbl} must be wiped to 0 rows on Account B switch");
    }
    svc.switch_account_scope(srv_a, 2002, Some("bob".into()));
    assert_eq!(svc.catalog_len(), 18);
    assert_eq!(svc.list_drafts().len(), 0);
    assert_eq!(svc.pending_telemetry_count(), 0);
    let b_asset = svc.get_asset(&sid0).unwrap();
    assert!(!b_asset.viewer_liked && !b_asset.viewer_bookmarked && !b_asset.viewer_following_author);
    assert!(svc.export_active_profile().disliked_scene_ids.is_empty());

    // Populate Account B, drop client, reopen same TempDir as Account C (process restart switch!)
    svc.toggle_like_local(&sid0).unwrap();
    tokio::task::spawn_blocking(move || drop(tg)).await.unwrap();

    let svc_c =
        LocalCodeSceneService::open_with_dir(dir.path(), srv_a, 3003, Some("charlie".into()), None)
            .unwrap();
    for (tbl, cnt) in count_all_code_scene_tables(&db_path) {
        assert_eq!(cnt, 0, "Table {tbl} must be wiped on reopen as Account C");
    }
    assert!(!svc_c.get_asset(&sid0).unwrap().viewer_liked);

    // Same user_id on a DIFFERENT server_url must also wipe!
    svc_c.toggle_like_local(&sid0).unwrap();
    svc_c.switch_account_scope(srv_b, 3003, Some("charlie".into()));
    for (tbl, cnt) in count_all_code_scene_tables(&db_path) {
        assert_eq!(cnt, 0, "Table {tbl} must be wiped when server_url changes");
    }

    // Guest (user_id == 0) -> authenticated user_id isolation
    let guest_dir = TempDir::new().unwrap();
    let guest_svc =
        LocalCodeSceneService::open_with_dir(guest_dir.path(), srv_a, 0, None, None).unwrap();
    guest_svc.load_feed();
    guest_svc.toggle_like_local(&sid0).unwrap();
    let _ = guest_svc.ai_generate_local(&AiGenerateSceneRequest {
        prompt: "guest draft".into(),
        ..Default::default()
    });
    guest_svc.switch_account_scope(srv_a, 4004, Some("dave".into()));
    assert!(!guest_svc.get_asset(&sid0).unwrap().viewer_liked);
    assert!(guest_svc.list_drafts().is_empty());
    assert_eq!(guest_svc.pending_telemetry_count(), 0);
}

// Group 9, 10, 11 & 12: Outbox dedup/retry, 6-step sync no-double-toggle, epoch fencing, and RAM clear_all parity
#[tokio::test]
async fn test_outbox_retry_sync_no_double_toggle_epoch_fence_and_in_memory_parity() {
    let srv = MockCodeSceneServer::start().await;
    let dir = TempDir::new().unwrap();
    let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
    let svc = LocalCodeSceneService::open_with_dir(
        dir.path(),
        &srv.base_url,
        1001,
        Some("alice".into()),
        Some(http),
    )
    .unwrap();
    let seed0 = official_seed_assets()[0].scene_id.clone();

    // 1. Outbox dedup + 503 failure retry preservation
    let dup_ev = SceneBehaviorEventDto {
        event_id: "fixed_dedup_ev_1".into(),
        user_id: 1001,
        scene_id: seed0.clone(),
        kind: SceneBehaviorEventKind::Dwell,
        dwell_ms: 4200,
        ..Default::default()
    };
    let r1 = svc.local_store().enqueue_code_scene_telemetry(dup_ev.clone());
    let r2 = svc.local_store().enqueue_code_scene_telemetry(dup_ev);
    assert_eq!(r1.outbox_id, r2.outbox_id);
    assert_eq!(svc.pending_telemetry_count(), 1);

    srv.fail_telemetry_503.store(true, Ordering::SeqCst);
    assert!(svc.flush_telemetry_outbox("valid-token").await.is_err());
    let after_fail = svc.local_store().list_code_scene_telemetry_outbox(1001, 10);
    assert_eq!(after_fail.len(), 1);
    assert_eq!(after_fail[0].retry_count, 1);
    assert!(after_fail[0].last_error.is_some());

    srv.fail_telemetry_503.store(false, Ordering::SeqCst);
    let flushed = svc.flush_telemetry_outbox("valid-token").await.unwrap();
    assert_eq!(flushed.events_acked, 1);
    assert_eq!(flushed.remaining_pending, 0);

    // 2. Offline work + 6-step sync convergence WITHOUT double-toggle!
    let local_pub = svc
        .publish_scene_local(&PublishCodeSceneRequest {
            title: "Offline Sync Scene".into(),
            kind: AssetKind::Animation,
            source_code: valid_cs1_source(),
            manifest: AssetManifest::for_kind(AssetKind::Animation),
            ..Default::default()
        })
        .unwrap();
    svc.add_comment_local(&seed0, "Offline sync comment", None)
        .unwrap();
    // Like once -> true; Bookmark twice -> true then false (should NOT flip server bookmark!)
    svc.toggle_like_local(&seed0).unwrap();
    svc.toggle_bookmark_local(&seed0).unwrap();
    svc.toggle_bookmark_local(&seed0).unwrap();
    svc.local_store()
        .enqueue_code_scene_telemetry(SceneBehaviorEventDto {
            event_id: "sync_dwell_ev_2".into(),
            user_id: 1001,
            scene_id: seed0.clone(),
            kind: SceneBehaviorEventKind::Complete,
            dwell_ms: 15000,
            completion_ratio: 1.0,
            ..Default::default()
        });

    let rep1 = svc.sync_with_server("valid-token").await.unwrap();
    assert_eq!(rep1.pushed_assets, 1);
    assert_eq!(rep1.pushed_comments, 1);
    assert_eq!(rep1.telemetry.events_acked, 1);
    assert!(rep1.pulled_assets >= 19);
    assert_eq!(srv.state.lock().like_calls, 1, "Like must be toggled EXACTLY once");
    assert_eq!(
        srv.state.lock().bookmark_calls,
        0,
        "Bookmark toggled twice (true->false) matches server false -> 0 server toggles!"
    );
    assert!(svc.get_asset(&seed0).unwrap().viewer_liked);
    assert!(!svc.get_asset(&seed0).unwrap().viewer_bookmarked);
    assert!(srv.state.lock().recommender.get_asset(&local_pub.asset.scene_id).is_some());

    // Second sync is a clean idempotent 0-mutation no-op
    let rep2 = svc.sync_with_server("valid-token").await.unwrap();
    assert_eq!(rep2.pushed_assets, 0);
    assert_eq!(rep2.pushed_comments, 0);
    assert_eq!(rep2.telemetry.events_acked, 0);
    assert_eq!(rep2.reconciled_social, 0);

    // 3. Mid-flight account switch fences stale async sync!
    srv.delay_ms.store(180, Ordering::SeqCst);
    svc.local_store()
        .enqueue_code_scene_telemetry(SceneBehaviorEventDto {
            event_id: "slow_alice_ev".into(),
            user_id: 1001,
            scene_id: seed0.clone(),
            kind: SceneBehaviorEventKind::Dwell,
            dwell_ms: 3000,
            ..Default::default()
        });
    let svc_bg = svc.clone();
    let sync_handle = tokio::spawn(async move { svc_bg.sync_with_server("valid-token").await });
    tokio::time::sleep(Duration::from_millis(40)).await;
    svc.switch_account_scope(&srv.base_url, 2002, Some("bob".into()));
    let _ = sync_handle.await.unwrap();
    srv.delay_ms.store(0, Ordering::SeqCst);

    // Account B MUST remain pristine (not contaminated by Account A's late response!)
    assert_eq!(svc.active_account_scope().1, 2002);
    assert!(!svc.get_asset(&seed0).unwrap().viewer_liked);
    assert_eq!(svc.pending_telemetry_count(), 0);

    // 4. In-memory LocalStore parity with clear_all()
    let mem_svc = LocalCodeSceneService::new_offline(55, Some("mem".into()));
    mem_svc.load_feed();
    mem_svc.toggle_like_local(&seed0).unwrap();
    mem_svc.add_comment_local(&seed0, "RAM comment", None).unwrap();
    let _ = mem_svc.ai_generate_local(&AiGenerateSceneRequest {
        prompt: "RAM draft".into(),
        ..Default::default()
    });
    mem_svc.local_store().clear_all();
    assert_eq!(mem_svc.list_drafts().len(), 0);
    assert_eq!(mem_svc.pending_telemetry_count(), 0);
    assert_eq!(
        mem_svc
            .list_comments_local(&seed0, CodeSceneCommentSort::Hot)
            .total,
        0
    );
    assert!(!mem_svc.get_asset(&seed0).unwrap().viewer_liked);
    let _ = HashSet::<String>::new();
}

// Iteration-2 Regressions: Guest->Auth 6-table isolation (3 paths), 503 fallback epoch fence,
// Step 4 server social hydration, topological parent-first remix sync, and live session counter refresh.
#[tokio::test]
async fn test_code_scene_guest_to_auth_6_table_isolation_all_3_paths() {
    let srv_url = "http://127.0.0.1:18090";
    let seeds = official_seed_assets();
    let sid0 = seeds[0].scene_id.clone();
    let orig_likes = seeds[0].likes_count;
    let orig_comments = seeds[0].comments_count;

    // Path (a): Live guest_svc.switch_account_scope(0 -> 4004)
    {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("client_cache.db");
        let guest = LocalCodeSceneService::open_with_dir(dir.path(), srv_url, 0, None, None).unwrap();
        guest.load_feed();
        guest.toggle_like_local(&sid0).unwrap();
        guest.add_comment_local(&sid0, "guest live comment", None).unwrap();
        let _ = guest.ai_generate_local(&AiGenerateSceneRequest { prompt: "guest draft".into(), ..Default::default() });
        let _ = guest.publish_scene_local(&PublishCodeSceneRequest {
            title: "Guest Local Scene".into(),
            kind: AssetKind::Animation,
            source_code: valid_cs1_source(),
            manifest: AssetManifest::for_kind(AssetKind::Animation),
            ..Default::default()
        }).unwrap();
        guest.switch_account_scope(srv_url, 4004, Some("dave".into()));
        for (tbl, cnt) in count_all_code_scene_tables(&db_path) {
            assert_eq!(cnt, 0, "Path (a): table {tbl} leaked guest rows after switch_account_scope");
        }
        let a = guest.get_asset(&sid0).unwrap();
        assert!(!a.viewer_liked);
        assert_eq!(a.likes_count, orig_likes);
        assert_eq!(a.comments_count, orig_comments);
        assert_eq!(guest.catalog_len(), 18);
    }

    // Path (b): Guest drop + reopen LocalCodeSceneService::open_with_dir as Account 4004
    {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("client_cache.db");
        {
            let guest = LocalCodeSceneService::open_with_dir(dir.path(), srv_url, 0, None, None).unwrap();
            guest.load_feed();
            guest.toggle_like_local(&sid0).unwrap();
            guest.add_comment_local(&sid0, "guest restart comment", None).unwrap();
            let _ = guest.publish_scene_local(&PublishCodeSceneRequest {
                title: "Guest Restart Scene".into(),
                kind: AssetKind::Animation,
                source_code: valid_cs1_source(),
                manifest: AssetManifest::for_kind(AssetKind::Animation),
                ..Default::default()
            }).unwrap();
        }
        let dave = LocalCodeSceneService::open_with_dir(dir.path(), srv_url, 4004, Some("dave".into()), None).unwrap();
        for (tbl, cnt) in count_all_code_scene_tables(&db_path) {
            assert_eq!(cnt, 0, "Path (b): table {tbl} leaked guest rows after reopen");
        }
        let a = dave.get_asset(&sid0).unwrap();
        assert!(!a.viewer_liked);
        assert_eq!(a.likes_count, orig_likes);
        assert_eq!(a.comments_count, orig_comments);
        assert_eq!(dave.catalog_len(), 18);
    }

    // Path (c): TelegramClient::open_with_dir -> Guest build_code_scene_service -> set_auth_session -> build_code_scene_service
    {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("client_cache.db");
        let cfg = ClientConfig {
            server_url: srv_url.to_string(),
            ..ClientConfig::default()
        };
        let tg = TelegramClient::open_with_dir(cfg, dir.path()).unwrap();
        let guest = tg.build_code_scene_service();
        guest.load_feed();
        guest.toggle_like_local(&sid0).unwrap();
        guest.add_comment_local(&sid0, "guest tg comment", None).unwrap();
        let _ = guest.ai_generate_local(&AiGenerateSceneRequest { prompt: "guest tg draft".into(), ..Default::default() });
        let _ = guest.publish_scene_local(&PublishCodeSceneRequest {
            title: "Guest TG Scene".into(),
            kind: AssetKind::Animation,
            source_code: valid_cs1_source(),
            manifest: AssetManifest::for_kind(AssetKind::Animation),
            ..Default::default()
        }).unwrap();

        tg.set_auth_session(mock_auth_session(4004, "dave", "tok_d"));
        for (tbl, cnt) in count_all_code_scene_tables(&db_path) {
            assert_eq!(cnt, 0, "Path (c): table {tbl} must be 0 immediately after set_auth_session");
        }
        let dave = tg.build_code_scene_service();
        for (tbl, cnt) in count_all_code_scene_tables(&db_path) {
            assert_eq!(cnt, 0, "Path (c): table {tbl} must remain 0 after build_code_scene_service");
        }
        let a = dave.get_asset(&sid0).unwrap();
        assert!(!a.viewer_liked);
        assert_eq!(a.likes_count, orig_likes);
        assert_eq!(a.comments_count, orig_comments);
        assert_eq!(dave.list_comments_local(&sid0, CodeSceneCommentSort::Newest).total, 0);
        assert_eq!(dave.catalog_len(), 18);

        tokio::task::spawn_blocking(move || drop(tg)).await.unwrap();
    }
}

#[tokio::test]
async fn test_code_scene_503_fallback_epoch_fence_server_hydration_topological_remix_and_session_refresh() {
    let srv = MockCodeSceneServer::start().await;
    let seeds = official_seed_assets();
    let seed0 = seeds[0].scene_id.clone();
    let seed1 = seeds[1].scene_id.clone();
    let seed1_author = seeds[1].author_user_id;

    // 1. Slow 503 offline fallback fenced by account_epoch on toggle_like + add_comment!
    {
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            2002,
            Some("bob".into()),
            Some(http),
        )
        .unwrap();
        srv.delay_ms.store(160, Ordering::SeqCst);
        srv.fail_all_503.store(true, Ordering::SeqCst);

        let svc1 = svc.clone();
        let s0_a = seed0.clone();
        let h_like = tokio::spawn(async move { svc1.toggle_like(&s0_a, Some("valid-token")).await });

        let svc2 = svc.clone();
        let s0_b = seed0.clone();
        let h_cmt = tokio::spawn(async move {
            svc2.add_comment(
                &s0_b,
                &CreateCodeSceneCommentRequest {
                    content: "Bob SECRET comment".into(),
                    parent_comment_id: None,
                },
                Some("valid-token"),
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(40)).await;
        svc.switch_account_scope(&srv.base_url, 3003, Some("charlie".into()));

        let res_like = h_like.await.unwrap();
        let res_cmt = h_cmt.await.unwrap();
        assert!(
            matches!(res_like, Err(TelegramError::Auth(_))),
            "Mid-flight switch must fence 503 toggle_like fallback with Auth error, got {res_like:?}"
        );
        assert!(
            matches!(res_cmt, Err(TelegramError::Auth(_))),
            "Mid-flight switch must fence 503 add_comment fallback with Auth error, got {res_cmt:?}"
        );
        assert!(!svc.get_asset(&seed0).unwrap().viewer_liked);
        assert_eq!(svc.list_comments_local(&seed0, CodeSceneCommentSort::Newest).total, 0);

        srv.fail_all_503.store(false, Ordering::SeqCst);
        srv.delay_ms.store(0, Ordering::SeqCst);
    }

    // 2. Fresh LocalCodeSceneService + load_feed() + sync_with_server hydrates server social flags!
    {
        {
            let mut g = srv.state.lock();
            g.recommender.toggle_like(1001, &seed1);
            g.recommender.toggle_bookmark(1001, &seed1);
            g.recommender.toggle_follow(1001, seed1_author);
        }
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http),
        )
        .unwrap();
        svc.load_feed();
        let rep = svc.sync_with_server("valid-token").await.unwrap();
        assert!(rep.errors.is_empty(), "sync errors: {:?}", rep.errors);

        let a = svc.get_asset(&seed1).unwrap();
        assert!(a.viewer_liked, "Step 4 must hydrate server viewer_liked");
        assert!(a.viewer_bookmarked, "Step 4 must hydrate server viewer_bookmarked");
        assert!(a.viewer_following_author, "Step 4 must hydrate server viewer_following_author");

        // Live session current_item() also reflects hydrated flags + live counter mutations!
        let opened = svc.open_scene_by_id(&seed1).unwrap();
        assert!(opened.asset.viewer_liked && opened.asset.viewer_bookmarked && opened.asset.viewer_following_author);
        let before_cmts = svc.current_item().unwrap().asset.comments_count;
        svc.add_comment_local(&seed1, "Live counter check", None).unwrap();
        assert_eq!(svc.current_item().unwrap().asset.comments_count, before_cmts + 1);
    }

    // 3. Offline parent + child + grandchild remixes sync topologically parent-first with 0 404s!
    {
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http),
        )
        .unwrap();
        let parent = svc
            .publish_scene_local(&PublishCodeSceneRequest {
                title: "Offline Parent Scene".into(),
                kind: AssetKind::Animation,
                source_code: valid_cs1_source(),
                manifest: AssetManifest::for_kind(AssetKind::Animation),
                ..Default::default()
            })
            .unwrap();
        let child = svc
            .publish_scene_local(&PublishCodeSceneRequest {
                title: "Offline Child Remix".into(),
                kind: AssetKind::Animation,
                source_code: valid_cs1_source(),
                manifest: AssetManifest::for_kind(AssetKind::Animation),
                remix_parent_id: Some(parent.asset.scene_id.clone()),
                ..Default::default()
            })
            .unwrap();
        let grandchild = svc
            .publish_scene_local(&PublishCodeSceneRequest {
                title: "Offline Grandchild Remix".into(),
                kind: AssetKind::Animation,
                source_code: valid_cs1_source(),
                manifest: AssetManifest::for_kind(AssetKind::Animation),
                remix_parent_id: Some(child.asset.scene_id.clone()),
                ..Default::default()
            })
            .unwrap();

        let rep = svc.sync_with_server("valid-token").await.unwrap();
        assert!(
            rep.errors.is_empty(),
            "Parent-first topological remix sync must have 0 errors, got {:?}",
            rep.errors
        );
        assert_eq!(rep.pushed_assets, 3);
        let g = srv.state.lock();
        assert!(g.recommender.get_asset(&parent.asset.scene_id).is_some());
        assert_eq!(
            g.recommender
                .get_asset(&child.asset.scene_id)
                .unwrap()
                .remix_parent_id
                .as_deref(),
            Some(parent.asset.scene_id.as_str())
        );
        assert_eq!(
            g.recommender
                .get_asset(&grandchild.asset.scene_id)
                .unwrap()
                .remix_parent_id
                .as_deref(),
            Some(child.asset.scene_id.as_str())
        );
    }
}

// Iteration-3 Regressions:
// (1) HideAuthor <-> Follow parity across online toggle, Step-4 sync+reopen, and offline HideAuthor.
// (2) Guest mid-flight 503 started BEFORE any local write fenced unconditionally by set_auth_session.
// (3) 3-level offline reply comment chain topological parent-first sync + remapped_comment_ids + retry.
#[tokio::test]
async fn test_code_scene_hide_author_follow_parity_all_3_subcases() {
    let seeds = official_seed_assets();
    let s1 = seeds.iter().find(|a| a.author_user_id == 777101).unwrap().clone();
    let s2 = seeds.iter().find(|a| a.author_user_id == 777102).unwrap().clone();
    let s3 = seeds.iter().find(|a| a.author_user_id == 777103).unwrap().clone();

    // Sub1: HideAuthor -> online toggle_follow(author, Some(tok)) un-hides in RAM & SQLite and
    // survives drop+reopen AND subsequent sync_with_server without resurrecting HideAuthor!
    {
        let srv = MockCodeSceneServer::start().await;
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http.clone()),
        )
        .unwrap();
        svc.load_feed();
        let _ = svc.open_scene_by_id(&s1.scene_id).unwrap();
        svc.record_interaction(SceneBehaviorEventKind::HideAuthor, None);
        assert!(svc.export_active_profile().hidden_author_ids.contains(&s1.author_user_id));

        let resp = svc.toggle_follow(s1.author_user_id, Some("valid-token")).await.unwrap();
        assert!(resp.following);
        assert!(svc.get_asset(&s1.scene_id).unwrap().viewer_following_author);
        let prof = svc.export_active_profile();
        assert!(prof.followed_creators.contains(&s1.author_user_id));
        assert!(!prof.hidden_author_ids.contains(&s1.author_user_id));

        // Subsequent sync must NOT replay stale queued HideAuthor!
        let rep = svc.sync_with_server("valid-token").await.unwrap();
        assert!(rep.errors.is_empty());
        assert!(svc.get_asset(&s1.scene_id).unwrap().viewer_following_author);

        drop(svc);
        let svc2 = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http),
        )
        .unwrap();
        assert!(svc2.get_asset(&s1.scene_id).unwrap().viewer_following_author);
        assert!(!svc2.export_active_profile().hidden_author_ids.contains(&s1.author_user_id));
    }

    // Sub2: Local HideAuthor -> cross-device server follow -> sync_with_server -> drop+reopen
    // MUST persist following=true and NOT resurrect HideAuthor!
    {
        let srv = MockCodeSceneServer::start().await;
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http.clone()),
        )
        .unwrap();
        svc.load_feed();
        let _ = svc.open_scene_by_id(&s2.scene_id).unwrap();
        svc.record_interaction(SceneBehaviorEventKind::HideAuthor, None);
        // Flush HideAuthor first so server & client both saw the earlier hide.
        let _ = svc.flush_telemetry_outbox("valid-token").await.unwrap();

        // Cross-device follow on server un-hides and follows s2.author_user_id.
        {
            let mut g = srv.state.lock();
            if let Some(mut p) = g.recommender.export_user_profile(1001) {
                p.hidden_author_ids.remove(&s2.author_user_id);
                p.creator_weights.remove(&s2.author_user_id);
                p.sanitize();
                g.recommender.import_user_profile(p);
            }
            let r = g.recommender.toggle_follow(1001, s2.author_user_id);
            assert!(r.following);
        }

        let rep = svc.sync_with_server("valid-token").await.unwrap();
        assert!(rep.errors.is_empty());
        assert!(svc.get_asset(&s2.scene_id).unwrap().viewer_following_author);
        assert!(!svc.export_active_profile().hidden_author_ids.contains(&s2.author_user_id));

        drop(svc);
        let svc2 = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http),
        )
        .unwrap();
        assert!(
            svc2.get_asset(&s2.scene_id).unwrap().viewer_following_author,
            "Sub2: drop+reopen must keep following=true without resurrecting HideAuthor"
        );
        assert!(
            !svc2.export_active_profile().hidden_author_ids.contains(&s2.author_user_id),
            "Sub2: HideAuthor must remain deactivated after reopen"
        );
    }

    // Sub3: Offline toggle_follow_local(A) -> record_interaction(HideAuthor) -> sync_with_server
    // MUST NOT replay stale Follow and re-follow/un-hide blocked author on server!
    {
        let srv = MockCodeSceneServer::start().await;
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http.clone()),
        )
        .unwrap();
        svc.load_feed();
        let flw = svc.toggle_follow_local(s3.author_user_id);
        assert!(flw.following);

        let _ = svc.open_scene_by_id(&s3.scene_id).unwrap();
        svc.record_interaction(SceneBehaviorEventKind::HideAuthor, None);
        assert!(!svc.get_asset(&s3.scene_id).unwrap().viewer_following_author);
        assert!(svc.export_active_profile().hidden_author_ids.contains(&s3.author_user_id));

        let rep = svc.sync_with_server("valid-token").await.unwrap();
        assert!(rep.errors.is_empty());
        {
            let g = srv.state.lock();
            assert_eq!(
                g.follow_calls, 0,
                "Sub3: stale offline Follow must NOT be replayed after HideAuthor!"
            );
            let srv_prof = g.recommender.export_user_profile(1001).unwrap();
            assert!(srv_prof.hidden_author_ids.contains(&s3.author_user_id));
            assert!(!srv_prof.followed_creators.contains(&s3.author_user_id));
        }
        assert!(!svc.get_asset(&s3.scene_id).unwrap().viewer_following_author);
        assert!(svc.export_active_profile().hidden_author_ids.contains(&s3.author_user_id));

        drop(svc);
        let svc2 = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http),
        )
        .unwrap();
        assert!(!svc2.get_asset(&s3.scene_id).unwrap().viewer_following_author);
        assert!(svc2.export_active_profile().hidden_author_ids.contains(&s3.author_user_id));
    }
}

#[tokio::test]
async fn test_code_scene_guest_midflight_503_before_any_write_fenced_by_set_auth_session() {
    let srv = MockCodeSceneServer::start().await;
    srv.delay_ms.store(160, Ordering::SeqCst);
    srv.fail_all_503.store(true, Ordering::SeqCst);

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("client_cache.db");
    let seeds = official_seed_assets();
    let seed0 = seeds[0].scene_id.clone();
    let orig_likes = seeds[0].likes_count;

    let cfg = ClientConfig {
        server_url: srv.base_url.clone(),
        ..ClientConfig::default()
    };
    let tg = TelegramClient::open_with_dir(cfg, dir.path()).unwrap();
    let guest = tg.build_code_scene_service();
    // Verify all 6 Code Scene tables are initially EMPTY (zero prior guest writes!).
    for (tbl, cnt) in count_all_code_scene_tables(&db_path) {
        assert_eq!(cnt, 0, "Precondition: {tbl} must start empty");
    }

    let guest_bg = guest.clone();
    let s0 = seed0.clone();
    let handle = tokio::spawn(async move { guest_bg.toggle_like(&s0, Some("valid-token")).await });

    tokio::time::sleep(Duration::from_millis(40)).await;
    tg.set_auth_session(mock_auth_session(101, "alice", "tok_101"));

    let res = handle.await.unwrap();
    assert!(
        matches!(res, Err(TelegramError::Auth(_))),
        "Guest mid-flight 503 started before any local write MUST be fenced with Auth error, got {res:?}"
    );
    for (tbl, cnt) in count_all_code_scene_tables(&db_path) {
        assert_eq!(cnt, 0, "Table {tbl} must remain at 0 rows after fenced Guest 503");
    }

    let auth_svc = tg.build_code_scene_service();
    let a = auth_svc.get_asset(&seed0).unwrap();
    assert!(!a.viewer_liked);
    assert_eq!(a.likes_count, orig_likes, "seed0 likes_count must remain unpolluted");

    tokio::task::spawn_blocking(move || drop(tg)).await.unwrap();
}

#[tokio::test]
async fn test_code_scene_offline_3_level_reply_comment_chain_topological_sync_and_retry() {
    let srv = MockCodeSceneServer::start().await;
    let seed0 = official_seed_assets()[0].scene_id.clone();

    // Part A: 3-level offline reply chain (root -> reply1 -> reply2) PLUS inverted-timestamp chain
    // must sync parent-first with exact remapped server parent_comment_ids and 0 404 errors!
    {
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http),
        )
        .unwrap();

        let root = svc.add_comment_local(&seed0, "root comment", None).unwrap();
        let reply1 = svc
            .add_comment_local(&seed0, "reply level 1", Some(root.comment.comment_id.clone()))
            .unwrap();
        let reply2 = svc
            .add_comment_local(&seed0, "reply level 2", Some(reply1.comment.comment_id.clone()))
            .unwrap();
        assert!(root.comment.comment_id.starts_with("local_csc_"));
        assert_eq!(reply1.comment.parent_comment_id.as_deref(), Some(root.comment.comment_id.as_str()));
        assert_eq!(reply2.comment.parent_comment_id.as_deref(), Some(reply1.comment.comment_id.as_str()));

        // Also insert an inverted-timestamp 3-level chain directly into local_store
        // (inv_reply2.ts < inv_reply1.ts == inv_root.ts) to prove topological sort ignores clock skew!
        svc.local_store()
            .upsert_code_scene_comment(gpui_telegram_client::CachedCodeSceneComment {
                comment: CodeSceneCommentDto {
                    comment_id: "local_csc_000_inv_reply2".into(),
                    scene_id: seed0.clone(),
                    author_user_id: 1001,
                    author_username: "alice".into(),
                    content: "inverted reply 2".into(),
                    parent_comment_id: Some("local_csc_001_inv_reply1".into()),
                    likes_count: 0,
                    viewer_liked: false,
                    created_at_ms: 1_700_000_000_001,
                },
                pending_sync: true,
            });
        svc.local_store()
            .upsert_code_scene_comment(gpui_telegram_client::CachedCodeSceneComment {
                comment: CodeSceneCommentDto {
                    comment_id: "local_csc_001_inv_reply1".into(),
                    scene_id: seed0.clone(),
                    author_user_id: 1001,
                    author_username: "alice".into(),
                    content: "inverted reply 1".into(),
                    parent_comment_id: Some("local_csc_999_inv_root".into()),
                    likes_count: 0,
                    viewer_liked: false,
                    created_at_ms: 1_700_000_000_002,
                },
                pending_sync: true,
            });
        svc.local_store()
            .upsert_code_scene_comment(gpui_telegram_client::CachedCodeSceneComment {
                comment: CodeSceneCommentDto {
                    comment_id: "local_csc_999_inv_root".into(),
                    scene_id: seed0.clone(),
                    author_user_id: 1001,
                    author_username: "alice".into(),
                    content: "inverted root".into(),
                    parent_comment_id: None,
                    likes_count: 0,
                    viewer_liked: false,
                    created_at_ms: 1_700_000_000_002,
                },
                pending_sync: true,
            });

        let rep = svc.sync_with_server("valid-token").await.unwrap();
        assert!(
            rep.errors.is_empty(),
            "3-level + inverted-ts reply chains must sync with 0 errors, got {:?}",
            rep.errors
        );
        assert_eq!(rep.pushed_comments, 6);
        assert!(svc.local_store().list_pending_code_scene_comments(1001).is_empty());

        let g = srv.state.lock();
        let cmts = g.comments.get(&seed0).unwrap();
        assert_eq!(cmts.len(), 6);
        let find = |txt: &str| cmts.iter().find(|c| c.content == txt).unwrap().clone();
        let s_root = find("root comment");
        let s_r1 = find("reply level 1");
        let s_r2 = find("reply level 2");
        assert!(s_root.comment_id.starts_with("srv_cmt_"));
        assert_eq!(s_root.parent_comment_id, None);
        assert_eq!(s_r1.parent_comment_id.as_deref(), Some(s_root.comment_id.as_str()));
        assert_eq!(s_r2.parent_comment_id.as_deref(), Some(s_r1.comment_id.as_str()));

        let s_iroot = find("inverted root");
        let s_ir1 = find("inverted reply 1");
        let s_ir2 = find("inverted reply 2");
        assert_eq!(s_iroot.parent_comment_id, None);
        assert_eq!(s_ir1.parent_comment_id.as_deref(), Some(s_iroot.comment_id.as_str()));
        assert_eq!(s_ir2.parent_comment_id.as_deref(), Some(s_ir1.comment_id.as_str()));
    }

    // Part B: Transient 503 on child reply AFTER parent succeeded persists remapped parent_comment_id
    // across drop+reopen so the next retry succeeds with 0 404s!
    {
        let srv2 = MockCodeSceneServer::start().await;
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv2.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv2.base_url,
            1001,
            Some("alice".into()),
            Some(http.clone()),
        )
        .unwrap();

        let root = svc.add_comment_local(&seed0, "retry root", None).unwrap();
        let _reply = svc
            .add_comment_local(&seed0, "retry child", Some(root.comment.comment_id.clone()))
            .unwrap();

        // Allow 1st comment POST (root) to succeed, fail 2nd comment POST (child) with 503!
        srv2.fail_comment_after_n.store(1, Ordering::SeqCst);
        let rep1 = svc.sync_with_server("valid-token").await.unwrap();
        assert_eq!(rep1.pushed_comments, 1);
        assert_eq!(rep1.errors.len(), 1);
        drop(svc);

        // Reopen from SQLite and retry after transient outage clears.
        srv2.fail_comment_after_n.store(0, Ordering::SeqCst);
        let svc2 = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv2.base_url,
            1001,
            Some("alice".into()),
            Some(http),
        )
        .unwrap();
        let rep2 = svc2.sync_with_server("valid-token").await.unwrap();
        assert!(
            rep2.errors.is_empty(),
            "Retry after parent remap must succeed without 404, got {:?}",
            rep2.errors
        );
        assert_eq!(rep2.pushed_comments, 1);

        let g = srv2.state.lock();
        let cmts = g.comments.get(&seed0).unwrap();
        assert_eq!(cmts.len(), 2);
        let s_root = cmts.iter().find(|c| c.content == "retry root").unwrap();
        let s_child = cmts.iter().find(|c| c.content == "retry child").unwrap();
        assert_eq!(s_child.parent_comment_id.as_deref(), Some(s_root.comment_id.as_str()));
        assert_eq!(
            srv2.comment_post_count.load(Ordering::SeqCst),
            3,
            "Expected 3 comment POSTs total (root ok + child 503 + child retry ok)"
        );
    }
}

// Iteration-4 Regressions:
// (1) Step-4 sync preserves pending HideAuthor outbox entries when Step-3 /telemetry transiently returns 503.
// (2) record_interaction(FollowAuthor) after HideAuthor un-hides in RAM, prunes outbox, and matches reopen.
// (3) Step-1 scene_id remap migrates & clears old_id social rows + local child remix_parent_ids cleanly.
#[tokio::test]
async fn test_code_scene_step4_preserves_pending_hide_author_on_telemetry_503() {
    let srv = MockCodeSceneServer::start().await;
    let dir = TempDir::new().unwrap();
    let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
    let seeds = official_seed_assets();
    let target_scene = seeds
        .iter()
        .find(|a| a.author_user_id == 777104)
        .unwrap_or(&seeds[0])
        .clone();
    let creator_uid = target_scene.author_user_id;

    let svc = LocalCodeSceneService::open_with_dir(
        dir.path(),
        &srv.base_url,
        1001,
        Some("alice".into()),
        Some(http.clone()),
    )
    .unwrap();
    svc.load_feed();

    // 1. Follow creator online (following == true on both server and client).
    let flw = svc
        .toggle_follow(creator_uid, Some("valid-token"))
        .await
        .unwrap();
    assert!(flw.following);
    assert!(svc.get_asset(&target_scene.scene_id).unwrap().viewer_following_author);

    // 2. Hide creator locally -> queued in code_scene_telemetry_outbox.
    let _ = svc.open_scene_by_id(&target_scene.scene_id).unwrap();
    svc.record_interaction(SceneBehaviorEventKind::HideAuthor, None);
    assert!(svc.export_active_profile().hidden_author_ids.contains(&creator_uid));
    assert!(!svc.export_active_profile().followed_creators.contains(&creator_uid));
    assert!(svc.pending_telemetry_count() >= 1);

    // 3. Sync #1: POST /telemetry transiently returns 503 while GET /assets returns 200 with stale server_following=true.
    srv.fail_telemetry_503.store(true, Ordering::SeqCst);
    let rep1 = svc.sync_with_server("valid-token").await.unwrap();
    assert!(!rep1.errors.is_empty(), "Expected transient telemetry 503 error in Sync #1");
    assert!(
        svc.export_active_profile().hidden_author_ids.contains(&creator_uid),
        "Step 4 must NOT wipe hidden_author_ids while HideAuthor is still pending in telemetry outbox"
    );
    assert!(!svc.export_active_profile().followed_creators.contains(&creator_uid));
    assert!(!svc.get_asset(&target_scene.scene_id).unwrap().viewer_following_author);
    svc.load_feed();
    assert!(
        !svc.items().iter().any(|i| i.asset.author_user_id == creator_uid),
        "Blocked creator must not leak into ForYou feed after telemetry 503 Sync #1"
    );

    // Drop & reopen BEFORE telemetry recovers: blocklist + pending HideAuthor must still be intact.
    drop(svc);
    let svc2 = LocalCodeSceneService::open_with_dir(
        dir.path(),
        &srv.base_url,
        1001,
        Some("alice".into()),
        Some(http.clone()),
    )
    .unwrap();
    assert!(svc2.export_active_profile().hidden_author_ids.contains(&creator_uid));
    assert!(!svc2.export_active_profile().followed_creators.contains(&creator_uid));
    assert!(svc2.pending_telemetry_count() >= 1);

    // 4. Telemetry recovers (200 OK) and Sync #2 runs -> server hides & unfollows, client stays hidden.
    srv.fail_telemetry_503.store(false, Ordering::SeqCst);
    let rep2 = svc2.sync_with_server("valid-token").await.unwrap();
    assert!(rep2.errors.is_empty(), "Sync #2 errors: {:?}", rep2.errors);
    assert_eq!(svc2.pending_telemetry_count(), 0);
    {
        let g = srv.state.lock();
        let srv_prof = g.recommender.export_user_profile(1001).unwrap();
        assert!(srv_prof.hidden_author_ids.contains(&creator_uid));
        assert!(!srv_prof.followed_creators.contains(&creator_uid));
    }
    assert!(svc2.export_active_profile().hidden_author_ids.contains(&creator_uid));
    assert!(!svc2.get_asset(&target_scene.scene_id).unwrap().viewer_following_author);
    svc2.load_feed();
    assert!(
        !svc2.items().iter().any(|i| i.asset.author_user_id == creator_uid),
        "Blocked creator must stay hidden in ForYou feed after Sync #2"
    );

    // 5. Drop & reopen after Sync #2 -> author MUST remain permanently hidden.
    drop(svc2);
    let svc3 = LocalCodeSceneService::open_with_dir(
        dir.path(),
        &srv.base_url,
        1001,
        Some("alice".into()),
        Some(http),
    )
    .unwrap();
    assert!(svc3.export_active_profile().hidden_author_ids.contains(&creator_uid));
    assert!(!svc3.get_asset(&target_scene.scene_id).unwrap().viewer_following_author);
    svc3.load_feed();
    assert!(
        !svc3.items().iter().any(|i| i.asset.author_user_id == creator_uid),
        "Blocked creator must stay hidden across drop+reopen"
    );
}

#[tokio::test]
async fn test_code_scene_record_interaction_follow_after_hide_parity() {
    let srv = MockCodeSceneServer::start().await;
    let dir = TempDir::new().unwrap();
    let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
    let seeds = official_seed_assets();
    let s1 = seeds
        .iter()
        .find(|a| a.author_user_id == 777101)
        .unwrap_or(&seeds[0])
        .clone();
    let creator_uid = s1.author_user_id;

    let svc = LocalCodeSceneService::open_with_dir(
        dir.path(),
        &srv.base_url,
        1001,
        Some("alice".into()),
        Some(http.clone()),
    )
    .unwrap();
    svc.load_feed();

    // 1. HideAuthor on active card s1 (marks s1 disliked + hides creator_uid + queues HideAuthor in outbox).
    let _ = svc.open_scene_by_id(&s1.scene_id).unwrap();
    svc.record_interaction(SceneBehaviorEventKind::HideAuthor, None);
    {
        let p = svc.export_active_profile();
        assert!(p.hidden_author_ids.contains(&creator_uid));
        assert!(!p.followed_creators.contains(&creator_uid));
        assert!(p.disliked_scene_ids.contains(&s1.scene_id));
    }
    let outbox_before = svc
        .local_store()
        .list_code_scene_telemetry_outbox(1001, usize::MAX);
    assert!(
        outbox_before
            .iter()
            .any(|r| r.event.kind == SceneBehaviorEventKind::HideAuthor),
        "HideAuthor must be queued in outbox before follow"
    );

    // 2. Re-open exact same card s1 and call record_interaction(FollowAuthor, None).
    let _ = svc.open_scene_by_id(&s1.scene_id).unwrap();
    let _ = svc.record_interaction(SceneBehaviorEventKind::FollowAuthor, None);

    // Immediate RAM state MUST reflect following=true, hidden=false, and stale HideAuthor pruned!
    {
        let p = svc.export_active_profile();
        assert!(
            p.followed_creators.contains(&creator_uid),
            "RAM profile must follow creator immediately after record_interaction(FollowAuthor)"
        );
        assert!(
            !p.hidden_author_ids.contains(&creator_uid),
            "RAM profile must un-hide creator immediately after record_interaction(FollowAuthor)"
        );
    }
    assert!(svc.get_asset(&s1.scene_id).unwrap().viewer_following_author);
    assert!(svc.current_item().unwrap().asset.viewer_following_author);
    let outbox_after = svc
        .local_store()
        .list_code_scene_telemetry_outbox(1001, usize::MAX);
    assert!(
        !outbox_after
            .iter()
            .any(|r| r.event.kind == SceneBehaviorEventKind::HideAuthor
                && r.event.author_user_id == Some(creator_uid)),
        "Stale queued HideAuthor must be pruned from outbox on record_interaction(FollowAuthor)"
    );

    // 3. Exact parity after drop + reopen!
    drop(svc);
    let svc2 = LocalCodeSceneService::open_with_dir(
        dir.path(),
        &srv.base_url,
        1001,
        Some("alice".into()),
        Some(http.clone()),
    )
    .unwrap();
    {
        let p = svc2.export_active_profile();
        assert!(p.followed_creators.contains(&creator_uid));
        assert!(!p.hidden_author_ids.contains(&creator_uid));
    }
    assert!(svc2.get_asset(&s1.scene_id).unwrap().viewer_following_author);

    // 4. Sync with server reconciles Follow cleanly without replaying stale HideAuthor!
    let rep = svc2.sync_with_server("valid-token").await.unwrap();
    assert!(rep.errors.is_empty(), "sync errors: {:?}", rep.errors);
    assert_eq!(srv.state.lock().follow_calls, 1);
    assert!(svc2.get_asset(&s1.scene_id).unwrap().viewer_following_author);
    assert!(!svc2.export_active_profile().hidden_author_ids.contains(&creator_uid));
    {
        let g = srv.state.lock();
        let srv_prof = g.recommender.export_user_profile(1001).unwrap();
        assert!(srv_prof.followed_creators.contains(&creator_uid));
        assert!(!srv_prof.hidden_author_ids.contains(&creator_uid));
    }

    drop(svc2);
    let svc3 = LocalCodeSceneService::open_with_dir(
        dir.path(),
        &srv.base_url,
        1001,
        Some("alice".into()),
        Some(http),
    )
    .unwrap();
    assert!(svc3.get_asset(&s1.scene_id).unwrap().viewer_following_author);
    assert!(!svc3.export_active_profile().hidden_author_ids.contains(&creator_uid));
}

#[tokio::test]
async fn test_code_scene_step1_remapped_scene_id_migrates_pending_social_and_child_remix() {
    let srv = MockCodeSceneServer::start().await;
    srv.force_remap_scene_ids.store(true, Ordering::SeqCst);

    let dir = TempDir::new().unwrap();
    let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
    let svc = LocalCodeSceneService::open_with_dir(
        dir.path(),
        &srv.base_url,
        1001,
        Some("alice".into()),
        Some(http.clone()),
    )
    .unwrap();

    // 1. Offline parent scene + offline like/bookmark/comment + offline child remix.
    let parent = svc
        .publish_scene_local(&PublishCodeSceneRequest {
            title: "Remapped Offline Parent".into(),
            kind: AssetKind::Animation,
            source_code: valid_cs1_source(),
            manifest: AssetManifest::for_kind(AssetKind::Animation),
            ..Default::default()
        })
        .unwrap();
    let old_parent_id = parent.asset.scene_id.clone();
    assert!(old_parent_id.starts_with("local_scene_"));

    assert!(svc.toggle_like_local(&old_parent_id).unwrap().liked);
    assert!(svc.toggle_bookmark_local(&old_parent_id).unwrap().bookmarked);
    svc.add_comment_local(&old_parent_id, "Comment on remapped parent", None)
        .unwrap();

    let child = svc
        .publish_scene_local(&PublishCodeSceneRequest {
            title: "Remapped Offline Child Remix".into(),
            kind: AssetKind::Animation,
            source_code: valid_cs1_source(),
            manifest: AssetManifest::for_kind(AssetKind::Animation),
            remix_parent_id: Some(old_parent_id.clone()),
            ..Default::default()
        })
        .unwrap();
    let old_child_id = child.asset.scene_id.clone();

    // 2. Sync #1: parent succeeds (remapped to srv_scene_*), child transiently returns 503.
    srv.fail_publish_after_n.store(1, Ordering::SeqCst);
    let rep1 = svc.sync_with_server("valid-token").await.unwrap();
    assert_eq!(rep1.pushed_assets, 1);
    assert_eq!(rep1.pushed_comments, 1);
    assert_eq!(rep1.errors.len(), 1, "Only child publish 503 expected, got {:?}", rep1.errors);

    let srv_parent_id = {
        let g = srv.state.lock();
        let list = g.recommender.list_assets(&ListCodeSceneAssetsQuery {
            q: Some("Remapped Offline Parent".into()),
            limit: Some(10),
            ..Default::default()
        });
        assert_eq!(list.assets.len(), 1);
        let sid = list.assets[0].scene_id.clone();
        assert_ne!(sid, old_parent_id);
        assert!(sid.starts_with("srv_scene_"));
        assert_eq!(g.like_calls, 1, "Like must reconcile exactly once under srv_scene_*");
        assert_eq!(g.bookmark_calls, 1, "Bookmark must reconcile exactly once under srv_scene_*");
        assert_eq!(g.comments.get(&sid).map(Vec::len), Some(1));
        sid
    };

    // Pending social rows MUST be completely empty (0 orphaned local_scene_* rows!).
    let pending_after_sync1 = svc.local_store().list_pending_code_scene_social(1001);
    assert!(
        pending_after_sync1.is_empty(),
        "Remapped scene_id must leave 0 pending social rows, found {:?}",
        pending_after_sync1
    );
    assert!(svc.get_asset(&old_parent_id).is_none());
    let synced_parent = svc.get_asset(&srv_parent_id).unwrap();
    assert!(synced_parent.viewer_liked);
    assert!(synced_parent.viewer_bookmarked);

    // Persisted local-only child MUST already carry the remapped canonical server parent ID!
    let local_only_after_sync1 = svc.local_store().list_local_only_code_scenes();
    assert_eq!(local_only_after_sync1.len(), 1);
    assert_eq!(local_only_after_sync1[0].asset.scene_id, old_child_id);
    assert_eq!(
        local_only_after_sync1[0].asset.remix_parent_id.as_deref(),
        Some(srv_parent_id.as_str())
    );

    // 3. Drop + reopen from SQLite, clear 503, and run Sync #2 -> child pushes cleanly & social calls stay 1!
    drop(svc);
    srv.fail_publish_after_n.store(0, Ordering::SeqCst);
    let svc2 = LocalCodeSceneService::open_with_dir(
        dir.path(),
        &srv.base_url,
        1001,
        Some("alice".into()),
        Some(http),
    )
    .unwrap();
    let rep2 = svc2.sync_with_server("valid-token").await.unwrap();
    assert!(rep2.errors.is_empty(), "Sync #2 errors: {:?}", rep2.errors);
    assert_eq!(rep2.pushed_assets, 1);
    assert_eq!(rep2.reconciled_social, 0);
    assert_eq!(
        srv.publish_post_count.load(Ordering::SeqCst),
        3,
        "Expected 3 publish POSTs total (parent ok + child 503 + child retry ok)"
    );
    assert!(svc2.local_store().list_pending_code_scene_social(1001).is_empty());

    {
        let g = srv.state.lock();
        assert_eq!(g.like_calls, 1, "Sync #2 MUST be idempotent (0 extra like calls!)");
        assert_eq!(g.bookmark_calls, 1, "Sync #2 MUST be idempotent (0 extra bookmark calls!)");
        let list = g.recommender.list_assets(&ListCodeSceneAssetsQuery {
            q: Some("Remapped Offline Child Remix".into()),
            limit: Some(10),
            ..Default::default()
        });
        assert_eq!(list.assets.len(), 1);
        assert!(list.assets[0].scene_id.starts_with("srv_scene_"));
        assert_eq!(
            list.assets[0].remix_parent_id.as_deref(),
            Some(srv_parent_id.as_str())
        );
    }
    let parent_after = svc2.get_asset(&srv_parent_id).unwrap();
    assert!(parent_after.viewer_liked);
    assert!(parent_after.viewer_bookmarked);
}

// Iteration-5 Regression:
// Authoritative Like/Bookmark/Unlike/Unbookmark/UnfollowAuthor reconciliation on hard-blocked
// scenes (`disliked_scene_ids` or `hidden_tags`) maintains 100% RAM == SQLite == Reopen == Server
// parity across all 6 sub-cases (A1/A2, B1/B2, C1/C2) without erasing blocklists or drifting counts.
#[tokio::test]
async fn test_code_scene_like_bookmark_hard_blocked_scenes_ram_sqlite_sync_parity() {
    let seeds = official_seed_assets();
    let s1 = seeds
        .iter()
        .find(|a| a.author_user_id == 777101)
        .unwrap_or(&seeds[0])
        .clone();
    let s2 = seeds
        .iter()
        .find(|a| a.scene_id != s1.scene_id)
        .unwrap()
        .clone();
    let s3 = seeds
        .iter()
        .find(|a| {
            a.scene_id != s1.scene_id
                && a.scene_id != s2.scene_id
                && !a.tags.is_empty()
        })
        .unwrap()
        .clone();
    let s3_tag = s3.tags[0].trim().to_ascii_lowercase();

    // Cases A1 & A2: Online toggle_like & toggle_bookmark after HideAuthor -> FollowAuthor
    {
        let srv = MockCodeSceneServer::start().await;
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http.clone()),
        )
        .unwrap();
        svc.load_feed();

        let base_likes = svc.get_asset(&s1.scene_id).unwrap().likes_count;
        let base_bm = svc.get_asset(&s1.scene_id).unwrap().bookmarks_count;

        // HideAuthor marks s1 in disliked_scene_ids and hides s1.author_user_id.
        let _ = svc.open_scene_by_id(&s1.scene_id).unwrap();
        svc.record_interaction(SceneBehaviorEventKind::HideAuthor, None);
        assert!(svc.export_active_profile().disliked_scene_ids.contains(&s1.scene_id));
        assert!(svc
            .export_active_profile()
            .hidden_author_ids
            .contains(&s1.author_user_id));

        // FollowAuthor un-hides the author, while s1 remains in disliked_scene_ids.
        let flw = svc
            .toggle_follow(s1.author_user_id, Some("valid-token"))
            .await
            .unwrap();
        assert!(flw.following);
        assert!(svc.export_active_profile().disliked_scene_ids.contains(&s1.scene_id));

        // Re-open s1 and toggle Like (A1) & Bookmark (A2) online.
        let _ = svc.open_scene_by_id(&s1.scene_id).unwrap();
        let like_resp = svc.toggle_like(&s1.scene_id, Some("valid-token")).await.unwrap();
        assert!(like_resp.liked, "A1: online toggle_like response must be true");
        assert_eq!(like_resp.likes_count, base_likes + 1);
        let a_after_like = svc.get_asset(&s1.scene_id).unwrap();
        assert!(a_after_like.viewer_liked, "A1: client RAM viewer_liked must be true");
        assert_eq!(a_after_like.likes_count, base_likes + 1);
        assert!(svc.current_item().unwrap().asset.viewer_liked);

        let bm_resp = svc
            .toggle_bookmark(&s1.scene_id, Some("valid-token"))
            .await
            .unwrap();
        assert!(bm_resp.bookmarked, "A2: online toggle_bookmark response must be true");
        assert_eq!(bm_resp.bookmarks_count, base_bm + 1);
        let a_after_bm = svc.get_asset(&s1.scene_id).unwrap();
        assert!(a_after_bm.viewer_bookmarked, "A2: client RAM viewer_bookmarked must be true");
        assert_eq!(a_after_bm.bookmarks_count, base_bm + 1);
        assert!(svc.current_item().unwrap().asset.viewer_bookmarked);
        assert!(
            svc.export_active_profile()
                .disliked_scene_ids
                .contains(&s1.scene_id),
            "A1/A2: disliked_scene_ids must stay intact"
        );

        // Drop & reopen must preserve true + counts + disliked_scene_ids.
        drop(svc);
        let svc2 = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http.clone()),
        )
        .unwrap();
        let reopened = svc2.get_asset(&s1.scene_id).unwrap();
        assert!(reopened.viewer_liked, "A1: reopened viewer_liked must remain true");
        assert!(reopened.viewer_bookmarked, "A2: reopened viewer_bookmarked must remain true");
        assert_eq!(reopened.likes_count, base_likes + 1);
        assert_eq!(reopened.bookmarks_count, base_bm + 1);
        assert!(svc2.export_active_profile().disliked_scene_ids.contains(&s1.scene_id));

        // Subsequent online toggle_like & toggle_bookmark cleanly toggle true -> false without counter drift!
        let unlike_resp = svc2.toggle_like(&s1.scene_id, Some("valid-token")).await.unwrap();
        assert!(!unlike_resp.liked);
        assert_eq!(unlike_resp.likes_count, base_likes);
        let unbm_resp = svc2
            .toggle_bookmark(&s1.scene_id, Some("valid-token"))
            .await
            .unwrap();
        assert!(!unbm_resp.bookmarked);
        assert_eq!(unbm_resp.bookmarks_count, base_bm);

        let after_untoggle = svc2.get_asset(&s1.scene_id).unwrap();
        assert!(!after_untoggle.viewer_liked);
        assert!(!after_untoggle.viewer_bookmarked);
        assert_eq!(after_untoggle.likes_count, base_likes);
        assert_eq!(after_untoggle.bookmarks_count, base_bm);
        assert!(svc2.export_active_profile().disliked_scene_ids.contains(&s1.scene_id));

        drop(svc2);
        let svc3 = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http),
        )
        .unwrap();
        let reopened_false = svc3.get_asset(&s1.scene_id).unwrap();
        assert!(!reopened_false.viewer_liked);
        assert!(!reopened_false.viewer_bookmarked);
        assert_eq!(reopened_false.likes_count, base_likes);
        assert_eq!(reopened_false.bookmarks_count, base_bm);
        assert!(svc3.export_active_profile().disliked_scene_ids.contains(&s1.scene_id));
    }

    // Cases B1 & B2: Offline record_interaction(DoubleTapLike / Bookmark) after NotInterested
    // + immediate sync_with_server (no reopen beforehand) -> RAM == SQLite == Reopen == Server!
    {
        let srv = MockCodeSceneServer::start().await;
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http.clone()),
        )
        .unwrap();
        svc.load_feed();

        let base_likes = svc.get_asset(&s2.scene_id).unwrap().likes_count;
        let base_bm = svc.get_asset(&s2.scene_id).unwrap().bookmarks_count;

        let _ = svc.open_scene_by_id(&s2.scene_id).unwrap();
        svc.record_interaction(SceneBehaviorEventKind::NotInterested, None);
        assert!(svc.export_active_profile().disliked_scene_ids.contains(&s2.scene_id));

        // Re-open s2 offline and fire DoubleTapLike & Bookmark (twice each to verify idempotency).
        let _ = svc.open_scene_by_id(&s2.scene_id).unwrap();
        let _ = svc.record_interaction(SceneBehaviorEventKind::DoubleTapLike, None);
        let _ = svc.record_interaction(SceneBehaviorEventKind::DoubleTapLike, None);
        let _ = svc.record_interaction(SceneBehaviorEventKind::Bookmark, None);
        let _ = svc.record_interaction(SceneBehaviorEventKind::Bookmark, None);

        // Immediate RAM state (before reopen or sync) MUST be true with +1 counts and intact dislike!
        let ram_pre = svc.get_asset(&s2.scene_id).unwrap();
        assert!(ram_pre.viewer_liked, "B1: immediate RAM viewer_liked must be true");
        assert!(ram_pre.viewer_bookmarked, "B2: immediate RAM viewer_bookmarked must be true");
        assert_eq!(ram_pre.likes_count, base_likes + 1, "B1: DoubleTapLike must not double-increment");
        assert_eq!(ram_pre.bookmarks_count, base_bm + 1, "B2: Bookmark must not double-increment");
        assert!(svc.current_item().unwrap().asset.viewer_liked);
        assert!(svc.current_item().unwrap().asset.viewer_bookmarked);
        assert!(svc.export_active_profile().disliked_scene_ids.contains(&s2.scene_id));

        // Immediate sync_with_server (WITHOUT drop/reopen beforehand!)
        let rep = svc.sync_with_server("valid-token").await.unwrap();
        assert!(rep.errors.is_empty(), "B1/B2 sync errors: {:?}", rep.errors);
        {
            let g = srv.state.lock();
            assert_eq!(g.like_calls, 1);
            assert_eq!(g.bookmark_calls, 1);
            let srv_asset = g.recommender.get_asset_for_user(&s2.scene_id, 1001).unwrap();
            assert!(srv_asset.viewer_liked);
            assert!(srv_asset.viewer_bookmarked);
            assert_eq!(srv_asset.likes_count, base_likes + 1);
            assert_eq!(srv_asset.bookmarks_count, base_bm + 1);
            assert!(g
                .recommender
                .export_user_profile(1001)
                .unwrap()
                .disliked_scene_ids
                .contains(&s2.scene_id));
        }

        // Post-sync RAM must remain true + counts + disliked_scene_ids intact!
        let post_sync = svc.get_asset(&s2.scene_id).unwrap();
        assert!(post_sync.viewer_liked, "B1: post-sync RAM viewer_liked must be true");
        assert!(post_sync.viewer_bookmarked, "B2: post-sync RAM viewer_bookmarked must be true");
        assert_eq!(post_sync.likes_count, base_likes + 1);
        assert_eq!(post_sync.bookmarks_count, base_bm + 1);
        assert!(svc.export_active_profile().disliked_scene_ids.contains(&s2.scene_id));

        // Drop + reopen after sync must match 100%, and 2nd sync is a clean idempotent no-op!
        drop(svc);
        let svc2 = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http),
        )
        .unwrap();
        let reopened = svc2.get_asset(&s2.scene_id).unwrap();
        assert!(reopened.viewer_liked, "B1: reopened viewer_liked must be true");
        assert!(reopened.viewer_bookmarked, "B2: reopened viewer_bookmarked must be true");
        assert_eq!(reopened.likes_count, base_likes + 1);
        assert_eq!(reopened.bookmarks_count, base_bm + 1);
        assert!(svc2.export_active_profile().disliked_scene_ids.contains(&s2.scene_id));

        let rep2 = svc2.sync_with_server("valid-token").await.unwrap();
        assert!(rep2.errors.is_empty());
        assert_eq!(rep2.reconciled_social, 0);
        assert_eq!(srv.state.lock().like_calls, 1);
        assert_eq!(srv.state.lock().bookmark_calls, 1);

        // Offline local toggle true -> false -> true -> false on hard-blocked scene has zero counter drift.
        let t1 = svc2.toggle_like_local(&s2.scene_id).unwrap();
        assert!(!t1.liked);
        assert_eq!(t1.likes_count, base_likes);
        let t2 = svc2.toggle_like_local(&s2.scene_id).unwrap();
        assert!(t2.liked);
        assert_eq!(t2.likes_count, base_likes + 1);
        let b1 = svc2.toggle_bookmark_local(&s2.scene_id).unwrap();
        assert!(!b1.bookmarked);
        assert_eq!(b1.bookmarks_count, base_bm);
        let b2 = svc2.toggle_bookmark_local(&s2.scene_id).unwrap();
        assert!(b2.bookmarked);
        assert_eq!(b2.bookmarks_count, base_bm + 1);

        // Verify record_interaction Unlike / Unbookmark / UnfollowAuthor symmetry on active hard-blocked card.
        let _ = svc2.open_scene_by_id(&s2.scene_id).unwrap();
        let _ = svc2.record_interaction(SceneBehaviorEventKind::FollowAuthor, None);
        assert!(svc2.get_asset(&s2.scene_id).unwrap().viewer_following_author);
        let _ = svc2.record_interaction(SceneBehaviorEventKind::Unlike, None);
        let _ = svc2.record_interaction(SceneBehaviorEventKind::Unbookmark, None);
        let _ = svc2.record_interaction(SceneBehaviorEventKind::UnfollowAuthor, None);
        let sym = svc2.get_asset(&s2.scene_id).unwrap();
        assert!(!sym.viewer_liked);
        assert_eq!(sym.likes_count, base_likes);
        assert!(!sym.viewer_bookmarked);
        assert_eq!(sym.bookmarks_count, base_bm);
        assert!(!sym.viewer_following_author);
        assert!(svc2.export_active_profile().disliked_scene_ids.contains(&s2.scene_id));
    }

    // Cases C1 & C2: Online toggle_like & toggle_bookmark on a HideTag deep-linked scene
    {
        let srv = MockCodeSceneServer::start().await;
        let dir = TempDir::new().unwrap();
        let http = CodeSceneHttpClient::from_base_url(&srv.base_url);
        let svc = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http.clone()),
        )
        .unwrap();
        svc.load_feed();

        let base_likes = svc.get_asset(&s3.scene_id).unwrap().likes_count;
        let base_bm = svc.get_asset(&s3.scene_id).unwrap().bookmarks_count;

        let _ = svc.open_scene_by_id(&s3.scene_id).unwrap();
        svc.record_interaction(SceneBehaviorEventKind::HideTag, Some(s3_tag.clone()));
        assert!(svc.export_active_profile().hidden_tags.contains(&s3_tag));

        // Deep-link back into the hidden-tag scene and toggle Like (C1) & Bookmark (C2) online.
        let _ = svc.open_scene_by_id(&s3.scene_id).unwrap();
        let like_resp = svc.toggle_like(&s3.scene_id, Some("valid-token")).await.unwrap();
        assert!(like_resp.liked, "C1: online toggle_like response must be true");
        assert_eq!(like_resp.likes_count, base_likes + 1);
        let bm_resp = svc
            .toggle_bookmark(&s3.scene_id, Some("valid-token"))
            .await
            .unwrap();
        assert!(bm_resp.bookmarked, "C2: online toggle_bookmark response must be true");
        assert_eq!(bm_resp.bookmarks_count, base_bm + 1);

        let a_live = svc.get_asset(&s3.scene_id).unwrap();
        assert!(a_live.viewer_liked, "C1: client RAM viewer_liked must be true");
        assert!(a_live.viewer_bookmarked, "C2: client RAM viewer_bookmarked must be true");
        assert_eq!(a_live.likes_count, base_likes + 1);
        assert_eq!(a_live.bookmarks_count, base_bm + 1);
        assert!(
            svc.export_active_profile().hidden_tags.contains(&s3_tag),
            "C1/C2: hidden_tags must stay intact"
        );

        // Drop & reopen preserves true + counts + hidden_tags!
        drop(svc);
        let svc2 = LocalCodeSceneService::open_with_dir(
            dir.path(),
            &srv.base_url,
            1001,
            Some("alice".into()),
            Some(http),
        )
        .unwrap();
        let reopened = svc2.get_asset(&s3.scene_id).unwrap();
        assert!(reopened.viewer_liked, "C1: reopened viewer_liked must remain true");
        assert!(reopened.viewer_bookmarked, "C2: reopened viewer_bookmarked must remain true");
        assert_eq!(reopened.likes_count, base_likes + 1);
        assert_eq!(reopened.bookmarks_count, base_bm + 1);
        assert!(svc2.export_active_profile().hidden_tags.contains(&s3_tag));

        // Toggle both back true -> false cleanly without counter drift, preserving hidden_tags!
        let ul = svc2.toggle_like(&s3.scene_id, Some("valid-token")).await.unwrap();
        assert!(!ul.liked);
        assert_eq!(ul.likes_count, base_likes);
        let ub = svc2
            .toggle_bookmark(&s3.scene_id, Some("valid-token"))
            .await
            .unwrap();
        assert!(!ub.bookmarked);
        assert_eq!(ub.bookmarks_count, base_bm);
        let final_a = svc2.get_asset(&s3.scene_id).unwrap();
        assert!(!final_a.viewer_liked);
        assert!(!final_a.viewer_bookmarked);
        assert_eq!(final_a.likes_count, base_likes);
        assert_eq!(final_a.bookmarks_count, base_bm);
        assert!(svc2.export_active_profile().hidden_tags.contains(&s3_tag));
    }
}
