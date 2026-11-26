//! Tier 3 Opaque-Box E2E Suite — Cross-Feature Pairwise Interactions (`30` Tests).
//! Implements `TEST_INFRA.md §7` (`TEST-CM-T3-01` .. `TEST-CM-T3-30`).

#[allow(dead_code)]
mod community_e2e_helpers;

use std::collections::HashSet;
use std::time::Duration;

use community_e2e_helpers::{
    CommunityAuthMode, CommunityHttpDriver, Legacy404Server, TestTelegramClient,
    WebhookCaptureServer, closed_local_url, define_community_gpui_harness,
    execute_bot_hermes_tool, now_ms, open_offline_community_host,
};
use e2e_suite::common::{EphemeralTestServer, connect_ws, recv_ws_frame_matching};
use e2e_suite::gpui_browser_harness::pump_until;
use gpui::AsyncApp;
use gpui_telegram_agent::ApprovalMode;
use gpui_telegram_agent::hermes::toolsets::ToolsetPreset;
use gpui_telegram_client::{ClientDomainEvent, CommunityRuntimeMode};
use gpui_telegram_community::{
    CommunityRecommender, PostCandidate, ViewerContext, build_starter_dataset,
    default_prefs_for_preset, extract_entities,
};
use gpui_telegram_desktop::views::apple_sidebar::AppleSidebarSection;
use gpui_telegram_desktop::views::chat_sidebar::{DialogDeliveryStatus, DialogItemModel};
use gpui_telegram_desktop::views::community::bookmarks_lists_view::compute_bookmark_folders;
use gpui_telegram_desktop::views::community::bot_studio_view::resolve_grok_avatar_pair;
use gpui_telegram_desktop::views::community::{
    ActiveCommunityInput, BookmarksListsSection, CommunityLayoutMode, CommunityModal,
    CommunityPopover, CommunityRoute, CommunityWorkspaceAction, ComposerFormatTab, ModConsoleTab,
};
use gpui_telegram_desktop::views::modal_boxes_suite::ModalDialogKind;
use gpui_telegram_desktop::views::nav_rail::NavDestination;
use gpui_telegram_desktop::views::shell::ActiveWorkspacePanel;
use gpui_telegram_desktop::views::sidebar::gemini_unified_sidebar::UnifiedSidebarAction;
use gpui_telegram_proto::community_contracts::{
    ActorFilter, ActorKind, ActorProfileResponse, AlgoPreset, AutoModActionKind, BotAutonomyMode,
    CommentsListResponse, CommunityDetailResponse, CommunityFlairDto, CommunityPostDto,
    CommunitySearchResultDto, CreatePostRequest, CreatePostResponse, FeedRequestDto,
    FeedResponseDto, FeedSort, FeedTab, ModActionKind, ModLogsResponse, ModQueueResponse, PollDto,
    PostDetailResponse, PostFormat, PostStatus, SummonBotRequest, TrendingItemKind,
    TrendingResponse, UpdateCommunityRequest,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use reqwest::Method;
use serde_json::json;

define_community_gpui_harness! {
    child_env: "CM_T3_CHILD_ENV",
    target_env: "CM_T3_CHILD_TARGET_ENV",
    after_ctors_env: "CM_T3_CHILD_AFTER_CTORS_ENV",
    targets: {
        "test_cm_t3_21_f15_f16_left_sidebar_modes_shortcut_unread_badge_and_3col_render" => body_t3_21,
        "test_cm_t3_22_f15_f18_centralized_cjk_ime_and_rich_composer_modal" => body_t3_22,
        "test_cm_t3_23_f15_f20_actor_profile_dm_bridge_and_post_card_share_to_chat" => body_t3_23,
        "test_cm_t3_24_f15_f16_post_card_media_lightbox_theme_locale_and_escape_stay" => body_t3_24,
        "test_cm_t3_25_f16_f17_identity_switcher_persistence_feed_to_detail_reply_and_summon" => body_t3_25,
        "test_cm_t3_26_f16_f21_right_rail_trending_click_to_explore_filtered_results" => body_t3_26,
        "test_cm_t3_27_f18_f19_create_community_flair_setup_composer_and_flair_filter" => body_t3_27,
        "test_cm_t3_28_f19_f21_automod_rule_mod_queue_and_public_mod_log_roundtrip" => body_t3_28,
        "test_cm_t3_29_f20_f16_custom_list_creation_feed_tab_and_bookmark_folder" => body_t3_29,
        "test_cm_t3_30_f21_f16_bot_studio_creation_act_as_switcher_and_live_test_post" => body_t3_30,
    }
}

// ============================================================================
// Pairwise Core, Auth, Governance & Content Interactions (T3-01 .. T3-07)
// ============================================================================

/// [Test ID] TEST-CM-T3-01
/// [Feature / Req] F02 x F11 / ORIGINAL_REQUEST.md (Human + Bot equal citizenship across all 3 REST auth modes)
/// [Preconditions] Ephemeral live server with Human Alice (`Bearer`), owned BotA (`X-Community-Actor-Id`), and owned BotB (`Authorization: Bot`).
/// [Opaque-Box Steps] Co-author posts, nested comments, cross-actor upvotes, and poll votes in the same thread across all 3 authentication modes.
/// [Observable Assertions] Every citizen writes with identical API affordances, poll buckets satisfy `all == human + bot`, and all 3 actors accrue accurate dual karma.
#[tokio::test]
async fn test_cm_t3_01_f02_f11_human_bot_equal_citizenship_three_auth_modes() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_01", "Pass1234!").await.unwrap();
    let bot_a = driver.register_community_bot(&alice.token, "bota_t3_01_bot", "Bot A", "manual").await.unwrap();
    let bot_b = driver.register_community_bot(&alice.token, "botb_t3_01_bot", "Bot B", "manual").await.unwrap();

    let h_auth = CommunityAuthMode::Human(alice.token.clone());
    let act_as_a = CommunityAuthMode::ActAsBot {
        owner_token: alice.token.clone(),
        actor_id: bot_a.actor_id.clone(),
    };
    let direct_b = CommunityAuthMode::DirectBot(bot_b.bot_token.clone());

    assert_eq!(
        driver
            .req(
                Method::POST,
                "/api/v1/community/communities",
                &h_auth,
                Some(json!({ "slug": "citizenship_t3_01", "name": "Citizenship Lab", "allow_bots": true })),
            )
            .await
            .unwrap()
            .0,
        201
    );

    // 1. Human creates root poll post
    let (st, pv) = driver
        .req(
            Method::POST,
            "/api/v1/community/posts",
            &h_auth,
            Some(json!({
                "community_slug": "citizenship_t3_01",
                "format": "poll",
                "title": "Human + Bot First-Class Poll",
                "body": "All three auth modes participate equally.",
                "poll": { "question": "Preferred consensus?", "options": ["Raft", "Paxos"], "multi_select": false, "duration_secs": 3600 }
            })),
        )
        .await
        .unwrap();
    assert_eq!(st, 201, "human poll create failed: {pv}");
    let root_pid = pv["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(pv["post"]["author_actor_id"], alice.actor_id);
    assert_eq!(pv["post"]["author_kind"], "human");

    // 2. Act-As-Bot creates a comment & upvotes Human's post & votes in poll
    let (st, c1) = driver
        .req(
            Method::POST,
            &format!("/api/v1/community/posts/{root_pid}/comments"),
            &act_as_a,
            Some(json!({ "body": "BotA analysis via X-Community-Actor-Id" })),
        )
        .await
        .unwrap();
    assert_eq!(st, 201, "act-as comment failed: {c1}");
    let c1_id = c1["comment"]["comment_id"].as_str().unwrap().to_string();
    assert_eq!(c1["comment"]["author_actor_id"], bot_a.actor_id);
    assert_eq!(c1["comment"]["author_kind"], "bot");
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{root_pid}/vote"), &act_as_a, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{root_pid}/poll-vote"), &act_as_a, Some(json!({ "option_ids": ["opt_0"] }))).await.unwrap().0, 200);

    // 3. Direct Bot Token replies to BotA, upvotes post & BotA comment, votes in poll, and authors its own post
    let (st, c2) = driver
        .req(
            Method::POST,
            &format!("/api/v1/community/posts/{root_pid}/comments"),
            &direct_b,
            Some(json!({ "parent_comment_id": c1_id, "body": "BotB reply via Authorization: Bot" })),
        )
        .await
        .unwrap();
    assert_eq!(st, 201, "direct bot comment failed: {c2}");
    let c2_id = c2["comment"]["comment_id"].as_str().unwrap().to_string();
    assert_eq!(c2["comment"]["author_actor_id"], bot_b.actor_id);
    assert_eq!(c2["comment"]["author_kind"], "bot");
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{root_pid}/vote"), &direct_b, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/comments/{c1_id}/vote"), &direct_b, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{root_pid}/poll-vote"), &direct_b, Some(json!({ "option_ids": ["opt_1"] }))).await.unwrap().0, 200);

    let (st, bp) = driver
        .req(
            Method::POST,
            "/api/v1/community/posts",
            &direct_b,
            Some(json!({ "community_slug": "citizenship_t3_01", "body": "BotB standalone post" })),
        )
        .await
        .unwrap();
    assert_eq!(st, 201);
    let bot_pid = bp["post"]["post_id"].as_str().unwrap().to_string();

    // 4. Human votes in poll, upvotes BotB post & comment, and Act-As-Bot upvotes BotB post
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{root_pid}/poll-vote"), &h_auth, Some(json!({ "option_ids": ["opt_0"] }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{bot_pid}/vote"), &h_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/comments/{c2_id}/vote"), &h_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{bot_pid}/vote"), &act_as_a, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);

    // Verify poll breakdown & dual karma across all 3 citizens
    let (st, det_v) = driver.req(Method::GET, &format!("/api/v1/community/posts/{root_pid}"), &h_auth, None).await.unwrap();
    assert_eq!(st, 200);
    let det: PostDetailResponse = serde_json::from_value(det_v).unwrap();
    let poll = det.post.poll.expect("poll must exist");
    assert_eq!(poll.total_votes, 3);
    assert_eq!(poll.total_human_votes, 1);
    assert_eq!(poll.total_bot_votes, 2);
    for opt in &poll.options {
        assert_eq!(opt.all_votes, opt.human_votes + opt.bot_votes);
    }
    assert_eq!(det.post.score, 3);

    let a_prof: ActorProfileResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/actors/{}", alice.actor_id), &h_auth, None).await.unwrap().1).unwrap();
    let ba_prof: ActorProfileResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/actors/{}", bot_a.actor_id), &h_auth, None).await.unwrap().1).unwrap();
    let bb_prof: ActorProfileResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/actors/{}", bot_b.actor_id), &h_auth, None).await.unwrap().1).unwrap();
    assert_eq!(a_prof.actor.post_karma, 2, "Alice earned +2 post_karma from BotA & BotB");
    assert_eq!(ba_prof.actor.comment_karma, 1, "BotA earned +1 comment_karma from BotB");
    assert_eq!(bb_prof.actor.post_karma, 2, "BotB earned +2 post_karma from Alice & BotA");
    assert_eq!(bb_prof.actor.comment_karma, 1, "BotB earned +1 comment_karma from Alice");
}

/// [Test ID] TEST-CM-T3-02
/// [Feature / Req] F02 x F11 / ORIGINAL_REQUEST.md (Act-As ownership security boundary across posts, comments, votes & mod actions)
/// [Preconditions] Ephemeral live server with Alice (owner of BotA, community owner) and Bob (owner of BotB).
/// [Opaque-Box Steps] Alice acts as BotA (allowed); Alice attempts Act-As BotB, Bob attempts Act-As BotA & u:alice, and BotB attempts cross-actor header override across post/comment/vote/mod endpoints.
/// [Observable Assertions] Legitimate owner Act-As returns 200/201; every cross-owner spoof returns 403 Forbidden with zero unauthorized writes persisted.
#[tokio::test]
async fn test_cm_t3_02_f02_f11_act_as_ownership_security_boundary() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_02", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_02", "Pass1234!").await.unwrap();
    let bot_a = driver.register_community_bot(&alice.token, "bota_t3_02_bot", "Bot A", "manual").await.unwrap();
    let bot_b = driver.register_community_bot(&bob.token, "botb_t3_02_bot", "Bot B", "manual").await.unwrap();

    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let alice_as_a = CommunityAuthMode::ActAsBot { owner_token: alice.token.clone(), actor_id: bot_a.actor_id.clone() };
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "sec_t3_02", "name": "Sec Lab", "allow_bots": true }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/sec_t3_02/mod-actions", &a_auth, Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": bot_a.actor_id }))).await.unwrap().0, 200);

    // Owner Alice acting as BotA succeeds across post, comment, vote, and mod action
    let (st, pv) = driver.req(Method::POST, "/api/v1/community/posts", &alice_as_a, Some(json!({ "community_slug": "sec_t3_02", "body": "Legit BotA post" }))).await.unwrap();
    assert_eq!(st, 201, "owner Act-As post failed: {pv}");
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &alice_as_a, Some(json!({ "body": "Legit BotA comment" }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/vote"), &alice_as_a, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/sec_t3_02/mod-actions", &alice_as_a, Some(json!({ "action": "pin", "target_kind": "post", "target_id": pid }))).await.unwrap().0, 200);

    // Unauthorized cross-owner Act-As attempts MUST all return 403 Forbidden
    let forbidden_auths = [
        CommunityAuthMode::ActAsBot { owner_token: alice.token.clone(), actor_id: bot_b.actor_id.clone() },
        CommunityAuthMode::ActAsBot { owner_token: bob.token.clone(), actor_id: bot_a.actor_id.clone() },
        CommunityAuthMode::ActAsBot { owner_token: bob.token.clone(), actor_id: alice.actor_id.clone() },
        CommunityAuthMode::DirectBotWithActorHeader { bot_token: bot_b.bot_token.clone(), actor_id: bot_a.actor_id.clone() },
    ];
    for spoof in &forbidden_auths {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", spoof, Some(json!({ "community_slug": "sec_t3_02", "body": "Spoof post" }))).await.unwrap().0, 403);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), spoof, Some(json!({ "body": "Spoof comment" }))).await.unwrap().0, 403);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/vote"), spoof, Some(json!({ "direction": -1 }))).await.unwrap().0, 403);
        assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/sec_t3_02/mod-actions", spoof, Some(json!({ "action": "lock", "target_kind": "post", "target_id": pid }))).await.unwrap().0, 403);
    }

    // Verify zero unauthorized state mutations occurred
    let det: PostDetailResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &a_auth, None).await.unwrap().1).unwrap();
    assert_eq!(det.total_comments, 1, "no spoofed comments may persist");
    assert_eq!(det.post.score, 1, "no spoofed downvotes may persist");
    assert!(!det.post.is_locked, "no spoofed lock may persist");
    let logs: ModLogsResponse = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/communities/sec_t3_02/mod-logs", &a_auth, None).await.unwrap().1).unwrap();
    assert!(!logs.logs.iter().any(|l| l.action == ModActionKind::Lock), "no spoofed mod log entry may exist");
}

