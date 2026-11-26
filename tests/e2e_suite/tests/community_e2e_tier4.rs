//! Tier 4 Opaque-Box E2E Suite — Real-World Multi-Actor Application Workloads (`10` Scenarios).
//! Implements `TEST_INFRA.md §8` (`TEST-CM-T4-01` .. `TEST-CM-T4-10`).
//!
//! Every scenario runs a full vertical-stack journey against genuine subsystems:
//! live `EphemeralTestServer` (Axum + SQLite V42 + Tantivy BM25 + 5s scheduler + GatewayHub WS),
//! standard `/bot/{token}/community.*` + `WebhookCaptureServer`, `HermesToolExecutor` via
//! `ServerPlatformImBridge`, `TelegramClient` online & offline/404 `LocalCommunityRuntime`,
//! and headless GPUI `Application::headless()` rendering `AppShell` + `CommunityWorkspace`.

#[allow(dead_code)]
mod community_e2e_helpers;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use community_e2e_helpers::{
    CommunityAuthMode, CommunityHttpDriver, HeadlessCommunityHost, Legacy404Server,
    TestTelegramClient, WebhookCaptureServer, define_community_gpui_harness,
    execute_bot_hermes_tool, now_ms, open_shell_for_community,
};
use e2e_suite::browser_e2e_fixtures::LatencyBudget;
use e2e_suite::common::{EphemeralTestServer, connect_ws, recv_ws_frame_matching};
use e2e_suite::gpui_browser_harness::{pump_until, pump_with_ticks};
use gpui::{AssetSource, AsyncApp};
use gpui_telegram_agent::ApprovalMode;
use gpui_telegram_agent::hermes::toolsets::ToolsetPreset;
use gpui_telegram_client::config::ClientConfig;
use gpui_telegram_client::{ClientDomainEvent, CommunityRuntimeMode, TelegramClient};
use gpui_telegram_community::default_prefs_for_preset;
use gpui_telegram_desktop::theme::{IconName, TelegramAssetSource};
use gpui_telegram_desktop::views::apple_sidebar::AppleSidebarSection;
use gpui_telegram_desktop::views::boxes::modal_types::ModalDialogKind;
use gpui_telegram_desktop::views::community::bot_studio_view::{
    clamp_grok_avatar_indices, resolve_grok_avatar_pair,
};
use gpui_telegram_desktop::views::community::{
    ActiveCommunityInput, CommunityLayoutMode, CommunityModal, CommunityPopover, CommunityRoute,
    CommunityWorkspaceAction, ComposerFormatTab, ComposerMediaDraft, NotificationCategoryTab,
    PollBreakdownMode,
};
use gpui_telegram_desktop::views::nav_rail::{NavDestination, NavRail};
use gpui_telegram_desktop::views::shell::{
    ActiveWorkspacePanel, nav_destination_for_shortcut_digit,
};
use gpui_telegram_desktop::views::sidebar::gemini_unified_sidebar::UnifiedSidebarAction;
use gpui_telegram_desktop::views::MainMenuCommand;
use gpui_telegram_proto::community_contracts::{
    ActorKind, ActorProfileResponse, AlgoPreferencesDto, AlgoPreset, AutoModActionKind,
    AutoModRuleDto, BotAutonomyMode, CommentSort, CommunityDetailResponse,
    CommunityIdentitiesResponse, CommunityNotificationKind, CommunitySearchResultDto,
    CreatePostRequest, CreatePostResponse, FeedRequestDto, FeedResponseDto, FeedSort, FeedTab,
    ModActionKind, ModLogsResponse, ModQueueResponse, NotificationsListResponse,
    PostDetailResponse, PostFormat, PostStatus, SummonBotRequest, TrendingResponse,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use reqwest::Method;
use serde_json::json;
use tempfile::TempDir;

define_community_gpui_harness! {
    child_env: "CM_T4_CHILD_ENV",
    target_env: "CM_T4_CHILD_TARGET_ENV",
    after_ctors_env: "CM_T4_CHILD_AFTER_CTORS_ENV",
    targets: {
        "test_cm_t4_01_human_bot_debate_lifecycle_bot_api_hermes_gpui_and_restart" => body_t4_01,
        "test_cm_t4_02_multi_signal_recommendation_personalization_closed_loop" => body_t4_02,
        "test_cm_t4_03_bot_studio_webhooks_hermes_and_five_hop_anti_loop" => body_t4_03,
        "test_cm_t4_04_rich_content_media_acl_cjk_ime_and_telegram_bridges" => body_t4_04,
        "test_cm_t4_08_global_explore_burst_trending_and_tantivy_bm25_discovery" => body_t4_08,
        "test_cm_t4_09_curation_lists_bookmark_folders_notifications_and_sidebar_badges" => body_t4_09,
        "test_cm_t4_10_full_desktop_headless_ux_parity_matrix_security_and_logout" => body_t4_10,
    }
}

/// Builds a lightweight multi-threaded Tokio runtime for running live `EphemeralTestServer`
/// instances alongside headless GPUI `Application::headless()` subprocesses.
fn new_t4_tokio_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build Tier 4 Tokio runtime")
}

/// Opens an authenticated headless GPUI `AppShell` connected LIVE (`offline_force_local = false`)
/// to an `EphemeralTestServer` loopback URL using an isolated RAII `TempDir`.
fn open_live_community_host(
    app: &mut AsyncApp,
    server_url: &str,
    user_id: u64,
    username: &str,
    token: &str,
    width: f32,
    height: f32,
) -> HeadlessCommunityHost {
    let dir = TempDir::new().expect("create TempDir for live GPUI client");
    let cfg = ClientConfig {
        server_url: server_url.to_string(),
        max_reconnect_attempts: 0,
        ..Default::default()
    };
    let client = Arc::new(
        TelegramClient::open_with_dir(cfg, dir.path()).expect("TelegramClient::open_with_dir"),
    );
    let mut host = open_shell_for_community(
        app,
        client,
        user_id,
        username,
        token,
        width,
        height,
        false,
    );
    host._dir = Some(dir);
    host
}

/// Counts maximum consecutive appearances of a specific author in a feed item slice.
fn max_consecutive_author(items: &[gpui_telegram_proto::community_contracts::FeedItemDto], actor_id: &str) -> usize {
    let mut best = 0usize;
    let mut cur = 0usize;
    for it in items {
        if it.post.author_actor_id == actor_id {
            cur += 1;
            best = best.max(cur);
        } else {
            cur = 0;
        }
    }
    best
}

/// Recursively collects all comment IDs and maximum tree depth from a nested comment forest.
fn walk_comment_forest(
    nodes: &[gpui_telegram_proto::community_contracts::CommunityCommentDto],
    ids: &mut Vec<String>,
    max_depth: &mut u32,
) {
    for n in nodes {
        ids.push(n.comment_id.clone());
        if n.depth > *max_depth {
            *max_depth = n.depth;
        }
        walk_comment_forest(&n.children, ids, max_depth);
    }
}

// ============================================================================
// TEST-CM-T4-01 — Complete Human + Bot First-Class Debate Lifecycle
// ============================================================================

/// [Test ID] TEST-CM-T4-01
/// [Feature / Req] F02, F03, F04, F05, F11, F12, F15, F16, F17 / ORIGINAL_REQUEST.md (Complete Human + Bot debate lifecycle)
/// [Preconditions] Ephemeral live server with V42 + Tantivy + rich seed; 2 Humans (Alice, Bob) and 2 Bots (`/api/v1/bots` + `/api/v1/fabushi-bots`).
/// [Opaque-Box Steps] Create `c/systems_design`, publish Article & Poll, debate across REST + `/bot/{token}/community.*` + `HermesToolExecutor`, inspect in live headless GPUI via left sidebar, then restart server.
/// [Observable Assertions] Human & Bot citizens share full parity, poll tallies exact 2 Human / 1 Bot votes, nested depth-3 forest & 6 sorts render cleanly in online GPUI, and dual karma + threads survive server restart.
#[test]
fn test_cm_t4_01_human_bot_debate_lifecycle_bot_api_hermes_gpui_and_restart() {
    run_community_gpui_test("test_cm_t4_01_human_bot_debate_lifecycle_bot_api_hermes_gpui_and_restart");
}

async fn body_t4_01(mut app: AsyncApp) {
    let rt = new_t4_tokio_rt();
    let mut server = rt.block_on(EphemeralTestServer::start()).expect("start live server");
    let base_url = server.base_url();
    let driver = CommunityHttpDriver::new(&base_url);

    let (alice, rust_bot_id, ai_bot_id, art_id, poll_id, quote_id, hermes_pid, c0_id, c3_id) =
        rt.block_on(async {
            let alice = driver.register_human("alice_t4_01", "Pass1234!").await.unwrap();
            let bob = driver.register_human("bob_t4_01", "Pass1234!").await.unwrap();
            let (rust_uid, rust_tok, _) = driver
                .create_bot(&alice.token, "rust_t4_01_bot", "Rust Bot")
                .await
                .unwrap();
            let (ai_uid, _ai_tok, _) = driver
                .create_fabushi_bot(&alice.token, "AI Reviewer Bot", Some("ai_reviewer_t4_01_bot"))
                .await
                .unwrap();
            let rust_bot_id = format!("b:{rust_uid}");
            let ai_bot_id = format!("b:{ai_uid}");

            let a_auth = CommunityAuthMode::Human(alice.token.clone());
            let b_auth = CommunityAuthMode::Human(bob.token.clone());
            let rust_auth = CommunityAuthMode::DirectBot(rust_tok.clone());

            let (st_id, id_val) = driver.req(Method::GET, "/api/v1/community/identities", &a_auth, None).await.unwrap();
            assert_eq!(st_id, 200);
            let ids: CommunityIdentitiesResponse = serde_json::from_value(id_val).unwrap();
            assert!(ids.bot_actors.iter().any(|b| b.actor_id == rust_bot_id), "rust_bot must sync");
            assert!(ids.bot_actors.iter().any(|b| b.actor_id == ai_bot_id), "fabushi bot must sync");
            assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &b_auth, None).await.unwrap().0, 200);

            let (st_c, cv) = driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({
                "slug": "systems_design",
                "name": "Systems Design",
                "description": "Human + Bot architecture debates",
                "category": "engineering",
                "allow_bots": true,
                "require_bot_disclosure": true,
                "rules": [{ "id": "r1", "title": "Cite benchmarks", "description": "Back claims with data", "order": 1 }],
                "post_flairs": [
                    { "id": "f1", "text": "RFC", "bg_color": "#1D4ED8", "text_color": "#FFFFFF", "is_mod_only": false },
                    { "id": "f2", "text": "Debate", "bg_color": "#0F766E", "text_color": "#FFFFFF", "is_mod_only": false }
                ]
            }))).await.unwrap();
            assert_eq!(st_c, 201, "create systems_design failed: {cv}");

            let (st_a, av) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
                "community_slug": "systems_design",
                "format": "article",
                "title": "Zero-Copy Actor Queues in Rust",
                "flair": "RFC",
                "body": "Comparing ring buffers vs MPSC channels for desktop event loops. cc @ai_reviewer_t4_01_bot"
            }))).await.unwrap();
            assert_eq!(st_a, 201, "create article failed: {av}");
            let art_id = av["post"]["post_id"].as_str().unwrap().to_string();

            let (st_p, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
                "community_slug": "systems_design",
                "format": "poll",
                "flair": "Debate",
                "body": "Preferred concurrency primitive for GPUI tasks?",
                "poll": { "question": "Preferred primitive?", "options": ["Actor Channels", "Lock-Free Ring", "Hybrid"], "multi_select": false, "duration_secs": 3600 }
            }))).await.unwrap();
            assert_eq!(st_p, 201, "create poll failed: {pv}");
            let poll_id = pv["post"]["post_id"].as_str().unwrap().to_string();
            let opt0 = pv["post"]["poll"]["options"][0]["option_id"].as_str().unwrap().to_string();
            let opt1 = pv["post"]["poll"]["options"][1]["option_id"].as_str().unwrap().to_string();

            // rust_bot comments via standard Bot API + votes in poll via DirectBot
            let (st_bc, bcv) = driver.bot_api(&rust_tok, "community.createComment", true, Some(json!({
                "post_id": art_id, "body": "Ring buffers cut allocation jitter by 40% under load."
            }))).await.unwrap();
            assert_eq!(st_bc, 200, "bot createComment failed: {bcv}");
            assert_eq!(bcv["ok"], true);
            let c0_id = bcv["result"]["comment"]["comment_id"].as_str().unwrap().to_string();

            assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{poll_id}/poll-vote"), &a_auth, Some(json!({ "option_ids": [opt0] }))).await.unwrap().0, 200);
            assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{poll_id}/poll-vote"), &b_auth, Some(json!({ "option_ids": [opt1] }))).await.unwrap().0, 200);
            assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{poll_id}/poll-vote"), &rust_auth, Some(json!({ "option_ids": [opt0] }))).await.unwrap().0, 200);

            // Nested depth 0 -> 1 -> 2 -> 3 debate branch + Bob quote + upvote rust_bot
            let (st_c1, c1v) = driver.req(Method::POST, &format!("/api/v1/community/posts/{art_id}/comments"), &b_auth, Some(json!({
                "parent_comment_id": c0_id, "body": "What about backpressure when consumers stall? @ai_reviewer_t4_01_bot"
            }))).await.unwrap();
            assert_eq!(st_c1, 201);
            let c1_id = c1v["comment"]["comment_id"].as_str().unwrap().to_string();
            let (st_c2, c2v) = driver.req(Method::POST, &format!("/api/v1/community/posts/{art_id}/comments"), &a_auth, Some(json!({
                "parent_comment_id": c1_id, "body": "Bounded ring with drop-oldest or wakeup backpressure."
            }))).await.unwrap();
            assert_eq!(st_c2, 201);
            let c2_id = c2v["comment"]["comment_id"].as_str().unwrap().to_string();
            let (st_c3, c3v) = driver.req(Method::POST, &format!("/api/v1/community/posts/{art_id}/comments"), &b_auth, Some(json!({
                "parent_comment_id": c2_id, "body": "Makes sense for UI telemetry frames."
            }))).await.unwrap();
            assert_eq!(st_c3, 201);
            let c3_id = c3v["comment"]["comment_id"].as_str().unwrap().to_string();
            assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/comments/{c0_id}/vote"), &b_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);

            let (st_q, qv) = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({
                "community_slug": "systems_design", "format": "quote", "quoted_post_id": art_id, "body": "Must-read RFC on desktop actor loops!"
            }))).await.unwrap();
            assert_eq!(st_q, 201, "Bob quote failed: {qv}");
            let quote_id = qv["post"]["post_id"].as_str().unwrap().to_string();

            // ai_reviewer_bot participates natively via HermesToolExecutor (ServerPlatformImBridge)
            let hp = execute_bot_hermes_tool(&server, ai_uid, "community_create_post", json!({
                "community_slug": "systems_design", "title": "Hermes Static Analysis Report", "body": "Verified lock-free ordering guarantees."
            }), ToolsetPreset::HermesTelegram, ApprovalMode::Prompt).await;
            assert!(hp.success, "Hermes create_post failed: {:?}", hp.error);
            let hermes_pid = hp.structured.as_ref().unwrap()["post"]["post_id"].as_str().unwrap().to_string();
            let hc = execute_bot_hermes_tool(&server, ai_uid, "community_comment", json!({
                "post_id": art_id, "parent_comment_id": c1_id, "body": "Backpressure watermark at 75% avoids tail latency spikes."
            }), ToolsetPreset::HermesTelegram, ApprovalMode::Prompt).await;
            assert!(hc.success, "Hermes comment failed: {:?}", hc.error);
            let hv = execute_bot_hermes_tool(&server, ai_uid, "community_vote", json!({
                "post_id": art_id, "direction": 1
            }), ToolsetPreset::HermesTelegram, ApprovalMode::Prompt).await;
            assert!(hv.success, "Hermes vote failed: {:?}", hv.error);

            (alice, rust_bot_id, ai_bot_id, art_id, poll_id, quote_id, hermes_pid, c0_id, c3_id)
        });

    // Live headless GPUI AppShell connected to EphemeralTestServer -> enter via Left Sidebar
    let host = open_live_community_host(&mut app, &base_url, alice.user_id as u64, &alice.username, &alice.token, 1440.0, 900.0);
    let shell = host.shell.clone();
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(false);
            sb.handle_unified_action(UnifiedSidebarAction::SelectDestination(NavDestination::Community), scx);
        });
    });
    assert!(pump_until(&mut app, Duration::from_secs(6), |cx| {
        let s = shell.read(cx);
        s.active_panel == ActiveWorkspacePanel::Community
            && s.community_workspace.as_ref().is_some_and(|w| {
                let st = &w.read(cx).state;
                !st.feed_loading && st.identities.is_some() && !st.communities.is_empty()
            })
    }).await, "Live GPUI CommunityWorkspace must bootstrap from left sidebar");
    let cw = host.cw(&app);
    assert_eq!(cw.read_with(&app, |w, _| w.state.runtime_mode).unwrap(), CommunityRuntimeMode::Online);

    // Inspect Article nested Human+Bot comment tree & cycle all 6 CommentSorts
    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(art_id.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == art_id && !d.comments.is_empty())
    }).await);
    cw.read_with(&app, |w, _| {
        let d = w.state.detail_post.as_ref().unwrap();
        let mut all_ids = Vec::new();
        let mut max_d = 0;
        walk_comment_forest(&d.comments, &mut all_ids, &mut max_d);
        assert!(all_ids.contains(&c0_id) && all_ids.contains(&c3_id), "nested branch missing: {all_ids:?}");
        assert!(max_d >= 3, "expected depth >= 3, got {max_d}");
        assert!(d.comments.iter().any(|c| c.author_kind == ActorKind::Bot));
    }).unwrap();
    for sort in [CommentSort::Best, CommentSort::Top, CommentSort::New, CommentSort::Controversial, CommentSort::Old, CommentSort::Qa] {
        let _ = cw.update(&mut app, |w, cx| w.set_detail_comment_sort(sort, cx));
        assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.detail_loading).await);
        host.draw(&mut app);
    }

    // Inspect Poll Human/Bot vote breakdown inside GPUI
    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(poll_id.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == poll_id && d.post.poll.is_some())
    }).await);
    cw.read_with(&app, |w, _| {
        let poll = w.state.detail_post.as_ref().unwrap().post.poll.as_ref().unwrap();
        assert_eq!(poll.total_votes, 3);
        assert_eq!(poll.total_human_votes, 2);
        assert_eq!(poll.total_bot_votes, 1);
    }).unwrap();
    for mode in [PollBreakdownMode::All, PollBreakdownMode::Humans, PollBreakdownMode::Bots] {
        let _ = cw.update(&mut app, |w, cx| { w.state.poll_breakdown_by_post.insert(poll_id.clone(), mode); cx.notify(); });
        host.draw(&mut app);
    }

    // Cleanly tear down GPUI AppShell FIRST so weak_shell dies, THEN call disconnect_ws() so
    // AppShell's detached spawn_ws_status_observer wakes on status_tx, breaks, and drops its
    // Arc<TelegramClient> (closing all pooled HTTP/1.1 keep-alive & WS sockets before restart).
    let mut host = host;
    let client_for_teardown = std::sync::Arc::clone(&host.client);
    let weak_client = std::sync::Arc::downgrade(&host.client);
    let _ = shell.update(&mut app, |s, _| {
        s.community_workspace = None;
    });
    if let Some(win) = host.win.take() {
        let _ = win.update(&mut app, |_, w, _| w.remove_window());
    }
    drop(cw);
    drop(shell);
    drop(host);
    drop(driver);
    let _ = app.update(|_| {});
    app.background_executor().timer(Duration::from_millis(60)).await;
    client_for_teardown.stop_tailnet_mesh();
    client_for_teardown.disconnect_ws();
    drop(client_for_teardown);
    let _ = pump_until(&mut app, Duration::from_secs(3), |_| weak_client.strong_count() == 0).await;

    rt.block_on(async {
        tokio::time::sleep(Duration::from_millis(120)).await;
        server.restart().await.expect("restart server cleanly");
        let d2 = CommunityHttpDriver::new(server.base_url());
        let a_auth = CommunityAuthMode::Human(alice.token.clone());

        let (st, pv) = d2.req(Method::GET, &format!("/api/v1/community/posts/{art_id}"), &a_auth, None).await.unwrap();
        assert_eq!(st, 200);
        let det: PostDetailResponse = serde_json::from_value(pv).unwrap();
        assert!(det.total_comments >= 5, "all Human+Bot debate comments must survive restart");

        let (st, plv) = d2.req(Method::GET, &format!("/api/v1/community/posts/{poll_id}"), &a_auth, None).await.unwrap();
        assert_eq!(st, 200);
        let poll = serde_json::from_value::<PostDetailResponse>(plv).unwrap().post.poll.unwrap();
        assert_eq!((poll.total_votes, poll.total_human_votes, poll.total_bot_votes), (3, 2, 1));

        assert_eq!(d2.req(Method::GET, &format!("/api/v1/community/posts/{quote_id}"), &a_auth, None).await.unwrap().0, 200);
        let (st_h, hpv) = d2.req(Method::GET, &format!("/api/v1/community/posts/{hermes_pid}"), &a_auth, None).await.unwrap();
        assert_eq!(st_h, 200);
        assert_eq!(hpv["post"]["author_actor_id"], ai_bot_id);

        let (_, apv) = d2.req(Method::GET, &format!("/api/v1/community/actors/{}", alice.actor_id), &a_auth, None).await.unwrap();
        let alice_prof: ActorProfileResponse = serde_json::from_value(apv).unwrap();
        assert!(alice_prof.actor.post_karma > 0, "Alice post_karma must persist after restart");

        let (_, bpv) = d2.req(Method::GET, &format!("/api/v1/community/actors/{rust_bot_id}"), &a_auth, None).await.unwrap();
        let bot_prof: ActorProfileResponse = serde_json::from_value(bpv).unwrap();
        assert!(bot_prof.actor.comment_karma > 0, "rust_bot comment_karma must persist after restart");

        drop(d2);
        server.stop().await;
    });
}

// ============================================================================
// TEST-CM-T4-02 — Multi-Signal Recommendation & Personalization Closed Loop
// ============================================================================

/// [Test ID] TEST-CM-T4-02
/// [Feature / Req] F02, F06, F08, F11, F16 / ORIGINAL_REQUEST.md (Multi-signal recommendation & personalization closed loop)
/// [Preconditions] Ephemeral live server seeded with a multi-community Human + Bot corpus (4 communities, NSFW, muted keyword, blocked actor, and 10-post single-author flood).
/// [Opaque-Box Steps] Connect viewer Charlie, inspect initial `ForYou` + `FeedExplanationDto`, follow Human+Bot, join `c/t4_02_rust`, submit positive & negative `/signals`, mute keyword & block spammer, cycle all 7 `AlgoPreset`s, then sync via headless GPUI Algo Tuner.
/// [Observable Assertions] Ranking adapts deterministically, Greedy MMR prevents single-author flooding, hidden/muted/blocked/NSFW posts never leak, all 7 presets apply cleanly, and GPUI Algo Tuner syncs bi-directionally with the live server.
#[test]
fn test_cm_t4_02_multi_signal_recommendation_personalization_closed_loop() {
    run_community_gpui_test("test_cm_t4_02_multi_signal_recommendation_personalization_closed_loop");
}