/// [Test ID] TEST-CM-T3-03
/// [Feature / Req] F03 x F02 / ORIGINAL_REQUEST.md (Community `allow_bots` & `bot_mod` governance toggle)
/// [Preconditions] Ephemeral live server with Owner Alice, Human Bob, and Bot promoted to `bot_mod` in `c/bots_ok_t3`.
/// [Opaque-Box Steps] Verify Bot posts & moderates in `c/bots_ok_t3`; verify `c/humans_only_t3` (`allow_bots=false`) allows Bob but blocks Bot (`403`); toggle `allow_bots` true then false.
/// [Observable Assertions] `bot_mod` moderates cleanly; `allow_bots=false` strictly returns 403 to Bots on posts & comments while toggling takes immediate effect.
#[tokio::test]
async fn test_cm_t3_03_f03_f02_community_allow_bots_and_bot_mod_governance() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_03", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_03", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "gov_t3_03_bot", "Gov Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "bots_ok_t3", "name": "Bots OK", "allow_bots": true }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "humans_only_t3", "name": "Humans Only", "allow_bots": false }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/bots_ok_t3/mod-actions", &a_auth, Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": bot.actor_id }))).await.unwrap().0, 200);

    // c/bots_ok_t3 permits Bot posts and bot_mod actions
    assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &bot_auth, Some(json!({ "community_slug": "bots_ok_t3", "body": "Bot post allowed" }))).await.unwrap().0, 201);
    let bob_ok_pid = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "bots_ok_t3", "body": "Bob post for bot_mod" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/bots_ok_t3/mod-actions", &bot_auth, Some(json!({ "action": "pin", "target_kind": "post", "target_id": bob_ok_pid }))).await.unwrap().0, 200);
    let comm_det: CommunityDetailResponse = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/communities/bots_ok_t3", &a_auth, None).await.unwrap().1).unwrap();
    assert!(comm_det.moderators.iter().any(|m| m.actor_id == bot.actor_id), "bot_mod must appear in community moderators");

    // c/humans_only_t3 permits Human Bob, blocks Bot posts & comments with 403
    let (st_hb, hb_v) = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "humans_only_t3", "body": "Human post in humans-only" }))).await.unwrap();
    assert_eq!(st_hb, 201);
    let hb_pid = hb_v["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &bot_auth, Some(json!({ "community_slug": "humans_only_t3", "body": "Bot blocked" }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{hb_pid}/comments"), &bot_auth, Some(json!({ "body": "Bot comment blocked" }))).await.unwrap().0, 403);

    // Toggle allow_bots -> true takes immediate effect
    assert_eq!(driver.req(Method::PATCH, "/api/v1/community/communities/humans_only_t3", &a_auth, Some(json!({ "allow_bots": true }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &bot_auth, Some(json!({ "community_slug": "humans_only_t3", "body": "Bot now allowed!" }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{hb_pid}/comments"), &bot_auth, Some(json!({ "body": "Bot comment now allowed!" }))).await.unwrap().0, 201);

    // Toggle allow_bots -> false immediately blocks Bot again
    assert_eq!(driver.req(Method::PATCH, "/api/v1/community/communities/humans_only_t3", &a_auth, Some(json!({ "allow_bots": false }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &bot_auth, Some(json!({ "community_slug": "humans_only_t3", "body": "Blocked again" }))).await.unwrap().0, 403);
}

/// [Test ID] TEST-CM-T3-04
/// [Feature / Req] F04 x F02 / ORIGINAL_REQUEST.md (Single & multi-choice Poll Human vs. Bot breakdown & atomic vote change)
/// [Preconditions] Ephemeral live server with 3 Humans (Alice, Bob, Carol) and 2 Bots (Bot1, Bot2).
/// [Opaque-Box Steps] Accumulate votes from all 5 citizens on both single-choice and multi-choice polls, then change votes for 1 Human and 1 Bot.
/// [Observable Assertions] `all_votes == human_votes + bot_votes` holds on every option before and after atomic vote transitions with exact bucket totals.
#[tokio::test]
async fn test_cm_t3_04_f04_f02_poll_human_vs_bot_breakdown_atomic_updates() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_04", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_04", "Pass1234!").await.unwrap();
    let carol = driver.register_human("carol_t3_04", "Pass1234!").await.unwrap();
    let bot1 = driver.register_community_bot(&alice.token, "b1_t3_04_bot", "Poll Bot 1", "manual").await.unwrap();
    let bot2 = driver.register_community_bot(&bob.token, "b2_t3_04_bot", "Poll Bot 2", "manual").await.unwrap();

    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let c_auth = CommunityAuthMode::Human(carol.token.clone());
    let bot1_auth = CommunityAuthMode::DirectBot(bot1.bot_token.clone());
    let bot2_auth = CommunityAuthMode::DirectBot(bot2.bot_token.clone());

    // Single-choice poll
    let (st, spv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
        "format": "poll",
        "body": "Single choice breakdown",
        "poll": { "question": "Systems language?", "options": ["Rust", "Go", "Zig"], "multi_select": false, "duration_secs": 3600 }
    }))).await.unwrap();
    assert_eq!(st, 201);
    let spid = spv["post"]["post_id"].as_str().unwrap().to_string();

    for (auth, opts) in [
        (&a_auth, vec!["opt_0"]),
        (&b_auth, vec!["opt_0"]),
        (&c_auth, vec!["opt_1"]),
        (&bot1_auth, vec!["opt_0"]),
        (&bot2_auth, vec!["opt_2"]),
    ] {
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{spid}/poll-vote"), auth, Some(json!({ "option_ids": opts }))).await.unwrap().0, 200);
    }
    let p1: PollDto = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/posts/{spid}"), &a_auth, None).await.unwrap().1["post"]["poll"].clone()).unwrap();
    assert_eq!((p1.total_votes, p1.total_human_votes, p1.total_bot_votes), (5, 3, 2));
    assert_eq!((p1.options[0].all_votes, p1.options[0].human_votes, p1.options[0].bot_votes), (3, 2, 1));
    assert_eq!((p1.options[1].all_votes, p1.options[1].human_votes, p1.options[1].bot_votes), (1, 1, 0));
    assert_eq!((p1.options[2].all_votes, p1.options[2].human_votes, p1.options[2].bot_votes), (1, 0, 1));

    // Change Bob opt_0 -> opt_1 and Bot1 opt_0 -> opt_2 atomically
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{spid}/poll-vote"), &b_auth, Some(json!({ "option_ids": ["opt_1"] }))).await.unwrap().0, 200);
    let (st, pv_after) = driver.req(Method::POST, &format!("/api/v1/community/posts/{spid}/poll-vote"), &bot1_auth, Some(json!({ "option_ids": ["opt_2"] }))).await.unwrap();
    assert_eq!(st, 200);
    let p1_after: PollDto = serde_json::from_value(pv_after).unwrap();
    assert_eq!((p1_after.total_votes, p1_after.total_human_votes, p1_after.total_bot_votes), (5, 3, 2));
    assert_eq!((p1_after.options[0].all_votes, p1_after.options[0].human_votes, p1_after.options[0].bot_votes), (1, 1, 0));
    assert_eq!((p1_after.options[1].all_votes, p1_after.options[1].human_votes, p1_after.options[1].bot_votes), (2, 2, 0));
    assert_eq!((p1_after.options[2].all_votes, p1_after.options[2].human_votes, p1_after.options[2].bot_votes), (2, 0, 2));
    for opt in &p1_after.options {
        assert_eq!(opt.all_votes, opt.human_votes + opt.bot_votes);
    }

    // Multi-choice poll
    let (st, mpv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
        "format": "poll",
        "body": "Multi choice breakdown",
        "poll": { "question": "Core crates?", "options": ["GPUI", "Tantivy", "SQLite"], "multi_select": true, "duration_secs": 3600 }
    }))).await.unwrap();
    assert_eq!(st, 201);
    let mpid = mpv["post"]["post_id"].as_str().unwrap().to_string();
    for (auth, opts) in [
        (&a_auth, vec!["opt_0", "opt_1"]),
        (&b_auth, vec!["opt_0", "opt_2"]),
        (&c_auth, vec!["opt_1"]),
        (&bot1_auth, vec!["opt_0", "opt_1", "opt_2"]),
        (&bot2_auth, vec!["opt_2"]),
    ] {
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{mpid}/poll-vote"), auth, Some(json!({ "option_ids": opts }))).await.unwrap().0, 200);
    }
    let (st, mv_after) = driver.req(Method::POST, &format!("/api/v1/community/posts/{mpid}/poll-vote"), &bot1_auth, Some(json!({ "option_ids": ["opt_0"] }))).await.unwrap();
    assert_eq!(st, 200);
    let mp: PollDto = serde_json::from_value(mv_after).unwrap();
    assert_eq!((mp.options[0].all_votes, mp.options[0].human_votes, mp.options[0].bot_votes), (3, 2, 1));
    assert_eq!((mp.options[1].all_votes, mp.options[1].human_votes, mp.options[1].bot_votes), (2, 2, 0));
    assert_eq!((mp.options[2].all_votes, mp.options[2].human_votes, mp.options[2].bot_votes), (2, 1, 1));
    for opt in &mp.options {
        assert_eq!(opt.all_votes, opt.human_votes + opt.bot_votes);
    }
}

/// [Test ID] TEST-CM-T3-05
/// [Feature / Req] F04 x F05 / ORIGINAL_REQUEST.md (Multi-post X Thread + Nested Reddit comment forest + Quote chain)
/// [Preconditions] Ephemeral live server with Humans Alice & Bob and Bot citizen.
/// [Opaque-Box Steps] Alice publishes a 3-part X Thread (1/3..3/3); Humans & Bot attach a 4-level nested Reddit comment forest to 2/3; Bob quotes 3/3.
/// [Observable Assertions] Linear `thread_chain` contains all 3 ordered parts, nested tree on 2/3 preserves depths 0..3 with consistent counts, and Quote links 3/3 cleanly.
#[tokio::test]
async fn test_cm_t3_05_f04_f05_multi_post_x_thread_nested_reddit_replies_and_quote() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_05", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_05", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "thread_t3_05_bot", "Thread Bot", "manual").await.unwrap();

    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "t3_05_threads", "name": "T3-05 Threads", "allow_bots": true }))).await.unwrap().0, 201);

    let (st, tv) = driver
        .req(
            Method::POST,
            "/api/v1/community/posts",
            &a_auth,
            Some(json!({
                "community_slug": "t3_05_threads",
                "format": "micro",
                "body": "1/3 Thread root: Fused Reddit+X architecture",
                "thread_parts": [
                    { "body": "2/3 Thread middle: Nested comment forest anchor", "media": [] },
                    { "body": "3/3 Thread finale: Quote target summary", "media": [] }
                ]
            })),
        )
        .await
        .unwrap();
    assert_eq!(st, 201, "create 3-part thread failed: {tv}");
    let created: CreatePostResponse = serde_json::from_value(tv).unwrap();
    assert_eq!(created.thread_posts.len(), 2, "thread_posts holds follow-up parts 2/3 and 3/3");
    let p1 = created.post.post_id.clone();
    let p2 = created.thread_posts[0].post_id.clone();
    let p3 = created.thread_posts[1].post_id.clone();
    assert_eq!((created.post.thread_index, created.post.thread_total), (0, 3));
    assert_eq!((created.thread_posts[0].thread_index, created.thread_posts[0].thread_total), (1, 3));
    assert_eq!((created.thread_posts[1].thread_index, created.thread_posts[1].thread_total), (2, 3));

    // Attach 4-level nested Reddit comment subtree to 2/3
    let c0 = driver.req(Method::POST, &format!("/api/v1/community/posts/{p2}/comments"), &b_auth, Some(json!({ "body": "Depth 0 Human reply" }))).await.unwrap().1["comment"]["comment_id"].as_str().unwrap().to_string();
    let c1 = driver.req(Method::POST, &format!("/api/v1/community/posts/{p2}/comments"), &bot_auth, Some(json!({ "parent_comment_id": c0, "body": "Depth 1 Bot reply" }))).await.unwrap().1["comment"]["comment_id"].as_str().unwrap().to_string();
    let c2 = driver.req(Method::POST, &format!("/api/v1/community/posts/{p2}/comments"), &a_auth, Some(json!({ "parent_comment_id": c1, "body": "Depth 2 OP reply" }))).await.unwrap().1["comment"]["comment_id"].as_str().unwrap().to_string();
    let c3 = driver.req(Method::POST, &format!("/api/v1/community/posts/{p2}/comments"), &bot_auth, Some(json!({ "parent_comment_id": c2, "body": "Depth 3 Bot rebuttal" }))).await.unwrap().1["comment"]["comment_id"].as_str().unwrap().to_string();

    // Bob quotes 3/3
    let (st, qv) = driver
        .req(
            Method::POST,
            "/api/v1/community/posts",
            &b_auth,
            Some(json!({ "format": "quote", "quoted_post_id": p3, "body": "Quoting part 3/3 with takeaways!" })),
        )
        .await
        .unwrap();
    assert_eq!(st, 201, "quote 3/3 failed: {qv}");
    let qid = qv["post"]["post_id"].as_str().unwrap().to_string();

    let det2: PostDetailResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/posts/{p2}"), &a_auth, None).await.unwrap().1).unwrap();
    let chain_ids: Vec<&str> = det2.thread_chain.iter().map(|p| p.post_id.as_str()).collect();
    assert_eq!(chain_ids, vec![p1.as_str(), p2.as_str(), p3.as_str()]);
    assert_eq!(det2.total_comments, 4);
    assert_eq!(det2.post.comments_count, 4);
    assert_eq!(det2.comments.len(), 1);
    let n0 = &det2.comments[0];
    assert_eq!((n0.comment_id.as_str(), n0.depth), (c0.as_str(), 0));
    let n1 = &n0.children[0];
    assert_eq!((n1.comment_id.as_str(), n1.depth, n1.author_kind), (c1.as_str(), 1, ActorKind::Bot));
    let n2 = &n1.children[0];
    assert_eq!((n2.comment_id.as_str(), n2.depth, n2.is_op), (c2.as_str(), 2, true));
    let n3 = &n2.children[0];
    assert_eq!((n3.comment_id.as_str(), n3.depth, n3.author_kind), (c3.as_str(), 3, ActorKind::Bot));

    let det_q: PostDetailResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/posts/{qid}"), &a_auth, None).await.unwrap().1).unwrap();
    assert_eq!(det_q.post.quoted_post_id.as_deref(), Some(p3.as_str()));
    assert_eq!(det_q.post.quoted_post.as_ref().map(|e| e.post_id.as_str()), Some(p3.as_str()));
    assert!(det_q.quote_chain.iter().any(|p| p.post_id == p3));

    let det3: PostDetailResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/posts/{p3}"), &a_auth, None).await.unwrap().1).unwrap();
    assert!(det3.post.quotes_count >= 1);
}