async fn body_t4_02(mut app: AsyncApp) {
    let rt = new_t4_tokio_rt();
    let mut server = rt.block_on(EphemeralTestServer::start()).expect("start live server");
    let base_url = server.base_url();
    let driver = CommunityHttpDriver::new(&base_url);

    let charlie = rt.block_on(async {
        let alice = driver.register_human("alice_t4_02", "Pass1234!").await.unwrap();
        let dave = driver.register_human("dave_t4_02", "Pass1234!").await.unwrap();
        let eve = driver.register_human("eve_t4_02", "Pass1234!").await.unwrap();
        let charlie = driver.register_human("charlie_t4_02", "Pass1234!").await.unwrap();
        let rust_bot = driver.register_community_bot(&alice.token, "rust_t4_02_bot", "Rust Recommender Bot", "manual").await.unwrap();
        let ml_bot = driver.register_community_bot(&alice.token, "ml_t4_02_bot", "ML Recommender Bot", "manual").await.unwrap();

        let a_auth = CommunityAuthMode::Human(alice.token.clone());
        let d_auth = CommunityAuthMode::Human(dave.token.clone());
        let e_auth = CommunityAuthMode::Human(eve.token.clone());
        let c_auth = CommunityAuthMode::Human(charlie.token.clone());
        let rb_auth = CommunityAuthMode::DirectBot(rust_bot.bot_token.clone());
        let mb_auth = CommunityAuthMode::DirectBot(ml_bot.bot_token.clone());

        for auth in [&a_auth, &d_auth, &e_auth, &c_auth] {
            assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", auth, None).await.unwrap().0, 200);
        }

        for (slug, name, is_nsfw) in [
            ("t4_02_rust", "T4 Rust Core", false),
            ("t4_02_ai", "T4 AI Agents", false),
            ("t4_02_noise", "T4 Noise", false),
            ("t4_02_nsfw", "T4 After Dark", true),
        ] {
            let (st, v) = driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({
                "slug": slug, "name": name, "allow_bots": true, "is_nsfw": is_nsfw
            }))).await.unwrap();
            assert_eq!(st, 201, "create {slug} failed: {v}");
        }

        // High-signal Rust Human & Bot posts + media post
        let (mid, _) = driver.upload_png(&alice.token, "rust_chart.png", (40, 140, 220, 255), None).await.unwrap();
        let p_rust_h = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "t4_02_rust", "format": "media",
            "title": "Rust GPUI Pipeline Deep Dive",
            "body": "Zero-copy rendering benchmarks #RustPerf",
            "media": [{ "media_id": mid, "kind": "image" }]
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
        let p_rust_b = driver.req(Method::POST, "/api/v1/community/posts", &rb_auth, Some(json!({
            "community_slug": "t4_02_rust", "body": "Bot benchmark breakdown for #RustPerf async runtimes"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
        for i in 0..4 {
            let _ = driver.req(Method::POST, "/api/v1/community/posts", &mb_auth, Some(json!({
                "community_slug": "t4_02_ai", "body": format!("Bot autonomous reasoning benchmark #{i} #AIAgents")
            }))).await.unwrap();
        }
        // Boost velocity on Rust posts
        for auth in [&d_auth, &rb_auth, &mb_auth] {
            let _ = driver.req(Method::POST, &format!("/api/v1/community/posts/{p_rust_h}/vote"), auth, Some(json!({ "direction": 1 }))).await;
            let _ = driver.req(Method::POST, &format!("/api/v1/community/posts/{p_rust_b}/vote"), auth, Some(json!({ "direction": 1 }))).await;
        }

        // Single-author 10-post flood by Dave to verify Greedy MMR diversity caps
        for i in 0..10 {
            let (st, _) = driver.req(Method::POST, "/api/v1/community/posts", &d_auth, Some(json!({
                "community_slug": "t4_02_rust", "body": format!("Dave flood message #{i} #RustPerf")
            }))).await.unwrap();
            assert_eq!(st, 201);
        }

        // Gated posts: NSFW, hide target, muted keyword target, blocked author Eve
        let nsfw_pid = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "t4_02_nsfw", "is_nsfw": true, "body": "Strict NSFW post T4-02"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
        let hide_pid = driver.req(Method::POST, "/api/v1/community/posts", &d_auth, Some(json!({
            "community_slug": "t4_02_noise", "body": "Noisy dismiss target post T4-02"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
        let kw_pid = driver.req(Method::POST, "/api/v1/community/posts", &d_auth, Some(json!({
            "community_slug": "t4_02_noise", "body": "Spam promo with t4_02_noisy_kw token"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
        let eve_pid = driver.req(Method::POST, "/api/v1/community/posts", &e_auth, Some(json!({
            "community_slug": "t4_02_noise", "body": "Blocked author Eve message T4-02"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

        // Inspect initial ForYou + explanation + default NSFW exclusion
        let (st0, fv0) = driver.req(Method::GET, "/api/v1/community/feed?tab=for_you&limit=40", &c_auth, None).await.unwrap();
        assert_eq!(st0, 200);
        let init_feed: FeedResponseDto = serde_json::from_value(fv0).unwrap();
        assert!(!init_feed.items.is_empty());
        assert!(init_feed.items.iter().all(|it| !it.post.is_nsfw), "allow_nsfw=false must exclude NSFW");
        assert!(!init_feed.items.iter().any(|it| it.post.post_id == nsfw_pid));
        let expl = init_feed.items[0].explanation.as_ref().expect("FeedExplanationDto must be present");
        assert!(!expl.primary_reason.is_empty() && !expl.signal_breakdown.is_empty());

        // Charlie follows Human Alice + Bot rust_bot, joins c/t4_02_rust, sends positive + negative signals & mutes/blocks
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/actors/{}/follow", alice.actor_id), &c_auth, None).await.unwrap().0, 200);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/actors/{}/follow", rust_bot.actor_id), &c_auth, None).await.unwrap().0, 200);
        assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/t4_02_rust/join", &c_auth, None).await.unwrap().0, 200);

        for (pid, kind, dwell) in [
            (&p_rust_h, "dwell", Some(15_000)),
            (&p_rust_h, "click", None),
            (&p_rust_b, "show_more", None),
            (&hide_pid, "not_interested", None),
            (&hide_pid, "hide_post", None),
        ] {
            let (st, sv) = driver.req(Method::POST, "/api/v1/community/signals", &c_auth, Some(json!({
                "post_id": pid, "kind": kind, "dwell_ms": dwell
            }))).await.unwrap();
            assert_eq!(st, 200, "signal {kind} failed: {sv}");
        }
        assert_eq!(driver.req(Method::POST, "/api/v1/community/mutes-blocks", &c_auth, Some(json!({
            "target_kind": "keyword", "target_value": "t4_02_noisy_kw", "action": "mute"
        }))).await.unwrap().0, 200);
        assert_eq!(driver.req(Method::POST, "/api/v1/community/mutes-blocks", &c_auth, Some(json!({
            "target_kind": "actor", "target_value": eve.actor_id, "action": "block"
        }))).await.unwrap().0, 200);

        // Fresh newest post for Chronological preset verification
        tokio::time::sleep(Duration::from_millis(25)).await;
        let newest_pid = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "t4_02_rust", "body": "Brand newest chronological anchor T4-02"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

        // Determinism + Gating + MMR diversity assertions
        let (_, fv_a) = driver.req(Method::GET, "/api/v1/community/feed?tab=for_you&limit=25", &c_auth, None).await.unwrap();
        let (_, fv_b) = driver.req(Method::GET, "/api/v1/community/feed?tab=for_you&limit=25", &c_auth, None).await.unwrap();
        let feed_a: FeedResponseDto = serde_json::from_value(fv_a).unwrap();
        let feed_b: FeedResponseDto = serde_json::from_value(fv_b).unwrap();
        let ids_a: Vec<&str> = feed_a.items.iter().map(|i| i.post.post_id.as_str()).collect();
        let ids_b: Vec<&str> = feed_b.items.iter().map(|i| i.post.post_id.as_str()).collect();
        assert_eq!(ids_a, ids_b, "ForYou feed ranking must be strictly deterministic");
        let id_set: HashSet<&str> = ids_a.iter().copied().collect();
        assert!(!id_set.contains(nsfw_pid.as_str()), "NSFW post leaked");
        assert!(!id_set.contains(hide_pid.as_str()), "hidden post leaked");
        assert!(!id_set.contains(kw_pid.as_str()), "muted keyword post leaked");
        assert!(!id_set.contains(eve_pid.as_str()), "blocked author Eve leaked");
        let max_run = max_consecutive_author(&feed_a.items[..feed_a.items.len().min(12)], &dave.actor_id);
        assert!(max_run <= 2, "MMR diversity must prevent Dave single-author consecutive flooding, got run={max_run}");

        // Cycle ALL 7 AlgoPresets via PUT /algo-prefs and observe feed adaptation
        let mut human_focused_bots = 0usize;
        let mut bot_heavy_bots = 0usize;
        for preset in [
            AlgoPreset::Balanced,
            AlgoPreset::RedditDeepDive,
            AlgoPreset::XRealTime,
            AlgoPreset::Discovery,
            AlgoPreset::HumanFocused,
            AlgoPreset::BotHeavy,
            AlgoPreset::Chronological,
        ] {
            let prefs = default_prefs_for_preset(preset);
            let (st_put, _) = driver.req(Method::PUT, "/api/v1/community/algo-prefs", &c_auth, Some(serde_json::to_value(&prefs).unwrap())).await.unwrap();
            assert_eq!(st_put, 200);
            let (st_f, fv) = driver.req(Method::GET, "/api/v1/community/feed?tab=for_you&limit=15", &c_auth, None).await.unwrap();
            assert_eq!(st_f, 200);
            let f: FeedResponseDto = serde_json::from_value(fv).unwrap();
            assert_eq!(f.applied_preset, preset);
            assert!(f.items.iter().all(|i| !i.post.is_nsfw && i.post.post_id != hide_pid && i.post.post_id != eve_pid));
            if preset == AlgoPreset::HumanFocused {
                human_focused_bots = f.items.iter().filter(|i| i.post.author_kind.is_bot_like()).count();
            } else if preset == AlgoPreset::BotHeavy {
                bot_heavy_bots = f.items.iter().filter(|i| i.post.author_kind.is_bot_like()).count();
            } else if preset == AlgoPreset::Chronological {
                assert_eq!(f.items[0].post.post_id, newest_pid, "Chronological must rank newest post #1");
            }
        }
        assert!(
            bot_heavy_bots >= human_focused_bots && bot_heavy_bots > 0,
            "BotHeavy ({bot_heavy_bots}) must surface >= bots than HumanFocused ({human_focused_bots})"
        );

        // Persist RedditDeepDive before booting live headless GPUI Algo Tuner
        let rdd = default_prefs_for_preset(AlgoPreset::RedditDeepDive);
        assert_eq!(driver.req(Method::PUT, "/api/v1/community/algo-prefs", &c_auth, Some(serde_json::to_value(&rdd).unwrap())).await.unwrap().0, 200);
        charlie
    });

    // Headless GPUI Algo Tuner reflects persisted server prefs & saves new preset back to server
    let host = open_live_community_host(&mut app, &base_url, charlie.user_id as u64, &charlie.username, &charlie.token, 1440.0, 900.0);
    let cw = host.open_community_and_wait(&mut app).await;
    assert_eq!(cw.read_with(&app, |w, _| w.state.runtime_mode).unwrap(), CommunityRuntimeMode::Online);
    assert_eq!(cw.read_with(&app, |w, _| w.state.algo_prefs.preset).unwrap(), AlgoPreset::RedditDeepDive);

    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_popover(CommunityPopover::AlgoTuner, cx);
        w.state.algo_draft = default_prefs_for_preset(AlgoPreset::BotHeavy);
    });
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| w.save_algo_prefs(cx));
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        cw.read(cx).state.algo_prefs.preset == AlgoPreset::BotHeavy && !cw.read(cx).state.feed_loading
    }).await);
    host.draw(&mut app);

    drop(cw);
    drop(host);
    rt.block_on(async {
        let (st, pv) = driver.req(Method::GET, "/api/v1/community/algo-prefs", &CommunityAuthMode::Human(charlie.token.clone()), None).await.unwrap();
        assert_eq!(st, 200);
        let saved: AlgoPreferencesDto = serde_json::from_value(pv).unwrap();
        assert_eq!(saved.preset, AlgoPreset::BotHeavy, "GPUI Algo Tuner update must persist on server");
        drop(driver);
        server.stop().await;
    });
}

// ============================================================================
// TEST-CM-T4-03 — Autonomous Bot Studio, Webhooks, Hermes & 5-Hop Anti-Loop
// ============================================================================

/// [Test ID] TEST-CM-T4-03
/// [Feature / Req] F02, F09, F11, F12, F13, F15, F16, F21 / ORIGINAL_REQUEST.md (Bot Studio, Webhooks, Hermes & 5-hop anti-loop)
/// [Preconditions] Ephemeral live server + `WebhookCaptureServer` + headless GPUI `AppShell` authenticated as Alice + Human Bob.
/// [Opaque-Box Steps] Open Community via `Cmd+Shift+C`, register & customize an 8×11 Grok-avatar `Reactive` Bot in `BotStudioView`, run live test post, switch Act-As identity and Quick-Post from Feed, configure `setWebhook` + live WS, have Bob mention `@bot`, execute Hermes tools, and run Bot<->Bot ping-pong past hop 5.
/// [Observable Assertions] Bot Studio registers live Bot & returns real token, Act-As post authors as Bot, webhook receives exact `X-Telegram-Bot-Api-Secret-Token`, `getUpdates` & reactive reply + WS fan-out succeed, and Bot<->Bot loop halts strictly at hop 5 (`bot_loop_budget_exceeded` on hop 6).
#[test]
fn test_cm_t4_03_bot_studio_webhooks_hermes_and_five_hop_anti_loop() {
    run_community_gpui_test("test_cm_t4_03_bot_studio_webhooks_hermes_and_five_hop_anti_loop");
}

async fn body_t4_03(mut app: AsyncApp) {
    let rt = new_t4_tokio_rt();
    let (mut server, mut capture) = rt.block_on(async {
        let srv = EphemeralTestServer::start().await.expect("start live server");
        let cap = WebhookCaptureServer::start().await.expect("start WebhookCaptureServer");
        (srv, cap)
    });
    let base_url = server.base_url();
    let driver = CommunityHttpDriver::new(&base_url);
    let (alice, bob) = rt.block_on(async {
        let a = driver.register_human("alice_t4_03", "Pass1234!").await.unwrap();
        let b = driver.register_human("bob_t4_03", "Pass1234!").await.unwrap();
        assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &CommunityAuthMode::Human(a.token.clone()), None).await.unwrap().0, 200);
        assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &CommunityAuthMode::Human(b.token.clone()), None).await.unwrap().0, 200);
        (a, b)
    });

    // Headless GPUI: Cmd+Shift+C -> BotStudioView (8x11 Grok avatar) -> register -> test post -> Act-As Quick-Post
    assert_eq!(clamp_grok_avatar_indices(999, usize::MAX), (7, 10));
    let (shape_name, color_name) = resolve_grok_avatar_pair(5, 8);
    assert!(!shape_name.is_empty() && !color_name.is_empty());

    let host = open_live_community_host(&mut app, &base_url, alice.user_id as u64, &alice.username, &alice.token, 1440.0, 900.0);
    host.key(&mut app, "cmd-shift-c");
    assert!(pump_until(&mut app, Duration::from_secs(6), |cx| {
        let s = host.shell.read(cx);
        s.active_panel == ActiveWorkspacePanel::Community
            && s.community_workspace.as_ref().is_some_and(|w| {
                let st = &w.read(cx).state;
                !st.feed_loading && st.identities.is_some()
            })
    }).await, "Cmd+Shift+C must open & bootstrap CommunityWorkspace");
    let cw = host.cw(&app);
    assert_eq!(cw.read_with(&app, |w, _| w.state.runtime_mode).unwrap(), CommunityRuntimeMode::Online);

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BotStudio, cx);
        w.set_bot_studio_avatar_indices(5, 8, cx);
        assert_eq!((w.state.bot_studio_shape_idx, w.state.bot_studio_color_idx), (5, 8));
        w.state.bot_studio_name = "T4 Autonomous Citizen".into();
        w.state.bot_studio_handle = "t4_studio_live_bot".into();
        w.state.bot_studio_persona = "Concise Rust & GPUI architecture citizen".into();
        w.state.bot_studio_autonomy = BotAutonomyMode::Reactive;
        w.submit_register_bot(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(6), |cx| {
        let st = &cw.read(cx).state;
        !st.bot_studio_submitting
            && st.identities.as_ref().is_some_and(|i| i.bot_actors.iter().any(|b| b.handle == "t4_studio_live_bot"))
    }).await, "BotStudio must register @t4_studio_live_bot on live server");

    let (bot_actor_id, bot_uid, bot_token) = cw.read_with(&app, |w, _| {
        let b = w.state.identities.as_ref().unwrap().bot_actors.iter().find(|b| b.handle == "t4_studio_live_bot").unwrap();
        let tok = w.state.bot_studio_tokens_by_actor.get(&b.actor_id).cloned()
            .or_else(|| w.state.bot_studio_last_token.clone())
            .expect("live Bot Studio registration must return Bot API token");
        assert_eq!(b.avatar_shape, shape_name);
        assert_eq!(b.avatar_color, color_name);
        (b.actor_id.clone(), b.linked_user_id.expect("bot linked_user_id"), tok)
    }).unwrap();

    // Live test post from BotStudioView
    let _ = cw.update(&mut app, |w, cx| {
        w.state.bot_studio_test_community = Some("rust".into());
        w.state.bot_studio_test_content = "Hello from Bot Studio live playground T4-03!".into();
        w.run_bot_studio_test_post(bot_actor_id.clone(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        !cw.read(cx).state.bot_studio_test_submitting && cw.read(cx).state.bot_studio_test_last_post_id.is_some()
    }).await);
    host.draw(&mut app);

    // Switch active Act-As identity to the newly created Bot and Quick-Post from main Feed
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.switch_active_actor(bot_actor_id.clone(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| !cw.read(cx).state.feed_loading).await);
    let _ = cw.update(&mut app, |w, cx| {
        w.state.quick_post_community = Some("rust".into());
        w.state.quick_post_body = "Act-As Feed post by T4 Studio Bot!".into();
        w.submit_quick_post(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        let st = &cw.read(cx).state;
        !st.quick_post_submitting
            && st.feed_items.iter().any(|i| i.post.body.contains("Act-As Feed post by T4 Studio Bot!") && i.post.author_actor_id == bot_actor_id && i.post.author_kind == ActorKind::Bot)
    }).await, "Act-As Quick-Post must persist authored as Bot citizen");
    host.draw(&mut app);
    drop(cw);
    drop(host);

    // Live server: setWebhook -> Bob mention -> Webhook secret + getUpdates + Reactive reply + WS + Hermes + 5-hop guard
    rt.block_on(async {
        let secret = "sec-t4-03-live";
        let (st_wh, wh_v) = driver.bot_api(&bot_token, "setWebhook", true, Some(json!({
            "url": capture.url(), "secret_token": secret
        }))).await.unwrap();
        assert_eq!(st_wh, 200, "setWebhook failed: {wh_v}");
        assert_eq!(wh_v["ok"], true);

        let mut ws = connect_ws(&server, &bob.token).await.expect("connect Bob WS");
        let b_auth = CommunityAuthMode::Human(bob.token.clone());
        let (st_mp, mpv) = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({
            "community_slug": "rust", "body": "Hey @t4_studio_live_bot please review our lock-free ring buffer!"
        }))).await.unwrap();
        assert_eq!(st_mp, 201, "Bob mention post failed: {mpv}");
        let mention_pid = mpv["post"]["post_id"].as_str().unwrap().to_string();

        // 1. Authenticated Webhook POST with X-Telegram-Bot-Api-Secret-Token
        let (got_secret, wh_payload) = capture.recv_timeout(Duration::from_secs(5)).await.expect("webhook POST must arrive");
        assert_eq!(got_secret.as_deref(), Some(secret));
        assert_eq!(wh_payload["community_mention"]["post_id"], mention_pid);

        // 2. getUpdates queue contains matching community_mention
        let (st_u, upd) = driver.bot_api_get(&bot_token, "getUpdates", true, "").await.unwrap();
        assert_eq!(st_u, 200);
        assert!(upd["result"].as_array().unwrap().iter().any(|u| {
            u.get("community_mention").is_some_and(|m| m["post_id"] == mention_pid)
        }), "getUpdates missing community_mention: {upd}");

        // 3. Reactive Bot reply + real-time WS UpdateEvent::CommunityEvent fan-out
        let _ = recv_ws_frame_matching(&mut ws, Duration::from_secs(6), |f| {
            matches!(
                f,
                WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, actor_id, .. })
                    if event_kind == "comment_created"
                        && post_id.as_deref() == Some(mention_pid.as_str())
                        && actor_id.as_deref() == Some(bot_actor_id.as_str())
            )
        }).await.expect("reactive bot reply WS event must arrive");

        // 4. Hermes tools run cleanly under studio-created bot identity
        let h_feed = execute_bot_hermes_tool(&server, bot_uid, "community_browse_feed", json!({ "community": "rust", "limit": 10 }), ToolsetPreset::HermesTelegram, ApprovalMode::Prompt).await;
        assert!(h_feed.success, "Hermes browse_feed failed: {:?}", h_feed.error);
        let h_vote = execute_bot_hermes_tool(&server, bot_uid, "community_vote", json!({ "post_id": mention_pid, "direction": 1 }), ToolsetPreset::HermesTelegram, ApprovalMode::Prompt).await;
        assert!(h_vote.success, "Hermes vote failed: {:?}", h_vote.error);

        // 5. Bot<->Bot 5-hop anti-loop hard stop + human turn reset
        // Switch studio bot to manual for deterministic exact hop 1..=5 -> hop 6 Bot API ping-pong
        assert_eq!(
            driver.req(Method::PATCH, &format!("/api/v1/community/actors/{bot_actor_id}"), &CommunityAuthMode::Human(alice.token.clone()), Some(json!({
                "bot_autonomy_mode": "manual"
            }))).await.unwrap().0,
            200
        );
        let bot_b = driver.register_community_bot(&alice.token, "t4_ping_b_bot", "Ping B Bot", "manual").await.unwrap();
        assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &CommunityAuthMode::Human(alice.token.clone()), Some(json!({
            "slug": "t4_03_antiloop", "name": "T4-03 Anti-Loop", "allow_bots": true
        }))).await.unwrap().0, 201);
        let (st_hp, hpv) = driver.req(Method::POST, "/api/v1/community/posts", &CommunityAuthMode::Human(alice.token.clone()), Some(json!({
            "community_slug": "t4_03_antiloop", "body": "Anti-loop stress thread for Bot A and Bot B"
        }))).await.unwrap();
        assert_eq!(st_hp, 201);
        let hop_pid = hpv["post"]["post_id"].as_str().unwrap().to_string();

        let mut parent_cid: Option<String> = None;
        for hop in 1..=5 {
            let (tok, target): (&str, &str) = if hop % 2 == 1 {
                (bot_token.as_str(), bot_b.handle.as_str())
            } else {
                (bot_b.bot_token.as_str(), "t4_studio_live_bot")
            };
            let (st, res) = driver.bot_api(tok, "community.createComment", true, Some(json!({
                "post_id": hop_pid,
                "parent_comment_id": parent_cid,
                "body": format!("Bot ping-pong turn {hop} @{target}")
            }))).await.unwrap();
            assert_eq!(st, 200, "hop {hop} (<= 5) must succeed: {res}");
            assert_eq!(res["ok"], true);
            parent_cid = Some(res["result"]["comment"]["comment_id"].as_str().unwrap().to_string());
        }

        // Hop 6 MUST be blocked cleanly with 400 bot_loop_budget_exceeded
        let (st6, res6) = driver.bot_api(&bot_b.bot_token, "community.createComment", true, Some(json!({
            "post_id": hop_pid,
            "parent_comment_id": parent_cid,
            "body": "Bot hop 6 MUST fail @t4_studio_live_bot"
        }))).await.unwrap();
        assert_eq!(st6, 400, "hop 6 must hit 5-hop ceiling: {res6}");
        assert_eq!(res6["ok"], false);
        assert!(res6["description"].as_str().unwrap_or("").contains("bot_loop_budget_exceeded"), "unexpected hop 6 error: {res6}");

        // Human intervention resets hop counter cleanly
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{hop_pid}/comments"), &b_auth, Some(json!({
            "body": "Human Bob moderating the discussion — resetting loop counter."
        }))).await.unwrap().0, 201);
        let (st_after, res_after) = driver.bot_api(&bot_b.bot_token, "community.createComment", true, Some(json!({
            "post_id": hop_pid, "body": "Bot resumes cleanly after human turn."
        }))).await.unwrap();
        assert_eq!(st_after, 200, "bot must resume after human turn: {res_after}");

        drop(ws);
        drop(driver);
        capture.shutdown().await;
        server.stop().await;
    });
}

// ============================================================================
// TEST-CM-T4-04 — Rich Content, Media ACL, CJK IME & Telegram Bridges
// ============================================================================

/// [Test ID] TEST-CM-T4-04
/// [Feature / Req] F04, F11, F15, F18, F20 / ORIGINAL_REQUEST.md (Rich content creation, media ACL, CJK IME & Telegram bridges)
/// [Preconditions] Ephemeral live server with Alice, Bob, and Eve; Alice uploads 2 public PNGs and 1 private DM PNG (`chat_id` excluding Bob).
/// [Opaque-Box Steps] In live headless GPUI as Alice, open `ComposerModal`, exercise CJK IME preedit/Escape/80ms Enter fence with `#开源社区` + `@bob_t4_04`, publish `Media` gallery + `1/3` Thread + `Draft`->`Published`; verify Bob fetches public PNGs (`200` byte-exact) & private PNG (`403`); in GPUI as Bob exercise `OpenMediaLightbox`, `SharePostToChat`, and `OpenDmWithActor`.
/// [Observable Assertions] CJK IME & 80ms fence protect against premature submit, Media/Thread/Draft->Published persist on live server, public media returns `200` byte-exact while private DM media stays `403`, and all 3 Telegram bridges mutate exact AppShell surfaces.
#[test]
fn test_cm_t4_04_rich_content_media_acl_cjk_ime_and_telegram_bridges() {
    run_community_gpui_test("test_cm_t4_04_rich_content_media_acl_cjk_ime_and_telegram_bridges");
}

async fn body_t4_04(mut app: AsyncApp) {
    let rt = new_t4_tokio_rt();
    let mut server = rt.block_on(EphemeralTestServer::start()).expect("start live server");
    let base_url = server.base_url();
    let driver = CommunityHttpDriver::new(&base_url);

    let (alice, bob, pub_mid_1, pub_bytes_1, pub_mid_2, pub_bytes_2, priv_mid) = rt.block_on(async {
        let alice = driver.register_human("alice_t4_04", "Pass1234!").await.unwrap();
        let bob = driver.register_human("bob_t4_04", "Pass1234!").await.unwrap();
        let eve = driver.register_human("eve_t4_04", "Pass1234!").await.unwrap();
        let a_auth = CommunityAuthMode::Human(alice.token.clone());
        let b_auth = CommunityAuthMode::Human(bob.token.clone());
        assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &a_auth, None).await.unwrap().0, 200);
        assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &b_auth, None).await.unwrap().0, 200);

        let (st_ch, ch_v) = driver.req(Method::POST, "/api/v1/chats", &a_auth, Some(json!({
            "title": "Alice-Eve Private DM", "chat_type": "group", "member_ids": [eve.user_id]
        }))).await.unwrap();
        assert!(st_ch == 201 || st_ch == 200, "create private chat failed {st_ch}: {ch_v}");
        let priv_chat_id = ch_v["chat_id"].as_i64().unwrap();

        let (m1, b1) = driver.upload_png(&alice.token, "gallery_1.png", (24, 160, 250, 255), None).await.unwrap();
        let (m2, b2) = driver.upload_png(&alice.token, "gallery_2.png", (220, 80, 120, 255), None).await.unwrap();
        let (mp, _) = driver.upload_png(&alice.token, "private_dm.png", (10, 20, 30, 255), Some(priv_chat_id)).await.unwrap();

        assert_eq!(driver.get_media(&bob.token, &m1).await.unwrap().0, 403, "unattached media must be 403");
        assert_eq!(driver.get_media(&bob.token, &mp).await.unwrap().0, 403, "private DM media must be 403");
        (alice, bob, m1, b1, m2, b2, mp)
    });

    // Phase 1: Alice in live headless GPUI ComposerModal (CJK IME + Media gallery + 1/3 Thread + Draft->Published)
    let host_a = open_live_community_host(&mut app, &base_url, alice.user_id as u64, &alice.username, &alice.token, 1440.0, 900.0);
    let cw_a = host_a.open_community_and_wait(&mut app).await;
    let shell_a = host_a.shell.clone();
    assert_eq!(cw_a.read_with(&app, |w, _| w.state.runtime_mode).unwrap(), CommunityRuntimeMode::Online);
    let _ = cw_a.update(&mut app, |w, cx| {
        w.select_feed_tab(FeedTab::All, cx);
        w.set_feed_sort(FeedSort::New, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| !cw_a.read(cx).state.feed_loading).await);

    // Open Composer -> Media tab -> test CJK IME preedit + progressive Escape + 80ms Enter fence
    let _ = cw_a.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.set_composer_tab(ComposerFormatTab::Media, cx);
        w.state.focus_input(ActiveCommunityInput::ComposerTitle);
    });
    assert!(shell_a.read_with(&app, |s, cx| s.is_community_input_active(cx)).unwrap());
    let _ = shell_a.update(&mut app, |s, cx| {
        s.update_ime_marked_text("待取消预编辑", cx);
        assert!(s.has_active_ime_preedit(cx));
    });
    host_a.key(&mut app, "escape");
    shell_a.read_with(&app, |s, cx| {
        assert!(!s.has_active_ime_preedit(cx), "Escape #1 must cancel IME preedit");
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    }).unwrap();
    assert_eq!(cw_a.read_with(&app, |w, _| w.state.active_modal).unwrap(), Some(CommunityModal::Composer));

    let _ = shell_a.update(&mut app, |s, cx| {
        s.update_ime_marked_text("中文社区画廊", cx);
        s.commit_ime_unmark_text(cx);
        assert_eq!(s.active_text_and_preedit(cx).0, "中文社区画廊");
        // Immediate Enter within 80ms IME commit fence must NOT submit composer
        assert!(s.handle_non_char_key("enter", false, cx));
    });
    cw_a.read_with(&app, |w, _| {
        assert!(!w.state.composer.submitting, "80ms IME fence must absorb immediate Enter on ComposerTitle");
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));
    }).unwrap();

    let _ = cw_a.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::ComposerBody);
        cx.notify();
    });
    let _ = shell_a.update(&mut app, |s, cx| {
        s.update_ime_marked_text("Rust + GPUI 图文实测 #开源社区 @bob_t4_04", cx);
        s.commit_ime_unmark_text(cx);
    });
    let _ = cw_a.update(&mut app, |w, cx| {
        w.state.composer.media_items = vec![
            ComposerMediaDraft { url_or_id: pub_mid_1.clone(), kind: "image".into(), alt_text: "Chart 1".into() },
            ComposerMediaDraft { url_or_id: pub_mid_2.clone(), kind: "image".into(), alt_text: "Chart 2".into() },
        ];
        w.submit_composer(PostStatus::Published, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(6), |cx| {
        let st = &cw_a.read(cx).state;
        !st.composer.submitting && st.active_modal.is_none() && !st.feed_loading
            && st.feed_items.iter().any(|i| i.post.title.as_deref() == Some("中文社区画廊"))
    }).await, "Media gallery post with CJK IME must publish to live server");
    let media_post_id = cw_a.read_with(&app, |w, _| {
        let p = &w.state.feed_items.iter().find(|i| i.post.title.as_deref() == Some("中文社区画廊")).unwrap().post;
        assert_eq!(p.format, PostFormat::Media);
        assert_eq!(p.media.len(), 2);
        assert!(p.tags.iter().any(|t| t == "开源社区"), "CJK hashtag missing: {:?}", p.tags);
        p.post_id.clone()
    }).unwrap();
    host_a.draw(&mut app);

    // 3-part 1/3 X Thread via ComposerModal
    let _ = cw_a.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.set_composer_tab(ComposerFormatTab::Thread, cx);
        w.state.composer.body = "Thread 1/3: GPUI layout fundamentals".into();
        w.state.composer.thread_parts = vec![
            "Thread 2/3: Virtualized feed scroll handles".into(),
            "Thread 3/3: Bilingual CJK IME architecture".into(),
        ];
        w.submit_composer(PostStatus::Published, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(6), |cx| {
        let st = &cw_a.read(cx).state;
        !st.composer.submitting && st.active_modal.is_none() && !st.feed_loading
            && st.feed_items.iter().any(|i| i.post.body.contains("Thread 1/3") && i.post.thread_total == 3)
    }).await, "3-part X thread must persist with thread_total == 3");

    // Draft -> load -> resume -> Published transition
    let _ = cw_a.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.state.composer.body = "Draft promoted live T4-04".into();
        w.submit_composer(PostStatus::Draft, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| !cw_a.read(cx).state.composer.submitting && cw_a.read(cx).state.active_modal.is_none()).await);
    let _ = cw_a.update(&mut app, |w, cx| { w.open_composer_modal(None, cx); w.load_composer_drafts(cx); });
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        let c = &cw_a.read(cx).state.composer;
        !c.drafts_loading && c.drafts.iter().any(|d| d.body.contains("Draft promoted live T4-04"))
    }).await);
    let draft_post = cw_a.read_with(&app, |w, _| {
        w.state.composer.drafts.iter().find(|d| d.body.contains("Draft promoted live T4-04")).unwrap().clone()
    }).unwrap();
    let _ = cw_a.update(&mut app, |w, cx| {
        w.resume_draft_into_composer(draft_post.clone(), cx);
        w.publish_draft_now(draft_post.post_id.clone(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        !cw_a.read(cx).state.composer.drafts.iter().any(|d| d.post_id == draft_post.post_id)
    }).await);
    drop(cw_a);
    drop(shell_a);
    drop(host_a);

    // Phase 2: Bob verifies Public Media 200 byte-exact vs Private DM Media 403
    rt.block_on(async {
        let (st1, got1) = driver.get_media(&bob.token, &pub_mid_1).await.unwrap();
        let (st2, got2) = driver.get_media(&bob.token, &pub_mid_2).await.unwrap();
        let (stp, _) = driver.get_media(&bob.token, &priv_mid).await.unwrap();
        assert_eq!(st1, 200, "public community PNG 1 must return 200 to Bob");
        assert_eq!(got1, pub_bytes_1);
        assert_eq!(st2, 200, "public community PNG 2 must return 200 to Bob");
        assert_eq!(got2, pub_bytes_2);
        assert_eq!(stp, 403, "private DM PNG MUST stay 403 Forbidden to Bob");
        let (st_dp, dpv) = driver.req(Method::GET, &format!("/api/v1/community/posts/{}", draft_post.post_id), &CommunityAuthMode::Human(bob.token.clone()), None).await.unwrap();
        assert_eq!(st_dp, 200, "promoted draft must be publicly visible: {dpv}");
        assert_eq!(dpv["post"]["status"], "published");
    });

    // Phase 3: Bob in live headless GPUI exercises OpenMediaLightbox, SharePostToChat & OpenDmWithActor
    let host_b = open_live_community_host(&mut app, &base_url, bob.user_id as u64, &bob.username, &bob.token, 1440.0, 900.0);
    let cw_b = host_b.open_community_and_wait(&mut app).await;
    let shell_b = host_b.shell.clone();

    let _ = shell_b.update(&mut app, |s, cx| {
        s.handle_community_action(CommunityWorkspaceAction::OpenMediaLightbox(pub_mid_1.clone()), cx);
    });
    assert!(shell_b.read_with(&app, |s, _| s.overlays.media_viewer_overlay.is_some()).unwrap());
    host_b.draw(&mut app);
    host_b.key(&mut app, "escape");
    shell_b.read_with(&app, |s, _| {
        assert!(s.overlays.media_viewer_overlay.is_none());
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    }).unwrap();

    let _ = shell_b.update(&mut app, |s, cx| {
        s.handle_community_action(CommunityWorkspaceAction::SharePostToChat {
            post_id: media_post_id.clone(),
            summary: "中文社区画廊 #开源社区".into(),
        }, cx);
    });
    shell_b.read_with(&app, |s, cx| {
        assert!(s.community_pending_share.is_some());
        assert!(matches!(
            &s.modal_container.read(cx).dialog.kind,
            Some(ModalDialogKind::ForwardMessages { preview_snippet, .. }) if preview_snippet.contains("中文社区画廊")
        ));
    }).unwrap();
    host_b.draw(&mut app);

    let _ = cw_b.update(&mut app, |w, cx| w.open_actor_profile(alice.username.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        cw_b.read(cx).state.actor_profile.as_ref().is_some_and(|p| p.actor.linked_user_id == Some(alice.user_id))
    }).await);
    let _ = shell_b.update(&mut app, |s, cx| {
        s.handle_community_action(CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: alice.user_id }, cx);
    });
    assert_eq!(shell_b.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats, "OpenDmWithActor must switch AppShell to Telegram Chats");

    drop(cw_b);
    drop(shell_b);
    drop(host_b);
    drop(driver);
    rt.block_on(server.stop());
}

// ============================================================================
// TEST-CM-T4-05 — Community Governance: AutoMod, Mod Queue, bot_mod & Wiki
// ============================================================================