/// [Test ID] TEST-CM-T3-06
/// [Feature / Req] F04 x F03 / ORIGINAL_REQUEST.md (Crosspost + Flair + Pinned lifecycle & original deletion safety)
/// [Preconditions] Ephemeral live server with Alice (owner of `c/rust_xp_t3` & `c/agents_xp_t3`) and Bob.
/// [Opaque-Box Steps] Bob publishes original post in `c/rust_xp_t3`; Alice crossposts into `c/agents_xp_t3` with flair `"DeepDive"` and pins it; Bob then deletes original post.
/// [Observable Assertions] Crosspost appears pinned and matches flair filter before & after original deletion; deleting original returns 404 on original while crosspost stays intact with stale origin preview stripped.
#[tokio::test]
async fn test_cm_t3_06_f04_f03_crosspost_flair_pinned_and_origin_delete_lifecycle() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_06", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_06", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "rust_xp_t3", "name": "Rust XP" }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({
        "slug": "agents_xp_t3",
        "name": "Agents XP",
        "post_flairs": [
            { "id": "f_dd", "text": "DeepDive", "bg_color": "#2563EB", "text_color": "#FFFFFF", "is_mod_only": false },
            { "id": "f_bn", "text": "Bench", "bg_color": "#059669", "text_color": "#FFFFFF", "is_mod_only": false }
        ]
    }))).await.unwrap().0, 201);

    let orig_id = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({
        "community_slug": "rust_xp_t3", "title": "Original Rust Post", "body": "Origin content to crosspost", "flair": "Original"
    }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

    let (st, xpv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
        "community_slug": "agents_xp_t3",
        "format": "crosspost",
        "crosspost_of_id": orig_id,
        "title": "Crossposted to Agents",
        "body": "Relevant to agent engineers!",
        "flair": "DeepDive"
    }))).await.unwrap();
    assert_eq!(st, 201, "crosspost create failed: {xpv}");
    let xp_id = xpv["post"]["post_id"].as_str().unwrap().to_string();

    // Unrelated flair post to verify flair filter discriminates
    let _other_id = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
        "community_slug": "agents_xp_t3", "body": "Benchmark post", "flair": "Bench"
    }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/agents_xp_t3/mod-actions", &a_auth, Some(json!({
        "action": "pin", "target_kind": "post", "target_id": xp_id
    }))).await.unwrap().0, 200);

    let comm_before: CommunityDetailResponse = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/communities/agents_xp_t3", &a_auth, None).await.unwrap().1).unwrap();
    assert!(comm_before.pinned_posts.iter().any(|p| p.post_id == xp_id));
    let xp_before: PostDetailResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/posts/{xp_id}"), &a_auth, None).await.unwrap().1).unwrap();
    assert_eq!(xp_before.post.crosspost_of_id.as_deref(), Some(orig_id.as_str()));
    assert!(xp_before.post.crosspost_origin.is_some());

    // Delete original post -> original returns 404, crosspost remains pinned & flair-searchable with sanitized origin
    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/posts/{orig_id}"), &b_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::GET, &format!("/api/v1/community/posts/{orig_id}"), &a_auth, None).await.unwrap().0, 404);

    let xp_after: PostDetailResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/posts/{xp_id}"), &a_auth, None).await.unwrap().1).unwrap();
    assert_eq!(xp_after.post.crosspost_of_id.as_deref(), Some(orig_id.as_str()));
    assert!(xp_after.post.crosspost_origin.is_none(), "deleted origin preview must be stripped safely");
    assert!(xp_after.post.is_pinned);

    let feed: FeedResponseDto = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/feed?tab=community&community=agents_xp_t3&flair=DeepDive", &a_auth, None).await.unwrap().1).unwrap();
    assert_eq!(feed.items.len(), 1);
    assert_eq!(feed.items[0].post.post_id, xp_id);
}

/// [Test ID] TEST-CM-T3-07
/// [Feature / Req] F05 x F08 / ORIGINAL_REQUEST.md (Nested Comment Tree + Mod Pin/Distinguish/Remove Tombstones & Post Lock)
/// [Preconditions] Ephemeral live server with Owner Alice and members Bob & Carol in `c/tree_mod_t3`.
/// [Opaque-Box Steps] Build nested comments + high-scored sibling; mod pins & distinguishes mod comment, removes mid-branch comment, deletes parent comment creating a tombstone, and locks post.
/// [Observable Assertions] Stickied distinguished comment pins at index 0 above higher score, tombstone & removed flags preserve child subtree, and subsequent replies return 403.
#[tokio::test]
async fn test_cm_t3_07_f05_f08_nested_comment_tree_mod_lock_distinguish_remove_tombstones() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_07", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_07", "Pass1234!").await.unwrap();
    let carol = driver.register_human("carol_t3_07", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let c_auth = CommunityAuthMode::Human(carol.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "tree_mod_t3", "name": "Tree Mod Lab" }))).await.unwrap().0, 201);
    let pid = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "tree_mod_t3", "body": "Debate thread for mod actions" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

    let c_high = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &b_auth, Some(json!({ "body": "Popular comment" }))).await.unwrap().1["comment"]["comment_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/comments/{c_high}/vote"), &a_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/comments/{c_high}/vote"), &c_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);

    let c_mod = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &a_auth, Some(json!({ "body": "Official moderator guidance" }))).await.unwrap().1["comment"]["comment_id"].as_str().unwrap().to_string();
    let c_branch = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &b_auth, Some(json!({ "body": "Parent branch to tombstone" }))).await.unwrap().1["comment"]["comment_id"].as_str().unwrap().to_string();
    let c_child = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &c_auth, Some(json!({ "parent_comment_id": c_branch, "body": "Child comment to mod-remove" }))).await.unwrap().1["comment"]["comment_id"].as_str().unwrap().to_string();
    let c_grand = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &b_auth, Some(json!({ "parent_comment_id": c_child, "body": "Grandchild surviving under tombstone" }))).await.unwrap().1["comment"]["comment_id"].as_str().unwrap().to_string();

    // Mod pins & distinguishes c_mod, removes c_child, deletes c_branch (tombstone), and locks post
    for payload in [
        json!({ "action": "pin", "target_kind": "comment", "target_id": c_mod }),
        json!({ "action": "distinguish", "target_kind": "comment", "target_id": c_mod }),
        json!({ "action": "remove", "target_kind": "comment", "target_id": c_child, "reason": "rule violation" }),
        json!({ "action": "lock", "target_kind": "post", "target_id": pid, "reason": "thread concluded" }),
    ] {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/tree_mod_t3/mod-actions", &a_auth, Some(payload)).await.unwrap().0, 200);
    }
    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/comments/{c_branch}"), &a_auth, None).await.unwrap().0, 200);

    let list: CommentsListResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}/comments?sort=top"), &b_auth, None).await.unwrap().1).unwrap();
    assert_eq!(list.comments[0].comment_id, c_mod, "stickied mod comment must pin above higher-scored sibling");
    assert!(list.comments[0].is_stickied && list.comments[0].is_mod_distinguished);

    let tomb = list.comments.iter().find(|c| c.comment_id == c_branch).expect("tombstoned parent must remain in tree");
    assert!(tomb.is_deleted && tomb.body == "[deleted]");
    let rem_child = tomb.children.iter().find(|c| c.comment_id == c_child).expect("removed child must stay attached");
    assert!(rem_child.is_removed);
    assert!(rem_child.children.iter().any(|c| c.comment_id == c_grand && !c.is_deleted && !c.is_removed), "grandchild must survive under tombstones");

    // Locked post strictly rejects further replies with 403
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &b_auth, Some(json!({ "body": "Reply on locked post" }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &c_auth, Some(json!({ "parent_comment_id": c_grand, "body": "Nested reply on locked post" }))).await.unwrap().0, 403);
}

// ============================================================================
// Pairwise Recommender, Trending, Search, AutoMod & Media Seams (T3-08 .. T3-14)
// ============================================================================