/// [Test ID] TEST-CM-T4-05
/// [Feature / Req] F03, F08, F11 / ORIGINAL_REQUEST.md (Community governance: AutoMod, mod queue, bot_mod, wiki v1->v2->v3 & transparent audit)
/// [Preconditions] Ephemeral live server with Owner Alice, regular member Bob, newcomer Carol, spammer Dan, and Alice's Bot `mod_helper_bot`.
/// [Opaque-Box Steps] Create `c/moderated_lab`, appoint `mod_helper_bot` as `bot_mod`, add regex + karma-floor AutoMod rules, edit Wiki v1->v2->v3, reject non-mod tampering (`403`), route newcomer & spam through AutoMod, execute full moderation suite via `Authorization: Bot`, and verify `mod-logs`.
/// [Observable Assertions] Non-mods & banned users get `403`, AutoMod-removed spam never enters feeds/trending/search, Wiki history tracks revisions 1..=3, `bot_mod` completes all actions under Bot auth, and every action is transparently logged in `mod-logs`.
#[tokio::test]
async fn test_cm_t4_05_community_governance_automod_bot_mod_wiki_and_audit() {
    let mut server = EphemeralTestServer::start().await.expect("start live server");
    let driver = CommunityHttpDriver::new(server.base_url());

    let alice = driver.register_human("alice_t4_05", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t4_05", "Pass1234!").await.unwrap();
    let carol = driver.register_human("carol_t4_05", "Pass1234!").await.unwrap();
    let dan = driver.register_human("dan_t4_05", "Pass1234!").await.unwrap();
    let mod_bot = driver
        .register_community_bot(&alice.token, "mod_helper_t4_05_bot", "Mod Helper Bot", "manual")
        .await
        .unwrap();

    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let c_auth = CommunityAuthMode::Human(carol.token.clone());
    let d_auth = CommunityAuthMode::Human(dan.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(mod_bot.bot_token.clone());

    for auth in [&a_auth, &b_auth, &c_auth, &d_auth] {
        assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", auth, None).await.unwrap().0, 200);
    }

    // Create c/moderated_lab (Wiki v1) + join + promote mod_helper_bot -> bot_mod
    let (st_c, cv) = driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({
        "slug": "moderated_lab",
        "name": "Moderated Governance Lab",
        "description": "Strict AutoMod + BotMod + Wiki governance",
        "allow_bots": true,
        "wiki_markdown": "# Moderated Lab v1\nInitial community charter.",
        "rules": [
            { "id": "r1", "title": "No Scam Links", "description": "Zero crypto scam spam", "order": 1 },
            { "id": "r2", "title": "Constructive Debate", "description": "Respect Human and Bot citizens", "order": 2 }
        ]
    }))).await.unwrap();
    assert_eq!(st_c, 201, "create moderated_lab failed: {cv}");
    for auth in [&b_auth, &c_auth, &d_auth, &bot_auth] {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/moderated_lab/join", auth, None).await.unwrap().0, 200);
    }
    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/communities/moderated_lab/mod-actions", &a_auth, Some(json!({
            "action": "promote_mod", "target_kind": "actor", "target_id": mod_bot.actor_id, "reason": "Appoint autonomous bot_mod"
        }))).await.unwrap().0,
        200
    );

    // Configure declarative AutoMod rules (regex spam blocker + karma floor)
    let t0 = now_ms();
    let rules = vec![
        AutoModRuleDto {
            rule_id: "am_regex_spam".into(),
            community_id: String::new(),
            name: "Crypto Scam Regex".into(),
            enabled: true,
            priority: 10,
            keywords: vec![],
            regex_pattern: Some("(?i)buy_crypto_scam_\\d+".into()),
            min_author_karma: None,
            min_account_age_secs: None,
            max_links: None,
            block_duplicate_spam: false,
            apply_to_bots: true,
            apply_to_humans: true,
            action: AutoModActionKind::Remove,
            assign_flair: None,
            auto_reply_template: None,
            created_at_ms: t0,
        },
        AutoModRuleDto {
            rule_id: "am_karma_floor".into(),
            community_id: String::new(),
            name: "Newcomer Karma Floor".into(),
            enabled: true,
            priority: 20,
            keywords: vec![],
            regex_pattern: None,
            min_author_karma: Some(5),
            min_account_age_secs: None,
            max_links: None,
            block_duplicate_spam: false,
            apply_to_bots: true,
            apply_to_humans: true,
            action: AutoModActionKind::FlagForReview,
            assign_flair: None,
            auto_reply_template: None,
            created_at_ms: t0,
        },
    ];
    let (st_patch, pv) = driver.req(Method::PATCH, "/api/v1/community/communities/moderated_lab", &a_auth, Some(json!({
        "automod_rules": rules
    }))).await.unwrap();
    assert_eq!(st_patch, 200, "patch automod rules failed: {pv}");

    // Non-moderator Bob MUST get 403 on community settings, wiki edit, mod-queue, and mod-actions
    assert_eq!(driver.req(Method::PATCH, "/api/v1/community/communities/moderated_lab", &b_auth, Some(json!({ "description": "hijacked" }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::PUT, "/api/v1/community/communities/moderated_lab/wiki", &b_auth, Some(json!({ "markdown": "vandal", "summary": "bad" }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::GET, "/api/v1/community/communities/moderated_lab/mod-queue", &b_auth, None).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/moderated_lab/mod-actions", &b_auth, Some(json!({
        "action": "ban_actor", "target_kind": "actor", "target_id": dan.actor_id
    }))).await.unwrap().0, 403);

    // Wiki versioning v1 -> v2 -> v3
    assert_eq!(driver.req(Method::PUT, "/api/v1/community/communities/moderated_lab/wiki", &a_auth, Some(json!({
        "markdown": "# Moderated Lab v2\nAdded AutoMod guidelines.", "summary": "v2 automod policy"
    }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::PUT, "/api/v1/community/communities/moderated_lab/wiki", &a_auth, Some(json!({
        "markdown": "# Moderated Lab v3\nAdded BotMod transparency section.", "summary": "v3 bot_mod charter"
    }))).await.unwrap().0, 200);
    let (_, det_v) = driver.req(Method::GET, "/api/v1/community/communities/moderated_lab", &b_auth, None).await.unwrap();
    let comm_det: CommunityDetailResponse = serde_json::from_value(det_v).unwrap();
    assert_eq!(comm_det.community.wiki_revision, 3);
    assert!(comm_det.wiki_revisions.len() >= 3);
    assert!(comm_det.wiki_revisions.iter().any(|r| r.summary == "v2 automod policy"));
    assert!(comm_det.wiki_revisions.iter().any(|r| r.summary == "v3 bot_mod charter"));

    // Newcomer Carol (karma=0 < 5) flagged for review; Spammer Dan auto-removed & hidden from feed/trending/search
    let (st_carol, carol_v) = driver.req(Method::POST, "/api/v1/community/posts", &c_auth, Some(json!({
        "community_slug": "moderated_lab", "body": "Hi everyone, excited to share my first Rust profiler!"
    }))).await.unwrap();
    assert_eq!(st_carol, 201);
    let carol_res: CreatePostResponse = serde_json::from_value(carol_v).unwrap();
    assert_eq!(carol_res.automod_action, AutoModActionKind::FlagForReview);
    let carol_pid = carol_res.post.post_id.clone();

    let (st_dan, dan_v) = driver.req(Method::POST, "/api/v1/community/posts", &d_auth, Some(json!({
        "community_slug": "moderated_lab", "body": "Hurry buy_crypto_scam_999 right now #ScamBurstT405"
    }))).await.unwrap();
    assert_eq!(st_dan, 201);
    let dan_res: CreatePostResponse = serde_json::from_value(dan_v).unwrap();
    assert_eq!(dan_res.automod_action, AutoModActionKind::Remove);
    assert!(dan_res.post.is_removed);
    let dan_pid = dan_res.post.post_id.clone();

    // AutoMod-removed spam MUST NOT appear in community feed, global feed, trending, or search
    let (_, cf_v) = driver.req(Method::GET, "/api/v1/community/feed?tab=community&community=moderated_lab&limit=30", &b_auth, None).await.unwrap();
    let cf: FeedResponseDto = serde_json::from_value(cf_v).unwrap();
    assert!(!cf.items.iter().any(|i| i.post.post_id == dan_pid), "removed spam leaked into community feed");
    let (_, tr_v) = driver.req(Method::GET, "/api/v1/community/trending?limit=30", &b_auth, None).await.unwrap();
    let tr: TrendingResponse = serde_json::from_value(tr_v).unwrap();
    assert!(!tr.topics.iter().any(|t| t.slug_or_tag.eq_ignore_ascii_case("scamburstt405")), "removed spam leaked into trending");
    let (_, sr_v) = driver.req(Method::GET, "/api/v1/community/search?q=ScamBurstT405", &b_auth, None).await.unwrap();
    let sr: CommunitySearchResultDto = serde_json::from_value(sr_v).unwrap();
    assert!(sr.posts.is_empty(), "removed spam leaked into Tantivy search");

    // Alice publishes announcement & heated discussion
    let ann_pid = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
        "community_slug": "moderated_lab", "title": "Weekly Governance Announcement", "body": "Read Wiki v3."
    }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    let heat_pid = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
        "community_slug": "moderated_lab", "body": "Heated debate thread to be locked"
    }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

    // mod_helper_bot executes full moderation suite strictly via `Authorization: Bot <token>`
    let (st_q, qv) = driver.req(Method::GET, "/api/v1/community/communities/moderated_lab/mod-queue", &bot_auth, None).await.unwrap();
    assert_eq!(st_q, 200, "bot_mod must read mod-queue: {qv}");
    let queue: ModQueueResponse = serde_json::from_value(qv).unwrap();
    let carol_qid = queue.items.iter().find(|i| i.target_id == carol_pid).expect("Carol in mod-queue").item_id.clone();
    let dan_qid = queue.items.iter().find(|i| i.target_id == dan_pid).expect("Dan in mod-queue").item_id.clone();

    for payload in [
        json!({ "action": "approve", "target_kind": "post", "target_id": carol_pid, "queue_item_id": carol_qid, "reason": "Verified newcomer post" }),
        json!({ "action": "remove", "target_kind": "post", "target_id": dan_pid, "queue_item_id": dan_qid, "reason": "Confirmed regex spam" }),
        json!({ "action": "pin", "target_kind": "post", "target_id": ann_pid, "reason": "Pin weekly governance notice" }),
        json!({ "action": "lock", "target_kind": "post", "target_id": heat_pid, "reason": "Cool down heated thread" }),
        json!({ "action": "ban_actor", "target_kind": "actor", "target_id": dan.actor_id, "reason": "Repeat scam offender" }),
    ] {
        let (st, v) = driver.req(Method::POST, "/api/v1/community/communities/moderated_lab/mod-actions", &bot_auth, Some(payload)).await.unwrap();
        assert_eq!(st, 200, "bot_mod action failed: {v}");
    }

    // Locked post rejects new comments (403); banned Dan gets 403 on post & comment
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{heat_pid}/comments"), &b_auth, Some(json!({ "body": "Reply on locked thread" }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &d_auth, Some(json!({ "community_slug": "moderated_lab", "body": "Banned Dan post" }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{ann_pid}/comments"), &d_auth, Some(json!({ "body": "Banned Dan comment" }))).await.unwrap().0, 403);

    // Transparent mod-logs readable by regular member Bob contain every Human & bot_mod action
    let (st_l, lv) = driver.req(Method::GET, "/api/v1/community/communities/moderated_lab/mod-logs?limit=50", &b_auth, None).await.unwrap();
    assert_eq!(st_l, 200);
    let logs: ModLogsResponse = serde_json::from_value(lv).unwrap();
    for expected in [ModActionKind::PromoteMod, ModActionKind::WikiEdit, ModActionKind::Approve, ModActionKind::Remove, ModActionKind::Pin, ModActionKind::Lock, ModActionKind::BanActor] {
        assert!(logs.logs.iter().any(|l| l.action == expected), "mod-logs missing {expected:?}");
    }
    assert!(
        logs.logs.iter().filter(|l| l.moderator_actor_id == mod_bot.actor_id && l.moderator_kind == ActorKind::Bot).count() >= 5,
        "all 5 bot_mod actions must be transparently attributed to mod_helper_bot"
    );

    drop(driver);
    server.stop().await;
}

// ============================================================================
// TEST-CM-T4-06 — Dual Online/Offline Resilience, Legacy-404 & Account Switch
// ============================================================================