/// [Test ID] TEST-CM-T3-08
/// [Feature / Req] F06 x F08 / ORIGINAL_REQUEST.md (AutoMod / User Mute-Block + 5-Stage Recommender negative gating)
/// [Preconditions] Ephemeral live server with Viewer Alice, Posters Bob/Carol/Dave, and communities `c/am_gate_t3`, `c/muted_comm_t3`, `c/clean_comm_t3`.
/// [Opaque-Box Steps] Create AutoMod-removed, blocked-actor, muted-community, muted-keyword, and clean posts; apply Alice's blocks/mutes; query `ForYou`, `Popular`, `Hot`, and `Rising`.
/// [Observable Assertions] All 4 negative-gated posts are strictly excluded across every feed variant while the clean post remains visible.
#[tokio::test]
async fn test_cm_t3_08_f06_f08_automod_mute_block_negative_gating_across_feeds() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_08", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_08", "Pass1234!").await.unwrap();
    let carol = driver.register_human("carol_t3_08", "Pass1234!").await.unwrap();
    let dave = driver.register_human("dave_t3_08", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let c_auth = CommunityAuthMode::Human(carol.token.clone());
    let d_auth = CommunityAuthMode::Human(dave.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({
        "slug": "am_gate_t3", "name": "AM Gate",
        "automod_rules": [{
            "rule_id": "am_t308", "community_id": "", "name": "Block Bad Phrase", "enabled": true, "priority": 1,
            "keywords": ["forbidden_am_phrase_t308"], "regex_pattern": null, "min_author_karma": null,
            "min_account_age_secs": null, "max_links": null, "block_duplicate_spam": false,
            "apply_to_bots": true, "apply_to_humans": true, "action": "remove",
            "assign_flair": null, "auto_reply_template": null, "created_at_ms": now_ms()
        }]
    }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "muted_comm_t3", "name": "Muted Comm" }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "clean_comm_t3", "name": "Clean Comm" }))).await.unwrap().0, 201);

    let (st_am, p_am_v) = driver.req(Method::POST, "/api/v1/community/posts", &c_auth, Some(json!({
        "community_slug": "am_gate_t3", "body": "Contains forbidden_am_phrase_t308 spam"
    }))).await.unwrap();
    assert_eq!(st_am, 201);
    assert_eq!(p_am_v["automod_action"], "remove");
    let p_am = p_am_v["post"]["post_id"].as_str().unwrap().to_string();
    let p_blocked = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "clean_comm_t3", "body": "Post from blocked Bob" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    let p_muted_c = driver.req(Method::POST, "/api/v1/community/posts", &d_auth, Some(json!({ "community_slug": "muted_comm_t3", "body": "Post in muted community" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    let p_muted_k = driver.req(Method::POST, "/api/v1/community/posts", &d_auth, Some(json!({ "community_slug": "clean_comm_t3", "body": "Alert containing secret_mute_kw_t308" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    let p_clean = driver.req(Method::POST, "/api/v1/community/posts", &c_auth, Some(json!({ "community_slug": "clean_comm_t3", "body": "Clean Rust systems engineering post T3-08" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

    for rule in [
        json!({ "target_kind": "actor", "target_value": bob.actor_id, "action": "block" }),
        json!({ "target_kind": "community", "target_value": "muted_comm_t3", "action": "mute" }),
        json!({ "target_kind": "keyword", "target_value": "secret_mute_kw_t308", "action": "mute" }),
    ] {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/mutes-blocks", &a_auth, Some(rule)).await.unwrap().0, 200);
    }

    for path in [
        "/api/v1/community/feed?tab=for_you&sort=algo&limit=100",
        "/api/v1/community/feed?tab=popular&sort=hot&limit=100",
        "/api/v1/community/feed?tab=all&sort=hot&limit=100",
        "/api/v1/community/feed?tab=all&sort=rising&limit=100",
    ] {
        let (st, fv) = driver.req(Method::GET, path, &a_auth, None).await.unwrap();
        assert_eq!(st, 200, "feed {path} failed: {fv}");
        let feed: FeedResponseDto = serde_json::from_value(fv).unwrap();
        let ids: HashSet<&str> = feed.items.iter().map(|i| i.post.post_id.as_str()).collect();
        assert!(!ids.contains(p_am.as_str()), "AutoMod-removed post leaked into {path}");
        assert!(!ids.contains(p_blocked.as_str()), "blocked actor post leaked into {path}");
        assert!(!ids.contains(p_muted_c.as_str()), "muted community post leaked into {path}");
        assert!(!ids.contains(p_muted_k.as_str()), "muted keyword post leaked into {path}");
        assert!(ids.contains(p_clean.as_str()), "clean post missing from {path}");
    }
}

/// [Test ID] TEST-CM-T3-09
/// [Feature / Req] F06 x F02 / ORIGINAL_REQUEST.md (ActorFilter + AlgoPreset `HumanFocused` vs `BotHeavy` composition shift)
/// [Preconditions] Ephemeral live server with Humans & Bots plus balanced pure-Rust candidate pool.
/// [Opaque-Box Steps] Query live feeds across `HumansOnly`, `BotsOnly`, `HumanBotDebates`, switch `/algo-prefs` between `HumanFocused` and `BotHeavy`, and compare equal-score recommender outputs.
/// [Observable Assertions] Every `ActorFilter` isolates exact citizen subsets, `BotHeavy` shifts top-slot composition toward Bots vs `HumanFocused`, and `FeedExplanationDto` exposes `"Citizen Balance"`.
#[tokio::test]
async fn test_cm_t3_09_f06_f02_actor_filter_and_algo_preset_human_vs_bot_composition() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_09", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_09", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "debate_t3_09_bot", "Debate Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "t3_09_debates", "name": "T3-09 Debates", "allow_bots": true }))).await.unwrap().0, 201);

    let debate_pid = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "t3_09_debates", "body": "Human+Bot architecture debate T3-09" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{debate_pid}/comments"), &bot_auth, Some(json!({ "body": "Bot perspective on T3-09" }))).await.unwrap().0, 201);
    let solo_h_pid = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "t3_09_debates", "body": "Solo human post T3-09" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    let _solo_b_pid = driver.req(Method::POST, "/api/v1/community/posts", &bot_auth, Some(json!({ "community_slug": "t3_09_debates", "body": "Solo bot post T3-09" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

    let h_feed: FeedResponseDto = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/feed?tab=all&actor_filter=humans_only&limit=50", &a_auth, None).await.unwrap().1).unwrap();
    assert!(!h_feed.items.is_empty() && h_feed.items.iter().all(|i| i.post.author_kind == ActorKind::Human));

    let b_feed: FeedResponseDto = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/feed?tab=all&actor_filter=bots_only&limit=50", &a_auth, None).await.unwrap().1).unwrap();
    assert!(!b_feed.items.is_empty() && b_feed.items.iter().all(|i| i.post.author_kind.is_bot_like()));

    let d_feed: FeedResponseDto = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/feed?tab=all&actor_filter=human_bot_debates&limit=50", &a_auth, None).await.unwrap().1).unwrap();
    assert!(d_feed.items.iter().any(|i| i.post.post_id == debate_pid), "Human+Bot debate must appear");
    assert!(!d_feed.items.iter().any(|i| i.post.post_id == solo_h_pid), "Solo human post must be excluded from debates");

    // Live server preset switch + FeedExplanationDto verification
    let hf_prefs = default_prefs_for_preset(AlgoPreset::HumanFocused);
    assert_eq!(driver.req(Method::PUT, "/api/v1/community/algo-prefs", &a_auth, Some(serde_json::to_value(&hf_prefs).unwrap())).await.unwrap().0, 200);
    let hf_live: FeedResponseDto = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/feed?tab=for_you&limit=20", &a_auth, None).await.unwrap().1).unwrap();
    assert_eq!(hf_live.applied_preset, AlgoPreset::HumanFocused);
    assert!(hf_live.items[0].explanation.as_ref().unwrap().signal_breakdown.iter().any(|s| s.label == "Citizen Balance"));

    let bh_prefs = default_prefs_for_preset(AlgoPreset::BotHeavy);
    assert_eq!(driver.req(Method::PUT, "/api/v1/community/algo-prefs", &a_auth, Some(serde_json::to_value(&bh_prefs).unwrap())).await.unwrap().0, 200);
    let bh_live: FeedResponseDto = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/feed?tab=for_you&limit=20", &a_auth, None).await.unwrap().1).unwrap();
    assert_eq!(bh_live.applied_preset, AlgoPreset::BotHeavy);

    // Controlled equal-score pool proves HumanFocused vs BotHeavy composition shift deterministically
    let t0 = now_ms();
    let base = build_starter_dataset(t0).posts[0].clone();
    let mut pool = Vec::new();
    for i in 0..10 {
        for (kind, tag) in [(ActorKind::Human, "h"), (ActorKind::Bot, "b")] {
            let mut p = base.clone();
            p.post_id = format!("pool_{tag}_{i}");
            p.author_actor_id = format!("{tag}:{i}");
            p.author_handle = format!("actor_{tag}_{i}");
            p.author_kind = kind;
            p.community_slug = Some(format!("comm_{}", i % 4));
            p.upvotes = 20;
            p.downvotes = 1;
            p.score = 19;
            p.created_at_ms = t0 - 300_000;
            pool.push(PostCandidate::from_post(p));
        }
    }
    let viewer_hf = ViewerContext { algo_prefs: hf_prefs, now_ms: t0, ..Default::default() };
    let viewer_bh = ViewerContext { algo_prefs: bh_prefs, now_ms: t0, ..Default::default() };
    let req = FeedRequestDto { tab: FeedTab::ForYou, sort: FeedSort::Algo, actor_filter: ActorFilter::AllCitizens, limit: 10, now_ms: Some(t0), ..Default::default() };
    let res_hf = CommunityRecommender::rank_feed(&viewer_hf, pool.clone(), &req);
    let res_bh = CommunityRecommender::rank_feed(&viewer_bh, pool, &req);
    let bots_hf = res_hf.items.iter().filter(|i| i.post.author_kind.is_bot_like()).count();
    let bots_bh = res_bh.items.iter().filter(|i| i.post.author_kind.is_bot_like()).count();
    assert!(bots_bh > bots_hf, "BotHeavy ({bots_bh} bots) must surface strictly more bots than HumanFocused ({bots_hf} bots)");
}

/// [Test ID] TEST-CM-T3-10
/// [Feature / Req] F06 x F07 / ORIGINAL_REQUEST.md (Trending velocity burst + Recommender ranking & explanation)
/// [Preconditions] Ephemeral live server with 4 Humans and 1 Bot.
/// [Opaque-Box Steps] Generate a multi-actor `#HyperVelocityRust2026` burst with heavy upvotes/comments on `p_burst` vs quiet `p_quiet`; fetch `/trending`, `ForYou`, and `Rising`.
/// [Observable Assertions] `#HyperVelocityRust2026` surfaces in `/trending`, `p_burst` outranks `p_quiet` in `ForYou` & `Rising`, and `p_burst` carries higher `"Rising Velocity"` explanation signal.
#[tokio::test]
async fn test_cm_t3_10_f06_f07_trending_velocity_boost_and_recommender_explanation() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_10", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_10", "Pass1234!").await.unwrap();
    let carol = driver.register_human("carol_t3_10", "Pass1234!").await.unwrap();
    let dave = driver.register_human("dave_t3_10", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "vel_t3_10_bot", "Velocity Bot", "manual").await.unwrap();

    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let c_auth = CommunityAuthMode::Human(carol.token.clone());
    let d_auth = CommunityAuthMode::Human(dave.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "quiet_t3_10", "name": "Quiet T3 10" }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "burst_t3_10", "name": "Burst T3 10" }))).await.unwrap().0, 201);

    let p_quiet = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "quiet_t3_10", "body": "Quiet routine status note T3-10" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    let p_burst = driver.req(Method::POST, "/api/v1/community/posts", &c_auth, Some(json!({ "community_slug": "burst_t3_10", "body": "Major milestone #HyperVelocityRust2026 benchmark drop!" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

    // Multi-actor hashtag burst across communities + rapid engagement on p_burst
    let burst_comms = ["rust", "systems", "ai_agents", "open_source"];
    for (idx, auth) in [&a_auth, &b_auth, &d_auth, &bot_auth].iter().enumerate() {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", auth, Some(json!({ "community_slug": burst_comms[idx], "body": format!("Burst post #{idx} discussing #HyperVelocityRust2026") }))).await.unwrap().0, 201);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{p_burst}/vote"), auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{p_burst}/comments"), auth, Some(json!({ "body": format!("Rapid velocity comment {idx}") }))).await.unwrap().0, 201);
    }

    let trend: TrendingResponse = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/trending?limit=25", &a_auth, None).await.unwrap().1).unwrap();
    let tag_topic = trend
        .topics
        .iter()
        .find(|t| t.slug_or_tag.to_ascii_lowercase().contains("hypervelocityrust2026"))
        .expect("#HyperVelocityRust2026 must surface in /trending");
    assert!(tag_topic.velocity_score > 0.0 && tag_topic.posts_count_24h >= 5);

    for path in ["/api/v1/community/feed?tab=for_you&limit=60", "/api/v1/community/feed?tab=all&sort=rising&limit=60"] {
        let feed: FeedResponseDto = serde_json::from_value(driver.req(Method::GET, path, &a_auth, None).await.unwrap().1).unwrap();
        let pos_burst = feed.items.iter().position(|i| i.post.post_id == p_burst).unwrap_or_else(|| panic!("p_burst missing from {path}"));
        let pos_quiet = feed.items.iter().position(|i| i.post.post_id == p_quiet).unwrap_or_else(|| panic!("p_quiet missing from {path}"));
        assert!(pos_burst < pos_quiet, "p_burst (idx {pos_burst}) must outrank p_quiet (idx {pos_quiet}) in {path}");
        if path.contains("for_you") {
            let exp_b = feed.items[pos_burst].explanation.as_ref().expect("ForYou explanation");
            let exp_q = feed.items[pos_quiet].explanation.as_ref().expect("ForYou explanation");
            assert!(!exp_b.primary_reason.is_empty());
            let vel_b = exp_b.signal_breakdown.iter().find(|s| s.label == "Rising Velocity").map(|s| s.contribution_pct).unwrap_or(0.0);
            let vel_q = exp_q.signal_breakdown.iter().find(|s| s.label == "Rising Velocity").map(|s| s.contribution_pct).unwrap_or(0.0);
            assert!(vel_b > 0.0 && vel_b > vel_q, "burst post velocity share ({vel_b}) must exceed quiet post ({vel_q})");
        }
    }
}

/// [Test ID] TEST-CM-T3-11
/// [Feature / Req] F07 x F10 / ORIGINAL_REQUEST.md (Entity extraction + Tantivy BM25 structured search & delete pruning)
/// [Preconditions] Ephemeral live server with Tantivy `CommunitySearchIndexer`, Human Alice, Bot `alpha_t3_11_bot`, and `c/systems_t3_11`.
/// [Opaque-Box Steps] Publish Bot post containing `#rust`, `@alpha_t3_11_bot`, `c/systems_t3_11`, and CJK text; run structured & CJK Tantivy searches; delete post and re-query.
/// [Observable Assertions] Entities extract cleanly, all structured/CJK searches return exact post hit, and deletion removes the document immediately from Tantivy results.
#[tokio::test]
async fn test_cm_t3_11_f07_f10_entity_extraction_and_tantivy_structured_search_delete() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_11", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "alpha_t3_11_bot", "Alpha Search Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "systems_t3_11", "name": "Systems T3 11", "allow_bots": true }))).await.unwrap().0, 201);

    let body_text = "Exploring #rust ZeroCopyT311 内核零拷贝异步架构 inside c/systems_t3_11 with @alpha_t3_11_bot";
    let ent = extract_entities(body_text);
    assert!(ent.hashtags.iter().any(|h| h == "rust"));
    assert!(ent.mentions.iter().any(|m| m == "alpha_t3_11_bot"));
    assert!(ent.communities.iter().any(|c| c == "systems_t3_11"));

    let (st, pv) = driver.req(Method::POST, "/api/v1/community/posts", &bot_auth, Some(json!({
        "community_slug": "systems_t3_11", "title": "ZeroCopyT311 Architecture", "body": body_text
    }))).await.unwrap();
    assert_eq!(st, 201, "create search post failed: {pv}");
    let created: CreatePostResponse = serde_json::from_value(pv).unwrap();
    let pid = created.post.post_id.clone();
    assert!(created.post.tags.iter().any(|t| t == "rust"), "hashtag #rust must persist in post.tags");

    for query_path in [
        "/api/v1/community/search?q=ZeroCopyT311",
        "/api/v1/community/search?q=%E9%9B%B6%E6%8B%B7%E8%B4%9D",
        "/api/v1/community/search?q=%23rust&community=systems_t3_11",
        "/api/v1/community/search?q=ZeroCopyT311&author=alpha_t3_11_bot&actor_kind=bot",
        "/api/v1/community/search?q=c:systems_t3_11%20from:@alpha_t3_11_bot",
    ] {
        let (st, sv) = driver.req(Method::GET, query_path, &a_auth, None).await.unwrap();
        assert_eq!(st, 200, "search {query_path} failed: {sv}");
        let res: CommunitySearchResultDto = serde_json::from_value(sv).unwrap();
        assert!(res.posts.iter().any(|p| p.post_id == pid), "search {query_path} missed post {pid}");
    }

    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/posts/{pid}"), &bot_auth, None).await.unwrap().0, 200);
    let after_del: CommunitySearchResultDto = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/search?q=ZeroCopyT311", &a_auth, None).await.unwrap().1).unwrap();
    assert!(!after_del.posts.iter().any(|p| p.post_id == pid), "deleted post must be pruned from Tantivy");
}

/// [Test ID] TEST-CM-T3-12
/// [Feature / Req] F08 x F09 / ORIGINAL_REQUEST.md (AutoMod enforces keyword & karma-floor rules equally against Bot citizens)
/// [Preconditions] Ephemeral live server with Owner Alice and 2 Bots (`spam_t3_12_bot`, `lowkarma_t3_12_bot`) in `c/equal_am_t3_12`.
/// [Opaque-Box Steps] Configure AutoMod keyword rule (`flag_for_review`), publish rule-violating Bot post; add karma-floor rule (`remove`), publish low-karma Bot post; resolve flagged item via mod-actions.
/// [Observable Assertions] Both Bot posts trigger AutoMod equally, enqueue in `mod-queue` with `source == "automod"`, and mod resolution records transparent `mod-logs` entry.
#[tokio::test]
async fn test_cm_t3_12_f08_f09_automod_enforces_equally_against_bot_citizens() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_12", "Pass1234!").await.unwrap();
    let bot_spam = driver.register_community_bot(&alice.token, "spam_t3_12_bot", "Spam Bot", "manual").await.unwrap();
    let bot_low = driver.register_community_bot(&alice.token, "lowkarma_t3_12_bot", "LowKarma Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let spam_auth = CommunityAuthMode::DirectBot(bot_spam.bot_token.clone());
    let low_auth = CommunityAuthMode::DirectBot(bot_low.bot_token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({
        "slug": "equal_am_t3_12", "name": "Equal AutoMod", "allow_bots": true,
        "automod_rules": [{
            "rule_id": "am_kw_t312", "community_id": "", "name": "Bot Keyword Guard", "enabled": true, "priority": 1,
            "keywords": ["bot_promo_t312"], "regex_pattern": null, "min_author_karma": null,
            "min_account_age_secs": null, "max_links": null, "block_duplicate_spam": false,
            "apply_to_bots": true, "apply_to_humans": true, "action": "flag_for_review",
            "assign_flair": null, "auto_reply_template": null, "created_at_ms": now_ms()
        }]
    }))).await.unwrap().0, 201);

    // 1. Keyword rule flags Bot citizen post to mod-queue
    let (st, pv1) = driver.req(Method::POST, "/api/v1/community/posts", &spam_auth, Some(json!({
        "community_slug": "equal_am_t3_12", "body": "Automated bot_promo_t312 broadcast"
    }))).await.unwrap();
    assert_eq!(st, 201);
    assert_eq!(pv1["automod_action"], "flag_for_review");
    let p_flagged = pv1["post"]["post_id"].as_str().unwrap().to_string();

    // 2. Add karma-floor rule -> 0-karma Bot post is auto-removed & enqueued
    assert_eq!(driver.req(Method::PATCH, "/api/v1/community/communities/equal_am_t3_12", &a_auth, Some(json!({
        "automod_rules": [{
            "rule_id": "am_kf_t312", "community_id": "", "name": "Bot Karma Floor", "enabled": true, "priority": 1,
            "keywords": [], "regex_pattern": null, "min_author_karma": 25,
            "min_account_age_secs": null, "max_links": null, "block_duplicate_spam": false,
            "apply_to_bots": true, "apply_to_humans": true, "action": "remove",
            "assign_flair": null, "auto_reply_template": null, "created_at_ms": now_ms()
        }]
    }))).await.unwrap().0, 200);

    let (st, pv2) = driver.req(Method::POST, "/api/v1/community/posts", &low_auth, Some(json!({
        "community_slug": "equal_am_t3_12", "body": "Zero-karma bot citizen post"
    }))).await.unwrap();
    assert_eq!(st, 201);
    assert_eq!(pv2["automod_action"], "remove");
    assert_eq!(pv2["post"]["is_removed"], true);
    let p_removed = pv2["post"]["post_id"].as_str().unwrap().to_string();

    let queue: ModQueueResponse = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/communities/equal_am_t3_12/mod-queue", &a_auth, None).await.unwrap().1).unwrap();
    let q_flagged = queue.items.iter().find(|it| it.target_id == p_flagged && it.source == "automod").expect("flagged bot post must be in mod-queue");
    assert!(queue.items.iter().any(|it| it.target_id == p_removed && it.source == "automod"), "karma-removed bot post must be in mod-queue");

    // Mod resolves flagged bot post -> transparent mod-logs entry recorded
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/equal_am_t3_12/mod-actions", &a_auth, Some(json!({
        "action": "remove", "target_kind": "post", "target_id": p_flagged, "reason": "Confirmed bot promo violation", "queue_item_id": q_flagged.item_id
    }))).await.unwrap().0, 200);
    let logs: ModLogsResponse = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/communities/equal_am_t3_12/mod-logs", &spam_auth, None).await.unwrap().1).unwrap();
    assert!(logs.logs.iter().any(|l| l.target_id == p_flagged && l.action == ModActionKind::Remove), "mod-logs must transparently record bot moderation");
}

/// [Test ID] TEST-CM-T3-13
/// [Feature / Req] F09 x F12 / ORIGINAL_REQUEST.md (5-hop anti-loop ceiling across Bot API & reactive responder)
/// [Preconditions] Ephemeral live server with Human Alice and 2 Reactive Bots (`ping_t3_13_bot`, `pong_t3_13_bot`) in `c/loop_t3_13`.
/// [Opaque-Box Steps] Alternate `/bot/{token}/community.createComment` mutual mentions between the 2 reactive Bots until hitting the 5-hop ceiling, wait for background effects, then have Human Alice reset the loop.
/// [Observable Assertions] Consecutive bot comments never exceed 5 across Bot API + reactive responder, hop 6 returns 400 `bot_loop_budget_exceeded`, and Human reply cleanly resets the budget.
#[tokio::test]
async fn test_cm_t3_13_f09_f12_five_hop_anti_loop_across_bot_api_and_reactive_responder() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_13", "Pass1234!").await.unwrap();
    let ping = driver.register_community_bot(&alice.token, "ping_t3_13_bot", "Ping Bot", "reactive").await.unwrap();
    let pong = driver.register_community_bot(&alice.token, "pong_t3_13_bot", "Pong Bot", "reactive").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "loop_t3_13", "name": "Loop Guard", "allow_bots": true }))).await.unwrap().0, 201);
    let pid = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "community_slug": "loop_t3_13", "body": "Anchor post for anti-loop test" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

    // First Bot API comment triggers live reactive responder asynchronously; then drive remaining hops up to ceiling
    let (st0, c0) = driver.bot_api(&ping.bot_token, "community.createComment", true, Some(json!({
        "post_id": pid, "body": "Turn 1: paging @pong_t3_13_bot for analysis"
    }))).await.unwrap();
    assert_eq!(st0, 200, "hop 1 failed: {c0}");
    let mut last_cid = c0["result"]["comment"]["comment_id"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut hit_loop_budget = false;
    for turn in 2..=8usize {
        let (tok, other_handle) = if turn % 2 == 0 { (&pong.bot_token, "ping_t3_13_bot") } else { (&ping.bot_token, "pong_t3_13_bot") };
        let (st, resp) = driver.bot_api(tok, "community.createComment", true, Some(json!({
            "post_id": pid, "parent_comment_id": last_cid, "body": format!("Turn {turn}: handing off to @{other_handle}")
        }))).await.unwrap();
        if st == 200 && resp["ok"] == true {
            last_cid = resp["result"]["comment"]["comment_id"].as_str().unwrap().to_string();
        } else {
            assert_eq!(st, 400, "expected 400 on loop cap: {resp}");
            assert_eq!(resp["ok"], false);
            assert!(resp["description"].as_str().unwrap_or("").contains("bot_loop_budget_exceeded"), "expected bot_loop_budget_exceeded, got {resp}");
            hit_loop_budget = true;
            break;
        }
    }
    assert!(hit_loop_budget, "5-hop anti-loop guard MUST trigger by hop 6");

    // Verify no runaway background reactive recursion pushed bot count above 5
    tokio::time::sleep(Duration::from_millis(250)).await;
    let det: PostDetailResponse = serde_json::from_value(driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &a_auth, None).await.unwrap().1).unwrap();
    assert!(det.total_comments <= 5, "total bot comments must never exceed 5-hop cap, got {}", det.total_comments);

    // Human reply resets consecutive bot hop counter so Bot API can resume cleanly
    let (st_h, _) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &a_auth, Some(json!({ "body": "Human moderator chime-in resets loop budget" }))).await.unwrap();
    assert_eq!(st_h, 201);
    let (st_resume, resume_v) = driver.bot_api(&ping.bot_token, "community.createComment", true, Some(json!({ "post_id": pid, "body": "Bot resumes cleanly after Human turn" }))).await.unwrap();
    assert_eq!(st_resume, 200, "bot reply after human reset must succeed: {resume_v}");
    assert_eq!(resume_v["ok"], true);
}

/// [Test ID] TEST-CM-T3-14
/// [Feature / Req] F11 x F04 / ORIGINAL_REQUEST.md (Public community media 200 OK + Private DM chat media 403 isolation)
/// [Preconditions] Ephemeral live server with Humans Alice & Bob and an Alice-only private chat.
/// [Opaque-Box Steps] Alice uploads PNG A (community) and PNG B (private chat); verify Bob gets 403 before publish and Alice cannot leak PNG B to a post; Alice attaches PNG A to a Media post; Bob fetches A and B.
/// [Observable Assertions] Bob downloads PNG A with 200 OK & exact PNG bytes while private chat PNG B remains strictly 403 Forbidden to Bob (and 200 to Alice).
#[tokio::test]
async fn test_cm_t3_14_f11_f04_public_community_media_200_and_private_dm_media_403() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_14", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_14", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());

    let (st_c, chat_v) = driver.req(Method::POST, "/api/v1/chats", &a_auth, Some(json!({ "title": "Alice Private DM", "chat_type": "group" }))).await.unwrap();
    assert!(st_c == 200 || st_c == 201, "create private chat failed ({st_c}): {chat_v}");
    let private_chat_id = chat_v["chat_id"].as_i64().or_else(|| chat_v["id"].as_i64()).unwrap();

    let (mid_a, png_a) = driver.upload_png(&alice.token, "public_a.png", (16, 200, 128, 255), None).await.unwrap();
    let (mid_b, png_b) = driver.upload_png(&alice.token, "private_b.png", (220, 48, 48, 255), Some(private_chat_id)).await.unwrap();

    // Before attaching A, Bob gets 403 on both A and B; attaching private chat media B to public post is 403
    assert_eq!(driver.get_media(&bob.token, &mid_a).await.unwrap().0, 403);
    assert_eq!(driver.get_media(&bob.token, &mid_b).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
        "format": "media", "body": "Attempt leak of private chat PNG", "media": [{ "media_id": mid_b, "kind": "image" }]
    }))).await.unwrap().0, 403);

    // Attach PNG A to published community post -> Bob receives 200 OK byte-exact for A, 403 for B
    assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
        "format": "media", "body": "Public community PNG A", "media": [{ "media_id": mid_a, "kind": "image" }]
    }))).await.unwrap().0, 201);

    let (st_a, bytes_a) = driver.get_media(&bob.token, &mid_a).await.unwrap();
    assert_eq!(st_a, 200, "public community PNG A must return 200 to Bob");
    assert_eq!(bytes_a, png_a, "Bob must receive exact PNG A bytes");
    assert_eq!(driver.get_media_meta(&bob.token, &mid_a).await.unwrap().0, 200);

    assert_eq!(driver.get_media(&bob.token, &mid_b).await.unwrap().0, 403, "private chat PNG B must stay 403 to Bob");
    assert_eq!(driver.get_media_meta(&bob.token, &mid_b).await.unwrap().0, 403);

    let (st_owner_b, bytes_b) = driver.get_media(&alice.token, &mid_b).await.unwrap();
    assert_eq!(st_owner_b, 200);
    assert_eq!(bytes_b, png_b);
}

// ============================================================================
// Pairwise Bot API, Webhooks, Hermes Agent, Scheduler & Client Seams (T3-15 .. T3-20)
// ============================================================================

/// [Test ID] TEST-CM-T3-15
/// [Feature / Req] F12 x F13 / ORIGINAL_REQUEST.md (Bot API write -> live TCP WS `UpdateEvent::CommunityEvent` + authenticated Webhook POST)
/// [Preconditions] Ephemeral live server + `WebhookCaptureServer` + Human Alice on TCP WS + registered Bot with `setWebhook(secret_token)`.
/// [Opaque-Box Steps] Bot calls `POST /bot/{token}/community.createPost`; Alice comments mentioning `@hook_ws_t3_15_bot`.
/// [Observable Assertions] Alice's WS immediately receives `post_created` & `comment_created` CommunityEvents; Webhook receives POST with exact `X-Telegram-Bot-Api-Secret-Token` and JSON payload.
#[tokio::test]
async fn test_cm_t3_15_f12_f13_bot_api_write_ws_fanout_and_authenticated_webhook() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let mut capture = WebhookCaptureServer::start().await.expect("start webhook capture");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_15", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "hook_ws_t3_15_bot", "Hook WS Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());

    let secret = "sec-t3-15-verified";
    let (st_wh, wh_v) = driver.bot_api(&bot.bot_token, "setWebhook", true, Some(json!({ "url": capture.url(), "secret_token": secret }))).await.unwrap();
    assert_eq!(st_wh, 200, "setWebhook failed: {wh_v}");
    assert_eq!(wh_v["ok"], true);

    let mut ws = connect_ws(&server, &alice.token).await.expect("connect Alice WS");

    // Bot publishes via standard Bot API -> Human TCP WS receives post_created immediately
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "t3_15_hooks", "name": "T3-15 Hooks", "allow_bots": true }))).await.unwrap().0, 201);

    let (st_bp, bp_v) = driver.bot_api(&bot.bot_token, "community.createPost", true, Some(json!({
        "community_slug": "t3_15_hooks", "body": "Bot API post broadcasting to live WS!"
    }))).await.unwrap();
    assert_eq!(st_bp, 200, "bot createPost failed: {bp_v}");
    assert_eq!(bp_v["ok"], true);
    let bot_pid = bp_v["result"]["post"]["post_id"].as_str().unwrap().to_string();

    let _ = recv_ws_frame_matching(&mut ws, Duration::from_secs(4), |f| {
        matches!(
            f,
            WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, actor_id, .. })
                if event_kind == "post_created"
                    && post_id.as_deref() == Some(bot_pid.as_str())
                    && actor_id.as_deref() == Some(bot.actor_id.as_str())
        )
    })
    .await
    .expect("Alice WS must receive post_created from Bot API write");

    // Human mentions Bot in comment -> Webhook receives authenticated POST + WS receives comment_created
    let (st_c, cv) = driver.req(Method::POST, &format!("/api/v1/community/posts/{bot_pid}/comments"), &a_auth, Some(json!({
        "body": "Great writeup @hook_ws_t3_15_bot, triggering webhook!"
    }))).await.unwrap();
    assert_eq!(st_c, 201);
    let cid = cv["comment"]["comment_id"].as_str().unwrap().to_string();

    let _ = recv_ws_frame_matching(&mut ws, Duration::from_secs(4), |f| {
        matches!(
            f,
            WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, comment_id, .. })
                if event_kind == "comment_created"
                    && post_id.as_deref() == Some(bot_pid.as_str())
                    && comment_id.as_deref() == Some(cid.as_str())
        )
    })
    .await
    .expect("Alice WS must receive comment_created");

    let (got_secret, payload) = capture.recv_timeout(Duration::from_secs(4)).await.expect("webhook POST must arrive within 4s");
    assert_eq!(got_secret.as_deref(), Some(secret));
    let event_obj = payload.get("community_mention").or_else(|| payload.get("community_comment")).expect("webhook must carry community_mention or community_comment");
    assert_eq!(event_obj["post_id"], bot_pid);
    assert_eq!(event_obj["from_handle"], "alice_t3_15");

    drop(ws);
    capture.shutdown().await;
}

/// [Test ID] TEST-CM-T3-16
/// [Feature / Req] F12 x F10 / ORIGINAL_REQUEST.md (Hermes Agent Community tools via `ServerPlatformImBridge` -> SQLite V42 + Tantivy search)
/// [Preconditions] Ephemeral live server with registered Bot citizen executed strictly with `user_id = None, bot_id = Some(bot_id)`.
/// [Opaque-Box Steps] Execute `community_create_post`, `community_comment`, `community_vote`, and `community_search` via `execute_bot_hermes_tool`; cross-check REST feed, REST search, and SQLite V42 tables.
/// [Observable Assertions] Every Hermes tool succeeds as `b:{bot_id}` and writes real rows persisted in SQLite V42 and indexed in Tantivy BM25.
#[tokio::test]
async fn test_cm_t3_16_f12_f10_hermes_agent_tools_sqlite_and_tantivy_search() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_16", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "hermes_t3_16_bot", "Hermes T3 16", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());

    let r_post = execute_bot_hermes_tool(
        &server,
        bot.bot_user_id,
        "community_create_post",
        json!({ "community_slug": "rust", "title": "HermesT316Quantum", "body": "Pure Bot Hermes tool write via ServerPlatformImBridge #hermest316" }),
        ToolsetPreset::HermesTelegram,
        ApprovalMode::Prompt,
    )
    .await;
    assert!(r_post.success, "community_create_post failed: {:?}", r_post.error);
    let post_id = r_post.structured.as_ref().unwrap()["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(r_post.structured.as_ref().unwrap()["post"]["author_actor_id"], bot.actor_id);

    let r_cmt = execute_bot_hermes_tool(
        &server,
        bot.bot_user_id,
        "community_comment",
        json!({ "post_id": post_id, "body": "Hermes pure-bot follow-up comment T3-16" }),
        ToolsetPreset::HermesTelegram,
        ApprovalMode::Prompt,
    )
    .await;
    assert!(r_cmt.success, "community_comment failed: {:?}", r_cmt.error);
    let cmt_id = r_cmt.structured.as_ref().unwrap()["comment"]["comment_id"].as_str().unwrap().to_string();

    let r_vote = execute_bot_hermes_tool(
        &server,
        bot.bot_user_id,
        "community_vote",
        json!({ "post_id": "seed_post_01", "direction": 1 }),
        ToolsetPreset::HermesTelegram,
        ApprovalMode::Prompt,
    )
    .await;
    assert!(r_vote.success, "community_vote failed: {:?}", r_vote.error);

    let r_search = execute_bot_hermes_tool(
        &server,
        bot.bot_user_id,
        "community_search",
        json!({ "query": "HermesT316Quantum" }),
        ToolsetPreset::HermesTelegram,
        ApprovalMode::Prompt,
    )
    .await;
    assert!(r_search.success, "community_search failed: {:?}", r_search.error);
    assert!(r_search.structured.as_ref().unwrap()["posts"].as_array().unwrap().iter().any(|p| p["post_id"] == post_id));

    // Independently verify persistence in REST feed, REST Tantivy search, and SQLite V42 tables
    let rest_search: CommunitySearchResultDto = serde_json::from_value(driver.req(Method::GET, "/api/v1/community/search?q=HermesT316Quantum", &a_auth, None).await.unwrap().1).unwrap();
    assert!(rest_search.posts.iter().any(|p| p.post_id == post_id && p.author_actor_id == bot.actor_id && p.author_kind == ActorKind::Bot));

    let (sql_author, sql_cmt_author): (String, String) = server
        .state()
        .unwrap()
        .db
        .pool()
        .with_reader(|conn| {
            let pa: String = conn.query_row("SELECT author_actor_id FROM community_posts WHERE post_id = ?1", rusqlite::params![post_id], |r| r.get(0)).map_err(|e| gpui_telegram_core::TelegramError::Database(e.to_string()))?;
            let ca: String = conn.query_row("SELECT author_actor_id FROM community_comments WHERE comment_id = ?1", rusqlite::params![cmt_id], |r| r.get(0)).map_err(|e| gpui_telegram_core::TelegramError::Database(e.to_string()))?;
            Ok((pa, ca))
        })
        .unwrap();
    assert_eq!(sql_author, bot.actor_id);
    assert_eq!(sql_cmt_author, bot.actor_id);
}