/// [Test ID] TEST-CM-T4-06
/// [Feature / Req] F02, F09, F11, F13, F14 / ORIGINAL_REQUEST.md (Dual online/offline resilience, legacy-404 fallback & account switch isolation)
/// [Preconditions] Live `EphemeralTestServer`, `Legacy404Server`, and `TestTelegramClient` backed by an isolated RAII `TempDir`.
/// [Opaque-Box Steps] Sync online & receive live WS `ClientDomainEvent::CommunityEvent`; stop server mid-session and verify automatic `<app_dir>/community.db` fallback + offline post/draft/bookmark/local Bot summon; reopen on same `TempDir` against `Legacy404Server`; switch session from Alice to Bob.
/// [Observable Assertions] Online sync & WS succeed, mid-session server loss transitions cleanly to `LocalFallback`, all offline artifacts & local Bot replies persist across reopen against HTTP 404, and switching to Bob wipes/isolates Alice's private data completely.
#[tokio::test]
async fn test_cm_t4_06_dual_online_offline_resilience_legacy_404_and_account_isolation() {
    let mut server = EphemeralTestServer::start().await.expect("start live server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let tc1 = TestTelegramClient::open(&server.base_url()).expect("open TestTelegramClient");
    let alice = tc1.register_and_bind(&driver, "alice_t4_06", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t4_06", "Pass1234!").await.unwrap();

    // Phase 1: Live Online Sync + Live WS ClientDomainEvent::CommunityEvent
    let online_feed = tc1.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc1.community().runtime_mode(), CommunityRuntimeMode::Online);
    assert!(!online_feed.items.is_empty());

    let online_post = tc1
        .community()
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Online synced post T4-06".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        driver.req(Method::GET, &format!("/api/v1/community/posts/{}", online_post.post.post_id), &CommunityAuthMode::Human(bob.token.clone()), None).await.unwrap().0,
        200
    );

    tc1.client().connect_ws();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let _ = tc1.client().poll_events();

    let (st_b, bv) = driver
        .req(Method::POST, "/api/v1/community/posts", &CommunityAuthMode::Human(bob.token.clone()), Some(json!({
            "community_slug": "rust", "body": "Bob live WS trigger T4-06"
        })))
        .await
        .unwrap();
    assert_eq!(st_b, 201);
    let bob_pid = bv["post"]["post_id"].as_str().unwrap().to_string();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let mut got_ws = false;
    while tokio::time::Instant::now() < deadline {
        for ev in tc1.client().poll_events() {
            if let ClientDomainEvent::CommunityEvent { event_kind, post_id, .. } = ev {
                if event_kind == "post_created" && post_id.as_deref() == Some(bob_pid.as_str()) {
                    got_ws = true;
                    break;
                }
            }
        }
        if got_ws {
            break;
        }
        tokio::time::sleep(Duration::from_millis(35)).await;
    }
    assert!(got_ws, "Live TelegramClient WS must deliver ClientDomainEvent::CommunityEvent");

    // Phase 2: Mid-Session Server Stop -> Transparent LocalFallback on <app_dir>/community.db
    tc1.client().disconnect_ws();
    drop(driver);
    server.stop().await;

    let offline_feed = tc1.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc1.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(!offline_feed.items.is_empty(), "local SQLite fallback must serve seeded feed");
    assert!(tc1.dir_path().join("community.db").exists(), "<app_dir>/community.db must exist");

    let off_post = tc1
        .community()
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Offline local post T4-06 for @ferris_bot".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let off_pid = off_post.post.post_id.clone();

    let off_draft = tc1
        .community()
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            status: PostStatus::Draft,
            body: "Alice private offline draft T4-06".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let off_draft_pid = off_draft.post.post_id.clone();

    let bm = tc1
        .community()
        .bookmark_post(
            &off_pid,
            &gpui_telegram_proto::community_contracts::BookmarkRequest {
                folder: Some("Offline_Reads".into()),
                remove: None,
            },
        )
        .await
        .unwrap();
    assert!(bm.bookmarked && bm.folder == "Offline_Reads");

    let summon = tc1
        .community()
        .summon_bot(
            &off_pid,
            &SummonBotRequest {
                bot_actor_id: Some("b:910001".into()),
                prompt: Some("Offline systems check".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(summon.triggered, "offline local bot summon must succeed: {}", summon.reason);
    let bot_reply_cid = summon.created_comment.expect("offline bot reply").comment_id;

    // Phase 3: Reopen on same TempDir against Legacy404Server -> verify offline persistence
    let temp_dir = tc1.shutdown_keep_dir().await.expect("preserve TempDir");
    let legacy = Legacy404Server::start().await.expect("start Legacy404Server");
    let tc2 = TestTelegramClient::open_in_dir(&legacy.base_url(), temp_dir).expect("reopen client");
    tc2.bind_auth(alice.user_id as u64, &alice.username, &alice.token);

    let leg_feed = tc2.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc2.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(!leg_feed.items.is_empty());

    let detail = tc2.community().get_post(&off_pid, None).await.unwrap();
    assert_eq!(detail.post.body, "Offline local post T4-06 for @ferris_bot");
    assert!(
        detail.comments.iter().any(|c| c.comment_id == bot_reply_cid && c.author_kind.is_bot_like()),
        "offline bot reply must survive reopen against Legacy404Server"
    );
    assert_eq!(tc2.community().get_post(&off_draft_pid, None).await.unwrap().post.status, PostStatus::Draft);
    let saved_a = tc2.community().get_feed(&FeedRequestDto { tab: FeedTab::Saved, limit: 20, ..Default::default() }).await.unwrap();
    assert!(saved_a.items.iter().any(|i| i.post.post_id == off_pid && i.post.viewer_bookmark_folder.as_deref() == Some("Offline_Reads")));

    // Phase 4: Alice -> Bob Account Switch Isolation on same TempDir
    tc2.bind_auth(bob.user_id as u64, &bob.username, &bob.token);
    assert!(tc2.community().get_post(&off_pid, None).await.is_err(), "Alice offline post must not leak to Bob");
    assert!(tc2.community().get_post(&off_draft_pid, None).await.is_err(), "Alice private draft must not leak to Bob");
    let saved_b = tc2.community().get_feed(&FeedRequestDto { tab: FeedTab::Saved, limit: 20, ..Default::default() }).await.unwrap();
    assert!(saved_b.items.is_empty(), "Alice bookmarks must not leak to Bob");

    let bob_off = tc2.community().create_post(&CreatePostRequest {
        community_slug: Some("rust".into()), body: "Bob isolated offline post T4-06".into(), ..Default::default()
    }).await.unwrap();
    assert_eq!(bob_off.post.author_actor_id, bob.actor_id);

    tc2.shutdown().await;
    legacy.shutdown().await;
}

// ============================================================================
// TEST-CM-T4-07 — Real-Time 5s Background Scheduler & Restart Persistence
// ============================================================================

/// [Test ID] TEST-CM-T4-07
/// [Feature / Req] F04, F09, F11, F13 / ORIGINAL_REQUEST.md (Real-time 5s background scheduler, poll expiry, reactive bot & restart persistence)
/// [Preconditions] Ephemeral live server with its live 5s background scheduler active, connected TCP WS observer for Bob, and a `Reactive` Bot.
/// [Opaque-Box Steps] Schedule a post due in ~1.5s mentioning `@react_t4_07_bot` (confirm 404 before due), create a Poll with Human + Bot votes expiring in ~1.5s, wait naturally for the live 5s background ticker to fire and emit WS events, close handles, restart server, and re-verify disk state.
/// [Observable Assertions] Within 12s the live 5s ticker publishes the scheduled post (`scheduled_post_published`), triggers reactive Bot comment (`comment_created`), closes the poll (`poll_closed`), rejects late poll votes (`410`), and preserves all state across `server.restart()`.
#[tokio::test]
async fn test_cm_t4_07_realtime_5s_scheduler_scheduled_publish_poll_expiry_bot_turns_restart() {
    let mut server = EphemeralTestServer::start().await.expect("start live server");
    let driver = CommunityHttpDriver::new(server.base_url());

    let alice = driver.register_human("alice_t4_07", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t4_07", "Pass1234!").await.unwrap();
    let react_bot = driver
        .register_community_bot(&alice.token, "react_t4_07_bot", "Reactive Scheduler Bot", "reactive")
        .await
        .unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(react_bot.bot_token.clone());
    assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &b_auth, None).await.unwrap().0, 200);

    let mut ws = connect_ws(&server, &bob.token).await.expect("connect Bob WS");

    // 1. Schedule post for now + 1500ms mentioning @react_t4_07_bot (must be invisible to Bob initially)
    let due_at = now_ms() + 1_500;
    let (st_s, sv) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "rust",
            "format": "micro",
            "status": "scheduled",
            "scheduled_at_ms": due_at,
            "body": "T407ScheduledRocket launching on 5s ticker! @react_t4_07_bot thoughts?"
        })))
        .await
        .unwrap();
    assert_eq!(st_s, 201, "create scheduled post failed: {sv}");
    let sched_pid = sv["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(
        driver.req(Method::GET, &format!("/api/v1/community/posts/{sched_pid}"), &b_auth, None).await.unwrap().0,
        404,
        "scheduled post must stay invisible before due time"
    );

    // 2. Create open Poll, cast Human + Bot votes while open, then set deadline to now + 1500ms
    let (st_p, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "rust",
            "format": "poll",
            "body": "T4-07 expiring poll",
            "poll": { "question": "Best scheduler?", "options": ["Tokio", "AsyncStd"], "multi_select": false, "duration_secs": 300 }
        })))
        .await
        .unwrap();
    assert_eq!(st_p, 201, "create poll failed: {pv}");
    let poll_pid = pv["post"]["post_id"].as_str().unwrap().to_string();
    let opt0 = pv["post"]["poll"]["options"][0]["option_id"].as_str().unwrap().to_string();
    let opt1 = pv["post"]["poll"]["options"][1]["option_id"].as_str().unwrap().to_string();

    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{poll_pid}/poll-vote"), &a_auth, Some(json!({ "option_ids": [opt0] }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{poll_pid}/poll-vote"), &bot_auth, Some(json!({ "option_ids": [opt1] }))).await.unwrap().0, 200);

    let expire_at = now_ms() + 1_500;
    server.state().unwrap().db.pool().with_writer(|conn| {
        conn.execute(
            "UPDATE community_posts SET poll_closes_at_ms = ?1 WHERE post_id = ?2",
            rusqlite::params![expire_at, poll_pid],
        )
        .map_err(|e| gpui_telegram_core::TelegramError::Database(e.to_string()))?;
        Ok(())
    }).unwrap();

    // 3. Wait naturally (up to 12s) for the live 5s background scheduler to fire & broadcast all 3 WS events
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    let mut got_sched_pub = false;
    let mut got_poll_closed = false;
    let mut got_reactive_reply = false;
    while !(got_sched_pub && got_poll_closed && got_reactive_reply) {
        let now_inst = tokio::time::Instant::now();
        if now_inst >= deadline {
            break;
        }
        let rem = deadline - now_inst;
        if let Ok(WsServerFrame::Update(UpdateEvent::CommunityEvent {
            event_kind,
            post_id,
            actor_id,
            ..
        })) = recv_ws_frame_matching(&mut ws, rem, |f| {
            matches!(f, WsServerFrame::Update(UpdateEvent::CommunityEvent { .. }))
        })
        .await
        {
            if event_kind == "scheduled_post_published" && post_id.as_deref() == Some(sched_pid.as_str()) {
                got_sched_pub = true;
            } else if event_kind == "poll_closed" && post_id.as_deref() == Some(poll_pid.as_str()) {
                got_poll_closed = true;
            } else if event_kind == "comment_created"
                && post_id.as_deref() == Some(sched_pid.as_str())
                && actor_id.as_deref() == Some(react_bot.actor_id.as_str())
            {
                got_reactive_reply = true;
            }
        }
    }
    assert!(got_sched_pub, "5s background scheduler must emit scheduled_post_published over WS");
    assert!(got_poll_closed, "5s background scheduler must emit poll_closed over WS");
    assert!(got_reactive_reply, "5s scheduler scheduled-post publish must wake reactive Bot & emit comment_created");

    // 4. Verify REST state before restart (late poll vote rejected with 410 Gone)
    let (st_sp, sp_v) = driver.req(Method::GET, &format!("/api/v1/community/posts/{sched_pid}"), &b_auth, None).await.unwrap();
    assert_eq!(st_sp, 200);
    let sp_det: PostDetailResponse = serde_json::from_value(sp_v).unwrap();
    assert_eq!(sp_det.post.status, PostStatus::Published);
    assert!(sp_det.comments.iter().any(|c| c.author_actor_id == react_bot.actor_id && c.author_kind == ActorKind::Bot));

    let (st_late, late_v) = driver
        .req(Method::POST, &format!("/api/v1/community/posts/{poll_pid}/poll-vote"), &b_auth, Some(json!({ "option_ids": [opt0] })))
        .await
        .unwrap();
    assert_eq!(st_late, 410, "late vote on closed poll must be rejected with 410 Gone: {late_v}");

    // 5. Close WS & HTTP handles cleanly before restarting server, then confirm disk persistence
    drop(ws);
    drop(driver);
    server.restart().await.expect("server restart cleanly");

    let d2 = CommunityHttpDriver::new(server.base_url());
    let (st_sp2, sp_v2) = d2.req(Method::GET, &format!("/api/v1/community/posts/{sched_pid}"), &b_auth, None).await.unwrap();
    assert_eq!(st_sp2, 200);
    let sp_det2: PostDetailResponse = serde_json::from_value(sp_v2).unwrap();
    assert_eq!(sp_det2.post.status, PostStatus::Published);
    assert!(sp_det2.comments.iter().any(|c| c.author_actor_id == react_bot.actor_id));

    let (st_pl2, pl_v2) = d2.req(Method::GET, &format!("/api/v1/community/posts/{poll_pid}"), &b_auth, None).await.unwrap();
    assert_eq!(st_pl2, 200);
    let poll2 = serde_json::from_value::<PostDetailResponse>(pl_v2).unwrap().post.poll.unwrap();
    assert!(poll2.is_closed, "poll.is_closed must persist after restart");
    assert_eq!((poll2.total_votes, poll2.total_human_votes, poll2.total_bot_votes), (2, 1, 1));
    assert_eq!(
        d2.req(Method::POST, &format!("/api/v1/community/posts/{poll_pid}/poll-vote"), &b_auth, Some(json!({ "option_ids": [opt0] }))).await.unwrap().0,
        410
    );

    drop(d2);
    server.stop().await;
}

// ============================================================================
// TEST-CM-T4-08 — Global Explore, Burst Trending & Tantivy BM25 Discovery
// ============================================================================

fn encode_query_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 2);
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// [Test ID] TEST-CM-T4-08
/// [Feature / Req] F07, F10, F11, F15, F21 / ORIGINAL_REQUEST.md (Global Explore, burst trending `#GPUICommunity2026` & Tantivy BM25 discovery)
/// [Preconditions] Ephemeral live server with 4 Humans, 2 Bots, and 5 topic communities populated with 32 English + CJK posts including a 12-post `#GPUICommunity2026` burst.
/// [Opaque-Box Steps] Verify `/trending` ranks `#GPUICommunity2026` at the top, run Tantivy BM25 search across all 7 tabs & structured filters (`c:`, `from:`, `kind:bot`, `has:poll`, `has:media`) + CJK, delete a post and confirm immediate search pruning, then drive live headless GPUI `ExploreSearchView` across themes & locales.
/// [Observable Assertions] `#GPUICommunity2026` surfaces in top trending with Human+Bot counts, every search tab & structured operator returns exact filtered hits, deleted post disappears immediately from Tantivy, and `ExploreSearchView` renders cleanly across all 4 themes and both locales.
#[test]
fn test_cm_t4_08_global_explore_burst_trending_and_tantivy_bm25_discovery() {
    run_community_gpui_test("test_cm_t4_08_global_explore_burst_trending_and_tantivy_bm25_discovery");
}

async fn body_t4_08(mut app: AsyncApp) {
    let rt = new_t4_tokio_rt();
    let mut server = rt.block_on(EphemeralTestServer::start()).expect("start live server");
    let base_url = server.base_url();
    let driver = CommunityHttpDriver::new(&base_url);

    let alice = rt.block_on(async {
        let alice = driver.register_human("alice_t4_08", "Pass1234!").await.unwrap();
        let bob = driver.register_human("bob_t4_08", "Pass1234!").await.unwrap();
        let carol = driver.register_human("carol_t4_08", "Pass1234!").await.unwrap();
        let dave = driver.register_human("dave_t4_08", "Pass1234!").await.unwrap();
        let rust_bot = driver.register_community_bot(&alice.token, "t4_08_rust_bot", "T408 Rust Bot", "manual").await.unwrap();
        let cjk_bot = driver.register_community_bot(&alice.token, "t4_08_cjk_bot", "T408 CJK Bot", "manual").await.unwrap();

        let a_auth = CommunityAuthMode::Human(alice.token.clone());
        let b_auth = CommunityAuthMode::Human(bob.token.clone());
        let c_auth = CommunityAuthMode::Human(carol.token.clone());
        let d_auth = CommunityAuthMode::Human(dave.token.clone());
        let rb_auth = CommunityAuthMode::DirectBot(rust_bot.bot_token.clone());
        let cb_auth = CommunityAuthMode::DirectBot(cjk_bot.bot_token.clone());

        for auth in [&a_auth, &b_auth, &c_auth, &d_auth] {
            assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", auth, None).await.unwrap().0, 200);
        }

        let slugs = ["t4_08_rust", "t4_08_gpui", "t4_08_ai", "t4_08_design", "t4_08_systems"];
        for slug in slugs {
            let (st, v) = driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({
                "slug": slug, "name": format!("Hub {slug}"), "allow_bots": true
            }))).await.unwrap();
            assert_eq!(st, 201, "create {slug} failed: {v}");
        }

        let (pub_mid, _) = driver.upload_png(&alice.token, "explore_fig.png", (60, 200, 120, 255), None).await.unwrap();
        let all_auths = [&a_auth, &b_auth, &c_auth, &d_auth, &rb_auth, &cb_auth];

        // 12 rapid multi-author burst posts for #GPUICommunity2026 across all 5 communities
        for (i, auth) in all_auths.iter().cycle().take(12).enumerate() {
            let slug = slugs[i % slugs.len()];
            let (st, pv) = driver.req(Method::POST, "/api/v1/community/posts", auth, Some(json!({
                "community_slug": slug,
                "title": format!("GPUI Community Milestone #{i}"),
                "body": format!("Rapid multi-actor launch discussion #{i} #GPUICommunity2026 @t4_08_rust_bot")
            }))).await.unwrap();
            assert_eq!(st, 201);
            let pid = pv["post"]["post_id"].as_str().unwrap();
            let _ = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/vote"), &a_auth, Some(json!({ "direction": 1 }))).await;
            let _ = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/vote"), &b_auth, Some(json!({ "direction": 1 }))).await;
        }

        // 6 CJK technical posts (Humans + Bot)
        for i in 0..6 {
            let auth = if i % 2 == 0 { &cb_auth } else { &c_auth };
            let slug = slugs[i % slugs.len()];
            assert_eq!(
                driver.req(Method::POST, "/api/v1/community/posts", auth, Some(json!({
                    "community_slug": slug,
                    "title": format!("零拷贝图形管线深度评测 {i}"),
                    "body": format!("异步并发渲染架构与桌面端优化第 {i} 期 #中文架构")
                }))).await.unwrap().0,
                201
            );
        }

        // 2 Media posts + 2 Poll posts + 9 general English posts + 1 delete-anchor post = 32 posts total
        for i in 0..2 {
            assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
                "community_slug": "t4_08_design", "format": "media",
                "body": format!("Visual showcase media post #{i}"),
                "media": [{ "media_id": pub_mid, "kind": "image" }]
            }))).await.unwrap().0, 201);
            assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &rb_auth, Some(json!({
                "community_slug": "t4_08_rust", "format": "poll",
                "body": format!("Community poll #{i}"),
                "poll": { "question": format!("T408 Poll {i}?"), "options": ["Option A", "Option B"], "duration_secs": 3600 }
            }))).await.unwrap().0, 201);
        }
        for i in 0..9 {
            let auth = all_auths[i % all_auths.len()];
            let slug = slugs[i % slugs.len()];
            assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", auth, Some(json!({
                "community_slug": slug, "body": format!("Corpus filler systems report #{i} in {slug}")
            }))).await.unwrap().0, 201);
        }
        let del_pid = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "t4_08_systems", "title": "ZephyrDeleteAnchor2026", "body": "Ephemeral ZephyrDeleteAnchor2026 doc"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

        // 1. Burst trending verification
        let (st_t, tv) = driver.req(Method::GET, "/api/v1/community/trending?limit=20", &a_auth, None).await.unwrap();
        assert_eq!(st_t, 200);
        let trending: TrendingResponse = serde_json::from_value(tv).unwrap();
        let burst_pos = trending.topics.iter().position(|t| t.slug_or_tag.eq_ignore_ascii_case("gpuicommunity2026"))
            .expect("#GPUICommunity2026 must appear in /trending");
        assert!(burst_pos < 3, "#GPUICommunity2026 must rank in top 3 trending, got rank {burst_pos}");
        let burst = &trending.topics[burst_pos];
        assert!(burst.posts_count_24h >= 12 && burst.human_posts_count >= 8 && burst.bot_posts_count >= 4);

        // 2. All 7 Search tabs + English/CJK BM25 + Structured Filters
        for (tab, q) in [
            ("top", "GPUICommunity2026"),
            ("latest", "GPUICommunity2026"),
            ("people", "alice_t4_08"),
            ("bots", "t4_08_rust_bot"),
            ("communities", "t4_08_rust"),
            ("media", ""),
            ("polls", ""),
        ] {
            let path = format!("/api/v1/community/search?tab={tab}&q={}", encode_query_value(q));
            let (st, sv) = driver.req(Method::GET, &path, &a_auth, None).await.unwrap();
            assert_eq!(st, 200, "search tab {tab} failed: {sv}");
            let res: CommunitySearchResultDto = serde_json::from_value(sv).unwrap();
            match tab {
                "people" => assert!(res.actors.iter().any(|a| a.handle == "alice_t4_08" && a.actor_kind == ActorKind::Human)),
                "bots" => assert!(res.actors.iter().any(|a| a.handle == "t4_08_rust_bot" && a.actor_kind.is_bot_like())),
                "communities" => assert!(res.communities.iter().any(|c| c.slug == "t4_08_rust")),
                "media" => assert!(res.posts.iter().any(|p| !p.media.is_empty())),
                "polls" => assert!(res.posts.iter().any(|p| p.poll.is_some())),
                _ => assert!(!res.posts.is_empty(), "tab {tab} must return matching posts"),
            }
        }

        let (_, cjk_v) = driver.req(Method::GET, &format!("/api/v1/community/search?q={}", encode_query_value("零拷贝图形管线")), &a_auth, None).await.unwrap();
        let cjk_res: CommunitySearchResultDto = serde_json::from_value(cjk_v).unwrap();
        assert!(cjk_res.posts.len() >= 4, "CJK BM25 search must match indexed CJK posts");

        type PostPred = fn(&gpui_telegram_proto::community_contracts::CommunityPostDto) -> bool;
        let filters: [(&str, PostPred); 5] = [
            ("c:t4_08_rust", |p| p.community_slug.as_deref() == Some("t4_08_rust")),
            ("from:t4_08_rust_bot", |p| p.author_handle == "t4_08_rust_bot"),
            ("kind:bot", |p| p.author_kind.is_bot_like()),
            ("has:poll", |p| p.poll.is_some()),
            ("has:media", |p| !p.media.is_empty()),
        ];
        for (query_expr, check) in filters {
            let (st, sv) = driver.req(Method::GET, &format!("/api/v1/community/search?q={}", encode_query_value(query_expr)), &a_auth, None).await.unwrap();
            assert_eq!(st, 200);
            let sr: CommunitySearchResultDto = serde_json::from_value(sv).unwrap();
            assert!(!sr.posts.is_empty(), "filter '{query_expr}' must return >=1 post");
            assert!(sr.posts.iter().all(|p| check(p)), "filter '{query_expr}' returned out-of-scope post");
        }

        // 3. Delete post -> immediate removal from Tantivy BM25 search
        let (_, before_del) = driver.req(Method::GET, "/api/v1/community/search?q=ZephyrDeleteAnchor2026", &a_auth, None).await.unwrap();
        assert!(serde_json::from_value::<CommunitySearchResultDto>(before_del).unwrap().posts.iter().any(|p| p.post_id == del_pid));
        assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/posts/{del_pid}"), &a_auth, None).await.unwrap().0, 200);
        let (_, after_del) = driver.req(Method::GET, "/api/v1/community/search?q=ZephyrDeleteAnchor2026", &a_auth, None).await.unwrap();
        assert!(serde_json::from_value::<CommunitySearchResultDto>(after_del).unwrap().posts.is_empty(), "deleted post must vanish from Tantivy immediately");

        alice
    });

    // 4. Live Headless GPUI ExploreSearchView across themes (Cmd+D) & locales (Cmd+L)
    let host = open_live_community_host(&mut app, &base_url, alice.user_id as u64, &alice.username, &alice.token, 1440.0, 900.0);
    let cw = host.open_community_and_wait(&mut app).await;
    assert_eq!(cw.read_with(&app, |w, _| w.state.runtime_mode).unwrap(), CommunityRuntimeMode::Online);

    let _ = cw.update(&mut app, |w, cx| w.open_explore_with_query("#GPUICommunity2026".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        let st = &cw.read(cx).state;
        !st.search_loading && st.search_result.as_ref().is_some_and(|r| !r.posts.is_empty())
    }).await);
    assert!(cw.read_with(&app, |w, _| w.state.trending.iter().any(|t| t.slug_or_tag.eq_ignore_ascii_case("gpuicommunity2026"))).unwrap());

    let _ = cw.update(&mut app, |w, cx| w.open_explore_with_query("is:bot c:t4_08_rust".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        let st = &cw.read(cx).state;
        !st.search_loading && st.search_result.as_ref().is_some_and(|r| !r.posts.is_empty())
    }).await);
    cw.read_with(&app, |w, _| {
        let res = w.state.search_result.as_ref().unwrap();
        assert!(res.posts.iter().all(|p| p.author_kind.is_bot_like() && p.community_slug.as_deref() == Some("t4_08_rust")));
    }).unwrap();

    // Render ExploreSearchView across all 4 themes & both locales
    let mut modes = HashSet::new();
    for _ in 0..4 {
        modes.insert(format!("{:?}", cw.read_with(&app, |w, _| w.palette.mode).unwrap()));
        host.draw(&mut app);
        host.key(&mut app, "cmd-d");
    }
    assert_eq!(modes.len(), 4);
    let lang1 = cw.read_with(&app, |w, _| w.language_id.clone()).unwrap();
    host.key(&mut app, "cmd-l");
    host.draw(&mut app);
    let lang2 = cw.read_with(&app, |w, _| w.language_id.clone()).unwrap();
    assert_ne!(lang1, lang2);

    drop(cw);
    drop(host);
    drop(driver);
    rt.block_on(server.stop());
}

// ============================================================================
// TEST-CM-T4-09 — Curation: Custom Feeds, Lists, Bookmarks & Sidebar Badges
// ============================================================================

/// [Test ID] TEST-CM-T4-09
/// [Feature / Req] F02, F11, F13, F15, F20 / ORIGINAL_REQUEST.md (Custom Feeds, X Lists, Bookmark folders & left-sidebar unread badge sync)
/// [Preconditions] Ephemeral live server with Alice, Bob, Carol, Bob's Bot `rust_t4_09_bot`, and 2 isolated communities (`c/t4_09_target`, `c/t4_09_outside`).
/// [Opaque-Box Steps] Curate "Rust + AI Bots" List, verify strict List feed isolation; bookmark 4 posts across "Reading List" & "Architecture" folders, move & remove cleanly; have Bob & Bot reply, mention, upvote, award & follow Alice (verifying WS + REST notifications); mount live headless GPUI `AppShell`, verify Expanded + Collapsed + `AppleAppSidebar` unread badges sync, then mark all read.
/// [Observable Assertions] List feed strictly excludes non-list sources, bookmark folder moves & removals persist, categorized notifications arrive via REST + WS, all left-sidebar surfaces display exact unread badge count, and mark-all-read zeroes every badge & server unread count.
#[test]
fn test_cm_t4_09_curation_lists_bookmark_folders_notifications_and_sidebar_badges() {
    run_community_gpui_test("test_cm_t4_09_curation_lists_bookmark_folders_notifications_and_sidebar_badges");
}

async fn body_t4_09(mut app: AsyncApp) {
    let rt = new_t4_tokio_rt();
    let mut server = rt.block_on(EphemeralTestServer::start()).expect("start live server");
    let base_url = server.base_url();
    let driver = CommunityHttpDriver::new(&base_url);

    let (alice, list_id, expected_unread) = rt.block_on(async {
        let alice = driver.register_human("alice_t4_09", "Pass1234!").await.unwrap();
        let bob = driver.register_human("bob_t4_09", "Pass1234!").await.unwrap();
        let carol = driver.register_human("carol_t4_09", "Pass1234!").await.unwrap();
        let rust_bot = driver
            .register_community_bot(&bob.token, "rust_t4_09_bot", "Rust T409 Bot", "manual")
            .await
            .unwrap();

        let a_auth = CommunityAuthMode::Human(alice.token.clone());
        let b_auth = CommunityAuthMode::Human(bob.token.clone());
        let c_auth = CommunityAuthMode::Human(carol.token.clone());
        let bot_auth = CommunityAuthMode::DirectBot(rust_bot.bot_token.clone());
        for auth in [&a_auth, &b_auth, &c_auth] {
            assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", auth, None).await.unwrap().0, 200);
        }

        for slug in ["t4_09_target", "t4_09_outside"] {
            assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({
                "slug": slug, "name": format!("Comm {slug}"), "allow_bots": true
            }))).await.unwrap().0, 201);
        }

        let p_list_1 = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({
            "community_slug": "t4_09_target", "body": "Bob curated Rust post"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
        let p_list_2 = driver.req(Method::POST, "/api/v1/community/posts", &bot_auth, Some(json!({
            "community_slug": "t4_09_target", "body": "Bot curated AI analysis"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
        let p_outside = driver.req(Method::POST, "/api/v1/community/posts", &c_auth, Some(json!({
            "community_slug": "t4_09_outside", "body": "Carol unrelated outside post"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
        let alice_pid = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "t4_09_target", "body": "Alice architecture thread for notifications"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
        let p_bm_3 = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "t4_09_target", "body": "Bookmarkable architecture RFC"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
        let p_bm_4 = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "t4_09_target", "body": "Temporary bookmark to remove"
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

        // 1. Custom List ("Rust + AI Bots") strict source isolation
        let (st_l, lv) = driver.req(Method::POST, "/api/v1/community/lists", &a_auth, Some(json!({
            "name": "Rust + AI Bots",
            "description": "Strict curated feed of Bob, Rust Bot & c/t4_09_target",
            "kind": "mixed",
            "is_private": false,
            "is_pinned": true,
            "member_actor_ids": [bob.actor_id, rust_bot.actor_id],
            "community_slugs": ["t4_09_target"]
        }))).await.unwrap();
        assert_eq!(st_l, 201, "create list failed: {lv}");
        let list_id = lv["list_id"].as_str().unwrap().to_string();

        let (st_lf, lfv) = driver.req(Method::GET, &format!("/api/v1/community/feed?tab=list&list_id={list_id}&limit=30"), &a_auth, None).await.unwrap();
        assert_eq!(st_lf, 200);
        let list_feed: FeedResponseDto = serde_json::from_value(lfv).unwrap();
        assert!(list_feed.items.iter().any(|i| i.post.post_id == p_list_1));
        assert!(list_feed.items.iter().any(|i| i.post.post_id == p_list_2));
        assert!(!list_feed.items.iter().any(|i| i.post.post_id == p_outside), "outside post must NEVER appear in scoped List feed");

        // 2. Bookmark 4 posts into 2 custom folders, move between folders & remove 1
        for (pid, folder) in [(&p_list_1, "Reading List"), (&p_list_2, "Reading List"), (&p_bm_3, "Architecture"), (&p_bm_4, "Architecture")] {
            assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/bookmark"), &a_auth, Some(json!({ "folder": folder }))).await.unwrap().0, 200);
        }
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{p_list_2}/bookmark"), &a_auth, Some(json!({ "folder": "Architecture" }))).await.unwrap().0, 200);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{p_bm_4}/bookmark"), &a_auth, Some(json!({ "remove": true }))).await.unwrap().0, 200);

        let (st_sv, svv) = driver.req(Method::GET, "/api/v1/community/feed?tab=saved&limit=30", &a_auth, None).await.unwrap();
        assert_eq!(st_sv, 200);
        let saved: FeedResponseDto = serde_json::from_value(svv).unwrap();
        assert!(saved.items.iter().any(|i| i.post.post_id == p_list_1 && i.post.viewer_bookmark_folder.as_deref() == Some("Reading List")));
        assert!(saved.items.iter().any(|i| i.post.post_id == p_list_2 && i.post.viewer_bookmark_folder.as_deref() == Some("Architecture")));
        assert!(saved.items.iter().any(|i| i.post.post_id == p_bm_3 && i.post.viewer_bookmark_folder.as_deref() == Some("Architecture")));
        assert!(!saved.items.iter().any(|i| i.post.post_id == p_bm_4), "removed bookmark must vanish");

        // 3. Bob & Bot trigger categorized notifications + live WS frames for Alice
        let mut ws = connect_ws(&server, &alice.token).await.expect("connect Alice WS");
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{alice_pid}/comments"), &b_auth, Some(json!({ "body": "Great architecture summary!" }))).await.unwrap().0, 201);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{alice_pid}/comments"), &bot_auth, Some(json!({ "body": "Agree @alice_t4_09 — solid latency numbers." }))).await.unwrap().0, 201);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{alice_pid}/vote"), &b_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{alice_pid}/award"), &b_auth, Some(json!({ "award_kind": "insightful", "message": "Top insight!" }))).await.unwrap().0, 200);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/actors/{}/follow", alice.actor_id), &b_auth, None).await.unwrap().0, 200);

        let mut ws_kinds = HashSet::new();
        let ws_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        while ws_kinds.len() < 4 && tokio::time::Instant::now() < ws_deadline {
            let rem = ws_deadline - tokio::time::Instant::now();
            if let Ok(WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, .. })) =
                recv_ws_frame_matching(&mut ws, rem, |f| matches!(f, WsServerFrame::Update(UpdateEvent::CommunityEvent { .. }))).await
            {
                ws_kinds.insert(event_kind);
            }
        }
        assert!(ws_kinds.contains("comment_created") && ws_kinds.contains("award_given") && ws_kinds.contains("actor_followed"), "missing WS kinds: {ws_kinds:?}");
        drop(ws);

        let (st_n, nv) = driver.req(Method::GET, "/api/v1/community/notifications?limit=40", &a_auth, None).await.unwrap();
        assert_eq!(st_n, 200);
        let notifs: NotificationsListResponse = serde_json::from_value(nv).unwrap();
        assert!(notifs.unread_count >= 4, "expected >=4 categorized unread notifications, got {}", notifs.unread_count);
        assert!(notifs.notifications.iter().any(|n| n.kind == CommunityNotificationKind::Reply));
        assert!(notifs.notifications.iter().any(|n| n.kind == CommunityNotificationKind::Mention));
        assert!(notifs.notifications.iter().any(|n| n.kind == CommunityNotificationKind::Award));
        assert!(notifs.notifications.iter().any(|n| n.kind == CommunityNotificationKind::Follow));

        (alice, list_id, notifs.unread_count as u32)
    });

    // 4. Live Headless GPUI: Left-Sidebar Unread Badge Sync (Expanded + Collapsed + AppleAppSidebar) -> Mark Read Clears to 0
    let host = open_live_community_host(&mut app, &base_url, alice.user_id as u64, &alice.username, &alice.token, 1440.0, 900.0);
    let cw = host.open_community_and_wait(&mut app).await;
    let shell = host.shell.clone();

    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        cw.read(cx).state.unread_notifications == expected_unread
            && shell.read(cx).apple_sidebar.read(cx).community_unread_count == expected_unread
    }).await, "left-sidebar badge must sync with server unread count ({expected_unread})");

    // Verify NavRail badge (backing Expanded #sidebar-nav-community & Collapsed #rail-community-btn) + AppleAppSidebar
    shell.read_with(&app, |s, cx| {
        let nav_badge = s.nav_rail.read(cx).items.iter().find(|i| i.destination == NavDestination::Community).unwrap().unread_badge;
        assert_eq!(nav_badge, expected_unread);
        assert_eq!(s.apple_sidebar.read(cx).community_unread_count, expected_unread);
    }).unwrap();
    let _ = shell.update(&mut app, |s, cx| s.sidebar.update(cx, |sb, _| sb.set_collapsed(false)));
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| s.sidebar.update(cx, |sb, _| sb.set_collapsed(true)));
    host.draw(&mut app);

    // Inspect custom list & bookmark folders + categorized notifications in GPUI, then mark all read
    let _ = cw.update(&mut app, |w, cx| {
        assert!(w.state.lists.iter().any(|l| l.list_id == list_id));
        w.navigate_to(CommunityRoute::Notifications, cx);
        for cat in NotificationCategoryTab::ALL_TABS {
            w.set_notification_category(cat, cx);
        }
    });
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| w.mark_all_notifications_read(cx));
    assert!(pump_until(&mut app, Duration::from_secs(5), |cx| {
        cw.read(cx).state.unread_notifications == 0
            && shell.read(cx).apple_sidebar.read(cx).community_unread_count == 0
            && shell.read(cx).nav_rail.read(cx).items.iter().find(|i| i.destination == NavDestination::Community).unwrap().unread_badge == 0
    }).await, "marking all notifications read must clear every left-sidebar unread badge to 0");
    host.draw(&mut app);

    drop(cw);
    drop(shell);
    drop(host);
    rt.block_on(async {
        let (_, nv2) = driver.req(Method::GET, "/api/v1/community/notifications?limit=10", &CommunityAuthMode::Human(alice.token.clone()), None).await.unwrap();
        assert_eq!(serde_json::from_value::<NotificationsListResponse>(nv2).unwrap().unread_count, 0);
        drop(driver);
        server.stop().await;
    });
}