/// [Test ID] TEST-CM-T3-17
/// [Feature / Req] F13 x F04 / ORIGINAL_REQUEST.md (Scheduler tick -> Scheduled post publish + Expired poll close -> WS events & late-vote rejection)
/// [Preconditions] Ephemeral live server with Alice, Bob (subscribed on TCP WS), a Scheduled post due soon, and a Poll expiring soon.
/// [Opaque-Box Steps] Verify scheduled post returns 404 to Bob initially & poll accepts open vote; advance past deadline, execute `run_community_background_tick`, inspect WS & REST.
/// [Observable Assertions] Scheduler publishes post, closes poll (rejecting late Bob vote with 410), and emits both `scheduled_post_published` and `poll_closed` over WS.
#[tokio::test]
async fn test_cm_t3_17_f13_f04_scheduler_publishes_scheduled_post_closes_poll_and_emits_ws() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t3_17", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t3_17", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    let due_ms = now_ms() + 1_200;
    let (st_s, sv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
        "format": "micro", "status": "scheduled", "scheduled_at_ms": due_ms, "body": "SchedT317Launch published by scheduler"
    }))).await.unwrap();
    assert_eq!(st_s, 201);
    let sched_pid = sv["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::GET, &format!("/api/v1/community/posts/{sched_pid}"), &b_auth, None).await.unwrap().0, 404, "future scheduled post must be hidden from Bob");

    let (st_p, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
        "format": "poll", "body": "Expiring poll T3-17",
        "poll": { "question": "Freeze release?", "options": ["Yes", "No"], "duration_secs": 3600 }
    }))).await.unwrap();
    assert_eq!(st_p, 201);
    let poll_pid = pv["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{poll_pid}/poll-vote"), &a_auth, Some(json!({ "option_ids": ["opt_0"] }))).await.unwrap().0, 200);

    let expire_ms = now_ms() + 1_200;
    server.state().unwrap().db.pool().with_writer(|conn| {
        conn.execute("UPDATE community_posts SET poll_closes_at_ms = ?1 WHERE post_id = ?2", rusqlite::params![expire_ms, poll_pid])
            .map_err(|e| gpui_telegram_core::TelegramError::Database(e.to_string()))?;
        Ok(())
    }).unwrap();

    let mut ws = connect_ws(&server, &bob.token).await.expect("connect Bob WS");
    tokio::time::sleep(Duration::from_millis(1_350)).await;
    gpui_telegram_server::routes::community::run_community_background_tick(server.state().unwrap()).await;

    let mut saw_sched = false;
    let mut saw_poll = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while (!saw_sched || !saw_poll) && tokio::time::Instant::now() < deadline {
        if let Ok(WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, .. })) =
            recv_ws_frame_matching(&mut ws, Duration::from_secs(2), |f| matches!(f, WsServerFrame::Update(UpdateEvent::CommunityEvent { .. }))).await
        {
            if event_kind == "scheduled_post_published" && post_id.as_deref() == Some(sched_pid.as_str()) {
                saw_sched = true;
            } else if event_kind == "poll_closed" && post_id.as_deref() == Some(poll_pid.as_str()) {
                saw_poll = true;
            }
        }
    }
    assert!(saw_sched, "WS must broadcast scheduled_post_published");
    assert!(saw_poll, "WS must broadcast poll_closed");

    let (st_got, got_s) = driver.req(Method::GET, &format!("/api/v1/community/posts/{sched_pid}"), &b_auth, None).await.unwrap();
    assert_eq!(st_got, 200);
    assert_eq!(got_s["post"]["status"], "published");

    let (_, got_p) = driver.req(Method::GET, &format!("/api/v1/community/posts/{poll_pid}"), &b_auth, None).await.unwrap();
    assert_eq!(got_p["post"]["poll"]["is_closed"], true);
    let (st_late, _) = driver.req(Method::POST, &format!("/api/v1/community/posts/{poll_pid}/poll-vote"), &b_auth, Some(json!({ "option_ids": ["opt_1"] }))).await.unwrap();
    assert!(st_late == 410 || st_late == 400, "closed poll must reject late votes, got {st_late}");
    drop(ws);
}

/// [Test ID] TEST-CM-T3-18
/// [Feature / Req] F14 x F11 / ORIGINAL_REQUEST.md (Client Online Sync -> Server Stop -> Automatic Offline `<app_dir>/community.db` Fallback & Reopen Persistence)
/// [Preconditions] Ephemeral live server + `TestTelegramClient` bound to Alice in isolated `TempDir`.
/// [Opaque-Box Steps] Sync & post online; stop live server mid-session; read feed, create offline post & summon local Bot on the same client; shut down keeping `TempDir` and reopen.
/// [Observable Assertions] Online writes hit server; post-stop calls transparently switch to `LocalFallback`, persist to `<app_dir>/community.db`, and survive client reopen intact.
#[tokio::test]
async fn test_cm_t3_18_f14_f11_client_online_sync_server_stop_offline_fallback_and_reopen() {
    let mut server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let tc = TestTelegramClient::open(&server.base_url()).unwrap();
    let alice = tc.register_and_bind(&driver, "alice_t3_18", "Pass1234!").await.unwrap();

    // Online phase
    let online_feed = tc.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc.community().runtime_mode(), CommunityRuntimeMode::Online);
    assert!(!online_feed.items.is_empty());
    let online_post = tc.community().create_post(&CreatePostRequest { community_slug: Some("rust".into()), body: "Online synced post T3-18".into(), ..Default::default() }).await.unwrap();
    assert_eq!(driver.req(Method::GET, &format!("/api/v1/community/posts/{}", online_post.post.post_id), &CommunityAuthMode::Human(alice.token.clone()), None).await.unwrap().0, 200);

    // Stop server cleanly mid-session
    drop(driver);
    server.stop().await;

    // Automatic offline fallback on same client
    let offline_feed = tc.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(!offline_feed.items.is_empty());
    assert!(tc.dir_path().join("community.db").exists());

    let offline_post = tc.community().create_post(&CreatePostRequest { community_slug: Some("rust".into()), body: "Offline fallback post T3-18 for @ferris_bot".into(), ..Default::default() }).await.unwrap();
    let off_pid = offline_post.post.post_id.clone();
    let summon = tc.community().summon_bot(&off_pid, &SummonBotRequest { bot_actor_id: Some("b:910001".into()), prompt: Some("Offline review".into()), ..Default::default() }).await.unwrap();
    assert!(summon.triggered);
    let reply_cid = summon.created_comment.expect("offline bot reply").comment_id;

    // Reopen on same TempDir -> offline post & bot reply persist cleanly
    let dir = tc.shutdown_keep_dir().await.expect("keep TempDir");
    let tc2 = TestTelegramClient::open_in_dir(&closed_local_url(), dir).unwrap();
    tc2.bind_auth(alice.user_id as u64, &alice.username, &alice.token);
    let reopened = tc2.community().get_post(&off_pid, None).await.unwrap();
    assert_eq!(reopened.post.body, "Offline fallback post T3-18 for @ferris_bot");
    assert!(reopened.comments.iter().any(|c| c.comment_id == reply_cid && c.author_kind.is_bot_like()));
    tc2.shutdown().await;
}

/// [Test ID] TEST-CM-T3-19
/// [Feature / Req] F14 x F13 / ORIGINAL_REQUEST.md (Live Server WS -> `TelegramClient::poll_events()` `ClientDomainEvent::CommunityEvent`)
/// [Preconditions] Ephemeral live server + connected `TestTelegramClient` (Alice) + second Human Bob.
/// [Opaque-Box Steps] Connect Alice's client WS; Bob creates a post mentioning `@alice_t3_19` and posts a comment on it; poll `client.poll_events()`.
/// [Observable Assertions] Alice's client receives both `post_created` and `comment_created` `ClientDomainEvent::CommunityEvent` items with matching IDs and valid JSON payload.
#[tokio::test]
async fn test_cm_t3_19_f14_f13_live_server_ws_to_client_domain_event() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let tc = TestTelegramClient::open(&server.base_url()).unwrap();
    let _alice = tc.register_and_bind(&driver, "alice_t3_19", "Pass1234!").await.unwrap();
    let _ = tc.community().get_identities().await.unwrap();
    let bob = driver.register_human("bob_t3_19", "Pass1234!").await.unwrap();
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    tc.client().connect_ws();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let _ = tc.client().poll_events();

    let (st_p, pv) = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "body": "Live client WS ping for @alice_t3_19" }))).await.unwrap();
    assert_eq!(st_p, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let (st_c, cv) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &b_auth, Some(json!({ "body": "Follow-up WS comment T3-19" }))).await.unwrap();
    assert_eq!(st_c, 201);
    let cid = cv["comment"]["comment_id"].as_str().unwrap().to_string();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let (mut saw_post, mut saw_comment) = (false, false);
    while (!saw_post || !saw_comment) && tokio::time::Instant::now() < deadline {
        for ev in tc.client().poll_events() {
            let payload_ok = ev.community_payload_json().is_some();
            if let ClientDomainEvent::CommunityEvent { event_kind, post_id, comment_id, .. } = ev {
                if event_kind == "post_created" && post_id.as_deref() == Some(pid.as_str()) && payload_ok {
                    saw_post = true;
                }
                if event_kind == "comment_created"
                    && post_id.as_deref() == Some(pid.as_str())
                    && comment_id.as_deref() == Some(cid.as_str())
                    && payload_ok
                {
                    saw_comment = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(35)).await;
    }
    assert!(saw_post, "ClientDomainEvent::CommunityEvent(post_created) must arrive");
    assert!(saw_comment, "ClientDomainEvent::CommunityEvent(comment_created) must arrive");
    tc.shutdown().await;
}

/// [Test ID] TEST-CM-T3-20
/// [Feature / Req] F14 x F02 / ORIGINAL_REQUEST.md (Legacy-404 Fallback + Account Switch Isolation on shared `TempDir`)
/// [Preconditions] `Legacy404Server` + single `TestTelegramClient` in isolated `TempDir`.
/// [Opaque-Box Steps] Alice uses offline fallback to create a published post, a private Draft, a custom-folder bookmark, and `BotHeavy` algo prefs; switch session to Bob on same `TempDir`.
/// [Observable Assertions] Fallback engages seamlessly on 404; switching to Bob wipes Alice's private drafts, bookmarks, posts, and algo prefs with zero cross-account leakage.
#[tokio::test]
async fn test_cm_t3_20_f14_f02_legacy_404_fallback_and_account_switch_isolation() {
    let legacy = Legacy404Server::start().await.expect("start Legacy404Server");
    let tc = TestTelegramClient::open(&legacy.base_url()).unwrap();
    tc.bind_auth(93201, "alice_t3_20", "tok_alice_t3_20");

    let feed = tc.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(!feed.items.is_empty());

    let pub_p = tc.community().create_post(&CreatePostRequest { community_slug: Some("rust".into()), body: "Alice offline published post T3-20".into(), ..Default::default() }).await.unwrap();
    let draft_p = tc.community().create_post(&CreatePostRequest { community_slug: Some("rust".into()), status: PostStatus::Draft, body: "Alice private secret draft T3-20".into(), ..Default::default() }).await.unwrap();
    let _ = tc.community().bookmark_post("seed_post_01", &gpui_telegram_proto::community_contracts::BookmarkRequest { folder: Some("AliceSecretFolder".into()), remove: None }).await.unwrap();
    let _ = tc.community().update_algo_prefs(&default_prefs_for_preset(AlgoPreset::BotHeavy)).await.unwrap();

    // Switch session to Bob on the exact same TempDir & client
    tc.bind_auth(93202, "bob_t3_20", "tok_bob_t3_20");
    let bob_ids = tc.community().get_identities().await.unwrap();
    assert_eq!(bob_ids.active_actor_id, "u:93202");

    let bob_saved = tc.community().get_feed(&FeedRequestDto { tab: FeedTab::Saved, limit: 25, ..Default::default() }).await.unwrap();
    assert!(bob_saved.items.is_empty(), "Alice's bookmarks must never leak to Bob");
    assert!(tc.community().get_post(&draft_p.post.post_id, None).await.is_err(), "Alice's private draft must be wiped on account switch");
    assert!(tc.community().get_post(&pub_p.post.post_id, None).await.is_err(), "Alice's account-local post must be wiped on account switch");
    let bob_prefs = tc.community().get_algo_prefs().await.unwrap();
    assert_eq!(bob_prefs.preset, AlgoPreset::Balanced, "Bob must receive fresh default algo prefs");

    tc.shutdown().await;
    legacy.shutdown().await;
}

// ============================================================================
// Pairwise Headless GPUI Desktop Surfaces & Bridges (T3-21 .. T3-30)
// ============================================================================

/// [Test ID] TEST-CM-T3-21
/// [Feature / Req] F15 x F16 / ORIGINAL_REQUEST.md (Left Sidebar Expanded + Collapsed + AppleAppSidebar + `Cmd+Shift+C` + Unread Badge Sync + 3-Col `CommunityWorkspace`)
/// [Preconditions] Headless GPUI `AppShell` authenticated at `1440x900` in isolated `TempDir`.
/// [Opaque-Box Steps] Toggle `Cmd+B` between Expanded & Collapsed sidebar, enter Community via Expanded/Collapsed `ChatSidebar`, `AppleAppSidebar`, and `Cmd+Shift+C`, sync `UnreadCountChanged(17)`, and draw 3-col layout.
/// [Observable Assertions] Every entry activates Community cleanly, `Cmd+B` toggles collapse width, unread badge `17` syncs to `NavRail` & `AppleAppSidebar`, and 3-column layout renders cleanly.
#[test]
fn test_cm_t3_21_f15_f16_left_sidebar_modes_shortcut_unread_badge_and_3col_render() {
    run_community_gpui_test("test_cm_t3_21_f15_f16_left_sidebar_modes_shortcut_unread_badge_and_3col_render");
}

async fn body_t3_21(mut app: AsyncApp) {
    let (host, _) = open_offline_community_host(&mut app, 1440.0, 900.0, 9321, "t3_21", false).await;
    let shell = host.shell.clone();

    // 1. Expanded ChatSidebar -> Community
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(false);
            sb.handle_unified_action(UnifiedSidebarAction::SelectDestination(NavDestination::Community), scx);
        });
    });
    assert!(pump_until(&mut app, Duration::from_secs(6), |cx| {
        let s = shell.read(cx);
        s.active_panel == ActiveWorkspacePanel::Community
            && s.community_workspace.as_ref().is_some_and(|w| !w.read(cx).state.feed_loading && w.read(cx).state.identities.is_some())
    }).await);
    let cw = host.cw(&app);
    host.draw_at(&mut app, 1440.0, 900.0);
    assert_eq!(cw.read_with(&app, |w, _| w.last_layout_mode).unwrap(), CommunityLayoutMode::ThreeColumn);

    // 2. Cmd+B toggles Expanded <-> Collapsed 64px rail while staying in Community
    let collapsed0 = shell.read_with(&app, |s, cx| s.sidebar.read(cx).is_collapsed).unwrap();
    host.key(&mut app, "cmd-b");
    let collapsed1 = shell.read_with(&app, |s, cx| s.sidebar.read(cx).is_collapsed).unwrap();
    assert_ne!(collapsed0, collapsed1, "Cmd+B must toggle ChatSidebar collapse");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);
    host.draw_at(&mut app, 1440.0, 900.0);

    // 3. Collapsed rail & AppleAppSidebar & Cmd+Shift+C transitions
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(true);
            sb.handle_unified_action(UnifiedSidebarAction::SelectDestination(NavDestination::Community), scx);
        });
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| shell.read(cx).active_panel == ActiveWorkspacePanel::Community).await);

    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.select_apple_sidebar_section(AppleSidebarSection::Community, cx);
    });
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    host.key(&mut app, "cmd-shift-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats);
    host.key(&mut app, "cmd-shift-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    // 4. UnreadCountChanged syncs badge across NavRail & AppleAppSidebar in both collapsed & expanded modes
    let _ = cw.update(&mut app, |w, cx| w.emit_action(CommunityWorkspaceAction::UnreadCountChanged(17), cx));
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| {
        let s = shell.read(cx);
        let nav_badge = s.nav_rail.read(cx).items.iter().find(|i| i.destination == NavDestination::Community).map(|i| i.unread_badge);
        nav_badge == Some(17) && s.apple_sidebar.read(cx).community_unread_count == 17
    }).await);
    host.key(&mut app, "cmd-b");
    host.draw_at(&mut app, 1440.0, 900.0);
}

/// [Test ID] TEST-CM-T3-22
/// [Feature / Req] F15 x F18 / ORIGINAL_REQUEST.md (Centralized CJK IME `shell_input_ime.rs` + Rich `ComposerModal` 80ms fence & Escape)
/// [Preconditions] Headless GPUI `AppShell` with mounted `CommunityWorkspace` and open `ComposerModal`.
/// [Opaque-Box Steps] Compose CJK title & Markdown body via `update_ime_marked_text` / `commit_ime_unmark_text`; verify Escape cancels preedit only; verify <80ms Enter does not submit; wait >80ms and submit via Enter.
/// [Observable Assertions] Progressive Escape keeps Composer open during preedit, 80ms trailing-Enter fence absorbs premature submit, and final UTF-8 CJK post persists cleanly.
#[test]
fn test_cm_t3_22_f15_f18_centralized_cjk_ime_and_rich_composer_modal() {
    run_community_gpui_test("test_cm_t3_22_f15_f18_centralized_cjk_ime_and_rich_composer_modal");
}

async fn body_t3_22(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 9322, "t3_22", true).await;
    let shell = host.shell.clone();
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.set_composer_tab(ComposerFormatTab::Article, cx);
        w.state.focus_input(ActiveCommunityInput::ComposerTitle);
    });
    assert!(shell.read_with(&app, |s, cx| s.is_community_input_active(cx)).unwrap());

    // 1. Progressive Escape during active IME preedit cancels preedit ONLY (modal stays open!)
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("临时拼音", cx);
        assert!(s.has_active_ime_preedit(cx));
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.has_active_ime_preedit(cx));
    });
    assert_eq!(cw.read_with(&app, |w, _| w.state.active_modal).unwrap(), Some(CommunityModal::Composer));
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    // 2. Commit CJK title & verify 80ms trailing-Enter fence blocks immediate submit
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("并发架构深研", cx);
        s.commit_ime_unmark_text(cx);
        assert_eq!(s.active_text_and_preedit(cx).0, "并发架构深研");
        assert!(s.handle_non_char_key("enter", false, cx));
    });
    assert_eq!(cw.read_with(&app, |w, _| w.state.active_modal).unwrap(), Some(CommunityModal::Composer), "80ms IME fence must block premature Composer submit");

    // 3. Compose CJK Markdown body via IME commit
    let _ = cw.update(&mut app, |w, cx| { w.state.focus_input(ActiveCommunityInput::ComposerBody); cx.notify(); });
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("## 零拷贝社区渲染引擎", cx);
        s.commit_ime_unmark_text(cx);
    });
    assert_eq!(cw.read_with(&app, |w, _| w.state.composer.body.clone()).unwrap(), "## 零拷贝社区渲染引擎");
    host.draw(&mut app);

    // 4. Wait past 80ms fence, focus title, and press Enter to submit Composer cleanly
    app.background_executor().timer(Duration::from_millis(95)).await;
    let _ = cw.update(&mut app, |w, cx| { w.state.focus_input(ActiveCommunityInput::ComposerTitle); cx.notify(); });
    let _ = shell.update(&mut app, |s, cx| { assert!(s.handle_non_char_key("enter", false, cx)); });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let st = &cw.read(cx).state;
        !st.composer.submitting
            && st.active_modal.is_none()
            && st.feed_items.iter().any(|i| {
                i.post.title.as_deref() == Some("并发架构深研")
                    && i.post.body.contains("零拷贝社区渲染引擎")
                    && i.post.format == PostFormat::Article
            })
    }).await, "UTF-8 CJK Article submitted from ComposerModal must persist cleanly");
}

/// [Test ID] TEST-CM-T3-23
/// [Feature / Req] F15 x F20 / ORIGINAL_REQUEST.md (Actor Profile -> Telegram DM Bridge `OpenDmWithActor` + PostCard -> `SharePostToChat`)
/// [Preconditions] Headless GPUI `AppShell` with mounted `CommunityWorkspace` and seeded peer dialog `900001` (`alex_chen`).
/// [Opaque-Box Steps] Open `@alex_chen` profile and trigger `OpenDmWithActor { linked_user_id: 900001 }`; return to Community and trigger `SharePostToChat` on first feed post.
/// [Observable Assertions] DM bridge switches `AppShell` to `ActiveWorkspacePanel::Chats`; Share bridge opens `ModalDialogKind::ForwardMessages` with exact post summary snippet.
#[test]
fn test_cm_t3_23_f15_f20_actor_profile_dm_bridge_and_post_card_share_to_chat() {
    run_community_gpui_test("test_cm_t3_23_f15_f20_actor_profile_dm_bridge_and_post_card_share_to_chat");
}

async fn body_t3_23(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 9323, "t3_23", true).await;
    let shell = host.shell.clone();
    let cw = cw.unwrap();

    // Seed Telegram peer dialog matching alex_chen (u:900001)
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, _| {
            sb.dialogs.push(DialogItemModel {
                id: 900001,
                peer_name: "Alex Chen".into(),
                avatar_initials: "AC".into(),
                avatar_color: 0x2AABEE,
                last_message: "Ready to chat".into(),
                timestamp: "10:00".into(),
                unread_count: 0,
                is_muted: false,
                is_pinned: false,
                is_online: true,
                typing_indicator: None,
                delivery_status: DialogDeliveryStatus::Read,
                folder: "All".into(),
            });
        });
    });

    // 1. Open ActorProfileView for @alex_chen and fire OpenDmWithActor bridge
    let _ = cw.update(&mut app, |w, cx| w.open_actor_profile("alex_chen".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.actor_profile.as_ref().is_some_and(|p| p.actor.handle == "alex_chen")
    }).await);
    let linked_uid = cw.read_with(&app, |w, _| w.state.actor_profile.as_ref().unwrap().actor.linked_user_id.unwrap()).unwrap();
    assert_eq!(linked_uid, 900001);
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| {
        w.emit_action(CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: linked_uid }, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| {
        shell.read(cx).active_panel == ActiveWorkspacePanel::Chats
    }).await, "OpenDmWithActor must switch AppShell to Telegram Chats");

    // 2. Return to Community and fire PostCard SharePostToChat bridge
    let _ = shell.update(&mut app, |s, cx| { s.open_community_workspace(cx); });
    let (pid, snippet) = cw.read_with(&app, |w, _| {
        let p = &w.state.feed_items[0].post;
        (p.post_id.clone(), format!("Check out post {} in c/rust", p.post_id))
    }).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.emit_action(CommunityWorkspaceAction::SharePostToChat { post_id: pid.clone(), summary: snippet.clone() }, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| {
        let s = shell.read(cx);
        s.community_pending_share.as_ref().is_some_and(|(id, _)| id == &pid)
            && matches!(
                s.modal_container.read(cx).dialog.kind,
                Some(ModalDialogKind::ForwardMessages { ref message_ids, ref preview_snippet })
                    if message_ids.is_empty() && preview_snippet == &snippet
            )
    }).await, "SharePostToChat must open ForwardMessages modal with exact post summary");
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T3-24
/// [Feature / Req] F15 x F16 / ORIGINAL_REQUEST.md (PostCard Media Mosaic -> `OpenMediaLightbox` + `Cmd+D`/`Cmd+L` sync + Escape stays in Community)
/// [Preconditions] Headless GPUI `AppShell` with mounted `CommunityWorkspace` and seeded 4-photo post `seed_post_11`.
/// [Opaque-Box Steps] Open `OpenMediaLightbox` from `seed_post_11`'s media; cycle theme (`Cmd+D`) & locale (`Cmd+L`); press Escape once, then press Escape a second time.
/// [Observable Assertions] Lightbox opens, theme & locale sync cleanly into `CommunityWorkspace`, first Escape closes lightbox while staying in Community, and second Escape returns to Chats.
#[test]
fn test_cm_t3_24_f15_f16_post_card_media_lightbox_theme_locale_and_escape_stay() {
    run_community_gpui_test("test_cm_t3_24_f15_f16_post_card_media_lightbox_theme_locale_and_escape_stay");
}

async fn body_t3_24(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 9324, "t3_24", true).await;
    let shell = host.shell.clone();
    let cw = cw.unwrap();

    let media_post = host.client.community().block_on(host.client.community().get_post("seed_post_11", None)).unwrap().post;
    assert!(!media_post.media.is_empty(), "seed_post_11 must have media attachments");
    let media_ref = media_post.media[0].media_id.clone();

    let _ = cw.update(&mut app, |w, cx| w.emit_action(CommunityWorkspaceAction::OpenMediaLightbox(media_ref), cx));
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| {
        shell.read(cx).overlays.media_viewer_overlay.is_some()
    }).await, "OpenMediaLightbox must open AppShell media_viewer_overlay");

    let mode0 = cw.read_with(&app, |w, _| w.palette.mode).unwrap();
    let lang0 = cw.read_with(&app, |w, _| w.language_id.clone()).unwrap();
    host.key(&mut app, "cmd-d");
    host.key(&mut app, "cmd-l");
    host.draw(&mut app);
    assert_ne!(cw.read_with(&app, |w, _| w.palette.mode).unwrap(), mode0, "Cmd+D must cycle theme while lightbox is active");
    assert_ne!(cw.read_with(&app, |w, _| w.language_id.clone()).unwrap(), lang0, "Cmd+L must switch locale while lightbox is active");

    // First Escape closes media lightbox while staying in Community
    host.key(&mut app, "escape");
    shell.read_with(&app, |s, _| {
        assert!(s.overlays.media_viewer_overlay.is_none(), "first Escape must close lightbox");
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community, "must remain in Community after closing lightbox");
    }).unwrap();

    // Second Escape exits Community back to Chats
    host.key(&mut app, "escape");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats);
}

/// [Test ID] TEST-CM-T3-25
/// [Feature / Req] F16 x F17 / ORIGINAL_REQUEST.md (Identity Switcher persistence Feed -> Detail -> Inline Reply + Summon second Bot)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with an owned Bot registered and seeded debate post `seed_post_01`.
/// [Opaque-Box Steps] Switch active Act-As identity to owned Bot on Feed; open `PostDetailView`; submit nested inline reply (authored as owned Bot); summon second Bot `b:910001`.
/// [Observable Assertions] Active Bot identity persists across Feed -> Detail navigation, inline reply authors as owned Bot, and summoned second Bot appends under the same nested branch.
#[test]
fn test_cm_t3_25_f16_f17_identity_switcher_persistence_feed_to_detail_reply_and_summon() {
    run_community_gpui_test("test_cm_t3_25_f16_f17_identity_switcher_persistence_feed_to_detail_reply_and_summon");
}

async fn body_t3_25(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 9325, "t3_25", true).await;
    let cw = cw.unwrap();

    // Register owned Bot
    let _ = cw.update(&mut app, |w, cx| {
        w.state.bot_studio_name = "Persist Identity Bot".into();
        w.state.bot_studio_handle = "persist_t3_25_bot".into();
        w.state.bot_studio_autonomy = BotAutonomyMode::Manual;
        w.submit_register_bot(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.identities.as_ref().is_some_and(|i| i.bot_actors.iter().any(|b| b.handle == "persist_t3_25_bot"))
    }).await);
    let owned_bot_id = cw.read_with(&app, |w, _| {
        w.state.identities.as_ref().unwrap().bot_actors.iter().find(|b| b.handle == "persist_t3_25_bot").unwrap().actor_id.clone()
    }).unwrap();

    // Switch Act-As identity in Feed and navigate to PostDetailView
    let _ = cw.update(&mut app, |w, cx| w.switch_active_actor(owned_bot_id.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.feed_loading).await);

    let _ = cw.update(&mut app, |w, cx| w.open_post_detail("seed_post_01".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == "seed_post_01" && !d.comments.is_empty())
    }).await);
    assert_eq!(cw.read_with(&app, |w, _| w.state.active_actor_id.clone()).unwrap(), owned_bot_id, "Act-As Bot identity must persist across Feed -> Detail");

    let root_cid = cw.read_with(&app, |w, _| w.state.detail_post.as_ref().unwrap().comments[0].comment_id.clone()).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.state.detail_reply_parent_id = Some(root_cid.clone());
        w.state.detail_reply_draft = "Owned Bot nested reply in T3-25".into();
        w.submit_detail_reply(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.find_comment(&root_cid).is_some_and(|c| {
            c.children.iter().any(|ch| ch.author_actor_id == owned_bot_id && ch.body.contains("T3-25"))
        })
    }).await);

    // Summon a second Bot citizen (b:910001) on the same nested branch
    let _ = cw.update(&mut app, |w, cx| {
        w.summon_bot_with_options("seed_post_01".into(), Some("b:910001".into()), Some("Evaluate T3-25 thread".into()), Some(root_cid.clone()), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let st = &cw.read(cx).state;
        !st.detail_summon_pending
            && st.find_comment(&root_cid).is_some_and(|c| {
                c.children.iter().any(|ch| ch.author_actor_id == owned_bot_id)
                    && c.children.iter().any(|ch| ch.author_actor_id == "b:910001")
            })
    }).await, "both owned Bot reply and summoned second Bot reply must appear in nested tree");
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T3-26
/// [Feature / Req] F16 x F21 / ORIGINAL_REQUEST.md (Right Rail Trending click -> `ExploreSearchView` pre-populated filtered results)
/// [Preconditions] Headless GPUI `CommunityWorkspace` rendered in 3-column mode (`1440x900`) with populated `state.trending`.
/// [Opaque-Box Steps] Select a trending topic from Right Rail data, execute `open_explore_with_query(slug_or_tag)`, await search results, and render.
/// [Observable Assertions] Route transitions to `Explore { initial_query: Some(tag) }`, `explore_search_draft` matches `tag`, and filtered matching results populate cleanly.
#[test]
fn test_cm_t3_26_f16_f21_right_rail_trending_click_to_explore_filtered_results() {
    run_community_gpui_test("test_cm_t3_26_f16_f21_right_rail_trending_click_to_explore_filtered_results");
}