// ============================================================================
// TEST-CM-T4-10 — Full Desktop Headless UX Parity Matrix, Security & Logout
// ============================================================================

/// [Test ID] TEST-CM-T4-10
/// [Feature / Req] F11, F15, F16, F17, F18, F19, F20, F21 / ORIGINAL_REQUEST.md (Full Desktop Headless UX Parity Matrix, Security & Logout)
/// [Preconditions] Ephemeral live server with Alice (+ owned Bot, owned `c/t4_10_matrix`) and Bob (+ owned Bot); compiled desktop asset source.
/// [Opaque-Box Steps] Verify `IconName::ALL == 64` + 10 `EXTRA` SVGs and `Cmd+1..9`/`0` preservation; verify non-owner Act-As `403`; boot live headless GPUI `AppShell` and open Community through all 7 left-sidebar/shortcut entries; traverse & draw all 9 views + `ComposerModal` across 3 viewports (`1440×900`, `1040×800`, `820×720`), 4 themes (`Cmd+D`), and 2 locales (`Cmd+L`) under `LatencyBudget` + `pump_with_ticks`; execute `AppShell::logout`.
/// [Observable Assertions] Asset & shortcut invariants hold, non-owner Act-As fails with `403`, all 7 entry points activate `ActiveWorkspacePanel::Community`, all 10 UI surfaces render within `LatencyBudget` across all breakpoints/themes/locales with non-blocking ticks, and `logout()` cleanly tears down `community_workspace` and resets badges to `0`.
#[test]
fn test_cm_t4_10_full_desktop_headless_ux_parity_matrix_security_and_logout() {
    run_community_gpui_test("test_cm_t4_10_full_desktop_headless_ux_parity_matrix_security_and_logout");
}

async fn body_t4_10(mut app: AsyncApp) {
    // 1. Asset Invariant: IconName::ALL == 64 & all 10 Community SVGs strictly in IconName::EXTRA
    assert_eq!(IconName::ALL.len(), 64, "IconName::ALL MUST remain strictly 64");
    let community_icons = [
        IconName::CommunityGlobe,
        IconName::UpvoteArrow,
        IconName::DownvoteArrow,
        IconName::Repost,
        IconName::Bookmark,
        IconName::FlameTrending,
        IconName::ShieldMod,
        IconName::SparkleAlgo,
        IconName::CommunityAward,
        IconName::CommunityThread,
    ];
    let assets = TelegramAssetSource::new();
    for icon in community_icons {
        assert!(!IconName::ALL.contains(&icon), "{icon:?} must NOT enter IconName::ALL");
        assert!(IconName::EXTRA.contains(&icon), "{icon:?} must exist in IconName::EXTRA");
        let raw = assets.load(icon.path()).unwrap().unwrap_or_else(|| panic!("missing SVG {}", icon.path()));
        let svg = std::str::from_utf8(&raw).unwrap();
        assert!(svg.contains("<svg") && svg.contains("viewBox=\"0 0 24 24\"") && svg.contains("currentColor"));
    }

    // 2. Shortcut Digit Preservation: Cmd+1..9 intact & "0" => None
    assert_eq!(nav_destination_for_shortcut_digit("1"), Some(NavDestination::Chats));
    for d in ["2", "3", "4", "5", "6", "7", "8"] {
        assert!(nav_destination_for_shortcut_digit(d).is_some(), "Cmd+{d} mapping must remain intact");
    }
    assert_eq!(nav_destination_for_shortcut_digit("9"), Some(NavDestination::Browser));
    assert_eq!(nav_destination_for_shortcut_digit("0"), None);
    assert_eq!(NavRail::new().items.len(), 11, "NavRail must contain exactly 11 destinations including Community");

    // 3. Boot Live Server & Verify Non-Owner Act-As 403 Security Boundary
    let rt = new_t4_tokio_rt();
    let mut server = rt.block_on(EphemeralTestServer::start()).expect("start live server");
    let base_url = server.base_url();
    let driver = CommunityHttpDriver::new(&base_url);

    let (alice, alice_bot, seed_pid) = rt.block_on(async {
        let alice = driver.register_human("alice_t4_10", "Pass1234!").await.unwrap();
        let bob = driver.register_human("bob_t4_10", "Pass1234!").await.unwrap();
        let alice_bot = driver.register_community_bot(&alice.token, "alice_t4_10_bot", "Alice Matrix Bot", "manual").await.unwrap();
        let bob_bot = driver.register_community_bot(&bob.token, "bob_t4_10_bot", "Bob Matrix Bot", "manual").await.unwrap();

        let a_auth = CommunityAuthMode::Human(alice.token.clone());
        let b_auth = CommunityAuthMode::Human(bob.token.clone());
        assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &a_auth, None).await.unwrap().0, 200);
        assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &b_auth, None).await.unwrap().0, 200);

        assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({
            "slug": "t4_10_matrix", "name": "T4-10 Parity Matrix", "allow_bots": true
        }))).await.unwrap().0, 201);
        let pid = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "t4_10_matrix", "title": "Parity Matrix Anchor", "body": "Live post for T4-10 matrix."
        }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

        // Cross-owner Act-As spoofing MUST return 403 Forbidden
        let spoof_bob_as_alice_bot = CommunityAuthMode::ActAsBot {
            owner_token: bob.token.clone(),
            actor_id: alice_bot.actor_id.clone(),
        };
        assert_eq!(
            driver.req(Method::POST, "/api/v1/community/posts", &spoof_bob_as_alice_bot, Some(json!({ "body": "Spoofed post" }))).await.unwrap().0,
            403,
            "Bob acting as Alice's bot must receive 403 Forbidden"
        );
        assert_eq!(
            driver.req(Method::POST, "/api/v1/community/communities/t4_10_matrix/mod-actions", &spoof_bob_as_alice_bot, Some(json!({
                "action": "pin", "target_kind": "post", "target_id": pid
            }))).await.unwrap().0,
            403
        );
        let spoof_alice_as_bob_bot = CommunityAuthMode::ActAsBot {
            owner_token: alice.token.clone(),
            actor_id: bob_bot.actor_id.clone(),
        };
        assert_eq!(
            driver.req(Method::POST, "/api/v1/community/posts", &spoof_alice_as_bob_bot, Some(json!({ "body": "Cross-owner spoof" }))).await.unwrap().0,
            403
        );
        (alice, alice_bot, pid)
    });

    // 4. Live Headless GPUI AppShell: Exercise ALL 7 Left-Sidebar & Shortcut Entry Points
    let host = open_live_community_host(&mut app, &base_url, alice.user_id as u64, &alice.username, &alice.token, 1440.0, 900.0);
    let shell = host.shell.clone();

    // (1) Expanded ChatSidebar (#sidebar-nav-community)
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(false);
            sb.handle_unified_action(UnifiedSidebarAction::SelectDestination(NavDestination::Community), scx);
        });
    });
    assert!(pump_until(&mut app, Duration::from_secs(6), |cx| {
        let s = shell.read(cx);
        s.active_panel == ActiveWorkspacePanel::Community
            && s.should_hide_composer(cx)
            && s.inspect_state(cx).window.active_panel == "Community"
            && s.community_workspace.as_ref().is_some_and(|w| !w.read(cx).state.feed_loading && w.read(cx).state.identities.is_some())
    }).await);

    // (2) Collapsed 64px rail (#rail-community-btn)
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(true);
            sb.handle_unified_action(UnifiedSidebarAction::SelectDestination(NavDestination::Community), scx);
        });
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| shell.read(cx).active_panel == ActiveWorkspacePanel::Community).await);

    // (3) AppleAppSidebar (AppleSidebarSection::Community)
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.select_apple_sidebar_section(AppleSidebarSection::Community, cx);
    });
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    // (4) NavRail (NavDestination::Community)
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.nav_rail.update(cx, |r, rcx| { r.select(NavDestination::Community); rcx.notify(); });
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| shell.read(cx).active_panel == ActiveWorkspacePanel::Community).await);

    // (5) AccountPopover (#account-popover-community) & (6) MainMenuDrawer (MainMenuCommand::Community)
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.sidebar.update(cx, |sb, _| sb.is_account_popover_open = true);
        s.handle_main_menu_command(MainMenuCommand::Community, cx);
        assert!(!s.sidebar.read(cx).is_account_popover_open);
    });
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.main_menu_drawer.update(cx, |d, _| d.open());
        s.handle_main_menu_command(MainMenuCommand::Community, cx);
        assert!(!s.main_menu_drawer.read(cx).is_open);
    });
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    // (7) Cmd+Shift+C toggles Community <-> Chats while plain Cmd+C stays on Chats
    host.key(&mut app, "cmd-shift-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats);
    host.key(&mut app, "cmd-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats);
    host.key(&mut app, "cmd-shift-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    // Restore expanded sidebar for 3-breakpoint matrix
    let _ = shell.update(&mut app, |s, cx| s.sidebar.update(cx, |sb, _| sb.set_collapsed(false)));
    let cw = host.cw(&app);
    assert_eq!(cw.read_with(&app, |w, _| w.state.runtime_mode).unwrap(), CommunityRuntimeMode::Online);

    // 5. Render ALL 9 Community Views + ComposerModal across 3 Viewports under LatencyBudget
    let mut budget = LatencyBudget::new(4_000.0, 25_000.0);
    let routes: Vec<(&str, CommunityRoute)> = vec![
        ("FeedView", CommunityRoute::Feed),
        ("PostDetailView", CommunityRoute::PostDetail(seed_pid.clone())),
        ("CommunityPageView", CommunityRoute::CommunityPage("t4_10_matrix".into())),
        ("ActorProfileView", CommunityRoute::ActorProfile(alice_bot.handle.clone())),
        ("ExploreSearchView", CommunityRoute::Explore { initial_query: Some("rust".into()) }),
        ("NotificationsView", CommunityRoute::Notifications),
        ("BookmarksListsView", CommunityRoute::BookmarksLists),
        ("ModQueueView", CommunityRoute::ModQueue(Some("t4_10_matrix".into()))),
        ("BotStudioView", CommunityRoute::BotStudio),
    ];
    for (label, route) in routes {
        let t_step = Instant::now();
        let _ = cw.update(&mut app, |w, cx| w.navigate_to(route, cx));
        for (vw, vh, exp_mode) in [
            (1440.0, 900.0, CommunityLayoutMode::ThreeColumn),
            (1040.0, 800.0, CommunityLayoutMode::TwoColumn),
            (820.0, 720.0, CommunityLayoutMode::OneColumn),
        ] {
            host.draw_at(&mut app, vw, vh);
            assert_eq!(cw.read_with(&app, |w, _| w.last_layout_mode).unwrap(), exp_mode, "{label} @ {vw}x{vh}");
        }
        budget.record(label, t_step.elapsed().as_secs_f64() * 1000.0);
    }

    // ComposerModal across all 3 breakpoints
    let t_comp = Instant::now();
    let _ = cw.update(&mut app, |w, cx| w.open_composer_modal(Some("t4_10_matrix".into()), cx));
    for (vw, vh) in [(1440.0, 900.0), (1040.0, 800.0), (820.0, 720.0)] {
        host.draw_at(&mut app, vw, vh);
    }
    let _ = cw.update(&mut app, |w, cx| w.close_composer_modal(cx));
    budget.record("ComposerModal", t_comp.elapsed().as_secs_f64() * 1000.0);

    // 4 Themes (Cmd+D) & 2 Locales (Cmd+L) + Non-Blocking GPUI Tick Audit
    let mut seen_themes = HashSet::new();
    for _ in 0..4 {
        seen_themes.insert(format!("{:?}", cw.read_with(&app, |w, _| w.palette.mode).unwrap()));
        host.draw_at(&mut app, 1440.0, 900.0);
        host.key(&mut app, "cmd-d");
    }
    assert_eq!(seen_themes.len(), 4, "Cmd+D must cycle all 4 desktop themes");
    let l1 = cw.read_with(&app, |w, _| w.language_id.clone()).unwrap();
    host.key(&mut app, "cmd-l");
    host.draw_at(&mut app, 1440.0, 900.0);
    let l2 = cw.read_with(&app, |w, _| w.language_id.clone()).unwrap();
    assert_ne!(l1, l2, "Cmd+L must switch bilingual locale");

    let _ = cw.update(&mut app, |w, cx| { w.navigate_to(CommunityRoute::Feed, cx); w.load_feed(cx); });
    let (reloaded, ticks, max_gap_ms) = pump_with_ticks(&mut app, Duration::from_secs(6), Duration::from_millis(15), |cx, _| {
        !cw.read(cx).state.feed_loading
    }).await;
    assert!(reloaded && ticks >= 1, "live feed reload must settle cleanly");
    assert!(max_gap_ms < 250, "GPUI main thread must stay non-blocking during live reload (max gap {max_gap_ms}ms)");
    budget.assert_within_budget().expect("T4-10 UX matrix must stay within LatencyBudget");

    // 6. Clean Logout Teardown (clears lazy community_workspace, active_panel & unread badges)
    let _ = shell.update(&mut app, |s, cx| {
        s.set_community_unread_badge(7, cx);
        s.logout(cx);
    });
    shell.read_with(&app, |s, cx| {
        assert!(!s.is_authenticated);
        assert!(s.community_workspace.is_none(), "logout must tear down community_workspace");
        assert!(s.community_pending_share.is_none());
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.apple_sidebar.read(cx).community_unread_count, 0);
        assert_eq!(s.nav_rail.read(cx).items.iter().find(|i| i.destination == NavDestination::Community).unwrap().unread_badge, 0);
    }).unwrap();

    drop(cw);
    drop(shell);
    drop(host);
    drop(driver);
    rt.block_on(server.stop());
}