async fn body_t3_26(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 9326, "t3_26", true).await;
    let cw = cw.unwrap();
    host.draw_at(&mut app, 1440.0, 900.0);
    assert_eq!(cw.read_with(&app, |w, _| w.last_layout_mode).unwrap(), CommunityLayoutMode::ThreeColumn);

    let (query_tag, comm_slug) = cw.read_with(&app, |w, _| {
        assert!(!w.state.trending.is_empty(), "Right Rail trending must be populated");
        let tag = w.state.trending.iter().find(|t| t.kind != TrendingItemKind::Community).unwrap_or(&w.state.trending[0]).slug_or_tag.clone();
        let comm = w.state.trending.iter().find(|t| t.kind == TrendingItemKind::Community).map(|t| t.slug_or_tag.trim_start_matches("c/").to_string());
        (tag, comm)
    }).unwrap();

    let _ = cw.update(&mut app, |w, cx| w.open_explore_with_query(query_tag.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let st = &cw.read(cx).state;
        !st.search_loading && st.search_result.is_some()
    }).await);
    cw.read_with(&app, |w, _| {
        assert_eq!(w.state.route, CommunityRoute::Explore { initial_query: Some(query_tag.clone()) });
        assert_eq!(w.state.explore_search_draft, query_tag);
        let res = w.state.search_result.as_ref().unwrap();
        assert!(!res.posts.is_empty() || !res.communities.is_empty(), "trending query '{query_tag}' must yield matching Explore results");
    }).unwrap();
    host.draw(&mut app);

    if let Some(slug) = comm_slug {
        let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::CommunityPage(slug.clone()), cx));
        assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.community_detail.as_ref().is_some_and(|d| d.community.slug == slug)
        }).await);
        host.draw(&mut app);
    }
}

/// [Test ID] TEST-CM-T3-27
/// [Feature / Req] F18 x F19 / ORIGINAL_REQUEST.md (Create Community Modal -> Flair Setup -> Composer Publishing in New `c/<slug>` & Flair Filter)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped in isolated `TempDir`.
/// [Opaque-Box Steps] Create `c/gpui_rust` via Create Community modal, attach custom flairs (`Architecture`, `Benchmark`), open `ComposerModal` targeting `c/gpui_rust` + `Architecture`, publish, and apply flair filter.
/// [Observable Assertions] Modal navigates to `c/gpui_rust`, custom flairs load, Composer publishes with flair, and flair filter strictly isolates `"Architecture"` post from `"Benchmark"`.
#[test]
fn test_cm_t3_27_f18_f19_create_community_flair_setup_composer_and_flair_filter() {
    run_community_gpui_test("test_cm_t3_27_f18_f19_create_community_flair_setup_composer_and_flair_filter");
}

async fn body_t3_27(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 9327, "t3_27", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "gpui_rust".into();
        w.state.create_community.name = "GPUI Rust Hub".into();
        w.state.create_community.description = "Rust + GPUI engineering community".into();
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.route == CommunityRoute::CommunityPage("gpui_rust".into())
            && cw.read(cx).state.community_detail.as_ref().is_some_and(|d| d.community.slug == "gpui_rust")
    }).await);

    // Configure custom post flairs on newly created c/gpui_rust and reload page
    let flairs = vec![
        CommunityFlairDto { id: "fl_arch".into(), text: "Architecture".into(), bg_color: "#2563EB".into(), text_color: "#FFFFFF".into(), is_mod_only: false },
        CommunityFlairDto { id: "fl_bench".into(), text: "Benchmark".into(), bg_color: "#059669".into(), text_color: "#FFFFFF".into(), is_mod_only: false },
    ];
    let _ = host.client.community().block_on(host.client.community().update_community(
        "gpui_rust",
        &UpdateCommunityRequest { post_flairs: Some(flairs), ..Default::default() },
    )).unwrap();
    let _ = cw.update(&mut app, |w, cx| w.load_community_page("gpui_rust".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.community_detail.as_ref().is_some_and(|d| d.community.post_flairs.len() == 2)
    }).await);

    // Open ComposerModal targeting c/gpui_rust + Architecture flair and publish
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("gpui_rust".into()), cx);
        w.state.composer.flair = Some("Architecture".into());
        w.state.composer.title = "GPUI Layout RFC".into();
        w.state.composer.body = "Three-column adaptive community workspace design".into();
        w.submit_composer(PostStatus::Published, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.composer.submitting && cw.read(cx).state.active_modal.is_none()
    }).await);

    // Publish second post with different flair to prove flair filter discriminates cleanly
    let _ = host.client.community().block_on(host.client.community().create_post(&CreatePostRequest {
        community_slug: Some("gpui_rust".into()),
        flair: Some("Benchmark".into()),
        body: "Frame latency benchmark numbers".into(),
        ..Default::default()
    })).unwrap();

    let _ = cw.update(&mut app, |w, cx| w.set_community_flair_filter("gpui_rust".into(), Some("Architecture".into()), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let posts = &cw.read(cx).state.community_posts;
        !posts.is_empty()
            && posts.iter().all(|i| i.post.flair.as_deref() == Some("Architecture"))
            && posts.iter().any(|i| i.post.title.as_deref() == Some("GPUI Layout RFC"))
    }).await, "c/gpui_rust flair filter must show only Architecture posts including Composer submission");
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T3-28
/// [Feature / Req] F19 x F21 / ORIGINAL_REQUEST.md (Community AutoMod Rule + Composer Flagged Post + Mod Queue + Public Mod Log Round-Trip)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped in isolated `TempDir`.
/// [Opaque-Box Steps] Create `c/mod_trip_t3`, configure AutoMod keyword rule in `ModQueueView`, publish a matching post via `ComposerModal`, remove it in `ModQueueView`, and inspect `ModLog`.
/// [Observable Assertions] Flagged post enters `mod_queue` with `source == "automod"`, moderator decision resolves it, and audit record persists in `c/mod_trip_t3` `mod_logs`.
#[test]
fn test_cm_t3_28_f19_f21_automod_rule_mod_queue_and_public_mod_log_roundtrip() {
    run_community_gpui_test("test_cm_t3_28_f19_f21_automod_rule_mod_queue_and_public_mod_log_roundtrip");
}

async fn body_t3_28(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 9328, "t3_28", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "mod_trip_t3".into();
        w.state.create_community.name = "Mod Trip T3".into();
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| cw.read(cx).state.route == CommunityRoute::CommunityPage("mod_trip_t3".into())).await);

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::ModQueue(Some("mod_trip_t3".into())), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.mod_loading && cw.read(cx).state.mod_community.is_some()).await);

    let _ = cw.update(&mut app, |w, cx| {
        w.set_mod_tab(ModConsoleTab::AutoModRules, cx);
        w.state.automod_draft.name = "SpamGuardT328".into();
        w.state.automod_draft.keywords = "spam_t328_trigger".into();
        w.state.automod_draft.action = AutoModActionKind::FlagForReview;
        w.submit_add_automod_rule(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.automod_draft.saving && cw.read(cx).state.mod_community.as_ref().is_some_and(|c| !c.automod_rules.is_empty())
    }).await);

    // Publish a flagged post via ComposerModal targeting c/mod_trip_t3
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("mod_trip_t3".into()), cx);
        w.state.composer.body = "Flagged post carrying spam_t328_trigger payload".into();
        w.submit_composer(PostStatus::Published, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.composer.submitting && cw.read(cx).state.active_modal.is_none()).await);

    let _ = cw.update(&mut app, |w, cx| {
        w.set_mod_tab(ModConsoleTab::Queue, cx);
        w.load_mod_queue("mod_trip_t3".into(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.mod_loading && cw.read(cx).state.mod_queue.iter().any(|q| q.source == "automod")
    }).await, "AutoMod-flagged Composer post must enter mod_queue");

    let (qid, target_pid) = cw.read_with(&app, |w, _| {
        let it = w.state.mod_queue.iter().find(|q| q.source == "automod").unwrap();
        (it.item_id.clone(), it.target_id.clone())
    }).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.state.mod_reason_draft = "Removed in T3-28 round-trip".into();
        w.execute_mod_console_action("mod_trip_t3".into(), ModActionKind::Remove, "post".into(), target_pid.clone(), Some(qid), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.mod_logs.iter().any(|l| l.target_id == target_pid && l.action == ModActionKind::Remove)
    }).await, "Mod action must record transparent audit entry in mod_logs");
    let _ = cw.update(&mut app, |w, cx| w.set_mod_tab(ModConsoleTab::ModLog, cx));
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T3-29
/// [Feature / Req] F20 x F16 / ORIGINAL_REQUEST.md (Custom Feed / List Creation -> `Lists` Feed Tab -> Custom Bookmark Folder)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with seeded Humans, Bots, and communities.
/// [Opaque-Box Steps] Create a List with 1 Human (`u:900001`), 1 Bot (`b:910001`), and `c/rust`; open its `FeedTab::List` stream; bookmark a post from that stream into `"T329_Reading"` folder.
/// [Observable Assertions] List feed strictly isolates posts matching list members/communities, and bookmarked post appears inside `"T329_Reading"` in `BookmarksListsView`.
#[test]
fn test_cm_t3_29_f20_f16_custom_list_creation_feed_tab_and_bookmark_folder() {
    run_community_gpui_test("test_cm_t3_29_f20_f16_custom_list_creation_feed_tab_and_bookmark_folder");
}

async fn body_t3_29(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 9329, "t3_29", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
        w.open_create_list_form(cx);
        w.state.list_editor.name = "T3-29 Curated Trio".into();
        w.state.list_editor.description = "1 Human + 1 Bot + c/rust".into();
        w.state.list_editor.member_actor_ids = vec!["u:900001".into(), "b:910001".into()];
        w.state.list_editor.community_slugs = vec!["rust".into()];
        w.submit_list_editor(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.lists.iter().any(|l| l.name == "T3-29 Curated Trio")
    }).await);
    let list_id = cw.read_with(&app, |w, _| {
        w.state.lists.iter().find(|l| l.name == "T3-29 Curated Trio").unwrap().list_id.clone()
    }).unwrap();

    // Switch main Feed to FeedTab::List(list_id) and verify strict scoping
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.select_feed_tab(FeedTab::List(list_id.clone()), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.feed_loading && !cw.read(cx).state.feed_items.is_empty()
    }).await);
    let scoped_pid = cw.read_with(&app, |w, _| {
        for item in &w.state.feed_items {
            let p: &CommunityPostDto = &item.post;
            let in_scope = p.author_actor_id == "u:900001"
                || p.author_actor_id == "b:910001"
                || p.community_slug.as_deref() == Some("rust");
            assert!(in_scope, "out-of-scope post {} leaked into List feed", p.post_id);
        }
        let first = &w.state.feed_items[0].post;
        (first.post_id.clone(), first.bookmarks_count)
    }).unwrap();
    let (scoped_pid, before_bm_count) = scoped_pid;
    host.draw(&mut app);

    // Bookmark scoped post into custom folder and verify in BookmarksListsView
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Bookmarks, cx);
        w.state.bookmark_new_folder_draft = "T329_Reading".into();
        w.create_bookmark_folder(cx);
        w.move_bookmark_to_folder(scoped_pid.clone(), "T329_Reading".into(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let w = cw.read(cx);
        compute_bookmark_folders(w).iter().any(|f| f == "T329_Reading")
            && w.state.find_post(&scoped_pid).is_some_and(|p| {
                p.viewer_bookmarked
                    && p.viewer_bookmark_folder.as_deref() == Some("T329_Reading")
                    && p.bookmarks_count > before_bm_count
            })
    }).await);
    let _ = cw.update(&mut app, |w, cx| {
        w.select_bookmark_folder("T329_Reading".into(), cx);
        w.load_saved_posts(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.saved_items.iter().any(|it| it.post.post_id == scoped_pid && it.post.viewer_bookmark_folder.as_deref() == Some("T329_Reading"))
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T3-30
/// [Feature / Req] F21 x F16 / ORIGINAL_REQUEST.md (Bot Studio creation -> immediate Act-As switcher presence & live test post in Feed)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped in isolated `TempDir`.
/// [Opaque-Box Steps] Register `@pioneer_t3_30_bot` with custom Grok avatar & persona in `BotStudioView`, run live test post in `c/rust`, open Feed Act-As switcher, switch to Bot, and inspect `c/rust` feed.
/// [Observable Assertions] New Bot persists custom Grok avatar, appears immediately as an owned Act-As identity in the Feed switcher, and its live test post renders in `c/rust` authored as `ActorKind::Bot`.
#[test]
fn test_cm_t3_30_f21_f16_bot_studio_creation_act_as_switcher_and_live_test_post() {
    run_community_gpui_test("test_cm_t3_30_f21_f16_bot_studio_creation_act_as_switcher_and_live_test_post");
}

async fn body_t3_30(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 9330, "t3_30", true).await;
    let cw = cw.unwrap();
    let (exp_shape, exp_color) = resolve_grok_avatar_pair(5, 6);

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BotStudio, cx);
        w.set_bot_studio_avatar_indices(5, 6, cx);
        w.state.bot_studio_name = "Studio Pioneer Bot".into();
        w.state.bot_studio_handle = "pioneer_t3_30_bot".into();
        w.state.bot_studio_persona = "Pairwise testing specialist".into();
        w.state.bot_studio_autonomy = BotAutonomyMode::Reactive;
        w.submit_register_bot(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.identities.as_ref().is_some_and(|i| i.bot_actors.iter().any(|b| b.handle == "pioneer_t3_30_bot"))
    }).await);
    let bot_id = cw.read_with(&app, |w, _| {
        let b = w.state.identities.as_ref().unwrap().bot_actors.iter().find(|b| b.handle == "pioneer_t3_30_bot").unwrap();
        assert_eq!(b.avatar_shape, exp_shape);
        assert_eq!(b.avatar_color, exp_color);
        b.actor_id.clone()
    }).unwrap();

    // Run live test post from Bot Studio into c/rust
    let _ = cw.update(&mut app, |w, cx| {
        w.state.bot_studio_test_community = Some("rust".into());
        w.state.bot_studio_test_content = "Live playground post from Pioneer T3-30!".into();
        w.run_bot_studio_test_post(bot_id.clone(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.bot_studio_test_submitting && cw.read(cx).state.bot_studio_test_last_post_id.is_some()
    }).await);
    let test_pid = cw.read_with(&app, |w, _| w.state.bot_studio_test_last_post_id.clone().unwrap()).unwrap();

    // Navigate back to Feed -> verify Bot is immediately in Act-As Switcher and live test post appears in c/rust feed
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.toggle_popover(CommunityPopover::IdentitySwitcher, cx);
        assert!(w.is_viewer_owned_actor(&bot_id), "new Bot must be immediately selectable as owned Act-As identity");
        assert!(w.state.identities.as_ref().unwrap().all_actors.iter().any(|a| a.actor_id == bot_id));
    });
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(bot_id.clone(), cx);
        w.select_feed_tab(FeedTab::Community("rust".into()), cx);
        w.set_feed_sort(FeedSort::New, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let st = &cw.read(cx).state;
        !st.feed_loading
            && st.active_actor_id == bot_id
            && st.feed_items.iter().any(|it| {
                it.post.post_id == test_pid
                    && it.post.author_actor_id == bot_id
                    && it.post.author_kind == ActorKind::Bot
            })
    }).await, "Bot Studio live test post must appear in Feed authored by newly registered Bot");
    host.draw(&mut app);
}
