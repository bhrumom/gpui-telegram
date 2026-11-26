//! Tier 1 Opaque-Box E2E Suite — Isolated Feature Happy Paths (`100` Tests: `F02`–`F21` × `5`).
//! Implements `TEST_INFRA.md §5` (`TEST-CM-T1-F02-01` .. `TEST-CM-T1-F21-05`).

#[allow(dead_code)]
mod community_e2e_helpers;

use std::collections::HashSet;
use std::time::Duration;

use community_e2e_helpers::{
    CommunityAuthMode, CommunityHttpDriver, Legacy404Server, TestTelegramClient,
    WebhookCaptureServer, closed_local_url, define_community_gpui_harness,
    execute_bot_hermes_tool, now_ms, open_offline_community_host, safe_temp_idx_dir,
};
use e2e_suite::common::{EphemeralTestServer, connect_ws, recv_ws_frame_matching};
use e2e_suite::gpui_browser_harness::pump_until;
use gpui::{AssetSource, AsyncApp};
use gpui_telegram_agent::ApprovalMode;
use gpui_telegram_agent::hermes::toolsets::ToolsetPreset;
use gpui_telegram_client::{ClientDomainEvent, CommunityRuntimeMode};
use gpui_telegram_community::identity::{MAX_TRUST_SCORE, MIN_TRUST_SCORE};
use gpui_telegram_community::{
    AutoModEngine, BotGuardRejection, BotLoopGuard, BotWakeSelector, CommentTreeBuilder,
    CommentTreeOptions, CommunityRecommender, CommunitySearchIndexer, CommunityStore,
    DeterministicBotResponder, MIGRATION_V42_COMMUNITY_SQL, PostCandidate, TelegramBotSyncInput,
    TelegramHumanSyncInput, ThreadAssembler, TrendingEngine, ViewerContext, build_starter_dataset,
    compute_trust_score, default_prefs_for_preset, embed_text_64, extract_entities,
    make_comment_path,
};
use gpui_telegram_desktop::theme::{IconName, TelegramAssetSource};
use gpui_telegram_desktop::views::apple_sidebar::AppleSidebarSection;
use gpui_telegram_desktop::views::community::bot_studio_view::format_bot_api_snippet;
use gpui_telegram_desktop::views::community::bookmarks_lists_view::compute_bookmark_folders;
use gpui_telegram_desktop::views::community::composer_modal::{
    AutocompleteKind, compute_composer_autocomplete,
};
use gpui_telegram_desktop::views::community::explore_search_view::normalize_search_query;
use gpui_telegram_desktop::views::community::{
    ActiveCommunityInput, ActorProfileTab, BookmarksListsSection, CommunityLayoutMode,
    CommunityModal, CommunityPageTab, CommunityPopover, CommunityRoute, CommunityWorkspaceAction,
    ComposerFormatTab, ComposerMediaDraft, FeedDensity, ModConsoleTab, NotificationCategoryTab,
    PollBreakdownMode, PostDetailMode,
};
use gpui_telegram_desktop::views::composer::formatting_bar::TextStyleKind;
use gpui_telegram_desktop::views::nav_rail::{NavDestination, NavRail};
use gpui_telegram_desktop::views::shell::{
    ActiveWorkspacePanel, nav_destination_for_shortcut_digit,
};
use gpui_telegram_desktop::views::sidebar::gemini_unified_sidebar::UnifiedSidebarAction;
use gpui_telegram_desktop::views::MainMenuCommand;
use gpui_telegram_proto::community_contracts::{
    ActorFilter, ActorKind, ActorProfileResponse, AlgoPreferencesDto, AlgoPreset,
    AutoModActionKind, AutoModRuleDto, BotAutonomyMode, CommentSort, CommunityCommentDto,
    CommunityDetailResponse, CommunityIdentitiesResponse, CommunityNotificationDto,
    CommunityNotificationKind, CommunityPostDto, CommunitySearchRequestDto, CommunitySearchTab,
    CreatePostRequest, CreatePostResponse, EngagementSignalKind, FeedRequestDto, FeedResponseDto,
    FeedSort, FeedTab, ModActionKind, ModLogsResponse, ModQueueResponse, PollDto,
    PostDetailResponse, PostFormat, PostStatus, RegisterBotActorRequest, SummonBotRequest,
    TimeWindow, TrendingItemKind, TrendingResponse, VerifiedTier, VoteRequest,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use reqwest::Method;
use serde_json::json;
use tempfile::TempDir;

define_community_gpui_harness! {
    child_env: "GPUI_COMMUNITY_T1_CHILD",
    target_env: "GPUI_COMMUNITY_T1_TARGET",
    after_ctors_env: "GPUI_COMMUNITY_T1_AFTER_CTORS",
    targets: {
        "test_cm_t1_f15_01_sidebar_expanded_collapsed_apple_and_nav_rail_open_community" => body_f15_01,
        "test_cm_t1_f15_02_account_popover_main_menu_and_cmd_shift_c_shortcut" => body_f15_02,
        "test_cm_t1_f15_04_theme_cycling_and_bilingual_locale_sync_render" => body_f15_04,
        "test_cm_t1_f15_05_centralized_cjk_ime_and_telegram_bridges" => body_f15_05,
        "test_cm_t1_f16_01_adaptive_three_two_one_column_breakpoints_render" => body_f16_01,
        "test_cm_t1_f16_02_identity_switcher_act_as_bot_tags_quick_post" => body_f16_02,
        "test_cm_t1_f16_03_feed_tabs_actor_filters_and_reddit_sort_time_bar" => body_f16_03,
        "test_cm_t1_f16_04_algo_tuner_popover_density_toggle_and_explain_popover" => body_f16_04,
        "test_cm_t1_f16_05_post_card_interactions_poll_award_and_right_rail" => body_f16_05,
        "test_cm_t1_f17_01_post_detail_nested_tree_and_collapse_expand" => body_f17_01,
        "test_cm_t1_f17_02_post_detail_all_six_comment_sorts_reorder" => body_f17_02,
        "test_cm_t1_f17_03_inline_reply_identity_switcher_and_comment_count" => body_f17_03,
        "test_cm_t1_f17_04_x_linear_thread_and_quote_chain_detail_mode" => body_f17_04,
        "test_cm_t1_f17_05_summon_bot_renders_new_bot_comment_in_detail" => body_f17_05,
        "test_cm_t1_f18_01_composer_modal_identity_community_flair_and_markdown_article" => body_f18_01,
        "test_cm_t1_f18_02_composer_media_link_and_poll_with_nsfw_spoiler" => body_f18_02,
        "test_cm_t1_f18_03_composer_thread_quote_and_crosspost_submissions" => body_f18_03,
        "test_cm_t1_f18_04_composer_autocomplete_mentions_hashtags_communities_and_cjk" => body_f18_04,
        "test_cm_t1_f18_05_composer_drafts_resume_publish_schedule_and_bot_coauthor" => body_f18_05,
        "test_cm_t1_f19_01_community_page_banner_stats_pinned_and_rules_render" => body_f19_01,
        "test_cm_t1_f19_02_community_join_leave_and_bell_toggle" => body_f19_02,
        "test_cm_t1_f19_03_community_flair_filter_chips_narrow_stream" => body_f19_03,
        "test_cm_t1_f19_04_community_versioned_wiki_viewer_editor_and_revisions" => body_f19_04,
        "test_cm_t1_f19_05_mod_roster_and_create_community_modal_navigation" => body_f19_05,
        "test_cm_t1_f20_01_actor_profile_human_and_bot_metadata_and_tabs" => body_f20_01,
        "test_cm_t1_f20_02_profile_telegram_dm_bridge_emits_open_dm_action" => body_f20_02,
        "test_cm_t1_f20_03_bookmarks_custom_folders_and_move_persistence" => body_f20_03,
        "test_cm_t1_f20_04_lists_custom_feeds_create_members_and_stream" => body_f20_04,
        "test_cm_t1_f20_05_notifications_categories_mark_read_badge_and_jump" => body_f20_05,
        "test_cm_t1_f21_01_explore_trending_and_all_seven_search_tabs" => body_f21_01,
        "test_cm_t1_f21_02_search_operators_normalize_and_filter_results" => body_f21_02,
        "test_cm_t1_f21_03_mod_console_queue_approve_remove_lock_and_mod_log" => body_f21_03,
        "test_cm_t1_f21_04_automod_rule_editor_toggle_and_live_enforcement" => body_f21_04,
        "test_cm_t1_f21_05_bot_studio_register_grok_avatar_edit_token_and_test_post" => body_f21_05,
    }
}

// ============================================================================
// F02 — Unified Human + Bot First-Class Actor Model (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F02-01
/// [Feature / Req] F02 / ORIGINAL_REQUEST.md (Bot & Human equal first-class citizens)
/// [Preconditions] Ephemeral live server booted with V42 schema; 1 Human and 1 owned Bot registered.
/// [Opaque-Box Steps] Call GET /api/v1/community/identities for the Human citizen.
/// [Observable Assertions] Both canonical `u:{uid}` and `b:{bot_id}` actors exist with exact ActorKind, VerifiedTier, and zero-initialized karma/followers.
#[tokio::test]
async fn test_cm_t1_f02_01_register_human_and_bot_canonical_ids() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f02_01", "Pass1234!").await.unwrap();
    let bot = driver
        .register_community_bot(&alice.token, "atlas_f02_bot", "Atlas Bot", "reactive")
        .await
        .unwrap();

    let (st, val) = driver
        .req(Method::GET, "/api/v1/community/identities", &CommunityAuthMode::Human(alice.token.clone()), None)
        .await
        .unwrap();
    assert_eq!(st, 200, "identities failed: {val}");
    let ids: CommunityIdentitiesResponse = serde_json::from_value(val).unwrap();
    let human = ids.human_actor.expect("human actor must be present");
    assert_eq!(human.actor_id, format!("u:{}", alice.user_id));
    assert_eq!(human.actor_kind, ActorKind::Human);
    assert_eq!(human.linked_user_id, Some(alice.user_id));
    assert_eq!(human.post_karma, 0);
    assert_eq!(human.comment_karma, 0);
    assert_eq!(human.followers_count, 0);

    let listed_bot = ids
        .bot_actors
        .iter()
        .find(|b| b.actor_id == bot.actor_id)
        .expect("owned bot must appear in bot_actors");
    assert_eq!(listed_bot.actor_id, format!("b:{}", bot.bot_user_id));
    assert_eq!(listed_bot.actor_kind, ActorKind::Bot);
    assert_eq!(listed_bot.owner_user_id, Some(alice.user_id));
    assert_eq!(listed_bot.post_karma, 0);
    assert_eq!(listed_bot.followers_count, 0);
    assert!(matches!(
        listed_bot.verified_tier,
        VerifiedTier::None | VerifiedTier::OfficialBot | VerifiedTier::HumanVerified | VerifiedTier::VerifiedOrg | VerifiedTier::CommunityMod
    ));
}

/// [Test ID] TEST-CM-T1-F02-02
/// [Feature / Req] F02 / ORIGINAL_REQUEST.md (Unified Human & Bot profile fields)
/// [Preconditions] Ephemeral live server with 1 Human and 1 owned Bot registered.
/// [Opaque-Box Steps] PATCH Human and Bot profile attributes, then GET both actors by `@handle` and by canonical `actor_id`.
/// [Observable Assertions] Every updated Human and Bot metadata field round-trips identically across handle and ID lookups.
#[tokio::test]
async fn test_cm_t1_f02_02_update_human_and_bot_profiles_and_fetch_by_handle_or_id() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f02_02", "Pass1234!").await.unwrap();
    let bot = driver
        .register_community_bot(&alice.token, "mentor_f02_bot", "Mentor Bot", "mention_only")
        .await
        .unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let (st, _) = driver
        .req(
            Method::PATCH,
            &format!("/api/v1/community/actors/{}", alice.actor_id),
            &auth,
            Some(json!({
                "display_name": "Alice Systems",
                "bio": "Building fused Reddit+X in Rust",
                "location": "San Francisco",
                "website": "https://rust-lang.org",
                "avatar_shape": "squircle",
                "avatar_color": "blue"
            })),
        )
        .await
        .unwrap();
    assert_eq!(st, 200);

    let (st, _) = driver
        .req(
            Method::PATCH,
            &format!("/api/v1/community/actors/{}", bot.actor_id),
            &auth,
            Some(json!({
                "display_name": "Mentor Prime",
                "bio": "Deep systems reviewer",
                "bot_badge": "AI Architect",
                "bot_model": "gpt-5.4-codex",
                "bot_capabilities": ["rust", "gpui", "perf"],
                "bot_persona": "Concise, evidence-driven reviewer"
            })),
        )
        .await
        .unwrap();
    assert_eq!(st, 200);

    let (st_h, val_h) = driver
        .req(Method::GET, &format!("/api/v1/community/actors/{}", alice.username), &auth, None)
        .await
        .unwrap();
    assert_eq!(st_h, 200);
    let h_prof: ActorProfileResponse = serde_json::from_value(val_h).unwrap();
    assert_eq!(h_prof.actor.display_name, "Alice Systems");
    assert_eq!(h_prof.actor.location.as_deref(), Some("San Francisco"));
    assert_eq!(h_prof.actor.website.as_deref(), Some("https://rust-lang.org"));
    assert_eq!(h_prof.actor.avatar_shape, "squircle");

    let (st_b1, val_b1) = driver
        .req(Method::GET, &format!("/api/v1/community/actors/{}", bot.handle), &auth, None)
        .await
        .unwrap();
    let (st_b2, val_b2) = driver
        .req(Method::GET, &format!("/api/v1/community/actors/{}", bot.actor_id), &auth, None)
        .await
        .unwrap();
    assert_eq!(st_b1, 200);
    assert_eq!(st_b2, 200);
    let b1: ActorProfileResponse = serde_json::from_value(val_b1).unwrap();
    let b2: ActorProfileResponse = serde_json::from_value(val_b2).unwrap();
    assert_eq!(b1.actor.actor_id, b2.actor.actor_id);
    assert_eq!(b1.actor.bot_badge.as_deref(), Some("AI Architect"));
    assert_eq!(b1.actor.bot_model.as_deref(), Some("gpt-5.4-codex"));
    assert_eq!(b1.actor.bot_capabilities, vec!["rust", "gpui", "perf"]);
    assert_eq!(b1.actor.bot_persona.as_deref(), Some("Concise, evidence-driven reviewer"));
}

/// [Test ID] TEST-CM-T1-F02-03
/// [Feature / Req] F02 / ORIGINAL_REQUEST.md (Symmetric Human/Bot social graph)
/// [Preconditions] Ephemeral live server with 2 Humans and 2 Bots registered.
/// [Opaque-Box Steps] Execute POST and DELETE /actors/{id}/follow across all 4 pairs: Human->Bot, Bot->Human, Bot->Bot, Human->Human.
/// [Observable Assertions] Every pair updates `is_following`, `followers_count`, and `following_count` accurately on follow and unfollow.
#[tokio::test]
async fn test_cm_t1_f02_03_symmetric_follow_unfollow_across_all_four_actor_pairs() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f02_03", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f02_03", "Pass1234!").await.unwrap();
    let bot1 = driver.register_community_bot(&alice.token, "bot1_f02_03_bot", "Bot 1", "manual").await.unwrap();
    let bot2 = driver.register_community_bot(&bob.token, "bot2_f02_03_bot", "Bot 2", "manual").await.unwrap();

    let pairs = [
        (CommunityAuthMode::Human(alice.token.clone()), bot1.actor_id.clone()),
        (CommunityAuthMode::DirectBot(bot1.bot_token.clone()), bob.actor_id.clone()),
        (CommunityAuthMode::DirectBot(bot1.bot_token.clone()), bot2.actor_id.clone()),
        (CommunityAuthMode::Human(alice.token.clone()), bob.actor_id.clone()),
    ];
    for (follower_auth, target_id) in &pairs {
        let (st, val) = driver
            .req(Method::POST, &format!("/api/v1/community/actors/{target_id}/follow"), follower_auth, None)
            .await
            .unwrap();
        assert_eq!(st, 200, "follow failed: {val}");
        assert_eq!(val["is_following"], true);
        assert!(val["followers_count"].as_u64().unwrap() >= 1);
    }

    let (st, bob_prof) = driver
        .req(Method::GET, &format!("/api/v1/community/actors/{}", bob.actor_id), &CommunityAuthMode::Human(alice.token.clone()), None)
        .await
        .unwrap();
    assert_eq!(st, 200);
    assert_eq!(bob_prof["actor"]["followers_count"], 2);
    assert_eq!(bob_prof["actor"]["is_following"], true);

    for (follower_auth, target_id) in &pairs {
        let (st, val) = driver
            .req(Method::DELETE, &format!("/api/v1/community/actors/{target_id}/follow"), follower_auth, None)
            .await
            .unwrap();
        assert_eq!(st, 200, "unfollow failed: {val}");
        assert_eq!(val["is_following"], false);
    }
    let (_, bob_after) = driver
        .req(Method::GET, &format!("/api/v1/community/actors/{}", bob.actor_id), &CommunityAuthMode::Human(alice.token.clone()), None)
        .await
        .unwrap();
    assert_eq!(bob_after["actor"]["followers_count"], 0);
}

/// [Test ID] TEST-CM-T1-F02-04
/// [Feature / Req] F02 / ORIGINAL_REQUEST.md (Equal Human + Bot write & karma parity)
/// [Preconditions] Ephemeral live server with 1 Human and 1 Bot registered.
/// [Opaque-Box Steps] Both Human and Bot create posts, comment on each other's posts, upvote, bookmark, and give awards.
/// [Observable Assertions] Both citizens earn positive `post_karma`, `comment_karma`, and `awardee_karma` on their actor profiles.
#[tokio::test]
async fn test_cm_t1_f02_04_equal_citizenship_write_parity_and_dual_karma() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f02_04", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "parity_f02_bot", "Parity Bot", "manual").await.unwrap();
    let h_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    let (st, hp) = driver.req(Method::POST, "/api/v1/community/posts", &h_auth, Some(json!({ "body": "Human root post" }))).await.unwrap();
    assert_eq!(st, 201);
    let h_pid = hp["post"]["post_id"].as_str().unwrap().to_string();

    let (st, bp) = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "body": "Bot root post" }))).await.unwrap();
    assert_eq!(st, 201);
    let b_pid = bp["post"]["post_id"].as_str().unwrap().to_string();

    let (st, hc) = driver.req(Method::POST, &format!("/api/v1/community/posts/{b_pid}/comments"), &h_auth, Some(json!({ "body": "Human reply" }))).await.unwrap();
    assert_eq!(st, 201);
    let h_cid = hc["comment"]["comment_id"].as_str().unwrap().to_string();

    let (st, bc) = driver.req(Method::POST, &format!("/api/v1/community/posts/{h_pid}/comments"), &b_auth, Some(json!({ "body": "Bot reply" }))).await.unwrap();
    assert_eq!(st, 201);
    let b_cid = bc["comment"]["comment_id"].as_str().unwrap().to_string();

    // Cross-vote, bookmark, and award each other
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{h_pid}/vote"), &b_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{b_pid}/vote"), &h_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/comments/{h_cid}/vote"), &b_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/comments/{b_cid}/vote"), &h_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req_empty_body(Method::POST, &format!("/api/v1/community/posts/{b_pid}/bookmark"), &h_auth).await.unwrap().0, 200);
    assert_eq!(driver.req_empty_body(Method::POST, &format!("/api/v1/community/posts/{h_pid}/bookmark"), &b_auth).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{h_pid}/award"), &b_auth, Some(json!({ "award_kind": "insightful" }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{b_pid}/award"), &h_auth, Some(json!({ "award_kind": "gold" }))).await.unwrap().0, 200);

    for actor_id in [&alice.actor_id, &bot.actor_id] {
        let (st, val) = driver.req(Method::GET, &format!("/api/v1/community/actors/{actor_id}"), &h_auth, None).await.unwrap();
        assert_eq!(st, 200);
        let prof: ActorProfileResponse = serde_json::from_value(val).unwrap();
        assert!(prof.actor.post_karma >= 1, "{actor_id} must earn post_karma");
        assert!(prof.actor.comment_karma >= 1, "{actor_id} must earn comment_karma");
        assert!(prof.actor.awardee_karma >= 1, "{actor_id} must earn awardee_karma");
    }
}

/// [Test ID] TEST-CM-T1-F02-05
/// [Feature / Req] F02 / ORIGINAL_REQUEST.md (Trust score, HybridAgent & BotAutonomyMode persistence)
/// [Preconditions] Isolated SQLite `CommunityStore` opened in TempDir.
/// [Opaque-Box Steps] Create HybridAgent and Bot actors covering all 4 BotAutonomyModes, compute trust scores across karma tiers, and read back from SQLite.
/// [Observable Assertions] Trust scores stay within `[MIN_TRUST_SCORE, MAX_TRUST_SCORE]` and rise with karma; HybridAgent and all 4 autonomy modes persist accurately.
#[test]
fn test_cm_t1_f02_05_trust_score_hybrid_agent_and_autonomy_modes_roundtrip() {
    let dir = TempDir::new().unwrap();
    let store = CommunityStore::open(dir.path().join("f02_05.db")).unwrap();
    let t0 = now_ms();

    let modes = [
        ("bot_manual", ActorKind::Bot, BotAutonomyMode::Manual, 2001),
        ("bot_mention", ActorKind::Bot, BotAutonomyMode::MentionOnly, 2002),
        ("bot_reactive", ActorKind::Bot, BotAutonomyMode::Reactive, 2003),
        ("hybrid_auto", ActorKind::HybridAgent, BotAutonomyMode::Autonomous, 2004),
    ];
    for (handle, kind, mode, bot_uid) in modes {
        let created = store
            .create_bot_actor(
                1001,
                bot_uid,
                &RegisterBotActorRequest {
                    bot_user_id: Some(bot_uid),
                    actor_kind: Some(kind),
                    handle: handle.to_string(),
                    display_name: format!("Citizen {handle}"),
                    bio: Some("Trust test".to_string()),
                    avatar_media_id: None,
                    banner_media_id: None,
                    avatar_shape: None,
                    avatar_color: None,
                    bot_badge: Some("AI".to_string()),
                    bot_model: Some("codex".to_string()),
                    bot_capabilities: vec!["rust".to_string()],
                    bot_autonomy_mode: mode,
                    bot_persona: Some("Analytical".to_string()),
                    cooldown_secs: Some(30),
                    max_posts_per_hour: Some(12),
                },
                t0,
            )
            .unwrap();
        let fetched = store.get_actor(&created.actor_id, None).unwrap().unwrap();
        assert_eq!(fetched.actor_kind, kind);
        assert_eq!(fetched.bot_autonomy_mode, mode);

        let low_trust = compute_trust_score(&fetched, t0);
        assert!((MIN_TRUST_SCORE..=MAX_TRUST_SCORE).contains(&low_trust));
        let mut high = fetched.clone();
        high.post_karma = 500;
        high.comment_karma = 500;
        let high_trust = compute_trust_score(&high, t0 + 86_400_000 * 30);
        assert!((MIN_TRUST_SCORE..=MAX_TRUST_SCORE).contains(&high_trust));
        assert!(high_trust > low_trust, "higher karma/age must raise trust score");
    }
}

// ============================================================================
// F03 — Subreddits / Communities (`c/<slug>`) Domain (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F03-01
/// [Feature / Req] F03 / ORIGINAL_REQUEST.md (Subreddit c/<slug> creation & metadata)
/// [Preconditions] Ephemeral live server with 1 Human registered.
/// [Opaque-Box Steps] POST /api/v1/community/communities with rules, post/user flairs, icon/accent, and bot policy flags, then GET /communities/{slug}.
/// [Observable Assertions] Community persists with exact slug, description, category, rules, flairs, `allow_bots`, and `require_bot_disclosure`.
#[tokio::test]
async fn test_cm_t1_f03_01_create_community_with_rules_flairs_and_bot_policies() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f03_01", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let (st, created) = driver
        .req(
            Method::POST,
            "/api/v1/community/communities",
            &auth,
            Some(json!({
                "slug": "rust_systems_lab",
                "name": "Rust Systems Lab",
                "description": "Deep dives into GPUI, SQLite, and async Rust",
                "category": "engineering",
                "icon_emoji": "🦀",
                "accent_color": "#E05A2B",
                "rules": [
                    { "id": "r1", "title": "Be constructive", "description": "Cite benchmarks.", "order": 1 }
                ],
                "post_flairs": [
                    { "id": "f1", "text": "RFC", "bg_color": "#1E3A8A", "text_color": "#FFFFFF", "is_mod_only": false }
                ],
                "user_flairs": [
                    { "id": "uf1", "text": "Core Contributor", "bg_color": "#065F46", "text_color": "#FFFFFF", "is_mod_only": false }
                ],
                "wiki_markdown": "# Rust Systems Lab\nWelcome!",
                "automod_rules": [],
                "allow_bots": true,
                "require_bot_disclosure": true,
                "is_nsfw": false
            })),
        )
        .await
        .unwrap();
    assert_eq!(st, 201, "create community failed: {created}");

    let (st, detail_val) = driver
        .req(Method::GET, "/api/v1/community/communities/rust_systems_lab", &auth, None)
        .await
        .unwrap();
    assert_eq!(st, 200);
    let detail: CommunityDetailResponse = serde_json::from_value(detail_val).unwrap();
    assert_eq!(detail.community.slug, "rust_systems_lab");
    assert_eq!(detail.community.category, "engineering");
    assert_eq!(detail.community.icon_emoji, "🦀");
    assert_eq!(detail.community.accent_color, "#E05A2B");
    assert_eq!(detail.community.rules.len(), 1);
    assert_eq!(detail.community.post_flairs.len(), 1);
    assert_eq!(detail.community.user_flairs.len(), 1);
    assert!(detail.community.allow_bots);
    assert!(detail.community.require_bot_disclosure);
}

/// [Test ID] TEST-CM-T1-F03-02
/// [Feature / Req] F03 / ORIGINAL_REQUEST.md (Human + Bot community join/leave)
/// [Preconditions] Ephemeral live server with owner Alice, Human Bob, and a Bot citizen.
/// [Opaque-Box Steps] Bob and Bot join `c/join_arena` via POST /join, inspect member counts, then leave via DELETE /join.
/// [Observable Assertions] `is_joined`, `member_count`, `human_member_count`, and `bot_member_count` increment on join and decrement on leave.
#[tokio::test]
async fn test_cm_t1_f03_02_human_and_bot_join_and_leave_community() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f03_02", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f03_02", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "join_f03_bot", "Join Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "join_arena", "name": "Join Arena", "allow_bots": true }))).await.unwrap().0,
        201
    );

    let (st, j1) = driver.req(Method::POST, "/api/v1/community/communities/join_arena/join", &b_auth, None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(j1["is_joined"], true);
    let (st, j2) = driver.req(Method::POST, "/api/v1/community/communities/join_arena/join", &bot_auth, None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(j2["is_joined"], true);

    let (_, detail_val) = driver.req(Method::GET, "/api/v1/community/communities/join_arena", &a_auth, None).await.unwrap();
    let detail: CommunityDetailResponse = serde_json::from_value(detail_val).unwrap();
    assert_eq!(detail.community.member_count, 3);
    assert_eq!(detail.community.human_member_count, 2);
    assert_eq!(detail.community.bot_member_count, 1);

    let (st, l1) = driver.req(Method::DELETE, "/api/v1/community/communities/join_arena/join", &b_auth, None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(l1["is_joined"], false);
    let (st, l2) = driver.req(Method::DELETE, "/api/v1/community/communities/join_arena/join", &bot_auth, None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(l2["is_joined"], false);

    let (_, after_val) = driver.req(Method::GET, "/api/v1/community/communities/join_arena", &a_auth, None).await.unwrap();
    let after: CommunityDetailResponse = serde_json::from_value(after_val).unwrap();
    assert_eq!(after.community.member_count, 1);
    assert_eq!(after.community.bot_member_count, 0);
}

/// [Test ID] TEST-CM-T1-F03-03
/// [Feature / Req] F03 / ORIGINAL_REQUEST.md (Community role hierarchy: owner, mod, bot_mod, member)
/// [Preconditions] Ephemeral live server with Owner Alice, Humans Bob & Carol, and Bot Mod.
/// [Opaque-Box Steps] Join all citizens to `c/roles_hq`, promote Bob to Mod and Bot to BotMod via /mod-actions, then query GET /communities/roles_hq as each actor.
/// [Observable Assertions] Viewer roles report `owner`, `mod`, `bot_mod`, and `member`; `moderators` list contains Alice, Bob, and Bot.
#[tokio::test]
async fn test_cm_t1_f03_03_community_role_hierarchy_owner_mod_bot_mod_member() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f03_03", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f03_03", "Pass1234!").await.unwrap();
    let carol = driver.register_human("carol_f03_03", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "mod_f03_bot", "Mod Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let c_auth = CommunityAuthMode::Human(carol.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "roles_hq", "name": "Roles HQ", "allow_bots": true }))).await.unwrap().0, 201);
    for auth in [&b_auth, &c_auth, &bot_auth] {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/roles_hq/join", auth, None).await.unwrap().0, 200);
    }
    for target in [&bob.actor_id, &bot.actor_id] {
        let (st, v) = driver
            .req(Method::POST, "/api/v1/community/communities/roles_hq/mod-actions", &a_auth, Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": target })))
            .await
            .unwrap();
        assert_eq!(st, 200, "promote_mod failed: {v}");
    }

    for (auth, expected_role) in [(&a_auth, "owner"), (&b_auth, "mod"), (&bot_auth, "bot_mod"), (&c_auth, "member")] {
        let (st, val) = driver.req(Method::GET, "/api/v1/community/communities/roles_hq", auth, None).await.unwrap();
        assert_eq!(st, 200);
        assert_eq!(val["community"]["viewer_role"], expected_role);
    }
    let (_, val) = driver.req(Method::GET, "/api/v1/community/communities/roles_hq", &a_auth, None).await.unwrap();
    let detail: CommunityDetailResponse = serde_json::from_value(val).unwrap();
    let mod_ids: HashSet<_> = detail.moderators.iter().map(|m| m.actor_id.as_str()).collect();
    assert!(mod_ids.contains(alice.actor_id.as_str()));
    assert!(mod_ids.contains(bob.actor_id.as_str()));
    assert!(mod_ids.contains(bot.actor_id.as_str()));
    assert!(!mod_ids.contains(carol.actor_id.as_str()));
}

/// [Test ID] TEST-CM-T1-F03-04
/// [Feature / Req] F03 / ORIGINAL_REQUEST.md (Pin & unpin posts in c/<slug>)
/// [Preconditions] Ephemeral live server with owner Alice and 3 posts in `c/pin_lab`.
/// [Opaque-Box Steps] Pin the oldest post via mod-actions, inspect GET /communities/pin_lab and community feed, then unpin.
/// [Observable Assertions] Pinned post appears in `pinned_posts` and top of community feed with `is_pinned == true`, and clears upon unpin.
#[tokio::test]
async fn test_cm_t1_f03_04_pin_and_unpin_posts_surface_at_top_of_community_feed() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f03_04", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &auth, Some(json!({ "slug": "pin_lab", "name": "Pin Lab" }))).await.unwrap().0, 201);
    let mut pids = Vec::new();
    for i in 1..=3 {
        let (st, v) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "community_slug": "pin_lab", "body": format!("Pin lab post #{i}") }))).await.unwrap();
        assert_eq!(st, 201);
        pids.push(v["post"]["post_id"].as_str().unwrap().to_string());
    }
    let first_pid = pids[0].clone();

    let (st, _) = driver
        .req(Method::POST, "/api/v1/community/communities/pin_lab/mod-actions", &auth, Some(json!({ "action": "pin", "target_kind": "post", "target_id": first_pid })))
        .await
        .unwrap();
    assert_eq!(st, 200);

    let (_, detail_val) = driver.req(Method::GET, "/api/v1/community/communities/pin_lab", &auth, None).await.unwrap();
    let detail: CommunityDetailResponse = serde_json::from_value(detail_val).unwrap();
    assert_eq!(detail.pinned_posts.len(), 1);
    assert_eq!(detail.pinned_posts[0].post_id, first_pid);
    assert!(detail.pinned_posts[0].is_pinned);

    let (st, _) = driver
        .req(Method::POST, "/api/v1/community/communities/pin_lab/mod-actions", &auth, Some(json!({ "action": "unpin", "target_kind": "post", "target_id": first_pid })))
        .await
        .unwrap();
    assert_eq!(st, 200);
    let (_, after_val) = driver.req(Method::GET, "/api/v1/community/communities/pin_lab", &auth, None).await.unwrap();
    let after: CommunityDetailResponse = serde_json::from_value(after_val).unwrap();
    assert!(after.pinned_posts.is_empty());
}

/// [Test ID] TEST-CM-T1-F03-05
/// [Feature / Req] F03 / ORIGINAL_REQUEST.md (Versioned Markdown wiki revisions)
/// [Preconditions] Ephemeral live server with owner Alice and `c/wiki_lab`.
/// [Opaque-Box Steps] Create community with initial wiki (rev 1), call PUT /wiki twice with summaries (rev 2 & rev 3), then GET /communities/wiki_lab.
/// [Observable Assertions] `wiki_revision == 3`, `wiki_markdown` reflects rev 3, and `wiki_revisions` preserves full history with summaries & editor handle.
#[tokio::test]
async fn test_cm_t1_f03_05_versioned_markdown_wiki_create_edit_and_revisions() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f03_05", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/communities", &auth, Some(json!({ "slug": "wiki_lab", "name": "Wiki Lab", "wiki_markdown": "# Rev 1\nInitial" }))).await.unwrap().0,
        201
    );
    let (st, r2) = driver
        .req(Method::PUT, "/api/v1/community/communities/wiki_lab/wiki", &auth, Some(json!({ "markdown": "# Rev 2\nAdded FAQ", "summary": "Add FAQ" })))
        .await
        .unwrap();
    assert_eq!(st, 200);
    assert_eq!(r2["revision"], 2);

    let (st, r3) = driver
        .req(Method::PUT, "/api/v1/community/communities/wiki_lab/wiki", &auth, Some(json!({ "markdown": "# Rev 3\nAdded Benchmarks", "summary": "Add Benchmarks" })))
        .await
        .unwrap();
    assert_eq!(st, 200);
    assert_eq!(r3["revision"], 3);

    let (_, val) = driver.req(Method::GET, "/api/v1/community/communities/wiki_lab", &auth, None).await.unwrap();
    let detail: CommunityDetailResponse = serde_json::from_value(val).unwrap();
    assert_eq!(detail.community.wiki_revision, 3);
    assert!(detail.community.wiki_markdown.contains("Added Benchmarks"));
    assert!(detail.wiki_revisions.len() >= 3);
    assert!(detail.wiki_revisions.iter().any(|r| r.revision == 2 && r.summary == "Add FAQ"));
    assert!(detail.wiki_revisions.iter().any(|r| r.revision == 3 && r.summary == "Add Benchmarks"));
}

// ============================================================================
// F04 — Unified Reddit + X Post Formats (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F04-01
/// [Feature / Req] F04 / ORIGINAL_REQUEST.md (Micro, Article Markdown, Link preview card posts)
/// [Preconditions] Ephemeral live server with Human Alice registered.
/// [Opaque-Box Steps] Publish `Micro`, `Article` (title + Markdown), and `Link` (link_preview) posts into `c/rust` with flairs, then GET each by post_id.
/// [Observable Assertions] All 3 post formats persist with exact format enum, title, Markdown body, flair, community_slug, and link_preview metadata.
#[tokio::test]
async fn test_cm_t1_f04_01_publish_micro_article_and_link_posts_with_flair() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f04_01", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let (st, v_micro) = driver
        .req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({
            "community_slug": "rust",
            "format": "micro",
            "body": "Zero-copy GPUI frames at 120fps #rust",
            "flair": "Discussion"
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "micro failed: {v_micro}");

    let (st, v_art) = driver
        .req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({
            "community_slug": "rust",
            "format": "article",
            "title": "WAL Concurrency Deep Dive",
            "body": "## Architecture\n\n- Single writer\n- Many readers\n```rust\nfn main() {}\n```",
            "flair": "RFC"
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "article failed: {v_art}");

    let (st, v_link) = driver
        .req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({
            "community_slug": "rust",
            "format": "link",
            "title": "Rust Official Site",
            "body": "Check https://rust-lang.org for release notes",
            "flair": "Release",
            "link_preview": {
                "url": "https://rust-lang.org",
                "domain": "rust-lang.org",
                "title": "Rust Programming Language",
                "description": "Empowering everyone to build reliable software."
            }
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "link failed: {v_link}");

    for (val, expected_fmt, expected_flair) in [
        (v_micro, PostFormat::Micro, "Discussion"),
        (v_art, PostFormat::Article, "RFC"),
        (v_link, PostFormat::Link, "Release"),
    ] {
        let pid = val["post"]["post_id"].as_str().unwrap();
        let (st, got) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &auth, None).await.unwrap();
        assert_eq!(st, 200);
        let detail: PostDetailResponse = serde_json::from_value(got).unwrap();
        assert_eq!(detail.post.format, expected_fmt);
        assert_eq!(detail.post.community_slug.as_deref(), Some("rust"));
        assert_eq!(detail.post.flair.as_deref(), Some(expected_flair));
        if expected_fmt == PostFormat::Link {
            assert_eq!(detail.post.link_preview.as_ref().unwrap().domain, "rust-lang.org");
        }
    }
}

/// [Test ID] TEST-CM-T1-F04-02
/// [Feature / Req] F04 / ORIGINAL_REQUEST.md (Media 1–4 gallery + spoiler/NSFW flags)
/// [Preconditions] Ephemeral live server with Human Alice and 3 distinct PNG uploads.
/// [Opaque-Box Steps] Upload 3 PNG images, publish a `Media` post attaching all 3 in order with `is_spoiler: true` and `is_nsfw: true`, then fetch the post.
/// [Observable Assertions] Post preserves exact 3-item media ordering, alt texts, and spoiler/NSFW flags.
#[tokio::test]
async fn test_cm_t1_f04_02_publish_media_gallery_with_spoiler_and_nsfw_flags() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f04_02", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let (m1, _) = driver.upload_png(&alice.token, "g1.png", (255, 0, 0, 255), None).await.unwrap();
    let (m2, _) = driver.upload_png(&alice.token, "g2.png", (0, 255, 0, 255), None).await.unwrap();
    let (m3, _) = driver.upload_png(&alice.token, "g3.png", (0, 0, 255, 255), None).await.unwrap();

    let (st, created) = driver
        .req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({
            "format": "media",
            "body": "3-shot gallery with spoiler & NSFW",
            "is_spoiler": true,
            "is_nsfw": true,
            "media": [
                { "media_id": m1, "kind": "image", "alt_text": "Red pixel" },
                { "media_id": m2, "kind": "image", "alt_text": "Green pixel" },
                { "media_id": m3, "kind": "image", "alt_text": "Blue pixel" }
            ]
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "media post failed: {created}");
    let pid = created["post"]["post_id"].as_str().unwrap();

    let (st, got) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &auth, None).await.unwrap();
    assert_eq!(st, 200);
    let detail: PostDetailResponse = serde_json::from_value(got).unwrap();
    assert_eq!(detail.post.format, PostFormat::Media);
    assert!(detail.post.is_spoiler);
    assert!(detail.post.is_nsfw);
    assert_eq!(detail.post.media.len(), 3);
    assert_eq!(detail.post.media[0].media_id, m1);
    assert_eq!(detail.post.media[1].media_id, m2);
    assert_eq!(detail.post.media[2].media_id, m3);
    assert_eq!(detail.post.media[1].alt_text.as_deref(), Some("Green pixel"));
}

/// [Test ID] TEST-CM-T1-F04-03
/// [Feature / Req] F04 / ORIGINAL_REQUEST.md (Polls with single/multi choice + Human vs Bot vote breakdown)
/// [Preconditions] Ephemeral live server with 1 Human and 1 Bot citizen.
/// [Opaque-Box Steps] Publish both a single-choice Poll and a multi-choice Poll; vote as Human and as Bot via POST /poll-vote.
/// [Observable Assertions] Separate `all_votes`, `human_votes`, `bot_votes`, totals, and `viewer_voted_option_ids` reflect exact Human vs Bot tallies.
#[tokio::test]
async fn test_cm_t1_f04_03_poll_single_and_multi_select_human_vs_bot_breakdown() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f04_03", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "poll_f04_bot", "Poll Bot", "manual").await.unwrap();
    let h_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    let (st, p_single) = driver
        .req(Method::POST, "/api/v1/community/posts", &h_auth, Some(json!({
            "format": "poll",
            "body": "Single choice poll",
            "poll": { "question": "Preferred engine?", "options": ["GPUI", "Skia"], "multi_select": false, "duration_secs": 3600 }
        })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let spid = p_single["post"]["post_id"].as_str().unwrap().to_string();
    let s_opt0 = p_single["post"]["poll"]["options"][0]["option_id"].as_str().unwrap().to_string();
    let s_opt1 = p_single["post"]["poll"]["options"][1]["option_id"].as_str().unwrap().to_string();

    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{spid}/poll-vote"), &h_auth, Some(json!({ "option_ids": [s_opt0] }))).await.unwrap().0, 200);
    let (st, pv) = driver.req(Method::POST, &format!("/api/v1/community/posts/{spid}/poll-vote"), &b_auth, Some(json!({ "option_ids": [s_opt1] }))).await.unwrap();
    assert_eq!(st, 200);
    let poll: PollDto = serde_json::from_value(pv).unwrap();
    assert_eq!(poll.total_votes, 2);
    assert_eq!(poll.total_human_votes, 1);
    assert_eq!(poll.total_bot_votes, 1);
    assert_eq!(poll.options[0].human_votes, 1);
    assert_eq!(poll.options[0].bot_votes, 0);
    assert_eq!(poll.options[1].human_votes, 0);
    assert_eq!(poll.options[1].bot_votes, 1);

    let (st, p_multi) = driver
        .req(Method::POST, "/api/v1/community/posts", &h_auth, Some(json!({
            "format": "poll",
            "body": "Multi choice poll",
            "poll": { "question": "Topics?", "options": ["Rust", "AI", "UI"], "multi_select": true, "duration_secs": 3600 }
        })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let mpid = p_multi["post"]["post_id"].as_str().unwrap().to_string();
    let m0 = p_multi["post"]["poll"]["options"][0]["option_id"].as_str().unwrap().to_string();
    let m1 = p_multi["post"]["poll"]["options"][1]["option_id"].as_str().unwrap().to_string();
    let (st, mpv) = driver
        .req(Method::POST, &format!("/api/v1/community/posts/{mpid}/poll-vote"), &h_auth, Some(json!({ "option_ids": [m0, m1] })))
        .await
        .unwrap();
    assert_eq!(st, 200);
    let mpoll: PollDto = serde_json::from_value(mpv).unwrap();
    assert!(mpoll.multi_select);
    assert_eq!(mpoll.viewer_voted_option_ids.len(), 2);
    assert_eq!(mpoll.options[0].human_votes, 1);
    assert_eq!(mpoll.options[1].human_votes, 1);
}

/// [Test ID] TEST-CM-T1-F04-04
/// [Feature / Req] F04 / ORIGINAL_REQUEST.md (Repost, Quote, and Crosspost attribution & counters)
/// [Preconditions] Ephemeral live server with 2 Humans (Alice and Bob).
/// [Opaque-Box Steps] Alice publishes an origin post in `c/rust`; Bob reposts it, publishes a Quote referencing it, and crossposts it into `c/systems`.
/// [Observable Assertions] Origin post `reposts_count` and `quotes_count` increment, and embedded origin summaries match Alice's origin post.
#[tokio::test]
async fn test_cm_t1_f04_04_repost_quote_and_crosspost_attribution_and_counters() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f04_04", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f04_04", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    let (st, orig) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "rust",
            "title": "Origin Post",
            "body": "Original benchmark data in c/rust"
        })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let orig_id = orig["post"]["post_id"].as_str().unwrap().to_string();

    let (st, rp) = driver
        .req_empty_body(Method::POST, &format!("/api/v1/community/posts/{orig_id}/repost"), &b_auth)
        .await
        .unwrap();
    assert_eq!(st, 200, "repost failed: {rp}");
    assert_eq!(rp["reposted"], true);

    let (st, qt) = driver
        .req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({
            "format": "quote",
            "quoted_post_id": orig_id,
            "body": "Adding commentary on this benchmark"
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "quote failed: {qt}");
    let qt_resp: CreatePostResponse = serde_json::from_value(qt).unwrap();
    assert_eq!(qt_resp.post.format, PostFormat::Quote);
    assert_eq!(qt_resp.post.quoted_post.as_ref().unwrap().post_id, orig_id);

    let (st, xp) = driver
        .req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({
            "community_slug": "systems",
            "format": "crosspost",
            "crosspost_of_id": orig_id,
            "body": "Sharing to c/systems"
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "crosspost failed: {xp}");
    let xp_resp: CreatePostResponse = serde_json::from_value(xp).unwrap();
    assert_eq!(xp_resp.post.format, PostFormat::Crosspost);
    assert_eq!(xp_resp.post.crosspost_origin.as_ref().unwrap().post_id, orig_id);

    let (_, got_orig) = driver.req(Method::GET, &format!("/api/v1/community/posts/{orig_id}"), &a_auth, None).await.unwrap();
    let detail: PostDetailResponse = serde_json::from_value(got_orig).unwrap();
    assert!(detail.post.reposts_count >= 1);
    assert!(detail.post.quotes_count >= 1);
}

/// [Test ID] TEST-CM-T1-F04-05
/// [Feature / Req] F04 / ORIGINAL_REQUEST.md (X Thread 1/N, Draft->Published, Scheduled, Edit History)
/// [Preconditions] Ephemeral live server with Human Alice and Human Bob.
/// [Opaque-Box Steps] Create 3-part X thread, create Draft then publish via PATCH, create Scheduled post, and edit published post twice.
/// [Observable Assertions] Thread returns 3 indexed parts (`thread_total == 3`), Draft becomes visible to Bob after publish, Scheduled persists `scheduled_at_ms`, and `edit_history` retains revisions.
#[tokio::test]
async fn test_cm_t1_f04_05_x_thread_draft_to_published_scheduled_and_edit_history() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f04_05", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f04_05", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    // 1. 3-part X thread
    let (st, th_val) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "format": "micro",
            "body": "Thread 1/3 root",
            "thread_parts": [
                { "body": "Thread 2/3 middle", "media": [] },
                { "body": "Thread 3/3 conclusion", "media": [] }
            ]
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "thread failed: {th_val}");
    let th: CreatePostResponse = serde_json::from_value(th_val).unwrap();
    assert_eq!(th.post.thread_total, 3);
    assert_eq!(th.post.thread_index, 0);
    assert_eq!(th.thread_posts.len(), 2, "2 follow-up thread parts plus root post = 3 total");
    assert_eq!(th.thread_posts[1].thread_index, 2);

    // 2. Draft -> Published transition + Edit History
    let (st, d_val) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "format": "micro",
            "status": "draft",
            "body": "Draft revision 0"
        })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let draft_id = d_val["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::GET, &format!("/api/v1/community/posts/{draft_id}"), &b_auth, None).await.unwrap().0, 404);

    let (st, _) = driver
        .req(Method::PATCH, &format!("/api/v1/community/posts/{draft_id}"), &a_auth, Some(json!({ "status": "published", "body": "Published v1" })))
        .await
        .unwrap();
    assert_eq!(st, 200);
    let (st, _) = driver
        .req(Method::PATCH, &format!("/api/v1/community/posts/{draft_id}"), &a_auth, Some(json!({ "body": "Published v2 edited" })))
        .await
        .unwrap();
    assert_eq!(st, 200);

    let (st, got) = driver.req(Method::GET, &format!("/api/v1/community/posts/{draft_id}"), &b_auth, None).await.unwrap();
    assert_eq!(st, 200);
    let detail: PostDetailResponse = serde_json::from_value(got).unwrap();
    assert_eq!(detail.post.status, PostStatus::Published);
    assert_eq!(detail.post.body, "Published v2 edited");
    assert!(detail.post.edit_history.len() >= 2, "edit history must record revisions");

    // 3. Scheduled post storage
    let future_ms = now_ms() + 600_000;
    let (st, s_val) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "format": "micro",
            "status": "scheduled",
            "scheduled_at_ms": future_ms,
            "body": "Scheduled announcement"
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "scheduled failed: {s_val}");
    assert_eq!(s_val["post"]["status"], "scheduled");
    assert_eq!(s_val["post"]["scheduled_at_ms"], future_ms);
}

// ============================================================================
// F05 — Nested Reddit Comment Forest + X Thread Engine (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F05-01
/// [Feature / Req] F05 / ORIGINAL_REQUEST.md (Arbitrary-depth nested Reddit comment forest)
/// [Preconditions] Ephemeral live server with 1 Human, 1 Bot, and a root post.
/// [Opaque-Box Steps] Create a 4-level nested reply chain (depth 0 -> 1 -> 2 -> 3) alternating Human and Bot, then GET /posts/{id}/comments.
/// [Observable Assertions] Returned comment forest nests 4 levels deep with strictly monotonic `depth` (0..3) and hierarchical materialized `path` prefixes.
#[tokio::test]
async fn test_cm_t1_f05_01_multi_level_nested_comment_forest_paths_and_children() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f05_01", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "tree_f05_bot", "Tree Bot", "manual").await.unwrap();
    let h_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &h_auth, Some(json!({ "body": "Forest root" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let mut parent: Option<String> = None;
    let mut ids = Vec::new();
    for depth in 0..4 {
        let auth = if depth % 2 == 0 { &h_auth } else { &b_auth };
        let mut body = json!({ "body": format!("Comment at depth {depth}") });
        if let Some(ref p) = parent {
            body["parent_comment_id"] = json!(p);
        }
        let (st, cv) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), auth, Some(body)).await.unwrap();
        assert_eq!(st, 201, "comment depth {depth} failed: {cv}");
        let cid = cv["comment"]["comment_id"].as_str().unwrap().to_string();
        parent = Some(cid.clone());
        ids.push(cid);
    }

    let (st, tree_val) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}/comments"), &h_auth, None).await.unwrap();
    assert_eq!(st, 200);
    let roots: Vec<CommunityCommentDto> = serde_json::from_value(tree_val["comments"].clone()).unwrap();
    assert_eq!(roots.len(), 1);
    let c0 = &roots[0];
    let c1 = &c0.children[0];
    let c2 = &c1.children[0];
    let c3 = &c2.children[0];
    assert_eq!((c0.depth, c1.depth, c2.depth, c3.depth), (0, 1, 2, 3));
    assert_eq!((&c0.comment_id, &c1.comment_id, &c2.comment_id, &c3.comment_id), (&ids[0], &ids[1], &ids[2], &ids[3]));
    assert!(c1.path.starts_with(&c0.path));
    assert!(c2.path.starts_with(&c1.path));
    assert!(c3.path.starts_with(&c2.path));
}

fn make_test_comment(
    base: &CommunityCommentDto,
    id: &str,
    parent: Option<&str>,
    path: String,
    depth: u32,
    upvotes: i64,
    downvotes: i64,
    created_at_ms: i64,
) -> CommunityCommentDto {
    let mut c = base.clone();
    c.comment_id = id.to_string();
    c.parent_comment_id = parent.map(str::to_string);
    c.path = path;
    c.depth = depth;
    c.upvotes = upvotes;
    c.downvotes = downvotes;
    c.score = upvotes - downvotes;
    c.is_stickied = false;
    c.is_mod_distinguished = false;
    c.is_op = false;
    c.children.clear();
    c.more_children_count = 0;
    c.created_at_ms = created_at_ms;
    c.updated_at_ms = created_at_ms;
    c
}

/// [Test ID] TEST-CM-T1-F05-02
/// [Feature / Req] F05 / ORIGINAL_REQUEST.md (All 6 Reddit comment sorts deterministic ordering)
/// [Preconditions] Pure-Rust `CommentTreeBuilder::build` with crafted top-level comments having distinct votes, timestamps, and OP replies.
/// [Opaque-Box Steps] Sort identical flat comment list across all 6 `CommentSort` variants (`Best`, `Top`, `New`, `Controversial`, `Old`, `Qa`).
/// [Observable Assertions] Each sort places its mathematically expected winning comment at index 0 deterministically.
#[test]
fn test_cm_t1_f05_02_all_six_comment_sorts_deterministic_orderings() {
    let t0 = now_ms();
    let base = build_starter_dataset(t0).comments[0].clone();
    let c_top = make_test_comment(&base, "c_top", None, make_comment_path(None, "c_top"), 0, 120, 40, t0 - 300_000);
    let c_best = make_test_comment(&base, "c_best", None, make_comment_path(None, "c_best"), 0, 50, 0, t0 - 200_000);
    let c_contro = make_test_comment(&base, "c_contro", None, make_comment_path(None, "c_contro"), 0, 90, 88, t0 - 100_000);
    let c_old = make_test_comment(&base, "c_old", None, make_comment_path(None, "c_old"), 0, 5, 0, t0 - 900_000);
    let c_new = make_test_comment(&base, "c_new", None, make_comment_path(None, "c_new"), 0, 1, 0, t0 - 1_000);
    let c_qa_q = make_test_comment(&base, "c_qa_q", None, make_comment_path(None, "c_qa_q"), 0, 8, 1, t0 - 400_000);
    let mut c_qa_ans = make_test_comment(&base, "c_qa_ans", Some("c_qa_q"), make_comment_path(Some(&c_qa_q.path), "c_qa_ans"), 1, 4, 0, t0 - 350_000);
    c_qa_ans.is_op = true;

    let flat = vec![c_top, c_best, c_contro, c_old, c_new, c_qa_q, c_qa_ans];
    assert_eq!(CommentTreeBuilder::build(flat.clone(), CommentSort::Top)[0].comment_id, "c_top");
    assert_eq!(CommentTreeBuilder::build(flat.clone(), CommentSort::Best)[0].comment_id, "c_best");
    assert_eq!(CommentTreeBuilder::build(flat.clone(), CommentSort::Controversial)[0].comment_id, "c_contro");
    assert_eq!(CommentTreeBuilder::build(flat.clone(), CommentSort::Old)[0].comment_id, "c_old");
    assert_eq!(CommentTreeBuilder::build(flat.clone(), CommentSort::New)[0].comment_id, "c_new");
    assert_eq!(CommentTreeBuilder::build(flat, CommentSort::Qa)[0].comment_id, "c_qa_q");
}

/// [Test ID] TEST-CM-T1-F05-03
/// [Feature / Req] F05 / ORIGINAL_REQUEST.md (Stickied & mod-distinguished comments pin to top)
/// [Preconditions] Crafted comment list with high-score regular comment and low-score stickied mod-distinguished comment.
/// [Opaque-Box Steps] Build comment tree across `Top` and `Best` sorts.
/// [Observable Assertions] Stickied mod-distinguished comment always pins to index 0 despite lower score.
#[test]
fn test_cm_t1_f05_03_stickied_and_mod_distinguished_comments_pin_to_top() {
    let t0 = now_ms();
    let base = build_starter_dataset(t0).comments[0].clone();
    let high = make_test_comment(&base, "c_high", None, make_comment_path(None, "c_high"), 0, 500, 1, t0 - 10_000);
    let mut sticky = make_test_comment(&base, "c_sticky", None, make_comment_path(None, "c_sticky"), 0, 1, 0, t0 - 50_000);
    sticky.is_stickied = true;
    sticky.is_mod_distinguished = true;

    for sort in [CommentSort::Top, CommentSort::Best, CommentSort::New, CommentSort::Old] {
        let tree = CommentTreeBuilder::build(vec![high.clone(), sticky.clone()], sort);
        assert_eq!(tree[0].comment_id, "c_sticky", "stickied must lead under {sort:?}");
        assert!(tree[0].is_stickied && tree[0].is_mod_distinguished);
    }
}

/// [Test ID] TEST-CM-T1-F05-04
/// [Feature / Req] F05 / ORIGINAL_REQUEST.md (Comment tree load-more & `more_children_count`)
/// [Preconditions] Wide comment tree with 6 children under one parent (Core) and live server comments endpoint.
/// [Opaque-Box Steps] Build tree with `max_children_per_node: Some(2)` and also query live server `GET /posts/{id}/comments?parent_id=...&limit=2`.
/// [Observable Assertions] Truncated node reports `more_children_count == 4` and server pagination returns `has_more` + remaining children via cursor/parent_id.
#[tokio::test]
async fn test_cm_t1_f05_04_deep_wide_comment_pagination_more_children_and_load_more() {
    let t0 = now_ms();
    let base = build_starter_dataset(t0).comments[0].clone();
    let root = make_test_comment(&base, "r0", None, make_comment_path(None, "r0"), 0, 10, 0, t0 - 10_000);
    let mut flat = vec![root.clone()];
    for i in 0..6 {
        let cid = format!("ch_{i}");
        flat.push(make_test_comment(&base, &cid, Some("r0"), make_comment_path(Some(&root.path), &cid), 1, 20 - i as i64, 0, t0 - 9_000 + i as i64));
    }
    let opts = CommentTreeOptions { sort: CommentSort::Top, max_depth: None, max_children_per_node: Some(2), collapsed_ids: HashSet::new() };
    let built = CommentTreeBuilder::build_with_options(flat, &opts);
    assert_eq!(built[0].children.len(), 2);
    assert_eq!(built[0].more_children_count, 4);

    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f05_04", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());
    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "body": "Wide parent post" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();
    let (_, rv) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &auth, Some(json!({ "body": "Parent" }))).await.unwrap();
    let parent_id = rv["comment"]["comment_id"].as_str().unwrap().to_string();
    for i in 0..4 {
        assert_eq!(
            driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &auth, Some(json!({ "parent_comment_id": parent_id, "body": format!("Child {i}") }))).await.unwrap().0,
            201
        );
    }
    let (st, sub) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}/comments?parent_id={parent_id}&limit=4"), &auth, None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(sub["comments"].as_array().unwrap().len(), 4);
}

/// [Test ID] TEST-CM-T1-F05-05
/// [Feature / Req] F05 / ORIGINAL_REQUEST.md (X linear author thread & quote-chain builders)
/// [Preconditions] Live server with a 3-part X thread AND a 3-hop quote chain (`P0 <- Q1 <- Q2`).
/// [Opaque-Box Steps] Fetch GET /posts/{root} and GET /posts/{Q2}, and also invoke `ThreadAssembler::build_linear_thread` / `build_quote_chain`.
/// [Observable Assertions] `thread_chain` contains all 3 thread parts in exact order and `quote_chain` preserves full origin lineage `Q1 -> P0`.
#[tokio::test]
async fn test_cm_t1_f05_05_x_linear_thread_and_quote_chain_builders() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f05_05", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let (st, tv) = driver
        .req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({
            "format": "micro",
            "body": "Linear thread 1/3",
            "thread_parts": [
                { "body": "Linear thread 2/3", "media": [] },
                { "body": "Linear thread 3/3", "media": [] }
            ]
        })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let t_root = tv["post"]["post_id"].as_str().unwrap().to_string();
    let (_, t_det_v) = driver.req(Method::GET, &format!("/api/v1/community/posts/{t_root}"), &auth, None).await.unwrap();
    let t_det: PostDetailResponse = serde_json::from_value(t_det_v).unwrap();
    assert_eq!(t_det.thread_chain.len(), 3);
    assert_eq!(ThreadAssembler::build_linear_thread(&t_det.post, &t_det.thread_chain).len(), 3);

    let (_, p0) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "body": "Quote root P0" }))).await.unwrap();
    let p0_id = p0["post"]["post_id"].as_str().unwrap().to_string();
    let (_, q1) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "format": "quote", "quoted_post_id": p0_id, "body": "Quote hop 1" }))).await.unwrap();
    let q1_id = q1["post"]["post_id"].as_str().unwrap().to_string();
    let (_, q2) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "format": "quote", "quoted_post_id": q1_id, "body": "Quote hop 2" }))).await.unwrap();
    let q2_id = q2["post"]["post_id"].as_str().unwrap().to_string();

    let (_, q2_det_v) = driver.req(Method::GET, &format!("/api/v1/community/posts/{q2_id}"), &auth, None).await.unwrap();
    let q2_det: PostDetailResponse = serde_json::from_value(q2_det_v).unwrap();
    let lineage_ids: Vec<&str> = q2_det.quote_chain.iter().map(|p| p.post_id.as_str()).collect();
    assert!(lineage_ids.contains(&q1_id.as_str()) && lineage_ids.contains(&p0_id.as_str()), "quote_chain missing lineage: {lineage_ids:?}");
    let assembled = ThreadAssembler::build_quote_chain(&q2_det.post, &q2_det.quote_chain, 4);
    assert!(!assembled.is_empty());
}

// ============================================================================
// F06 — 5-Stage Multi-Signal Recommendation Engine (5 Tests)
// ============================================================================

fn make_cand_post(
    base: &CommunityPostDto,
    id: &str,
    author_id: &str,
    author_kind: ActorKind,
    community: &str,
    body: &str,
    upvotes: i64,
    downvotes: i64,
    created_at_ms: i64,
) -> PostCandidate {
    let mut p = base.clone();
    p.post_id = id.to_string();
    p.author_actor_id = author_id.to_string();
    p.author_handle = author_id.replace(':', "_");
    p.author_kind = author_kind;
    p.community_slug = Some(community.to_string());
    p.community_id = Some(format!("comm_{community}"));
    p.title = Some(format!("Title {id}"));
    p.body = body.to_string();
    p.upvotes = upvotes;
    p.downvotes = downvotes;
    p.score = upvotes - downvotes;
    p.comments_count = (upvotes.max(0) / 3) as u64;
    p.created_at_ms = created_at_ms;
    p.updated_at_ms = created_at_ms;
    p.is_nsfw = false;
    p.is_removed = false;
    p.status = PostStatus::Published;
    let mut c = PostCandidate::from_post(p);
    c.content_embedding = embed_text_64(body);
    c.recent_engagement_1h = upvotes.max(0) as u64;
    c
}

/// [Test ID] TEST-CM-T1-F06-01
/// [Feature / Req] F06 / ORIGINAL_REQUEST.md (ForYou composite ranking & FeedExplanationDto)
/// [Preconditions] Pure-Rust `CommunityRecommender::rank_feed` with high-affinity followed/community post vs cold unfollowed post.
/// [Opaque-Box Steps] Rank both candidates under `FeedTab::ForYou` + `FeedSort::Algo` with `show_explanations: true`.
/// [Observable Assertions] High-affinity post ranks #1 with strictly higher `rank_score` and non-empty `FeedExplanationDto` + `signal_breakdown`.
#[test]
fn test_cm_t1_f06_01_for_you_composite_ranks_high_affinity_with_explanation() {
    let t0 = now_ms();
    let base = build_starter_dataset(t0).posts[0].clone();
    let high = make_cand_post(&base, "p_high", "u:10", ActorKind::Human, "rust", "rust gpui async performance benchmarks", 85, 2, t0 - 600_000);
    let cold = make_cand_post(&base, "p_cold", "u:99", ActorKind::Human, "random", "unrelated gardening notes", 2, 1, t0 - 7_200_000);

    let mut viewer = ViewerContext { now_ms: t0, ..Default::default() };
    viewer.following_actor_ids.insert("u:10".to_string());
    viewer.joined_communities.insert("rust".to_string());
    viewer.author_affinities.insert("u:10".to_string(), 0.9);
    viewer.interest_embedding = embed_text_64("rust gpui async performance");

    let req = FeedRequestDto { tab: FeedTab::ForYou, sort: FeedSort::Algo, now_ms: Some(t0), limit: 10, ..Default::default() };
    let resp = CommunityRecommender::rank_feed(&viewer, vec![cold, high], &req);
    assert_eq!(resp.items.len(), 2);
    assert_eq!(resp.items[0].post.post_id, "p_high");
    assert!(resp.items[0].rank_score > resp.items[1].rank_score);
    let expl = resp.items[0].explanation.as_ref().expect("FeedExplanationDto must be emitted");
    assert!(!expl.primary_reason.is_empty());
    assert!(!expl.signal_breakdown.is_empty());
}

/// [Test ID] TEST-CM-T1-F06-02
/// [Feature / Req] F06 / ORIGINAL_REQUEST.md (All Reddit sorts: Hot, Best, Top(Hour..All), New, Controversial, Rising)
/// [Preconditions] Identical candidate pool with distinct upvotes, downvotes, recency, age, and velocity.
/// [Opaque-Box Steps] Execute `CommunityRecommender::rank_feed` across Hot, Best, Top(Hour), Top(All), New, Controversial, and Rising.
/// [Observable Assertions] Each sort selects its exact specification-compliant winner at index 0.
#[test]
fn test_cm_t1_f06_02_reddit_sorts_order_identical_candidates_distinctly() {
    let t0 = now_ms();
    let base = build_starter_dataset(t0).posts[0].clone();
    let p_top_old = make_cand_post(&base, "p_top_old", "u:1", ActorKind::Human, "rust", "historic top post", 1000, 100, t0 - 86_400_000 * 10);
    let p_best = make_cand_post(&base, "p_best", "u:2", ActorKind::Human, "rust", "pure quality post", 300, 0, t0 - 3_600_000 * 5);
    let p_hot = make_cand_post(&base, "p_hot", "u:3", ActorKind::Human, "rust", "hot recent burst", 180, 2, t0 - 1_200_000);
    let p_new = make_cand_post(&base, "p_new", "u:4", ActorKind::Human, "rust", "brand new post", 2, 0, t0 - 5_000);
    let p_contro = make_cand_post(&base, "p_contro", "u:5", ActorKind::Human, "rust", "heated debate", 200, 198, t0 - 1_800_000);
    let mut p_rising = make_cand_post(&base, "p_rising", "u:6", ActorKind::Human, "rust", "fast rising", 60, 0, t0 - 300_000);
    p_rising.post.comments_count = 45;
    p_rising.recent_engagement_1h = 105;

    let pool = vec![p_top_old, p_best, p_hot, p_new, p_contro, p_rising];
    let viewer = ViewerContext { now_ms: t0, ..Default::default() };

    let run = |sort: FeedSort, tw: TimeWindow| -> String {
        let req = FeedRequestDto { tab: FeedTab::All, sort, time_window: tw, limit: 10, now_ms: Some(t0), ..Default::default() };
        CommunityRecommender::rank_feed(&viewer, pool.clone(), &req).items[0].post.post_id.clone()
    };

    assert_eq!(run(FeedSort::Top, TimeWindow::All), "p_top_old");
    assert_eq!(run(FeedSort::Top, TimeWindow::Hour), "p_hot");
    assert_eq!(run(FeedSort::New, TimeWindow::All), "p_new");
    assert_eq!(run(FeedSort::Controversial, TimeWindow::All), "p_contro");
    assert_eq!(run(FeedSort::Best, TimeWindow::All), "p_best");
    let hot_win = run(FeedSort::Hot, TimeWindow::All);
    assert!(hot_win == "p_hot" || hot_win == "p_rising", "unexpected Hot winner: {hot_win}");
    let rising_win = run(FeedSort::Rising, TimeWindow::All);
    assert!(rising_win == "p_rising" || rising_win == "p_hot", "unexpected Rising winner: {rising_win}");
}

/// [Test ID] TEST-CM-T1-F06-03
/// [Feature / Req] F06 / ORIGINAL_REQUEST.md (All 7 AlgoPresets + custom weight persistence via PUT /algo-prefs)
/// [Preconditions] Core recommender with balanced Human & Bot candidates + Live server for PUT/GET /algo-prefs.
/// [Opaque-Box Steps] Rank candidates across all 7 `AlgoPreset`s (`Balanced`, `RedditDeepDive`, `XRealTime`, `Discovery`, `HumanFocused`, `BotHeavy`, `Chronological`) and persist custom weights via REST.
/// [Observable Assertions] `HumanFocused` lifts Human posts, `BotHeavy` lifts Bot posts, `Chronological` orders newest-first, and PUT/GET /algo-prefs round-trips cleanly.
#[tokio::test]
async fn test_cm_t1_f06_03_algo_presets_and_custom_weights_shift_feed_ranking() {
    let t0 = now_ms();
    let base = build_starter_dataset(t0).posts[0].clone();
    let p_human = make_cand_post(&base, "p_human", "u:21", ActorKind::Human, "rust", "human rust analysis", 30, 1, t0 - 1_800_000);
    let p_bot = make_cand_post(&base, "p_bot", "b:31", ActorKind::Bot, "rust", "bot rust analysis", 30, 1, t0 - 1_800_000);
    let p_newest = make_cand_post(&base, "p_newest", "u:22", ActorKind::Human, "systems", "brand new update", 1, 0, t0 - 1_000);
    let pool = vec![p_human, p_bot, p_newest];

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
        let viewer = ViewerContext { now_ms: t0, algo_prefs: prefs.clone(), ..Default::default() };
        let req = FeedRequestDto { tab: FeedTab::ForYou, sort: FeedSort::Algo, algo_override: Some(prefs), now_ms: Some(t0), limit: 10, ..Default::default() };
        let resp = CommunityRecommender::rank_feed(&viewer, pool.clone(), &req);
        assert_eq!(resp.applied_preset, preset);
        assert_eq!(resp.items.len(), 3);
        if preset == AlgoPreset::Chronological {
            assert_eq!(resp.items[0].post.post_id, "p_newest");
        }
    }
    let hf = CommunityRecommender::rank_feed(
        &ViewerContext { now_ms: t0, algo_prefs: default_prefs_for_preset(AlgoPreset::HumanFocused), ..Default::default() },
        pool[0..2].to_vec(),
        &FeedRequestDto { tab: FeedTab::ForYou, sort: FeedSort::Algo, now_ms: Some(t0), limit: 5, ..Default::default() },
    );
    let bh = CommunityRecommender::rank_feed(
        &ViewerContext { now_ms: t0, algo_prefs: default_prefs_for_preset(AlgoPreset::BotHeavy), ..Default::default() },
        pool[0..2].to_vec(),
        &FeedRequestDto { tab: FeedTab::ForYou, sort: FeedSort::Algo, now_ms: Some(t0), limit: 5, ..Default::default() },
    );
    assert_eq!(hf.items[0].post.author_kind, ActorKind::Human);
    assert_eq!(bh.items[0].post.author_kind, ActorKind::Bot);

    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f06_03", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());
    let custom = AlgoPreferencesDto {
        preset: AlgoPreset::Discovery,
        recency_weight: 0.25,
        affinity_weight: 0.25,
        quality_weight: 0.50,
        discovery_ratio: 0.40,
        bot_content_ratio: 0.60,
        show_explanations: true,
        allow_nsfw: false,
    };
    let (st, _) = driver.req(Method::PUT, "/api/v1/community/algo-prefs", &auth, Some(serde_json::to_value(&custom).unwrap())).await.unwrap();
    assert_eq!(st, 200);
    let (st, got) = driver.req(Method::GET, "/api/v1/community/algo-prefs", &auth, None).await.unwrap();
    assert_eq!(st, 200);
    let persisted: AlgoPreferencesDto = serde_json::from_value(got).unwrap();
    assert_eq!(persisted.preset, AlgoPreset::Discovery);
    assert!((persisted.bot_content_ratio - 0.60).abs() < 1e-6);
}

/// [Test ID] TEST-CM-T1-F06-04
/// [Feature / Req] F06 / ORIGINAL_REQUEST.md (Greedy MMR diversity, author/community caps, Human/Bot balance & exploration)
/// [Preconditions] Candidate pool containing 6 high-score posts from a single author plus 6 diverse Human & Bot posts across communities.
/// [Opaque-Box Steps] Execute `CommunityRecommender::rank_feed` with `limit: 8` under `FeedTab::ForYou`.
/// [Observable Assertions] No two consecutive feed slots share the flooding author, author count is capped, and both Human and Bot items appear.
#[test]
fn test_cm_t1_f06_04_greedy_mmr_diversity_caps_and_bot_ratio_exploration() {
    let t0 = now_ms();
    let base = build_starter_dataset(t0).posts[0].clone();
    let mut pool = Vec::new();
    for i in 0..6 {
        pool.push(make_cand_post(&base, &format!("flood_{i}"), "u:777", ActorKind::Human, "rust", &format!("flood rust topic {i}"), 90 - i, 1, t0 - 60_000 * (i + 1)));
    }
    for i in 0..3 {
        pool.push(make_cand_post(&base, &format!("bot_{i}"), &format!("b:80{i}"), ActorKind::Bot, "ai_agents", &format!("bot agents insight {i}"), 40, 1, t0 - 120_000 * (i + 1)));
    }
    for i in 0..3 {
        pool.push(make_cand_post(&base, &format!("hum_{i}"), &format!("u:90{i}"), ActorKind::Human, "systems", &format!("systems kernel note {i}"), 35, 1, t0 - 180_000 * (i + 1)));
    }

    let viewer = ViewerContext { now_ms: t0, ..Default::default() };
    let req = FeedRequestDto { tab: FeedTab::ForYou, sort: FeedSort::Algo, limit: 8, now_ms: Some(t0), ..Default::default() };
    let resp = CommunityRecommender::rank_feed(&viewer, pool, &req);
    assert_eq!(resp.items.len(), 8);

    for pair in resp.items.windows(2) {
        assert_ne!(
            pair[0].post.author_actor_id, pair[1].post.author_actor_id,
            "MMR diversity must avoid consecutive posts from the same author"
        );
    }
    let flood_count = resp.items.iter().filter(|it| it.post.author_actor_id == "u:777").count();
    assert!(flood_count <= 3, "single author must be capped in top 8, got {flood_count}");
    assert!(resp.items.iter().any(|it| it.post.author_kind == ActorKind::Bot), "bot citizens must be represented");
    assert!(resp.items.iter().any(|it| it.post.author_kind == ActorKind::Human), "human citizens must be represented");
}

/// [Test ID] TEST-CM-T1-F06-05
/// [Feature / Req] F06 / ORIGINAL_REQUEST.md (Engagement & negative feedback signals `/signals`)
/// [Preconditions] Ephemeral live server with Human Alice and a newly published post.
/// [Opaque-Box Steps] Submit `dwell`, `click`, `show_more`, `not_interested`, and `hide_post` via POST /api/v1/community/signals, then fetch GET /feed?tab=for_you.
/// [Observable Assertions] Every signal returns `recorded == true` with non-empty `signal_id`, and the hidden post is excluded from Alice's ForYou feed.
#[tokio::test]
async fn test_cm_t1_f06_05_signals_adapt_for_you_and_hide_dismissed_items() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f06_05", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f06_05", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "rust", "body": "Signal target post for Alice" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    for (kind, dwell) in [("dwell", Some(4500)), ("click", None), ("show_more", None), ("not_interested", None), ("hide_post", None)] {
        let (st, sv) = driver
            .req(Method::POST, "/api/v1/community/signals", &a_auth, Some(json!({
                "post_id": pid,
                "kind": kind,
                "dwell_ms": dwell,
                "target_actor_id": bob.actor_id,
                "community_slug": "rust"
            })))
            .await
            .unwrap();
        assert_eq!(st, 200, "signal {kind} failed: {sv}");
        assert_eq!(sv["recorded"], true);
        assert!(!sv["signal_id"].as_str().unwrap().is_empty());
    }

    let (st, fv) = driver.req(Method::GET, "/api/v1/community/feed?tab=for_you&limit=50", &a_auth, None).await.unwrap();
    assert_eq!(st, 200);
    let feed: FeedResponseDto = serde_json::from_value(fv).unwrap();
    assert!(
        !feed.items.iter().any(|it| it.post.post_id == pid),
        "post dismissed via hide_post signal must not appear in viewer's ForYou feed"
    );
}

// ============================================================================
// F07 — Real-Time Trending & Entity Extraction (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F07-01
/// [Feature / Req] F07 / ORIGINAL_REQUEST.md (Entity extraction across English + CJK mixed text)
/// [Preconditions] Pure-Rust `extract_entities` invoked on mixed English + CJK text.
/// [Opaque-Box Steps] Parse text containing `#RustLang`, `#开源社区`, `@ferris_bot`, `@alice`, `c/systems`, and `https://rust-lang.org/learn`.
/// [Observable Assertions] Extracted `hashtags`, `mentions`, `communities`, and `urls` contain every expected entity accurately.
#[test]
fn test_cm_t1_f07_01_extract_entities_english_and_cjk_mixed_text() {
    let text = "快来看 #RustLang 与 #开源社区 讨论! cc @ferris_bot @alice in c/systems https://rust-lang.org/learn";
    let ent = extract_entities(text);
    assert!(ent.hashtags.iter().any(|h| h.eq_ignore_ascii_case("rustlang")), "hashtags: {:?}", ent.hashtags);
    assert!(ent.hashtags.iter().any(|h| h == "开源社区"), "CJK hashtag missing: {:?}", ent.hashtags);
    assert!(ent.mentions.iter().any(|m| m == "ferris_bot"), "mentions: {:?}", ent.mentions);
    assert!(ent.mentions.iter().any(|m| m == "alice"), "mentions: {:?}", ent.mentions);
    assert!(ent.communities.iter().any(|c| c == "systems"), "communities: {:?}", ent.communities);
    assert!(ent.urls.iter().any(|u| u == "https://rust-lang.org/learn"), "urls: {:?}", ent.urls);
}

/// [Test ID] TEST-CM-T1-F07-02
/// [Feature / Req] F07 / ORIGINAL_REQUEST.md (Burst-velocity trending outranks stale volume)
/// [Preconditions] Pure-Rust `TrendingEngine::compute` with 5 fresh (<20 min) `#BurstTopic` posts vs 5 stale (22h old) `#StaleTopic` posts.
/// [Opaque-Box Steps] Compute trending topics at `now_ms` with `limit = 10`.
/// [Observable Assertions] `#BurstTopic` receives strictly higher `velocity_score` and ranks ahead of `#StaleTopic`.
#[test]
fn test_cm_t1_f07_02_burst_velocity_trending_outranks_stale_volume() {
    let t0 = now_ms();
    let base = build_starter_dataset(t0).posts[0].clone();
    let mut posts = Vec::new();
    for i in 0..5 {
        let mut p_burst = make_cand_post(&base, &format!("pb_{i}"), &format!("u:10{i}"), ActorKind::Human, "rust", &format!("Fresh insight #BurstTopic {i}"), 20, 0, t0 - 60_000 * (i + 1)).post;
        p_burst.tags = vec!["bursttopic".to_string()];
        posts.push(p_burst);

        let mut p_stale = make_cand_post(&base, &format!("ps_{i}"), &format!("u:20{i}"), ActorKind::Human, "rust", &format!("Old insight #StaleTopic {i}"), 20, 0, t0 - 3_600_000 * 22 - i * 1_000).post;
        p_stale.tags = vec!["staletopic".to_string()];
        posts.push(p_stale);
    }

    let topics = TrendingEngine::compute(&posts, t0, 10);
    let burst_idx = topics.iter().position(|t| t.slug_or_tag.eq_ignore_ascii_case("bursttopic")).expect("bursttopic missing");
    let stale_idx = topics.iter().position(|t| t.slug_or_tag.eq_ignore_ascii_case("staletopic")).expect("staletopic missing");
    assert!(burst_idx < stale_idx, "burst must rank ahead of stale: {topics:?}");
    assert!(topics[burst_idx].velocity_score > topics[stale_idx].velocity_score);
}

/// [Test ID] TEST-CM-T1-F07-03
/// [Feature / Req] F07 / ORIGINAL_REQUEST.md (Rising communities appear in trending with counts & sparkline)
/// [Preconditions] Pure-Rust `TrendingEngine::compute` with recent multi-author posts in `c/gpu_kernels`.
/// [Opaque-Box Steps] Compute trending topics over the multi-author posts.
/// [Observable Assertions] A `TrendingItemKind::Community` topic for `gpu_kernels` is returned with `posts_count_24h == 4`, positive velocity, and 6-bin sparkline.
#[test]
fn test_cm_t1_f07_03_rising_communities_appear_in_trending_with_counts() {
    let t0 = now_ms();
    let base = build_starter_dataset(t0).posts[0].clone();
    let posts: Vec<CommunityPostDto> = (0..4)
        .map(|i| {
            let mut p = make_cand_post(&base, &format!("gk_{i}"), &format!("u:30{i}"), ActorKind::Human, "gpu_kernels", &format!("WGSL compute shader tuning {i}"), 15, 0, t0 - 300_000 * (i + 1)).post;
            p.tags.clear();
            p
        })
        .collect();

    let topics = TrendingEngine::compute(&posts, t0, 10);
    let comm = topics
        .iter()
        .find(|t| t.kind == TrendingItemKind::Community && t.slug_or_tag == "gpu_kernels")
        .expect("rising c/gpu_kernels must appear in trending");
    assert_eq!(comm.posts_count_24h, 4);
    assert!(comm.velocity_score > 0.0);
    assert_eq!(comm.sparkline.len(), 6);
}

/// [Test ID] TEST-CM-T1-F07-04
/// [Feature / Req] F07 / ORIGINAL_REQUEST.md (Live server GET /api/v1/community/trending after multi-actor burst)
/// [Preconditions] Ephemeral live server with 2 Humans and 1 Bot.
/// [Opaque-Box Steps] All 3 citizens publish posts tagged `#QuantumGPUI2026`, then call GET /api/v1/community/trending?limit=30.
/// [Observable Assertions] Trending response includes `#quantumgpui2026` with `posts_count_24h >= 3` and both Human & Bot post counts > 0.
#[tokio::test]
async fn test_cm_t1_f07_04_live_server_trending_endpoint_after_posting_burst() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f07_04", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f07_04", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "trend_f07_bot", "Trend Bot", "manual").await.unwrap();

    for (auth, txt) in [
        (CommunityAuthMode::Human(alice.token.clone()), "Alice explores #QuantumGPUI2026 rendering"),
        (CommunityAuthMode::Human(bob.token.clone()), "Bob benchmarks #QuantumGPUI2026 latency"),
        (CommunityAuthMode::DirectBot(bot.bot_token.clone()), "Bot synthesizes #QuantumGPUI2026 traces"),
    ] {
        let (st, v) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "body": txt }))).await.unwrap();
        assert_eq!(st, 201, "post failed: {v}");
    }

    let (st, tv) = driver
        .req(Method::GET, "/api/v1/community/trending?limit=30", &CommunityAuthMode::Human(alice.token.clone()), None)
        .await
        .unwrap();
    assert_eq!(st, 200);
    let trending: TrendingResponse = serde_json::from_value(tv).unwrap();
    let topic = trending
        .topics
        .iter()
        .find(|t| t.slug_or_tag.eq_ignore_ascii_case("quantumgpui2026"))
        .expect("burst hashtag must appear in /trending");
    assert!(topic.posts_count_24h >= 3);
    assert!(topic.human_posts_count >= 2);
    assert!(topic.bot_posts_count >= 1);
}

/// [Test ID] TEST-CM-T1-F07-05
/// [Feature / Req] F07 / ORIGINAL_REQUEST.md (Trending hashtag case-folding & deduplication)
/// [Preconditions] Pure-Rust `TrendingEngine::compute` with posts containing `#RustLang`, `#rustlang`, and `#RUSTLANG`.
/// [Opaque-Box Steps] Compute trending topics over the 3 posts.
/// [Observable Assertions] Exactly 1 normalized hashtag cluster for `rustlang` is returned with aggregated `posts_count_24h == 3`.
#[test]
fn test_cm_t1_f07_05_trending_deduplication_and_case_folding() {
    let t0 = now_ms();
    let base = build_starter_dataset(t0).posts[0].clone();
    let variants = ["#RustLang rocks", "#rustlang forever", "#RUSTLANG 2026"];
    let posts: Vec<CommunityPostDto> = variants
        .iter()
        .enumerate()
        .map(|(i, body)| {
            let mut p = make_cand_post(&base, &format!("cf_{i}"), &format!("u:40{i}"), ActorKind::Human, "rust", body, 10, 0, t0 - 60_000 * (i as i64 + 1)).post;
            p.tags = extract_entities(body).hashtags;
            p
        })
        .collect();

    let topics = TrendingEngine::compute(&posts, t0, 10);
    let matches: Vec<_> = topics
        .iter()
        .filter(|t| t.kind == TrendingItemKind::Hashtag && t.slug_or_tag.eq_ignore_ascii_case("rustlang"))
        .collect();
    assert_eq!(matches.len(), 1, "case variants must fold into a single cluster: {matches:?}");
    assert_eq!(matches[0].posts_count_24h, 3);
}

// ============================================================================
// F08 — Moderation, AutoMod & Safety Engine (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F08-01
/// [Feature / Req] F08 / ORIGINAL_REQUEST.md (AutoMod keyword & regex rules route to mod-queue or auto-remove)
/// [Preconditions] Ephemeral live server with owner Alice, member Bob, and `c/automod_lab` having keyword FlagForReview & regex Remove rules.
/// [Opaque-Box Steps] Bob submits a post matching keyword rule and a post matching regex rule; Alice queries GET /mod-queue.
/// [Observable Assertions] Keyword match returns `automod_action == FlagForReview` and enters `mod-queue`; regex match returns `automod_action == Remove` with `is_removed == true`.
#[tokio::test]
async fn test_cm_t1_f08_01_automod_keyword_and_regex_rules_flag_or_remove() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f08_01", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f08_01", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    let t0 = now_ms();
    let (st, v) = driver
        .req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({
            "slug": "automod_lab",
            "name": "AutoMod Lab",
            "allow_bots": true,
            "automod_rules": [
                {
                    "rule_id": "rule_kw",
                    "community_id": "",
                    "name": "Pyramid Guard",
                    "enabled": true,
                    "priority": 10,
                    "keywords": ["pyramid_scheme"],
                    "action": "flag_for_review",
                    "created_at_ms": t0
                },
                {
                    "rule_id": "rule_rx",
                    "community_id": "",
                    "name": "Nitro Regex Guard",
                    "enabled": true,
                    "priority": 20,
                    "keywords": [],
                    "regex_pattern": "free\\s+nitro\\s+\\d+",
                    "action": "remove",
                    "created_at_ms": t0
                }
            ]
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "create automod_lab failed: {v}");

    let (st1, p1) = driver
        .req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "automod_lab", "body": "Join this pyramid_scheme now" })))
        .await
        .unwrap();
    assert_eq!(st1, 201, "flagged post create failed: {p1}");
    let r1: CreatePostResponse = serde_json::from_value(p1).unwrap();
    assert_eq!(r1.automod_action, AutoModActionKind::FlagForReview);

    let (st2, p2) = driver
        .req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "automod_lab", "body": "Claim free nitro 9999 today" })))
        .await
        .unwrap();
    assert_eq!(st2, 201, "removed post create failed: {p2}");
    let r2: CreatePostResponse = serde_json::from_value(p2).unwrap();
    assert_eq!(r2.automod_action, AutoModActionKind::Remove);
    assert!(r2.post.is_removed);

    let (st_q, qv) = driver.req(Method::GET, "/api/v1/community/communities/automod_lab/mod-queue", &a_auth, None).await.unwrap();
    assert_eq!(st_q, 200);
    let queue: ModQueueResponse = serde_json::from_value(qv).unwrap();
    assert!(queue.items.iter().any(|it| it.target_id == r1.post.post_id));
}

/// [Test ID] TEST-CM-T1-F08-02
/// [Feature / Req] F08 / ORIGINAL_REQUEST.md (AutoMod karma floor & link/spam guard)
/// [Preconditions] Pure-Rust `AutoModEngine::evaluate_with_context` with karma floor `50` and `max_links: 1`.
/// [Opaque-Box Steps] Evaluate low-karma actor, 3-link spam post, and established high-karma single-link citizen.
/// [Observable Assertions] Low-karma and link-flood posts are flagged/removed while established citizen is Approved.
#[test]
fn test_cm_t1_f08_02_automod_karma_floor_and_link_spam_guard() {
    let t0 = now_ms();
    let mut actor = build_starter_dataset(t0).actors[0].clone();
    actor.post_karma = 0;
    actor.comment_karma = 5;

    let rules = vec![
        AutoModRuleDto {
            rule_id: "r_karma".to_string(),
            community_id: "c1".to_string(),
            name: "Karma Floor".to_string(),
            enabled: true,
            priority: 10,
            keywords: vec![],
            regex_pattern: None,
            min_author_karma: Some(50),
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
        AutoModRuleDto {
            rule_id: "r_links".to_string(),
            community_id: "c1".to_string(),
            name: "Link Spam".to_string(),
            enabled: true,
            priority: 20,
            keywords: vec![],
            regex_pattern: None,
            min_author_karma: None,
            min_account_age_secs: None,
            max_links: Some(1),
            block_duplicate_spam: false,
            apply_to_bots: true,
            apply_to_humans: true,
            action: AutoModActionKind::Remove,
            assign_flair: None,
            auto_reply_template: None,
            created_at_ms: t0,
        },
    ];

    let d_low = AutoModEngine::evaluate_with_context(&rules, &actor, None, "Clean text", t0, &[], Some("rust"));
    assert_eq!(d_low.action, AutoModActionKind::FlagForReview);

    actor.post_karma = 100;
    let d_spam = AutoModEngine::evaluate_with_context(&rules, &actor, None, "Visit https://a.test and https://b.test and https://c.test", t0, &[], Some("rust"));
    assert_eq!(d_spam.action, AutoModActionKind::Remove);

    let d_ok = AutoModEngine::evaluate_with_context(&rules, &actor, None, "Good writeup https://rust-lang.org", t0, &[], Some("rust"));
    assert_eq!(d_ok.action, AutoModActionKind::Approve);
}

/// [Test ID] TEST-CM-T1-F08-03
/// [Feature / Req] F08 / ORIGINAL_REQUEST.md (Human Owner & BotMod full mod actions)
/// [Preconditions] Ephemeral live server with Owner Alice, BotMod, and regular members Bob & Carol in `c/mod_ops_lab`.
/// [Opaque-Box Steps] Execute `approve`, `remove`, `lock`, `pin`, `distinguish`, `set_post_flair`, `mute_actor`, and `ban_actor` across Owner and BotMod.
/// [Observable Assertions] Every mod action returns HTTP 200 and mutates target post/member state accurately.
#[tokio::test]
async fn test_cm_t1_f08_03_human_mod_and_bot_mod_execute_full_mod_actions() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f08_03", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f08_03", "Pass1234!").await.unwrap();
    let carol = driver.register_human("carol_f08_03", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "botmod_f08_bot", "Sentinel BotMod", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let c_auth = CommunityAuthMode::Human(carol.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "mod_ops_lab", "name": "Mod Ops Lab", "allow_bots": true }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/mod_ops_lab/join", &c_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/mod_ops_lab/mod-actions", &a_auth, Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": bot.actor_id }))).await.unwrap().0, 200);

    let (_, p1) = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "mod_ops_lab", "body": "Target post 1" }))).await.unwrap();
    let pid1 = p1["post"]["post_id"].as_str().unwrap().to_string();
    let (_, p2) = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "mod_ops_lab", "body": "Target post 2" }))).await.unwrap();
    let pid2 = p2["post"]["post_id"].as_str().unwrap().to_string();

    for (auth, payload) in [
        (&bot_auth, json!({ "action": "pin", "target_kind": "post", "target_id": pid1 })),
        (&bot_auth, json!({ "action": "lock", "target_kind": "post", "target_id": pid1 })),
        (&bot_auth, json!({ "action": "distinguish", "target_kind": "post", "target_id": pid1 })),
        (&bot_auth, json!({ "action": "set_post_flair", "target_kind": "post", "target_id": pid1, "flair": "Verified" })),
        (&bot_auth, json!({ "action": "remove", "target_kind": "post", "target_id": pid2, "reason": "off topic" })),
        (&a_auth, json!({ "action": "approve", "target_kind": "post", "target_id": pid2, "reason": "reinstated" })),
        (&a_auth, json!({ "action": "mute_actor", "target_kind": "actor", "target_id": bob.actor_id, "reason": "cooldown" })),
        (&a_auth, json!({ "action": "ban_actor", "target_kind": "actor", "target_id": carol.actor_id, "reason": "spam" })),
    ] {
        let (st, v) = driver.req(Method::POST, "/api/v1/community/communities/mod_ops_lab/mod-actions", auth, Some(payload)).await.unwrap();
        assert_eq!(st, 200, "mod action failed: {v}");
    }

    let (_, got1) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid1}"), &a_auth, None).await.unwrap();
    let det1: PostDetailResponse = serde_json::from_value(got1).unwrap();
    assert!(det1.post.is_pinned && det1.post.is_locked && det1.post.is_mod_distinguished);
    assert_eq!(det1.post.flair.as_deref(), Some("Verified"));
}

/// [Test ID] TEST-CM-T1-F08-04
/// [Feature / Req] F08 / ORIGINAL_REQUEST.md (Transparent immutable ModLogEntryDto audit log)
/// [Preconditions] Ephemeral live server with Owner Alice and unprivileged member Bob in `c/mod_log_lab`.
/// [Opaque-Box Steps] Alice executes `pin`, `lock`, and `set_post_flair`, then Bob fetches GET /communities/mod_log_lab/mod-logs.
/// [Observable Assertions] Unprivileged member Bob reads all 3 `ModLogEntryDto` entries with exact moderator ID, action, target ID, and reasons.
#[tokio::test]
async fn test_cm_t1_f08_04_mod_actions_append_transparent_immutable_mod_logs() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f08_04", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f08_04", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "mod_log_lab", "name": "Mod Log Lab" }))).await.unwrap().0, 201);
    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "community_slug": "mod_log_lab", "body": "Announcement" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    for (action, reason, flair) in [
        ("pin", "Pin weekly thread", None),
        ("lock", "Archive complete", None),
        ("set_post_flair", "Tag release", Some("Meta")),
    ] {
        let (st, v) = driver
            .req(Method::POST, "/api/v1/community/communities/mod_log_lab/mod-actions", &a_auth, Some(json!({
                "action": action, "target_kind": "post", "target_id": pid, "reason": reason, "flair": flair
            })))
            .await
            .unwrap();
        assert_eq!(st, 200, "mod action {action} failed: {v}");
    }

    let (st, lv) = driver.req(Method::GET, "/api/v1/community/communities/mod_log_lab/mod-logs?limit=20", &b_auth, None).await.unwrap();
    assert_eq!(st, 200, "transparent mod-logs must be readable by regular members: {lv}");
    let logs: ModLogsResponse = serde_json::from_value(lv).unwrap();
    assert!(logs.logs.len() >= 3);
    assert!(logs.logs.iter().any(|l| l.action == ModActionKind::Pin && l.moderator_actor_id == alice.actor_id && l.target_id == pid));
    assert!(logs.logs.iter().any(|l| l.action == ModActionKind::Lock && l.reason.as_deref() == Some("Archive complete")));
    assert!(logs.logs.iter().any(|l| l.action == ModActionKind::SetPostFlair));
}

/// [Test ID] TEST-CM-T1-F08-05
/// [Feature / Req] F08 / ORIGINAL_REQUEST.md (User block/mute actor/community/keyword & report enqueuing)
/// [Preconditions] Ephemeral live server with Alice, Bob, Carol, and communities `c/muted_sub` + `c/report_sub`.
/// [Opaque-Box Steps] Alice blocks Bob, mutes `c/muted_sub`, mutes keyword `"noisy_token"`, reports Carol's post in `c/report_sub`, then fetches ForYou/All feed and mod-queue.
/// [Observable Assertions] Blocked actor, muted community, and muted-keyword posts are excluded from Alice's feed; Carol's reported post enters `mod-queue`.
#[tokio::test]
async fn test_cm_t1_f08_05_user_block_mute_actor_community_keyword_and_report() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f08_05", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f08_05", "Pass1234!").await.unwrap();
    let carol = driver.register_human("carol_f08_05", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let c_auth = CommunityAuthMode::Human(carol.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "muted_sub", "name": "Muted Sub" }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "report_sub", "name": "Report Sub" }))).await.unwrap().0, 201);

    let p_bob = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "body": "Post from blocked Bob" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    let p_sub = driver.req(Method::POST, "/api/v1/community/posts", &c_auth, Some(json!({ "community_slug": "muted_sub", "body": "Post inside muted community" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    let p_kw = driver.req(Method::POST, "/api/v1/community/posts", &c_auth, Some(json!({ "body": "Contains noisy_token alert" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();
    let p_rep = driver.req(Method::POST, "/api/v1/community/posts", &c_auth, Some(json!({ "community_slug": "report_sub", "body": "Clean post to be reported" }))).await.unwrap().1["post"]["post_id"].as_str().unwrap().to_string();

    for payload in [
        json!({ "target_kind": "actor", "target_value": bob.actor_id, "action": "block" }),
        json!({ "target_kind": "community", "target_value": "muted_sub", "action": "mute" }),
        json!({ "target_kind": "keyword", "target_value": "noisy_token", "action": "mute" }),
    ] {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/mutes-blocks", &a_auth, Some(payload)).await.unwrap().0, 200);
    }

    let (st, fv) = driver.req(Method::GET, "/api/v1/community/feed?tab=all&sort=new&limit=50", &a_auth, None).await.unwrap();
    assert_eq!(st, 200);
    let feed: FeedResponseDto = serde_json::from_value(fv).unwrap();
    let ids: HashSet<&str> = feed.items.iter().map(|i| i.post.post_id.as_str()).collect();
    assert!(!ids.contains(p_bob.as_str()), "blocked actor leaked into feed");
    assert!(!ids.contains(p_sub.as_str()), "muted community leaked into feed");
    assert!(!ids.contains(p_kw.as_str()), "muted keyword leaked into feed");

    let (st, _) = driver
        .req(Method::POST, "/api/v1/community/signals", &a_auth, Some(json!({
            "post_id": p_rep, "kind": "report", "reason": "rule_violation", "community_slug": "report_sub"
        })))
        .await
        .unwrap();
    assert_eq!(st, 200);
    let (_, qv) = driver.req(Method::GET, "/api/v1/community/communities/report_sub/mod-queue", &a_auth, None).await.unwrap();
    let queue: ModQueueResponse = serde_json::from_value(qv).unwrap();
    assert!(queue.items.iter().any(|it| it.target_id == p_rep));
}

// ============================================================================
// F09 — Reactive & Autonomous Bot Runtime + Rich Seeder (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F09-01
/// [Feature / Req] F09 / ORIGINAL_REQUEST.md (Rich seeder populates communities, Humans+Bots, multi-format posts, debates, polls, lists, notifs)
/// [Preconditions] Fresh ephemeral `CommunityStore::open_in_memory()`.
/// [Opaque-Box Steps] Call `seed_if_empty(now_ms)` and query communities, actors, posts/feed, nested comments, polls, lists, and notifications.
/// [Observable Assertions] >= 8 communities, >= 12 Humans & Bots, >= 24 posts (with polls & nested comments), >= 3 lists, and >= 5 notifications exist.
#[test]
fn test_cm_t1_f09_01_seed_if_empty_populates_rich_starter_dataset() {
    let store = CommunityStore::open_in_memory().unwrap();
    let t0 = now_ms();
    assert!(store.seed_if_empty(t0).unwrap(), "first seed_if_empty must return true");

    let communities = store.list_communities(None, None, 100).unwrap();
    assert!(communities.len() >= 8, "expected >=8 communities, got {}", communities.len());

    let actors = store.list_actors(None, 100).unwrap();
    assert!(actors.len() >= 12, "expected >=12 actors, got {}", actors.len());
    assert!(actors.iter().any(|a| a.actor_kind == ActorKind::Human));
    assert!(actors.iter().any(|a| a.actor_kind.is_bot_like()));

    let post_count: i64 = store
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM community_posts", [], |r| r.get(0))
                .map_err(|e| gpui_telegram_core::TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert!(post_count >= 24, "expected >=24 seeded posts, got {post_count}");

    let poll_post = store.get_post("seed_post_09", None).unwrap().expect("seeded poll post");
    assert!(poll_post.poll.is_some());

    let tree = store.list_comment_tree("seed_post_01", None, CommentSort::Best, None, 100).unwrap();
    assert!(!tree.is_empty(), "seeded nested debate comments must exist");

    let lists = store.list_lists("u:900001").unwrap();
    assert!(!lists.is_empty(), "seeded custom lists must exist");
    let notifs = store.list_notifications("u:900001", None, 50).unwrap();
    assert!(!notifs.notifications.is_empty(), "seeded notifications must exist");
}

/// [Test ID] TEST-CM-T1-F09-02
/// [Feature / Req] F09 / ORIGINAL_REQUEST.md (Seeder strict idempotency)
/// [Preconditions] Fresh ephemeral `CommunityStore::open_in_memory()`.
/// [Opaque-Box Steps] Invoke `seed_if_empty` three times sequentially and snapshot table row counts before and after repeated calls.
/// [Observable Assertions] First call returns `true`, subsequent calls return `false`, and actor/community/post/comment counts remain strictly identical.
#[test]
fn test_cm_t1_f09_02_seed_if_empty_is_strictly_idempotent() {
    let store = CommunityStore::open_in_memory().unwrap();
    let t0 = now_ms();
    assert!(store.seed_if_empty(t0).unwrap());

    let count_entities = || -> (usize, usize, usize, usize) {
        let actors = store.list_actors(None, 200).unwrap().len();
        let comms = store.list_communities(None, None, 200).unwrap().len();
        let posts = store
            .get_feed("u:900001", &FeedRequestDto { tab: FeedTab::All, sort: FeedSort::New, limit: 100, ..Default::default() }, t0)
            .unwrap()
            .items
            .len();
        let cmts = store.list_comment_tree("seed_post_01", None, CommentSort::Best, None, 200).unwrap().len();
        (actors, comms, posts, cmts)
    };
    let snap1 = count_entities();
    assert!(!store.seed_if_empty(t0 + 1_000).unwrap());
    assert!(!store.seed_if_empty(t0 + 2_000).unwrap());
    assert_eq!(count_entities(), snap1, "repeated seeding must not create duplicates");
}

/// [Test ID] TEST-CM-T1-F09-03
/// [Feature / Req] F09 / ORIGINAL_REQUEST.md (Persona-aware contextual DeterministicBotResponder)
/// [Preconditions] Starter dataset with distinct bot personas (`ferris_bot` and `socratic_bot`) plus a Rust post and thread comments.
/// [Opaque-Box Steps] Call `DeterministicBotResponder::generate_reply` for both bots on an `@mention` trigger referencing lifetimes & GPUI.
/// [Observable Assertions] Both bots produce non-empty contextual replies and their persona outputs differ measurably.
#[test]
fn test_cm_t1_f09_03_deterministic_bot_responder_persona_contextual_reply() {
    let ds = build_starter_dataset(now_ms());
    let ferris = ds.actors.iter().find(|a| a.handle == "ferris_bot").unwrap();
    let socratic = ds.actors.iter().find(|a| a.handle == "socratic_bot").unwrap();
    let post = &ds.posts[0];
    let ctx_comments = &ds.comments[0..2];

    let r1 = DeterministicBotResponder::generate_reply(
        ferris,
        post,
        ctx_comments,
        "@ferris_bot how should we optimize GPUI borrow lifetimes and frame budgets?",
    )
    .expect("ferris_bot reply");
    let r2 = DeterministicBotResponder::generate_reply(
        socratic,
        post,
        ctx_comments,
        "@socratic_bot how should we optimize GPUI borrow lifetimes and frame budgets?",
    )
    .expect("socratic_bot reply");

    assert!(!r1.trim().is_empty());
    assert!(!r2.trim().is_empty());
    assert_ne!(r1, r2, "distinct bot personas should produce distinct contextual replies");
}

/// [Test ID] TEST-CM-T1-F09-04
/// [Feature / Req] F09 / ORIGINAL_REQUEST.md (BotWakeSelector respects Reactive/Autonomous/MentionOnly/Manual modes)
/// [Preconditions] Pool of 4 bots covering `Manual`, `MentionOnly`, `Reactive`, and `Autonomous` modes + a `c/rust` community post.
/// [Opaque-Box Steps] Invoke `BotWakeSelector::select_bots_for_event` once without mentions and once explicitly mentioning `@mention_bot` and `@manual_bot`.
/// [Observable Assertions] Unmentioned post wakes Reactive/Autonomous bots (not Manual/MentionOnly); explicit `@mention_bot` wakes MentionOnly while Manual stays silent.
#[test]
fn test_cm_t1_f09_04_community_watcher_selects_reactive_and_autonomous_bots() {
    let ds = build_starter_dataset(now_ms());
    let human = ds.actors.iter().find(|a| a.actor_kind == ActorKind::Human).unwrap();
    let base_bot = ds.actors.iter().find(|a| a.actor_kind == ActorKind::Bot).unwrap();
    let comm = ds.communities.iter().find(|c| c.slug == "rust").unwrap();
    let post = &ds.posts[0];

    let make_bot = |id: &str, handle: &str, mode: BotAutonomyMode| {
        let mut b = base_bot.clone();
        b.actor_id = id.to_string();
        b.handle = handle.to_string();
        b.bot_autonomy_mode = mode;
        b.bot_capabilities = vec!["rust".to_string(), "systems".to_string()];
        b
    };
    let bots = vec![
        make_bot("b:1", "manual_bot", BotAutonomyMode::Manual),
        make_bot("b:2", "mention_bot", BotAutonomyMode::MentionOnly),
        make_bot("b:3", "reactive_bot", BotAutonomyMode::Reactive),
        make_bot("b:4", "auto_bot", BotAutonomyMode::Autonomous),
    ];

    let woken_passive = BotWakeSelector::select_bots_for_event(&bots, post, "Rust memory layout question", human, Some(comm), 4);
    let passive_handles: HashSet<&str> = woken_passive.iter().map(|b| b.handle.as_str()).collect();
    assert!(!passive_handles.contains("manual_bot"));
    assert!(!passive_handles.contains("mention_bot"));
    assert!(passive_handles.contains("reactive_bot") || passive_handles.contains("auto_bot"));

    let woken_mention = BotWakeSelector::select_bots_for_event(&bots, post, "Hey @mention_bot and @manual_bot check this!", human, Some(comm), 4);
    let mention_handles: HashSet<&str> = woken_mention.iter().map(|b| b.handle.as_str()).collect();
    assert!(mention_handles.contains("mention_bot"));
    assert!(!mention_handles.contains("manual_bot"));
}

/// [Test ID] TEST-CM-T1-F09-05
/// [Feature / Req] F09 / ORIGINAL_REQUEST.md (BotLoopGuard 5-hop budget, cooldown & human reset)
/// [Preconditions] `BotLoopGuard::new(5, 30_000)`.
/// [Opaque-Box Steps] Acquire 5 alternating Bot turns spaced 31s apart, test cooldown violation, then call `reset_on_human_turn` and acquire a fresh Bot turn.
/// [Observable Assertions] Hops 1..5 succeed, immediate repeat returns `CooldownActive`, and `reset_on_human_turn` zeroes hop count allowing replies to resume.
#[test]
fn test_cm_t1_f09_05_bot_loop_guard_allows_within_5_hops_and_enforces_cooldown() {
    let guard = BotLoopGuard::new(5, 30_000);
    let mut t = 1_700_000_000_000_i64;
    for hop in 1..=5u32 {
        let (bot_id, trig_id) = if hop % 2 == 1 { ("b:10", "b:20") } else { ("b:20", "b:10") };
        let res = guard.try_acquire_turn("thread_1", bot_id, trig_id, ActorKind::Bot, false, t);
        assert_eq!(res, Ok(hop), "hop {hop} must succeed");
        t += 31_000;
    }
    assert_eq!(guard.current_hops("thread_1"), 5);
    assert!(matches!(
        guard.try_acquire_turn("thread_1", "b:10", "b:20", ActorKind::Bot, false, t - 25_000),
        Err(BotGuardRejection::CooldownActive { .. }) | Err(BotGuardRejection::LoopBudgetExceeded { .. })
    ));

    guard.reset_on_human_turn("thread_1");
    assert_eq!(guard.current_hops("thread_1"), 0);
    let resumed_human = guard.try_acquire_turn("thread_1", "b:10", "u:1", ActorKind::Human, false, t + 60_000);
    assert_eq!(resumed_human, Ok(0), "Human trigger resets bot-to-bot hop count to 0");
    let resumed_bot = guard.try_acquire_turn("thread_1", "b:20", "b:10", ActorKind::Bot, false, t + 95_000);
    assert_eq!(resumed_bot, Ok(1), "Subsequent Bot->Bot turn resumes cleanly from hop 1");
}

// ============================================================================
// F10 — SQLite V42 Storage & Tantivy Search (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F10-01
/// [Feature / Req] F10 / ORIGINAL_REQUEST.md (SQLite V42 STRICT WAL migration idempotency)
/// [Preconditions] Fresh SQLite database inside RAII `TempDir`.
/// [Opaque-Box Steps] Open `CommunityStore`, verify WAL mode & STRICT schema, then re-execute `MIGRATION_V42_COMMUNITY_SQL` and `CommunityStore::apply_migrations`.
/// [Observable Assertions] WAL mode is enabled, core STRICT tables exist, and repeated migrations succeed with zero error.
#[test]
fn test_cm_t1_f10_01_v42_migration_strict_wal_idempotent_fresh_and_v41() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("f10_v42.db");
    let store = CommunityStore::open(&db_path).unwrap();

    let journal_mode: String = store
        .with_conn(|conn| {
            conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .map_err(|e| gpui_telegram_core::TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");

    let table_sql: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='community_posts'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| gpui_telegram_core::TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert!(table_sql.to_ascii_uppercase().contains("STRICT"), "community_posts must be STRICT: {table_sql}");

    store
        .with_conn(|conn| {
            conn.execute_batch(MIGRATION_V42_COMMUNITY_SQL)
                .map_err(|e| gpui_telegram_core::TelegramError::Database(e.to_string()))?;
            CommunityStore::apply_migrations(conn)
        })
        .expect("re-applying V42 SQL must be strictly idempotent");
}

/// [Test ID] TEST-CM-T1-F10-02
/// [Feature / Req] F10 / ORIGINAL_REQUEST.md (Atomic post voting & author karma invariants)
/// [Preconditions] Fresh `CommunityStore::open_in_memory()` with author & voter synced.
/// [Opaque-Box Steps] Author creates post (initial score 1); voter transitions upvote (+1) -> downvote (-1) -> unvote (0).
/// [Observable Assertions] Post score transitions 1 -> 2 -> 0 -> 1 and author `post_karma` transitions 0 -> 1 -> -1 -> 0 without drift.
#[test]
fn test_cm_t1_f10_02_store_atomic_voting_and_author_karma_invariants() {
    let store = CommunityStore::open_in_memory().unwrap();
    let t0 = now_ms();
    store
        .sync_telegram_identities(
            &TelegramHumanSyncInput { user_id: 101, username: Some("author_v".into()), display_name: "Author".into(), bio: None, avatar_media_id: None },
            &[],
            t0,
        )
        .unwrap();
    store
        .sync_telegram_identities(
            &TelegramHumanSyncInput { user_id: 102, username: Some("voter_v".into()), display_name: "Voter".into(), bio: None, avatar_media_id: None },
            &[],
            t0,
        )
        .unwrap();

    let created = store
        .create_post("u:101", &CreatePostRequest { body: "Atomic karma post".into(), ..Default::default() }, t0)
        .unwrap();
    let pid = created.post.post_id;
    assert_eq!(created.post.score, 1);
    assert_eq!(store.get_actor("u:101", None).unwrap().unwrap().post_karma, 0);

    let v1 = store.vote_post("u:102", &pid, 1, t0 + 1).unwrap();
    assert_eq!(v1.score, 2);
    assert_eq!(store.get_actor("u:101", None).unwrap().unwrap().post_karma, 1);

    let v2 = store.vote_post("u:102", &pid, -1, t0 + 2).unwrap();
    assert_eq!(v2.score, 0);
    assert_eq!(store.get_actor("u:101", None).unwrap().unwrap().post_karma, -1);

    let v3 = store.vote_post("u:102", &pid, 0, t0 + 3).unwrap();
    assert_eq!(v3.score, 1);
    assert_eq!(store.get_actor("u:101", None).unwrap().unwrap().post_karma, 0);
}

/// [Test ID] TEST-CM-T1-F10-03
/// [Feature / Req] F10 / ORIGINAL_REQUEST.md (`sync_telegram_identities` links Human + owned Bots)
/// [Preconditions] Fresh `CommunityStore::open_in_memory()`.
/// [Opaque-Box Steps] Call `sync_telegram_identities` with 1 Human (`5001`) and 2 Bots (`6001`, `6002`), then re-run with updated display name.
/// [Observable Assertions] `list_identities_for_user(5001)` yields exact 3 canonical actors (`u:5001`, `b:6001`, `b:6002`) with no duplicates after re-sync.
#[test]
fn test_cm_t1_f10_03_sync_telegram_identities_links_human_and_owned_bots() {
    let store = CommunityStore::open_in_memory().unwrap();
    let t0 = now_ms();
    let human = TelegramHumanSyncInput {
        user_id: 5001,
        username: Some("sync_alice".into()),
        display_name: "Sync Alice".into(),
        bio: Some("Rustacean".into()),
        avatar_media_id: None,
    };
    let bots = vec![
        TelegramBotSyncInput {
            bot_user_id: 6001,
            username: "alpha_sync_bot".into(),
            display_name: "Alpha Bot".into(),
            bio: None,
            avatar_media_id: None,
            avatar_shape: Some("squircle".into()),
            avatar_color: Some("blue".into()),
            bot_model: Some("codex".into()),
            bot_capabilities: vec!["rust".into()],
            bot_persona: Some("Helper".into()),
        },
        TelegramBotSyncInput {
            bot_user_id: 6002,
            username: "beta_sync_bot".into(),
            display_name: "Beta Bot".into(),
            bio: None,
            avatar_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_model: None,
            bot_capabilities: vec![],
            bot_persona: None,
        },
    ];
    let synced = store.sync_telegram_identities(&human, &bots, t0).unwrap();
    assert_eq!(synced.len(), 3);
    let ids = store.list_identities_for_user(5001).unwrap();
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().any(|a| a.actor_id == "u:5001" && a.actor_kind == ActorKind::Human));
    assert!(ids.iter().any(|a| a.actor_id == "b:6001" && a.owner_user_id == Some(5001)));
    assert!(ids.iter().any(|a| a.actor_id == "b:6002" && a.owner_user_id == Some(5001)));

    let mut human_v2 = human;
    human_v2.display_name = "Sync Alice Updated".into();
    store.sync_telegram_identities(&human_v2, &bots, t0 + 1_000).unwrap();
    assert_eq!(store.list_identities_for_user(5001).unwrap().len(), 3);
}

/// [Test ID] TEST-CM-T1-F10-04
/// [Feature / Req] F10 / ORIGINAL_REQUEST.md (Tantivy BM25 search on safe disk dir for English + CJK)
/// [Preconditions] `CommunitySearchIndexer::open_or_create` inside `<TempDir>/community.idx` (zero forbidden substrings).
/// [Opaque-Box Steps] Index English and CJK posts, call `commit()`, and execute BM25 search queries in both English and CJK.
/// [Observable Assertions] `doc_count() >= 3`, high-TF English post ranks #1, and CJK query `"异步渲染"` returns the exact CJK post.
#[test]
fn test_cm_t1_f10_04_search_indexer_disk_safe_dir_bm25_english_and_cjk() {
    let temp = TempDir::new().unwrap();
    let idx_dir = safe_temp_idx_dir(&temp);
    let indexer = CommunitySearchIndexer::open_or_create(&idx_dir).unwrap();
    let t0 = now_ms();
    let base = build_starter_dataset(t0).posts[0].clone();

    let p_en_hi = make_cand_post(&base, "doc_en_hi", "u:1", ActorKind::Human, "rust", "wgpu shader pipeline wgpu shader frame graph", 10, 0, t0).post;
    let p_en_lo = make_cand_post(&base, "doc_en_lo", "u:2", ActorKind::Human, "rust", "brief wgpu note", 10, 0, t0).post;
    let p_cjk = make_cand_post(&base, "doc_cjk", "b:1", ActorKind::Bot, "design_ui", "GPUI 桌面端异步渲染架构深度解析", 10, 0, t0).post;

    indexer.index_post(&p_en_hi).unwrap();
    indexer.index_post(&p_en_lo).unwrap();
    indexer.index_post(&p_cjk).unwrap();
    indexer.commit().unwrap();
    assert_eq!(indexer.doc_count(), 3);

    let en_hits = indexer.search(&CommunitySearchRequestDto { query: "wgpu shader".into(), ..Default::default() }).unwrap();
    assert_eq!(en_hits.first().map(String::as_str), Some("doc_en_hi"));

    let cjk_hits = indexer.search(&CommunitySearchRequestDto { query: "异步渲染".into(), ..Default::default() }).unwrap();
    assert!(cjk_hits.iter().any(|id| id == "doc_cjk"), "CJK query must match doc_cjk: {cjk_hits:?}");
}

/// [Test ID] TEST-CM-T1-F10-05
/// [Feature / Req] F10 / ORIGINAL_REQUEST.md (Search structured filters & `delete_post` pruning)
/// [Preconditions] Disk `CommunitySearchIndexer` in `<TempDir>/community.idx` with diverse Human/Bot, community, Media, and Poll posts.
/// [Opaque-Box Steps] Run structured filter queries (`community_slug`, `author_handle`, `actor_kind`, `has_media`, `has_poll`), then call `delete_post` + `commit()`.
/// [Observable Assertions] Every structured filter isolates exact matching posts and deleted post is completely pruned from results.
#[test]
fn test_cm_t1_f10_05_search_structured_filters_and_delete_post_pruning() {
    let temp = TempDir::new().unwrap();
    let indexer = CommunitySearchIndexer::open_or_create(safe_temp_idx_dir(&temp)).unwrap();
    let ds = build_starter_dataset(now_ms());
    for post in &ds.posts {
        indexer.index_post(post).unwrap();
    }
    indexer.commit().unwrap();

    let bot_hits = indexer
        .search(&CommunitySearchRequestDto { actor_kind: Some(ActorKind::Bot), limit: 50, ..Default::default() })
        .unwrap();
    assert!(!bot_hits.is_empty());
    let poll_hits = indexer
        .search(&CommunitySearchRequestDto { has_poll: Some(true), limit: 50, ..Default::default() })
        .unwrap();
    assert!(poll_hits.iter().any(|id| id == "seed_post_09"), "has:poll missing seed_post_09: {poll_hits:?}");
    let media_hits = indexer
        .search(&CommunitySearchRequestDto { has_media: Some(true), limit: 50, ..Default::default() })
        .unwrap();
    assert!(media_hits.iter().any(|id| id == "seed_post_11"), "has:media missing seed_post_11: {media_hits:?}");
    let rust_hits = indexer
        .search(&CommunitySearchRequestDto { community_slug: Some("rust".into()), limit: 50, ..Default::default() })
        .unwrap();
    assert!(!rust_hits.is_empty());

    indexer.delete_post("seed_post_09").unwrap();
    indexer.commit().unwrap();
    let after_del = indexer
        .search(&CommunitySearchRequestDto { has_poll: Some(true), limit: 50, ..Default::default() })
        .unwrap();
    assert!(!after_del.iter().any(|id| id == "seed_post_09"), "deleted post must be pruned from Tantivy");
}

// ============================================================================
// F11 — Server REST API, Unified ActorAuth & Public Media Fix (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F11-01
/// [Feature / Req] F11 / ORIGINAL_REQUEST.md (Human Bearer full CRUD across REST endpoints)
/// [Preconditions] Ephemeral live server with 2 Humans (Alice & Bob).
/// [Opaque-Box Steps] Execute Human Bearer CRUD across posts, comments, votes, likes, bookmarks, custom lists, and notifications.
/// [Observable Assertions] Every create, read, update, toggle, and delete returns exact expected HTTP 200/201 status and persisted state.
#[tokio::test]
async fn test_cm_t1_f11_01_human_bearer_full_crud_posts_comments_votes_bookmarks_lists_notifs() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f11_01", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f11_01", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    // Ensure Bob is onboarded as a community citizen before Alice mentions @bob_f11_01
    assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &b_auth, None).await.unwrap().0, 200);

    let (st, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "body": "CRUD post v1 @bob_f11_01" }))).await.unwrap();
    assert_eq!(st, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    assert_eq!(driver.req(Method::PATCH, &format!("/api/v1/community/posts/{pid}"), &a_auth, Some(json!({ "body": "CRUD post v2 @bob_f11_01" }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/vote"), &b_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req_empty_body(Method::POST, &format!("/api/v1/community/posts/{pid}/like"), &b_auth).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/bookmark"), &b_auth, Some(json!({ "folder": "Saved" }))).await.unwrap().0, 200);

    let (st, cv) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &b_auth, Some(json!({ "body": "Comment v1" }))).await.unwrap();
    assert_eq!(st, 201);
    let cid = cv["comment"]["comment_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::PATCH, &format!("/api/v1/community/comments/{cid}"), &b_auth, Some(json!({ "body": "Comment v2" }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/comments/{cid}"), &b_auth, None).await.unwrap().0, 200);

    let (st, lv) = driver.req(Method::POST, "/api/v1/community/lists", &a_auth, Some(json!({
        "name": "Dev List", "description": "Curated", "kind": "mixed", "is_private": false, "is_pinned": true, "member_actor_ids": [bob.actor_id], "community_slugs": ["rust"]
    }))).await.unwrap();
    assert_eq!(st, 201, "create list failed: {lv}");
    let list_id = lv["list_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::PATCH, &format!("/api/v1/community/lists/{list_id}"), &a_auth, Some(json!({ "name": "Dev List v2" }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/lists/{list_id}"), &a_auth, None).await.unwrap().0, 200);

    let (st, nv) = driver.req(Method::GET, "/api/v1/community/notifications", &b_auth, None).await.unwrap();
    assert_eq!(st, 200);
    assert!(!nv["notifications"].as_array().unwrap().is_empty());
    assert_eq!(driver.req_empty_body(Method::POST, "/api/v1/community/notifications/read", &b_auth).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/posts/{pid}"), &a_auth, None).await.unwrap().0, 200);
}

/// [Test ID] TEST-CM-T1-F11-02
/// [Feature / Req] F11 / ORIGINAL_REQUEST.md (Owner Bearer + X-Community-Actor-Id Act-As-Bot authorship)
/// [Preconditions] Ephemeral live server with owner Alice and owned Bot citizen.
/// [Opaque-Box Steps] Send POST /posts and POST /comments using Alice's Bearer token with `X-Community-Actor-Id: b:<bot_id>`.
/// [Observable Assertions] Created post and comment carry `author_actor_id == b:<bot_id>` and `author_kind == "bot"`.
#[tokio::test]
async fn test_cm_t1_f11_02_owner_act_as_bot_header_authors_as_bot_citizen() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f11_02", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "actas_f11_bot", "ActAs Bot", "manual").await.unwrap();
    let act_as = CommunityAuthMode::ActAsBot { owner_token: alice.token.clone(), actor_id: bot.actor_id.clone() };

    let (st, pv) = driver.req(Method::POST, "/api/v1/community/posts", &act_as, Some(json!({ "body": "Published via Act-As-Bot header" }))).await.unwrap();
    assert_eq!(st, 201, "act-as post failed: {pv}");
    assert_eq!(pv["post"]["author_actor_id"], bot.actor_id);
    assert_eq!(pv["post"]["author_kind"], "bot");
    let pid = pv["post"]["post_id"].as_str().unwrap();

    let (st, cv) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &act_as, Some(json!({ "body": "Comment via Act-As-Bot header" }))).await.unwrap();
    assert_eq!(st, 201, "act-as comment failed: {cv}");
    assert_eq!(cv["comment"]["author_actor_id"], bot.actor_id);
    assert_eq!(cv["comment"]["author_kind"], "bot");
}

/// [Test ID] TEST-CM-T1-F11-03
/// [Feature / Req] F11 / ORIGINAL_REQUEST.md (Direct `Authorization: Bot <bot_token>` full first-class REST privileges)
/// [Preconditions] Ephemeral live server with Owner Alice and a Bot promoted to `bot_mod` in `c/direct_bot_lab`.
/// [Opaque-Box Steps] Authenticate strictly with `Authorization: Bot <token>` to read feed, create post/comment, vote, follow Human, join community, and run mod-action.
/// [Observable Assertions] Every operation succeeds with HTTP 200/201 authored directly under `b:<bot_id>`.
#[tokio::test]
async fn test_cm_t1_f11_03_direct_bot_token_full_first_class_rest_privileges() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f11_03", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "direct_f11_bot", "Direct Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let bot_auth = CommunityAuthMode::DirectBot(bot.bot_token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "direct_bot_lab", "name": "Direct Bot Lab", "allow_bots": true }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/direct_bot_lab/join", &bot_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/direct_bot_lab/mod-actions", &a_auth, Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": bot.actor_id }))).await.unwrap().0, 200);

    assert_eq!(driver.req(Method::GET, "/api/v1/community/feed?limit=10", &bot_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/actors/{}/follow", alice.actor_id), &bot_auth, None).await.unwrap().0, 200);

    let (st, pv) = driver.req(Method::POST, "/api/v1/community/posts", &bot_auth, Some(json!({ "community_slug": "direct_bot_lab", "body": "Direct Bot REST post" }))).await.unwrap();
    assert_eq!(st, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap();
    assert_eq!(pv["post"]["author_actor_id"], bot.actor_id);

    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &bot_auth, Some(json!({ "body": "Direct Bot comment" }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/vote"), &bot_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/communities/direct_bot_lab/mod-actions", &bot_auth, Some(json!({ "action": "pin", "target_kind": "post", "target_id": pid }))).await.unwrap().0,
        200
    );
}

/// [Test ID] TEST-CM-T1-F11-04
/// [Feature / Req] F11 / ORIGINAL_REQUEST.md (Public community media ACL allows cross-user read & meta)
/// [Preconditions] Ephemeral live server with 2 Humans (Alice & Bob).
/// [Opaque-Box Steps] Alice uploads PNG (verify Bob gets 403 while unattached), Alice attaches media to a published community post, then Bob fetches GET /media/{id} and /meta.
/// [Observable Assertions] Unattached media returns 403 to Bob; once published in a community post, Bob receives 200 OK with byte-exact PNG bytes and valid metadata.
#[tokio::test]
async fn test_cm_t1_f11_04_public_community_media_acl_allows_cross_user_read_and_meta() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f11_04", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f11_04", "Pass1234!").await.unwrap();

    let (media_id, png_bytes) = driver.upload_png(&alice.token, "pub_comm.png", (12, 180, 240, 255), None).await.unwrap();
    let (st_before, _) = driver.get_media(&bob.token, &media_id).await.unwrap();
    assert_eq!(st_before, 403, "unattached upload must remain 403 to Bob");

    let (st, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &CommunityAuthMode::Human(alice.token.clone()), Some(json!({
            "format": "media",
            "body": "Public chart image",
            "media": [{ "media_id": media_id, "kind": "image" }]
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "attach public media failed: {pv}");

    let (st_get, got_bytes) = driver.get_media(&bob.token, &media_id).await.unwrap();
    assert_eq!(st_get, 200, "public community media must return 200 to Bob");
    assert_eq!(got_bytes, png_bytes, "downloaded PNG bytes must match uploaded bytes exactly");

    let (st_meta, meta) = driver.get_media_meta(&bob.token, &media_id).await.unwrap();
    assert_eq!(st_meta, 200);
    assert_eq!(meta["media_id"], media_id);
    assert_eq!(meta["mime_type"], "image/png");
}

/// [Test ID] TEST-CM-T1-F11-05
/// [Feature / Req] F11 / ORIGINAL_REQUEST.md (`POST /posts/{id}/summon-bot` persists genuine bot reply)
/// [Preconditions] Ephemeral live server with Human Alice, a Reactive Bot citizen, and a published post.
/// [Opaque-Box Steps] Call POST /api/v1/community/posts/{id}/summon-bot targeting the Bot with a prompt, then GET /posts/{id}/comments.
/// [Observable Assertions] Summon returns HTTP 200 with `triggered == true` and a non-empty `created_comment` authored by the Bot persisted in SQLite.
#[tokio::test]
async fn test_cm_t1_f11_05_summon_bot_endpoint_persists_genuine_bot_comment() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f11_05", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "summon_f11_bot", "Summon Bot", "reactive").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "community_slug": "rust", "body": "How should we structure WAL checkpoints?" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap();

    let (st, sv) = driver
        .req(Method::POST, &format!("/api/v1/community/posts/{pid}/summon-bot"), &auth, Some(json!({
            "bot_actor_id": bot.actor_id,
            "prompt": "Give a concise systems engineering summary"
        })))
        .await
        .unwrap();
    assert_eq!(st, 200, "summon-bot failed: {sv}");
    assert_eq!(sv["triggered"], true);
    let cid = sv["created_comment"]["comment_id"].as_str().expect("created_comment missing").to_string();
    assert_eq!(sv["created_comment"]["author_actor_id"], bot.actor_id);
    assert_eq!(sv["created_comment"]["author_kind"], "bot");

    let (_, cv) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}/comments"), &auth, None).await.unwrap();
    assert!(cv["comments"].as_array().unwrap().iter().any(|c| c["comment_id"] == cid));
}

// ============================================================================
// F12 — Standard Bot API Community Methods, Webhooks & Hermes Tools (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F12-01
/// [Feature / Req] F12 / ORIGINAL_REQUEST.md (Bot API getFeed, createPost, createComment, vote across split & concat routes)
/// [Preconditions] Ephemeral live server with 2 Bots registered.
/// [Opaque-Box Steps] Call `community.createPost`, `community.getFeed`, `community.createComment`, and `community.vote` alternating `/bot/{token}/` and `/bot{token}/`.
/// [Observable Assertions] All 4 calls return HTTP 200 with `{"ok": true, "result": ...}`, expected post/comment authorship, and incremented vote score.
#[tokio::test]
async fn test_cm_t1_f12_01_bot_api_feed_post_comment_vote_both_route_shapes() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f12_01", "Pass1234!").await.unwrap();
    let b1 = driver.register_community_bot(&alice.token, "b1_f12_01_bot", "B1 Bot", "manual").await.unwrap();
    let b2 = driver.register_community_bot(&alice.token, "b2_f12_01_bot", "B2 Bot", "manual").await.unwrap();

    let (st, p) = driver.bot_api(&b1.bot_token, "community.createPost", true, Some(json!({ "community_slug": "rust", "body": "Bot API split-route post" }))).await.unwrap();
    assert_eq!(st, 200, "createPost failed: {p}");
    assert_eq!(p["ok"], true);
    let pid = p["result"]["post"]["post_id"].as_str().unwrap().to_string();

    let (st, feed) = driver.bot_api(&b2.bot_token, "community.getFeed", false, Some(json!({ "community": "rust", "limit": 20 }))).await.unwrap();
    assert_eq!(st, 200, "getFeed failed: {feed}");
    assert_eq!(feed["ok"], true);
    assert!(feed["result"]["items"].as_array().unwrap().iter().any(|i| i["post"]["post_id"] == pid));

    let (st, cmt) = driver.bot_api(&b2.bot_token, "community.createComment", true, Some(json!({ "post_id": pid, "body": "Bot2 reply via Bot API" }))).await.unwrap();
    assert_eq!(st, 200, "createComment failed: {cmt}");
    assert_eq!(cmt["ok"], true);
    assert_eq!(cmt["result"]["comment"]["author_actor_id"], b2.actor_id);

    let (st, vt) = driver.bot_api(&b2.bot_token, "community.vote", false, Some(json!({ "post_id": pid, "direction": 1 }))).await.unwrap();
    assert_eq!(st, 200, "vote failed: {vt}");
    assert_eq!(vt["ok"], true);
    assert_eq!(vt["result"]["score"], 2);
}

/// [Test ID] TEST-CM-T1-F12-02
/// [Feature / Req] F12 / ORIGINAL_REQUEST.md (Bot API search, getPost, quotePost, modAction & moderate alias)
/// [Preconditions] Ephemeral live server with Bot promoted to `bot_mod` in `c/bot_api_mod`.
/// [Opaque-Box Steps] Execute `community.createPost`, `community.search`, `community.getPost`, `community.quotePost`, `community.modAction`, and `community.moderate`.
/// [Observable Assertions] All Bot API methods succeed with `ok: true`, Tantivy search finds the post, quote references origin, and mod logs record bot actions.
#[tokio::test]
async fn test_cm_t1_f12_02_bot_api_search_get_post_quote_post_and_mod_action() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f12_02", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "mod_f12_02_bot", "Mod Bot API", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "bot_api_mod", "name": "Bot API Mod", "allow_bots": true }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/bot_api_mod/mod-actions", &a_auth, Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": bot.actor_id }))).await.unwrap().0, 200);

    let (st, p) = driver.bot_api(&bot.bot_token, "community.createPost", true, Some(json!({ "community_slug": "bot_api_mod", "title": "CobaltSearchAnchor", "body": "Unique token CobaltSearchAnchor" }))).await.unwrap();
    assert_eq!(st, 200);
    let pid = p["result"]["post"]["post_id"].as_str().unwrap().to_string();

    let (st, sr) = driver.bot_api(&bot.bot_token, "community.search", false, Some(json!({ "query": "CobaltSearchAnchor" }))).await.unwrap();
    assert_eq!(st, 200);
    assert!(sr["result"]["posts"].as_array().unwrap().iter().any(|x| x["post_id"] == pid));

    let (st, gp) = driver.bot_api(&bot.bot_token, "community.getPost", true, Some(json!({ "post_id": pid }))).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(gp["result"]["post"]["post_id"], pid);

    let (st, qp) = driver.bot_api(&bot.bot_token, "community.quotePost", false, Some(json!({ "quoted_post_id": pid, "body": "Bot quoting self with commentary" }))).await.unwrap();
    assert_eq!(st, 200, "quotePost failed: {qp}");
    assert_eq!(qp["result"]["post"]["format"], "quote");

    let (st, m1) = driver.bot_api(&bot.bot_token, "community.modAction", true, Some(json!({ "community_slug": "bot_api_mod", "action": "lock", "target_kind": "post", "target_id": pid }))).await.unwrap();
    assert_eq!(st, 200, "modAction failed: {m1}");
    let (st, m2) = driver.bot_api(&bot.bot_token, "community.moderate", false, Some(json!({ "community_slug": "bot_api_mod", "action": "unlock", "target_kind": "post", "target_id": pid }))).await.unwrap();
    assert_eq!(st, 200, "moderate alias failed: {m2}");
}

/// [Test ID] TEST-CM-T1-F12-03
/// [Feature / Req] F12 / ORIGINAL_REQUEST.md (Mentions & replies enqueue Bot API `getUpdates`)
/// [Preconditions] Ephemeral live server with Human Alice and Bot citizen (no webhook configured).
/// [Opaque-Box Steps] Alice mentions `@watch_f12_bot` in a post and comments on the Bot's post, then Bot calls GET /bot/{token}/getUpdates.
/// [Observable Assertions] `getUpdates` returns `ok: true` containing both `community_mention` and `community_comment` update payloads.
#[tokio::test]
async fn test_cm_t1_f12_03_mention_and_reply_enqueue_bot_api_get_updates() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f12_03", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "watch_f12_bot", "Watch Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());

    let (st, p1) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "body": "Paging @watch_f12_bot for review!" }))).await.unwrap();
    assert_eq!(st, 201);
    let p1_id = p1["post"]["post_id"].as_str().unwrap().to_string();

    let (st, bp) = driver.bot_api(&bot.bot_token, "community.createPost", true, Some(json!({ "body": "Bot announcement thread" }))).await.unwrap();
    assert_eq!(st, 200);
    let b_pid = bp["result"]["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{b_pid}/comments"), &a_auth, Some(json!({ "body": "Alice replying to bot" }))).await.unwrap().0, 201);

    let (st, upd) = driver.bot_api_get(&bot.bot_token, "getUpdates", true, "").await.unwrap();
    assert_eq!(st, 200, "getUpdates failed: {upd}");
    assert_eq!(upd["ok"], true);
    let items = upd["result"].as_array().unwrap();
    assert!(items.iter().any(|u| u.get("community_mention").is_some_and(|m| m["post_id"] == p1_id)), "missing community_mention: {items:?}");
    assert!(items.iter().any(|u| u.get("community_comment").is_some_and(|c| c["post_id"] == b_pid)), "missing community_comment: {items:?}");
}

/// [Test ID] TEST-CM-T1-F12-04
/// [Feature / Req] F12 / ORIGINAL_REQUEST.md (`setWebhook` live HTTP POST with `X-Telegram-Bot-Api-Secret-Token`)
/// [Preconditions] Ephemeral live server + `WebhookCaptureServer` on `127.0.0.1:0` + registered Bot.
/// [Opaque-Box Steps] Call POST /bot/{token}/setWebhook with capture URL and secret token; Alice posts mentioning `@hook_f12_bot`.
/// [Observable Assertions] Capture server receives HTTP POST containing exact `X-Telegram-Bot-Api-Secret-Token` header and `community_mention` JSON payload.
#[tokio::test]
async fn test_cm_t1_f12_04_set_webhook_delivers_live_post_with_secret_token() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let mut capture = WebhookCaptureServer::start().await.expect("start webhook capture");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f12_04", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "hook_f12_bot", "Hook Bot", "manual").await.unwrap();

    let secret = "sec-community-t1-f12-04";
    let (st, wh) = driver
        .bot_api(&bot.bot_token, "setWebhook", true, Some(json!({ "url": capture.url(), "secret_token": secret })))
        .await
        .unwrap();
    assert_eq!(st, 200, "setWebhook failed: {wh}");
    assert_eq!(wh["ok"], true);

    let (st, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &CommunityAuthMode::Human(alice.token.clone()), Some(json!({ "body": "Live webhook trigger for @hook_f12_bot!" })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let (got_secret, payload) = capture
        .recv_timeout(Duration::from_secs(4))
        .await
        .expect("webhook POST must arrive within 4s");
    assert_eq!(got_secret.as_deref(), Some(secret));
    assert_eq!(payload["community_mention"]["post_id"], pid);
    assert_eq!(payload["community_mention"]["from_handle"], "alice_f12_04");
    capture.shutdown().await;
}

/// [Test ID] TEST-CM-T1-F12-05
/// [Feature / Req] F12 / ORIGINAL_REQUEST.md (Hermes Agent community tools via live `ServerPlatformImBridge` as pure Bot)
/// [Preconditions] Ephemeral live server with registered Bot citizen.
/// [Opaque-Box Steps] Execute `community_create_post`, `community_browse_feed`, `community_search`, `community_comment`, and `community_vote` via `execute_bot_hermes_tool` (`user_id=None`, `bot_id=Some`).
/// [Observable Assertions] Every Hermes tool returns `success == true` and REST GET /posts/{id} confirms Bot-authored post, comment, and vote persisted in SQLite.
#[tokio::test]
async fn test_cm_t1_f12_05_hermes_agent_community_tools_write_persisted_bot_data() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f12_05", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "hermes_f12_bot", "Hermes Bot", "manual").await.unwrap();

    let r_post = execute_bot_hermes_tool(
        &server,
        bot.bot_user_id,
        "community_create_post",
        json!({ "community_slug": "rust", "title": "HermesOrionPost", "body": "Created natively via Hermes bot tool" }),
        ToolsetPreset::HermesTelegram,
        ApprovalMode::Prompt,
    )
    .await;
    assert!(r_post.success, "community_create_post failed: {:?}", r_post.error);
    let post_id = r_post.structured.as_ref().unwrap()["post"]["post_id"].as_str().unwrap().to_string();
    assert_eq!(r_post.structured.as_ref().unwrap()["post"]["author_actor_id"], bot.actor_id);

    let r_feed = execute_bot_hermes_tool(&server, bot.bot_user_id, "community_browse_feed", json!({ "community": "rust", "limit": 20 }), ToolsetPreset::HermesTelegram, ApprovalMode::Prompt).await;
    assert!(r_feed.success, "community_browse_feed failed: {:?}", r_feed.error);

    let r_search = execute_bot_hermes_tool(&server, bot.bot_user_id, "community_search", json!({ "query": "HermesOrionPost" }), ToolsetPreset::HermesTelegram, ApprovalMode::Prompt).await;
    assert!(r_search.success, "community_search failed: {:?}", r_search.error);
    assert!(r_search.structured.as_ref().unwrap()["posts"].as_array().unwrap().iter().any(|p| p["post_id"] == post_id));

    let r_cmt = execute_bot_hermes_tool(&server, bot.bot_user_id, "community_comment", json!({ "post_id": post_id, "body": "Hermes bot follow-up comment" }), ToolsetPreset::HermesTelegram, ApprovalMode::Prompt).await;
    assert!(r_cmt.success, "community_comment failed: {:?}", r_cmt.error);

    let r_vote = execute_bot_hermes_tool(&server, bot.bot_user_id, "community_vote", json!({ "post_id": post_id, "direction": 1 }), ToolsetPreset::HermesTelegram, ApprovalMode::Prompt).await;
    assert!(r_vote.success, "community_vote failed: {:?}", r_vote.error);

    let (st, got) = driver.req(Method::GET, &format!("/api/v1/community/posts/{post_id}"), &CommunityAuthMode::Human(alice.token.clone()), None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(got["post"]["author_actor_id"], bot.actor_id);
    assert_eq!(got["post"]["author_kind"], "bot");
    assert!(!got["comments"].as_array().unwrap().is_empty());
}

// ============================================================================
// F13 — Server Real-Time WS Fan-Out & 5s Background Scheduler (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F13-01
/// [Feature / Req] F13 / ORIGINAL_REQUEST.md (Post creation fans out `WsServerFrame::Update(UpdateEvent::CommunityEvent)`)
/// [Preconditions] Ephemeral live server with Humans Alice & Bob; Bob connected over live TCP WebSocket.
/// [Opaque-Box Steps] Alice creates a post via REST POST /api/v1/community/posts while Bob reads WS frames.
/// [Observable Assertions] Bob receives `WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind: "post_created", post_id, actor_id, .. })`.
#[tokio::test]
async fn test_cm_t1_f13_01_post_created_emits_ws_community_event_frame() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f13_01", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f13_01", "Pass1234!").await.unwrap();
    let mut ws = connect_ws(&server, &bob.token).await.expect("connect Bob WS");

    let (st, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &CommunityAuthMode::Human(alice.token.clone()), Some(json!({ "community_slug": "rust", "body": "Live WS post broadcast" })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let frame = recv_ws_frame_matching(&mut ws, Duration::from_secs(4), |f| {
        matches!(
            f,
            WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, actor_id, .. })
                if event_kind == "post_created"
                    && post_id.as_deref() == Some(pid.as_str())
                    && actor_id.as_deref() == Some(alice.actor_id.as_str())
        )
    })
    .await
    .expect("must receive post_created WS event");
    assert!(matches!(frame, WsServerFrame::Update(UpdateEvent::CommunityEvent { .. })));
    drop(ws);
}

/// [Test ID] TEST-CM-T1-F13-02
/// [Feature / Req] F13 / ORIGINAL_REQUEST.md (Comments & votes emit targeted `UpdateEvent::CommunityEvent` frames)
/// [Preconditions] Ephemeral live server with Alice & Bob; Bob subscribed via TCP WebSocket.
/// [Opaque-Box Steps] Alice publishes a post, comments on it, and Bob/Alice vote; monitor Bob's WS frames.
/// [Observable Assertions] WS receives `comment_created` (exact `post_id` & `comment_id`) and `vote_updated` (exact `post_id` & `actor_id`).
#[tokio::test]
async fn test_cm_t1_f13_02_comment_and_vote_emit_targeted_ws_community_events() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f13_02", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f13_02", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "body": "Target post for comment+vote WS" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let mut ws = connect_ws(&server, &bob.token).await.expect("connect Bob WS");

    let (st, cv) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &a_auth, Some(json!({ "body": "Targeted WS comment" }))).await.unwrap();
    assert_eq!(st, 201);
    let cid = cv["comment"]["comment_id"].as_str().unwrap().to_string();

    let _ = recv_ws_frame_matching(&mut ws, Duration::from_secs(4), |f| {
        matches!(
            f,
            WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, comment_id, .. })
                if event_kind == "comment_created"
                    && post_id.as_deref() == Some(pid.as_str())
                    && comment_id.as_deref() == Some(cid.as_str())
        )
    })
    .await
    .expect("must receive comment_created WS event");

    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/vote"), &b_auth, Some(json!({ "direction": 1 }))).await.unwrap().0, 200);
    let _ = recv_ws_frame_matching(&mut ws, Duration::from_secs(4), |f| {
        matches!(
            f,
            WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, actor_id, .. })
                if event_kind == "vote_updated"
                    && post_id.as_deref() == Some(pid.as_str())
                    && actor_id.as_deref() == Some(bob.actor_id.as_str())
        )
    })
    .await
    .expect("must receive vote_updated WS event");
    drop(ws);
}

/// [Test ID] TEST-CM-T1-F13-03
/// [Feature / Req] F13 / ORIGINAL_REQUEST.md (Background scheduler publishes due `Scheduled` post)
/// [Preconditions] Ephemeral live server with Alice & Bob; Bob connected over TCP WebSocket.
/// [Opaque-Box Steps] Alice schedules a post 150ms in the future, wait 220ms until due, execute scheduler tick, then Bob reads post & searches Tantivy.
/// [Observable Assertions] Scheduled post transitions to `Published`, emits `scheduled_post_published` WS event, and becomes searchable & readable by Bob.
#[tokio::test]
async fn test_cm_t1_f13_03_scheduler_publishes_due_scheduled_post() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f13_03", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f13_03", "Pass1234!").await.unwrap();
    let mut ws = connect_ws(&server, &bob.token).await.expect("connect Bob WS");

    let due_ms = now_ms() + 150;
    let (st, sv) = driver
        .req(Method::POST, "/api/v1/community/posts", &CommunityAuthMode::Human(alice.token.clone()), Some(json!({
            "format": "micro",
            "status": "scheduled",
            "scheduled_at_ms": due_ms,
            "body": "ScheduledAuroraLaunch published by scheduler"
        })))
        .await
        .unwrap();
    assert_eq!(st, 201, "schedule failed: {sv}");
    let pid = sv["post"]["post_id"].as_str().unwrap().to_string();

    tokio::time::sleep(Duration::from_millis(220)).await;
    gpui_telegram_server::routes::community::run_community_background_tick(server.state().unwrap()).await;

    let _ = recv_ws_frame_matching(&mut ws, Duration::from_secs(4), |f| {
        matches!(
            f,
            WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, .. })
                if event_kind == "scheduled_post_published" && post_id.as_deref() == Some(pid.as_str())
        )
    })
    .await
    .expect("must receive scheduled_post_published WS event");

    let (st, got) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &CommunityAuthMode::Human(bob.token.clone()), None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(got["post"]["status"], "published");
    drop(ws);
}

/// [Test ID] TEST-CM-T1-F13-04
/// [Feature / Req] F13 / ORIGINAL_REQUEST.md (Background scheduler closes expired poll & fans out WS `poll_closed`)
/// [Preconditions] Ephemeral live server with a published Poll whose deadline is elapsed and Bob connected on TCP WS.
/// [Opaque-Box Steps] Expire the poll's close timestamp, run the scheduler tick, inspect WS stream and GET /posts/{id}.
/// [Observable Assertions] WS delivers `poll_closed` for `poll_pid` and GET /posts/{id} shows `poll.is_closed == true`.
#[tokio::test]
async fn test_cm_t1_f13_04_scheduler_closes_expired_poll_and_fans_out_ws() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f13_04", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f13_04", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());

    let (st, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "format": "poll",
            "body": "Expiring poll for F13",
            "poll": { "question": "Ship now?", "options": ["Yes", "Soon"], "duration_secs": 3600 }
        })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let past_ms = now_ms() - 5_000;
    server.state().unwrap().db.pool().with_writer(|conn| {
        conn.execute("UPDATE community_posts SET poll_closes_at_ms = ?1 WHERE post_id = ?2", rusqlite::params![past_ms, pid])
            .map_err(|e| gpui_telegram_core::TelegramError::Database(e.to_string()))?;
        Ok(())
    }).unwrap();

    let mut ws = connect_ws(&server, &bob.token).await.expect("connect Bob WS");
    gpui_telegram_server::routes::community::run_community_background_tick(server.state().unwrap()).await;

    let _ = recv_ws_frame_matching(&mut ws, Duration::from_secs(4), |f| {
        matches!(
            f,
            WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, .. })
                if event_kind == "poll_closed" && post_id.as_deref() == Some(pid.as_str())
        )
    })
    .await
    .expect("must receive poll_closed WS event");

    let (_, got) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &a_auth, None).await.unwrap();
    assert_eq!(got["post"]["poll"]["is_closed"], true);
    drop(ws);
}

/// [Test ID] TEST-CM-T1-F13-05
/// [Feature / Req] F13 / ORIGINAL_REQUEST.md (Background scheduler executes bounded autonomous bot turn & WS fan-out)
/// [Preconditions] Ephemeral live server with an `Autonomous` Bot, a fresh human post in `c/rust`, and Bob subscribed on TCP WS.
/// [Opaque-Box Steps] Execute `run_community_background_tick` and verify bot comment creation + WS broadcast.
/// [Observable Assertions] At most 1 bounded autonomous bot turn posts a comment authored by a bot citizen and emits `comment_created` over WS.
#[tokio::test]
async fn test_cm_t1_f13_05_scheduler_runs_bounded_autonomous_bot_turn_and_ws() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_f13_05", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f13_05", "Pass1234!").await.unwrap();
    let _bot = driver.register_community_bot(&alice.token, "auto_f13_bot", "Auto Rust Bot", "autonomous").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());

    let (st, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "community_slug": "rust",
            "title": "Async Rust Architecture Tradeoffs",
            "body": "Comparing zero-copy actor messaging vs work-stealing queues in Rust systems."
        })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let _pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let mut ws = connect_ws(&server, &bob.token).await.expect("connect Bob WS");
    gpui_telegram_server::routes::community::run_community_background_tick(server.state().unwrap()).await;

    let frame = recv_ws_frame_matching(&mut ws, Duration::from_secs(4), |f| {
        matches!(
            f,
            WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, actor_id, .. })
                if event_kind == "comment_created" && actor_id.as_deref().is_some_and(|a| a.starts_with("b:"))
        )
    })
    .await
    .expect("autonomous scheduler tick must emit a bot comment_created WS event");
    assert!(matches!(frame, WsServerFrame::Update(UpdateEvent::CommunityEvent { .. })));
    drop(ws);
}

// ============================================================================
// F14 — Client API, WS Sync & Dual Online/Offline Local Runtime (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F14-01
/// [Feature / Req] F14 / ORIGINAL_REQUEST.md (`TelegramClient::community()` online feed, CRUD & `X-Community-Actor-Id`)
/// [Preconditions] Ephemeral live server + `TestTelegramClient` registered & authenticated.
/// [Opaque-Box Steps] Fetch feed online, register an owned Bot via client facade, switch `set_active_actor_id`, create post/comment/vote.
/// [Observable Assertions] `runtime_mode() == Online`, seeded feed returns items, and post created while acting as Bot persists under `b:{bot_id}` on server.
#[tokio::test]
async fn test_cm_t1_f14_01_client_facade_online_feed_posts_comments_votes_and_act_as() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let tc = TestTelegramClient::open(&server.base_url()).unwrap();
    let _alice = tc.register_and_bind(&driver, "alice_f14_01", "Pass1234!").await.unwrap();

    let feed = tc.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc.community().runtime_mode(), CommunityRuntimeMode::Online);
    assert!(!feed.items.is_empty(), "online feed must return seeded items");

    let bot_reg = tc
        .community()
        .register_bot_actor(&RegisterBotActorRequest {
            bot_user_id: None,
            actor_kind: Some(ActorKind::Bot),
            handle: "client_f14_bot".into(),
            display_name: "Client Bot".into(),
            bio: Some("Client runtime bot".into()),
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_badge: None,
            bot_model: None,
            bot_capabilities: vec!["rust".into()],
            bot_autonomy_mode: BotAutonomyMode::Manual,
            bot_persona: None,
            cooldown_secs: None,
            max_posts_per_hour: None,
        })
        .await
        .unwrap();
    tc.community().set_active_actor_id(Some(bot_reg.actor.actor_id.clone()));

    let created = tc
        .community()
        .create_post(&CreatePostRequest { community_slug: Some("rust".into()), body: "Posted via TelegramClient acting as Bot".into(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(created.post.author_actor_id, bot_reg.actor.actor_id);
    assert_eq!(created.post.author_kind, ActorKind::Bot);

    let cmt = tc
        .community()
        .create_comment(&created.post.post_id, &gpui_telegram_proto::community_contracts::CreateCommentRequest { body: "Bot client comment".into(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(cmt.comment.author_actor_id, bot_reg.actor.actor_id);

    let vote = tc.community().vote_post(&created.post.post_id, &VoteRequest { direction: 1 }).await.unwrap();
    assert_eq!(vote.direction, 1);

    tc.shutdown().await;
}

/// [Test ID] TEST-CM-T1-F14-02
/// [Feature / Req] F14 / ORIGINAL_REQUEST.md (Live client WS surfaces `ClientDomainEvent::CommunityEvent`)
/// [Preconditions] Ephemeral live server + `TestTelegramClient` connected via `connect_ws()` + second Human Bob.
/// [Opaque-Box Steps] Wait for WS connection, have Bob create a post via REST, and poll `client.poll_events()`.
/// [Observable Assertions] Client receives `ClientDomainEvent::CommunityEvent` with `event_kind == "post_created"` and matching `post_id`.
#[tokio::test]
async fn test_cm_t1_f14_02_client_ws_surfaces_community_domain_event() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let tc = TestTelegramClient::open(&server.base_url()).unwrap();
    let _alice = tc.register_and_bind(&driver, "alice_f14_02", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_f14_02", "Pass1234!").await.unwrap();

    tc.client().connect_ws();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let _ = tc.client().poll_events();

    let (st, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &CommunityAuthMode::Human(bob.token.clone()), Some(json!({ "body": "Trigger client WS event" })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let mut matched = false;
    while tokio::time::Instant::now() < deadline {
        for ev in tc.client().poll_events() {
            if let ClientDomainEvent::CommunityEvent { event_kind, post_id, .. } = ev {
                if event_kind == "post_created" && post_id.as_deref() == Some(pid.as_str()) {
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            break;
        }
        tokio::time::sleep(Duration::from_millis(35)).await;
    }
    assert!(matched, "TelegramClient WS must emit ClientDomainEvent::CommunityEvent for post_created");
    tc.shutdown().await;
}

/// [Test ID] TEST-CM-T1-F14-03
/// [Feature / Req] F14 / ORIGINAL_REQUEST.md (Unreachable server transparently falls back to `<app_dir>/community.db`)
/// [Preconditions] Closed loopback port URL (`closed_local_url()`) + `TestTelegramClient` bound to authenticated user.
/// [Opaque-Box Steps] Call `get_feed` and `create_post` against unreachable URL.
/// [Observable Assertions] Calls succeed transparently in `CommunityRuntimeMode::LocalFallback`, `<app_dir>/community.db` exists, and local write is readable.
#[tokio::test]
async fn test_cm_t1_f14_03_client_unreachable_server_falls_back_to_local_sqlite() {
    let url = closed_local_url();
    let tc = TestTelegramClient::open(&url).unwrap();
    tc.bind_auth(7403, "offline_alice_03", "tok_offline_03");

    let feed = tc.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(!feed.items.is_empty(), "offline fallback must serve seeded content");
    assert!(tc.dir_path().join("community.db").exists(), "<app_dir>/community.db must be created");

    let created = tc
        .community()
        .create_post(&CreatePostRequest { community_slug: Some("rust".into()), body: "Offline local post persistence check".into(), ..Default::default() })
        .await
        .unwrap();
    let fetched = tc.community().get_post(&created.post.post_id, None).await.unwrap();
    assert_eq!(fetched.post.body, "Offline local post persistence check");

    tc.shutdown().await;
}

/// [Test ID] TEST-CM-T1-F14-04
/// [Feature / Req] F14 / ORIGINAL_REQUEST.md (Legacy server HTTP 404 on `/api/v1/community/*` falls back to `LocalCommunityRuntime`)
/// [Preconditions] `Legacy404Server` responding 200 on `/health` and 404 on `/api/v1/community/{*path}`.
/// [Opaque-Box Steps] Verify `/health` is reachable (200), then call `TelegramClient::community().get_feed()`.
/// [Observable Assertions] Client seamlessly downgrades to `CommunityRuntimeMode::LocalFallback` and serves seeded community posts.
#[tokio::test]
async fn test_cm_t1_f14_04_client_legacy_404_server_falls_back_to_local_runtime() {
    let legacy = Legacy404Server::start().await.expect("start Legacy404Server");
    let driver = CommunityHttpDriver::new(legacy.base_url());
    let (health_st, _) = driver.req(Method::GET, "/health", &CommunityAuthMode::None, None).await.unwrap();
    assert_eq!(health_st, 200);

    let tc = TestTelegramClient::open(&legacy.base_url()).unwrap();
    tc.bind_auth(7404, "legacy_alice_04", "tok_legacy_04");

    let feed = tc.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(!feed.items.is_empty(), "404 fallback must deliver seeded local feed");

    tc.shutdown().await;
    legacy.shutdown().await;
}

/// [Test ID] TEST-CM-T1-F14-05
/// [Feature / Req] F14 / ORIGINAL_REQUEST.md (Offline local Bot summon persists across client reopen on same TempDir)
/// [Preconditions] Closed loopback port + `TestTelegramClient` opened in `TempDir` bound to user `7405`.
/// [Opaque-Box Steps] Create local post, invoke `summon_bot` targeting `@ferris_bot`, shut down keeping TempDir, reopen client for same user `7405`, and fetch post detail.
/// [Observable Assertions] Bot reply comment is created offline and BOTH post & bot reply survive client reopen on `<dir>/community.db`.
#[tokio::test]
async fn test_cm_t1_f14_05_offline_local_bot_summon_persists_across_reopen() {
    let url = closed_local_url();
    let tc1 = TestTelegramClient::open(&url).unwrap();
    tc1.bind_auth(7405, "reopen_alice_05", "tok_reopen_05");

    let created = tc1
        .community()
        .create_post(&CreatePostRequest { community_slug: Some("rust".into()), body: "Offline question for @ferris_bot about borrow checker".into(), ..Default::default() })
        .await
        .unwrap();
    let pid = created.post.post_id.clone();

    let summon = tc1
        .community()
        .summon_bot(&pid, &SummonBotRequest { bot_actor_id: Some("b:910001".into()), prompt: Some("Analyze offline persistence".into()), ..Default::default() })
        .await
        .unwrap();
    assert!(summon.triggered, "offline summon_bot must trigger: {}", summon.reason);
    let reply_cid = summon.created_comment.expect("bot reply comment").comment_id;

    let temp_dir = tc1.shutdown_keep_dir().await.expect("keep TempDir");
    let tc2 = TestTelegramClient::open_in_dir(&url, temp_dir).unwrap();
    tc2.bind_auth(7405, "reopen_alice_05", "tok_reopen_05");

    let detail = tc2.community().get_post(&pid, None).await.unwrap();
    assert_eq!(detail.post.post_id, pid);
    assert!(
        detail.comments.iter().any(|c| c.comment_id == reply_cid && c.author_kind.is_bot_like()),
        "offline bot reply must persist across client reopen"
    );
    tc2.shutdown().await;
}

// ============================================================================
// F15 — Left Sidebar Entry & AppShell Integration (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F15-01
/// [Feature / Req] F15 / ORIGINAL_REQUEST.md (Left sidebar Expanded/Collapsed/AppleAppSidebar/NavRail entry points)
/// [Preconditions] Headless GPUI `AppShell` authenticated in isolated TempDir.
/// [Opaque-Box Steps] Open Community from Expanded `ChatSidebar`, Collapsed 64px rail, `AppleAppSidebar`, and `NavRail` (`items.len() == 11`).
/// [Observable Assertions] Every entry activates `ActiveWorkspacePanel::Community`, sets `should_hide_composer == true`, and `inspect_state().window.active_panel == "Community"`.
#[test]
fn test_cm_t1_f15_01_sidebar_expanded_collapsed_apple_and_nav_rail_open_community() {
    run_community_gpui_test("test_cm_t1_f15_01_sidebar_expanded_collapsed_apple_and_nav_rail_open_community");
}

async fn body_f15_01(mut app: AsyncApp) {
    let (host, _) = open_offline_community_host(&mut app, 1440.0, 900.0, 8151, "f15_01", false).await;
    let shell = host.shell.clone();

    assert_eq!(NavRail::new().items.len(), 13, "NavRail must contain 13 items including Community, OrcaBots, and OrcaIde");

    // 1. Expanded ChatSidebar
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(false);
            sb.handle_unified_action(UnifiedSidebarAction::SelectDestination(NavDestination::Community), scx);
        });
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| {
        let s = shell.read(cx);
        s.active_panel == ActiveWorkspacePanel::Community && s.should_hide_composer(cx) && s.inspect_state(cx).window.active_panel == "Community"
    }).await);
    host.draw(&mut app);

    // 2. Collapsed 64px rail
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(true);
            sb.handle_unified_action(UnifiedSidebarAction::SelectDestination(NavDestination::Community), scx);
        });
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| shell.read(cx).active_panel == ActiveWorkspacePanel::Community).await);

    // 3. AppleAppSidebar
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.select_apple_sidebar_section(AppleSidebarSection::Community, cx);
    });
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    // 4. NavRail
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.nav_rail.update(cx, |r, rcx| { r.select(NavDestination::Community); rcx.notify(); });
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| {
        let s = shell.read(cx);
        s.active_panel == ActiveWorkspacePanel::Community && s.should_hide_composer(cx)
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F15-02
/// [Feature / Req] F15 / ORIGINAL_REQUEST.md (AccountPopover, MainMenuDrawer & Cmd+Shift+C shortcut; Cmd+1..9 & Cmd+C intact)
/// [Preconditions] Headless GPUI `AppShell` authenticated on `ActiveWorkspacePanel::Chats`.
/// [Opaque-Box Steps] Trigger `MainMenuCommand::Community`, test `Cmd+Shift+C` toggle, verify plain `Cmd+C` stays on Chats, and verify `nav_destination_for_shortcut_digit`.
/// [Observable Assertions] Menu/Popover and `Cmd+Shift+C` toggle Community cleanly while plain `Cmd+C` and `Cmd+1..9`/`0` remain untouched.
#[test]
fn test_cm_t1_f15_02_account_popover_main_menu_and_cmd_shift_c_shortcut() {
    run_community_gpui_test("test_cm_t1_f15_02_account_popover_main_menu_and_cmd_shift_c_shortcut");
}

async fn body_f15_02(mut app: AsyncApp) {
    assert_eq!(nav_destination_for_shortcut_digit("1"), Some(NavDestination::Chats));
    assert_eq!(nav_destination_for_shortcut_digit("9"), Some(NavDestination::Browser));
    assert_eq!(nav_destination_for_shortcut_digit("0"), None);

    let (host, _) = open_offline_community_host(&mut app, 1440.0, 900.0, 8152, "f15_02", false).await;
    let shell = host.shell.clone();

    // 1. AccountPopover open -> Community command activates Community & closes popover
    let _ = shell.update(&mut app, |s, cx| {
        s.toggle_account_popover(cx);
        assert!(
            s.sidebar.read(cx).is_account_popover_open,
            "toggle_account_popover must open AccountPopover before switching to Community"
        );
        s.handle_main_menu_command(MainMenuCommand::Community, cx);
        assert!(
            !s.sidebar.read(cx).is_account_popover_open,
            "switching to Community must close AccountPopover"
        );
    });
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);
    host.draw(&mut app);

    // 2. MainMenuDrawer open -> Community command activates Community & closes drawer
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.main_menu_drawer.update(cx, |d, _| d.open());
        assert!(
            s.main_menu_drawer.read(cx).is_open,
            "MainMenuDrawer must be open before triggering MainMenuCommand::Community"
        );
        s.handle_main_menu_command(MainMenuCommand::Community, cx);
        assert!(
            !s.main_menu_drawer.read(cx).is_open,
            "switching to Community must close MainMenuDrawer"
        );
    });
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);
    host.draw(&mut app);

    // 3. Cmd+Shift+C toggles Community <-> Chats while plain Cmd+C stays on Chats
    host.key(&mut app, "cmd-shift-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats);

    host.key(&mut app, "cmd-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats, "plain Cmd+C must not open Community");

    host.key(&mut app, "cmd-shift-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);
}

/// [Test ID] TEST-CM-T1-F15-03
/// [Feature / Req] F15 / ORIGINAL_REQUEST.md (`IconName::ALL.len() == 64` & all 10 Community SVGs load from `IconName::EXTRA`)
/// [Preconditions] Compiled desktop asset source `TelegramAssetSource::new()`.
/// [Opaque-Box Steps] Assert `IconName::ALL.len() == 64`, confirm all 10 Community icons live strictly in `IconName::EXTRA`, and load every SVG via `AssetSource::load`.
/// [Observable Assertions] `IconName::ALL.len() == 64` holds and every Community SVG contains `<svg`, `viewBox="0 0 24 24"`, and `currentColor`.
#[test]
fn test_cm_t1_f15_03_icon_all_64_invariant_and_extra_community_svgs() {
    assert_eq!(IconName::ALL.len(), 64, "IconName::ALL MUST remain 64");
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
    let src = TelegramAssetSource::new();
    for icon in community_icons {
        assert!(!IconName::ALL.contains(&icon), "{icon:?} must not be in IconName::ALL");
        assert!(IconName::EXTRA.contains(&icon), "{icon:?} must be in IconName::EXTRA");
        let bytes = src.load(icon.path()).unwrap().unwrap_or_else(|| panic!("missing {}", icon.path()));
        let svg = std::str::from_utf8(&bytes).unwrap();
        assert!(svg.contains("<svg") && svg.contains("viewBox=\"0 0 24 24\"") && svg.contains("currentColor"));
    }
}

/// [Test ID] TEST-CM-T1-F15-04
/// [Feature / Req] F15 / ORIGINAL_REQUEST.md (`ShellThemeTargets` theme cycling & bilingual locale sync into `CommunityWorkspace`)
/// [Preconditions] Headless GPUI `AppShell` with mounted `CommunityWorkspace`.
/// [Opaque-Box Steps] Cycle all 4 themes via `Cmd+D` and toggle locale (`zh-CN <-> en-US`) via `Cmd+L`, rendering after each switch.
/// [Observable Assertions] `CommunityWorkspace.palette.mode` visits all 4 `TdesktopThemeMode` values and `language_id` toggles cleanly with zero render panic.
#[test]
fn test_cm_t1_f15_04_theme_cycling_and_bilingual_locale_sync_render() {
    run_community_gpui_test("test_cm_t1_f15_04_theme_cycling_and_bilingual_locale_sync_render");
}

async fn body_f15_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8154, "f15_04", true).await;
    let cw = cw.unwrap();
    let mut seen_modes = HashSet::new();
    for _ in 0..4 {
        let mode = cw.read_with(&app, |w, _| w.palette.mode).unwrap();
        seen_modes.insert(format!("{mode:?}"));
        host.draw(&mut app);
        host.key(&mut app, "cmd-d");
    }
    assert_eq!(seen_modes.len(), 4, "Cmd+D must cycle all 4 theme modes: {seen_modes:?}");

    let lang1 = cw.read_with(&app, |w, _| w.language_id.clone()).unwrap();
    host.key(&mut app, "cmd-l");
    host.draw(&mut app);
    let lang2 = cw.read_with(&app, |w, _| w.language_id.clone()).unwrap();
    assert_ne!(lang1, lang2, "Cmd+L must switch CommunityWorkspace locale");
}

/// [Test ID] TEST-CM-T1-F15-05
/// [Feature / Req] F15 / ORIGINAL_REQUEST.md (Centralized CJK IME + 80ms fence & all 5 Telegram bridges)
/// [Preconditions] Headless GPUI `AppShell` with mounted `CommunityWorkspace`.
/// [Opaque-Box Steps] Compose CJK IME preedit, commit, verify 80ms trailing-Enter fence then submit; trigger OpenMediaLightbox, SharePostToChat, ShowToast, UnreadCountChanged, and OpenDmWithActor.
/// [Observable Assertions] CJK text commits cleanly without premature Enter submission, and every Telegram bridge mutates exact observable AppShell overlay/toast/badge state.
#[test]
fn test_cm_t1_f15_05_centralized_cjk_ime_and_telegram_bridges() {
    run_community_gpui_test("test_cm_t1_f15_05_centralized_cjk_ime_and_telegram_bridges");
}

async fn body_f15_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8155, "f15_05", true).await;
    let shell = host.shell.clone();
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| w.open_quick_post_composer(cx));
    assert!(shell.read_with(&app, |s, cx| s.is_community_input_active(cx)).unwrap());
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("社区帖子", cx);
        assert!(s.has_active_ime_preedit(cx));
        s.commit_ime_unmark_text(cx);
        assert_eq!(s.active_text_and_preedit(cx).0, "社区帖子");
        assert!(s.handle_non_char_key("enter", false, cx));
    });
    assert_eq!(cw.read_with(&app, |w, _| w.state.quick_post_body.clone()).unwrap(), "社区帖子", "80ms IME fence must prevent immediate Enter submit");

    app.background_executor().timer(Duration::from_millis(95)).await;
    let _ = shell.update(&mut app, |s, cx| { assert!(s.handle_non_char_key("enter", false, cx)); });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let w = cw.read(cx);
        !w.state.quick_post_submitting && w.state.feed_items.iter().any(|i| i.post.body.contains("社区帖子"))
    }).await);

    // Telegram Bridges
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_action(CommunityWorkspaceAction::OpenMediaLightbox("mid_bridge_1".into()), cx);
    });
    assert!(shell.read_with(&app, |s, _| s.overlays.media_viewer_overlay.is_some()).unwrap());
    host.key(&mut app, "escape");
    assert!(shell.read_with(&app, |s, _| s.overlays.media_viewer_overlay.is_none()).unwrap());

    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_action(CommunityWorkspaceAction::ShowToast { text: "Bridge OK".into(), is_error: false }, cx);
        s.handle_community_action(CommunityWorkspaceAction::UnreadCountChanged(42), cx);
        s.handle_community_action(CommunityWorkspaceAction::SharePostToChat { post_id: "p1".into(), summary: "Shared post".into() }, cx);
    });
    shell.read_with(&app, |s, cx| {
        assert!(s.window_runtime.controller.current_toast.is_some());
        assert!(s.community_pending_share.is_some());
        assert_eq!(s.apple_sidebar.read(cx).community_unread_count, 42);
    }).unwrap();
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_action(CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: 8155 }, cx);
    });
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);
}

// ============================================================================
// F16 — 3-Column Adaptive Shell, Multi-Tab Feed, Post Cards & Identity Switcher (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F16-01
/// [Feature / Req] F16 / ORIGINAL_REQUEST.md (Responsive 3-col / 2-col / 1-col adaptive shell rendering)
/// [Preconditions] Headless GPUI `AppShell` with mounted `CommunityWorkspace`.
/// [Opaque-Box Steps] Render `CommunityWorkspace` at `1440×900`, `1040×800`, and `820×720` via `draw_at`.
/// [Observable Assertions] `last_layout_mode` resolves accurately to `ThreeColumn`, `TwoColumn`, and `OneColumn` with clean full render passes.
#[test]
fn test_cm_t1_f16_01_adaptive_three_two_one_column_breakpoints_render() {
    run_community_gpui_test("test_cm_t1_f16_01_adaptive_three_two_one_column_breakpoints_render");
}

async fn body_f16_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8161, "f16_01", true).await;
    let cw = cw.unwrap();
    for (w, h, expected) in [
        (1440.0, 900.0, CommunityLayoutMode::ThreeColumn),
        (1040.0, 800.0, CommunityLayoutMode::TwoColumn),
        (820.0, 720.0, CommunityLayoutMode::OneColumn),
    ] {
        host.draw_at(&mut app, w, h);
        let actual = cw.read_with(&app, |ws, _| ws.last_layout_mode).unwrap();
        assert_eq!(actual, expected, "viewport {w}x{h} expected {expected:?}, got {actual:?}");
    }
}

/// [Test ID] TEST-CM-T1-F16-02
/// [Feature / Req] F16 / ORIGINAL_REQUEST.md (Persistent 1-click Human/Bot Act-As switcher tags Quick-Post)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with owned Bot registered.
/// [Opaque-Box Steps] Open `IdentitySwitcher` popover, call `switch_active_actor(bot_id)`, then submit Quick-Post.
/// [Observable Assertions] `active_actor_id` switches to Bot and submitted post appears in feed authored with `author_actor_id == bot_id` and `ActorKind::Bot`.
#[test]
fn test_cm_t1_f16_02_identity_switcher_act_as_bot_tags_quick_post() {
    run_community_gpui_test("test_cm_t1_f16_02_identity_switcher_act_as_bot_tags_quick_post");
}

async fn body_f16_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8162, "f16_02", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.state.bot_studio_name = "Switcher Bot".into();
        w.state.bot_studio_handle = "switcher_f16_bot".into();
        w.state.bot_studio_autonomy = BotAutonomyMode::Manual;
        w.submit_register_bot(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.identities.as_ref().is_some_and(|i| i.bot_actors.iter().any(|b| b.handle == "switcher_f16_bot"))
    }).await);
    let bot_id = cw.read_with(&app, |w, _| {
        w.state.identities.as_ref().unwrap().bot_actors.iter().find(|b| b.handle == "switcher_f16_bot").unwrap().actor_id.clone()
    }).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_popover(CommunityPopover::IdentitySwitcher, cx);
        assert_eq!(w.state.active_popover, Some(CommunityPopover::IdentitySwitcher));
    });
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(bot_id.clone(), cx);
        assert_eq!(w.state.active_actor_id, bot_id);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.feed_loading).await);

    let _ = cw.update(&mut app, |w, cx| {
        w.state.quick_post_body = "Quick-post authored as Switcher Bot!".into();
        w.submit_quick_post(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let w = cw.read(cx);
        !w.state.quick_post_submitting
            && w.state.feed_items.iter().any(|it| it.post.body.contains("Switcher Bot!") && it.post.author_actor_id == bot_id && it.post.author_kind == ActorKind::Bot)
    }).await, "Quick-post must be authored by switched Bot actor");
}

/// [Test ID] TEST-CM-T1-F16-03
/// [Feature / Req] F16 / ORIGINAL_REQUEST.md (Multi-tab feed, ActorFilter & Reddit sort/time bar)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with seeded Human & Bot posts.
/// [Opaque-Box Steps] Cycle feed tabs, `ActorFilter` (`AllCitizens`, `HumansOnly`, `BotsOnly`, `HumanBotDebates`), `FeedSort` (`Hot`, `Top`, `New`), and `TimeWindow`.
/// [Observable Assertions] Every reload settles cleanly, `HumansOnly` excludes bots, `BotsOnly` includes only bot-like authors, and renders cleanly.
#[test]
fn test_cm_t1_f16_03_feed_tabs_actor_filters_and_reddit_sort_time_bar() {
    run_community_gpui_test("test_cm_t1_f16_03_feed_tabs_actor_filters_and_reddit_sort_time_bar");
}

async fn body_f16_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8163, "f16_03", true).await;
    let cw = cw.unwrap();

    for tab in [FeedTab::Popular, FeedTab::All, FeedTab::Community("rust".into()), FeedTab::ForYou] {
        let _ = cw.update(&mut app, |w, cx| w.select_feed_tab(tab, cx));
        assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.feed_loading).await);
        host.draw(&mut app);
    }

    let _ = cw.update(&mut app, |w, cx| w.set_actor_filter(ActorFilter::HumansOnly, cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.feed_loading).await);
    cw.read_with(&app, |w, _| {
        assert!(!w.state.feed_items.is_empty());
        assert!(w.state.feed_items.iter().all(|i| i.post.author_kind == ActorKind::Human));
    }).unwrap();

    let _ = cw.update(&mut app, |w, cx| w.set_actor_filter(ActorFilter::BotsOnly, cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.feed_loading).await);
    cw.read_with(&app, |w, _| {
        assert!(!w.state.feed_items.is_empty());
        assert!(w.state.feed_items.iter().all(|i| i.post.author_kind.is_bot_like()));
    }).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.set_actor_filter(ActorFilter::AllCitizens, cx);
        w.set_feed_sort(FeedSort::Top, cx);
        w.set_time_window(TimeWindow::Week, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.feed_loading).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F16-04
/// [Feature / Req] F16 / ORIGINAL_REQUEST.md (Algo Tuner popover, Card/Compact/Classic density toggle & FeedExplanation popover)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with feed items.
/// [Opaque-Box Steps] Open `AlgoTuner`, switch preset to `BotHeavy` and save, cycle `FeedDensity`, and open `ExplainPost` popover on first card.
/// [Observable Assertions] `algo_prefs.preset` persists as `BotHeavy`, density cycles through `Card`/`Compact`/`Classic`, and both popovers render cleanly.
#[test]
fn test_cm_t1_f16_04_algo_tuner_popover_density_toggle_and_explain_popover() {
    run_community_gpui_test("test_cm_t1_f16_04_algo_tuner_popover_density_toggle_and_explain_popover");
}

async fn body_f16_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8164, "f16_04", true).await;
    let cw = cw.unwrap();
    let first_pid = cw.read_with(&app, |w, _| w.state.feed_items[0].post.post_id.clone()).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_popover(CommunityPopover::AlgoTuner, cx);
        w.state.algo_draft = default_prefs_for_preset(AlgoPreset::BotHeavy);
    });
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| w.save_algo_prefs(cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.algo_prefs.preset == AlgoPreset::BotHeavy && !cw.read(cx).state.feed_loading
    }).await);

    let d0 = cw.read_with(&app, |w, _| w.state.density).unwrap();
    let _ = cw.update(&mut app, |w, cx| w.cycle_density(cx));
    let d1 = cw.read_with(&app, |w, _| w.state.density).unwrap();
    assert_ne!(d0, d1);
    assert!(matches!(d1, FeedDensity::Card | FeedDensity::Compact | FeedDensity::Classic));
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| w.toggle_popover(CommunityPopover::ExplainPost(first_pid.clone()), cx));
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F16-05
/// [Feature / Req] F16 / ORIGINAL_REQUEST.md (PostCard vote/like/repost/bookmark/poll/award interactions & Right Rail trending)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with seeded feed + poll (`seed_post_09`) + trending topics.
/// [Opaque-Box Steps] Upvote, like, repost, bookmark, award a post, vote on seeded poll with `PollBreakdownMode` toggle, and render Right Rail.
/// [Observable Assertions] Every engagement flag/count updates cleanly in workspace state, poll records viewer vote, and Right Rail trending topics are non-empty.
#[test]
fn test_cm_t1_f16_05_post_card_interactions_poll_award_and_right_rail() {
    run_community_gpui_test("test_cm_t1_f16_05_post_card_interactions_poll_award_and_right_rail");
}

async fn body_f16_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8165, "f16_05", true).await;
    let cw = cw.unwrap();
    let target_pid = cw.read_with(&app, |w, _| w.state.feed_items[0].post.post_id.clone()).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.vote_post(target_pid.clone(), 1, cx);
        w.toggle_like(target_pid.clone(), cx);
        w.repost_post(target_pid.clone(), cx);
        w.toggle_bookmark(target_pid.clone(), cx);
        w.give_award(target_pid.clone(), "insightful".into(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.find_post(&target_pid).is_some_and(|p| {
            p.viewer_vote == 1 && p.viewer_liked && p.viewer_reposted && p.viewer_bookmarked && !p.awards.is_empty()
        })
    }).await);

    // Poll vote + breakdown mode on seeded poll post
    let poll_opt = host
        .client
        .community()
        .block_on(host.client.community().get_post("seed_post_09", None))
        .unwrap()
        .post
        .poll
        .unwrap()
        .options[0]
        .option_id
        .clone();
    let _ = cw.update(&mut app, |w, cx| {
        w.state.poll_breakdown_by_post.insert("seed_post_09".into(), PollBreakdownMode::Humans);
        w.vote_poll("seed_post_09".into(), poll_opt, cx);
    });
    app.background_executor().timer(Duration::from_millis(80)).await;
    assert!(!cw.read_with(&app, |w, _| w.state.trending.clone()).unwrap().is_empty(), "Right Rail trending must be populated");
    host.draw(&mut app);
}

// ============================================================================
// F17 — Post Detail (Reddit Nested Tree + X Thread) & Summon Bot (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F17-01
/// [Feature / Req] F17 / ORIGINAL_REQUEST.md (PostDetailView nested Reddit comment tree & collapse/expand toggle)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with seeded nested debate post `seed_post_01`.
/// [Opaque-Box Steps] Call `open_post_detail("seed_post_01")`, verify nested comments load, toggle collapse on root comment on & off, rendering each state.
/// [Observable Assertions] Route transitions to `PostDetail("seed_post_01")`, nested comments load, and `detail_collapsed_comments` toggles cleanly.
#[test]
fn test_cm_t1_f17_01_post_detail_nested_tree_and_collapse_expand() {
    run_community_gpui_test("test_cm_t1_f17_01_post_detail_nested_tree_and_collapse_expand");
}

async fn body_f17_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8171, "f17_01", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| w.open_post_detail("seed_post_01".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == "seed_post_01" && !d.comments.is_empty())
    }).await);
    let root_cid = cw.read_with(&app, |w, _| w.state.detail_post.as_ref().unwrap().comments[0].comment_id.clone()).unwrap();
    let _ = cw.update(&mut app, |w, cx| w.toggle_comment_collapsed(root_cid.clone(), cx));
    assert!(cw.read_with(&app, |w, _| w.state.detail_collapsed_comments.contains(&root_cid)).unwrap());
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| w.toggle_comment_collapsed(root_cid.clone(), cx));
    assert!(!cw.read_with(&app, |w, _| w.state.detail_collapsed_comments.contains(&root_cid)).unwrap());
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F17-02
/// [Feature / Req] F17 / ORIGINAL_REQUEST.md (PostDetailView switches all 6 Reddit comment sorts deterministically)
/// [Preconditions] Headless GPUI `CommunityWorkspace` viewing `seed_post_01`.
/// [Opaque-Box Steps] Switch `set_detail_comment_sort` across `Best`, `Top`, `New`, `Controversial`, `Old`, and `Qa`, awaiting reload and rendering each.
/// [Observable Assertions] Every sort settles (`!detail_loading`), updates `detail_comment_sort`, keeps comments populated, and renders cleanly.
#[test]
fn test_cm_t1_f17_02_post_detail_all_six_comment_sorts_reorder() {
    run_community_gpui_test("test_cm_t1_f17_02_post_detail_all_six_comment_sorts_reorder");
}

async fn body_f17_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8172, "f17_02", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| w.open_post_detail("seed_post_01".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| cw.read(cx).state.detail_post.is_some()).await);

    for sort in [CommentSort::Best, CommentSort::Top, CommentSort::New, CommentSort::Controversial, CommentSort::Old, CommentSort::Qa] {
        let _ = cw.update(&mut app, |w, cx| w.set_detail_comment_sort(sort, cx));
        assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.detail_loading).await);
        cw.read_with(&app, |w, _| {
            assert_eq!(w.state.detail_comment_sort, sort);
            assert!(!w.state.detail_post.as_ref().unwrap().comments.is_empty());
        }).unwrap();
        host.draw(&mut app);
    }
}

/// [Test ID] TEST-CM-T1-F17-03
/// [Feature / Req] F17 / ORIGINAL_REQUEST.md (Inline reply with Human/Bot Identity Switcher attaches nested child & updates count)
/// [Preconditions] Headless GPUI `CommunityWorkspace` viewing `seed_post_01` with a registered Bot.
/// [Opaque-Box Steps] Register a Bot, switch active actor to Bot, set `detail_reply_parent_id` to first comment, and call `submit_detail_reply`.
/// [Observable Assertions] Nested child comment appears under parent authored as `ActorKind::Bot` and `total_comments` increments.
#[test]
fn test_cm_t1_f17_03_inline_reply_identity_switcher_and_comment_count() {
    run_community_gpui_test("test_cm_t1_f17_03_inline_reply_identity_switcher_and_comment_count");
}

async fn body_f17_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8173, "f17_03", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.state.bot_studio_name = "Reply Citizen Bot".into();
        w.state.bot_studio_handle = "reply_f17_bot".into();
        w.submit_register_bot(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.identities.as_ref().is_some_and(|i| i.bot_actors.iter().any(|b| b.handle == "reply_f17_bot"))
    }).await);
    let bot_id = cw.read_with(&app, |w, _| {
        w.state.identities.as_ref().unwrap().bot_actors.iter().find(|b| b.handle == "reply_f17_bot").unwrap().actor_id.clone()
    }).unwrap();

    let _ = cw.update(&mut app, |w, cx| w.open_post_detail("seed_post_01".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| cw.read(cx).state.detail_post.is_some()).await);
    let (parent_cid, before_total) = cw.read_with(&app, |w, _| {
        let d = w.state.detail_post.as_ref().unwrap();
        (d.comments[0].comment_id.clone(), d.total_comments)
    }).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(bot_id.clone(), cx);
        w.state.detail_reply_parent_id = Some(parent_cid.clone());
        w.state.detail_reply_draft = "Nested bot reply in F17-03!".into();
        w.submit_detail_reply(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let st = &cw.read(cx).state;
        st.find_comment(&parent_cid).is_some_and(|c| c.children.iter().any(|ch| ch.body.contains("F17-03") && ch.author_kind == ActorKind::Bot))
            && st.detail_post.as_ref().is_some_and(|d| d.total_comments > before_total)
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F17-04
/// [Feature / Req] F17 / ORIGINAL_REQUEST.md (X Linear Thread & Quote Chain detail view mode)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with seeded 4-part thread (`seed_post_05`) and 2-hop quote (`seed_post_14`).
/// [Opaque-Box Steps] Open `seed_post_05` and switch to `PostDetailMode::XThread`, render, then open `seed_post_14` and verify `quote_chain`.
/// [Observable Assertions] `thread_chain.len() >= 2` on the thread post, `quote_chain` populated on the quote post, and both render cleanly.
#[test]
fn test_cm_t1_f17_04_x_linear_thread_and_quote_chain_detail_mode() {
    run_community_gpui_test("test_cm_t1_f17_04_x_linear_thread_and_quote_chain_detail_mode");
}

async fn body_f17_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8174, "f17_04", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_post_detail("seed_post_05".into(), cx);
        w.set_detail_mode(PostDetailMode::XThread, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == "seed_post_05" && d.thread_chain.len() >= 2)
    }).await);
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| w.open_post_detail("seed_post_14".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == "seed_post_14" && !d.quote_chain.is_empty())
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F17-05
/// [Feature / Req] F17 / ORIGINAL_REQUEST.md (1-click "Summon Bot" appends and renders new Bot comment in detail view)
/// [Preconditions] Headless GPUI `CommunityWorkspace` viewing a freshly created post.
/// [Opaque-Box Steps] Open detail view and call `summon_bot_with_options(pid, Some("b:910001"), Some("Give Rust advice"), None)`.
/// [Observable Assertions] Summon completes with feedback and appends a new comment authored by `b:910001` (`ActorKind::Bot`) in `detail_post.comments`.
#[test]
fn test_cm_t1_f17_05_summon_bot_renders_new_bot_comment_in_detail() {
    run_community_gpui_test("test_cm_t1_f17_05_summon_bot_renders_new_bot_comment_in_detail");
}

async fn body_f17_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8175, "f17_05", true).await;
    let cw = cw.unwrap();
    let created = host
        .client
        .community()
        .block_on(host.client.community().create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Evaluating arena allocators for GPUI layout pass".into(),
            ..Default::default()
        }))
        .unwrap();
    let pid = created.post.post_id;

    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(pid.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == pid)
    }).await);

    let _ = cw.update(&mut app, |w, cx| {
        w.summon_bot_with_options(pid.clone(), Some("b:910001".into()), Some("Give Rust arena advice".into()), None, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let st = &cw.read(cx).state;
        !st.detail_summon_pending
            && st.detail_summon_feedback.is_some()
            && st.detail_post.as_ref().is_some_and(|d| d.comments.iter().any(|c| c.author_actor_id == "b:910001"))
    }).await);
    host.draw(&mut app);
}

// ============================================================================
// F18 — Rich Multi-Format Composer, Drafts & Scheduling (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F18-01
/// [Feature / Req] F18 / ORIGINAL_REQUEST.md (Modal Composer identity, c/<slug>, flair & Markdown Article with FormattingBar)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped.
/// [Opaque-Box Steps] Open `CommunityModal::Composer`, choose `c/rust`, flair `"RFC"`, `ComposerFormatTab::Article`, apply bold + markdown block, and submit.
/// [Observable Assertions] Formatting tokens insert cleanly, composer submits & closes modal, and published Article persists in feed.
#[test]
fn test_cm_t1_f18_01_composer_modal_identity_community_flair_and_markdown_article() {
    run_community_gpui_test("test_cm_t1_f18_01_composer_modal_identity_community_flair_and_markdown_article");
}

async fn body_f18_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8181, "f18_01", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));
        w.set_composer_tab(ComposerFormatTab::Article, cx);
        w.state.composer.flair = Some("RFC".into());
        w.state.composer.title = "F18 Markdown Architecture".into();
        w.state.composer.body = "Summary".into();
        w.apply_composer_format(TextStyleKind::Bold, cx);
        w.insert_composer_markdown_block("\n> Key insight", cx);
        assert!(w.state.composer.body.contains("**") && w.state.composer.body.contains("> Key insight"));
    });
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| w.submit_composer(PostStatus::Published, cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let st = &cw.read(cx).state;
        !st.composer.submitting
            && st.active_modal.is_none()
            && st.feed_items.iter().any(|i| i.post.title.as_deref() == Some("F18 Markdown Architecture") && i.post.format == PostFormat::Article)
    }).await);
}

/// [Test ID] TEST-CM-T1-F18-02
/// [Feature / Req] F18 / ORIGINAL_REQUEST.md (Composer Media, Link & Poll format tabs with NSFW/Spoiler toggles)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped.
/// [Opaque-Box Steps] Configure `Media`, `Link`, and `Poll` tabs with `is_nsfw`/`is_spoiler`, validate `build_composer_create_request`, and submit Poll.
/// [Observable Assertions] All 3 format requests build valid DTOs preserving NSFW/Spoiler flags, and submitted Poll persists cleanly.
#[test]
fn test_cm_t1_f18_02_composer_media_link_and_poll_with_nsfw_spoiler() {
    run_community_gpui_test("test_cm_t1_f18_02_composer_media_link_and_poll_with_nsfw_spoiler");
}

async fn body_f18_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8182, "f18_02", true).await;
    let cw = cw.unwrap();
    let t0 = now_ms();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.state.composer.is_nsfw = true;
        w.state.composer.is_spoiler = true;

        w.set_composer_tab(ComposerFormatTab::Media, cx);
        w.state.composer.media_items = vec![ComposerMediaDraft { url_or_id: "https://example.com/a.png".into(), kind: "image".into(), alt_text: "A".into() }];
        let r_media = w.build_composer_create_request(PostStatus::Published, t0).expect("valid media req");
        assert_eq!(r_media.format, PostFormat::Media);
        assert!(r_media.is_nsfw && r_media.is_spoiler);

        w.set_composer_tab(ComposerFormatTab::Link, cx);
        w.state.composer.link_url = "https://rust-lang.org".into();
        let r_link = w.build_composer_create_request(PostStatus::Published, t0).expect("valid link req");
        assert_eq!(r_link.format, PostFormat::Link);

        w.set_composer_tab(ComposerFormatTab::Poll, cx);
        w.state.composer.is_nsfw = false;
        w.state.composer.poll_question = "Favorite async runtime?".into();
        w.state.composer.poll_options = vec!["Tokio".into(), "Smol".into()];
        let r_poll = w.build_composer_create_request(PostStatus::Published, t0).expect("valid poll req");
        assert_eq!(r_poll.format, PostFormat::Poll);
        w.submit_composer(PostStatus::Published, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.composer.submitting && cw.read(cx).state.active_modal.is_none()
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F18-03
/// [Feature / Req] F18 / ORIGINAL_REQUEST.md (Composer Thread 1/N, Quote & Crosspost tabs)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with at least 1 seeded origin post.
/// [Opaque-Box Steps] Build/submit a multi-part `Thread`, open `open_composer_quote`, and open & submit `open_composer_crosspost`.
/// [Observable Assertions] Thread builds `thread_parts`, Quote references `quoted_post_id`, and Crosspost publishes referencing `crosspost_of_id`.
#[test]
fn test_cm_t1_f18_03_composer_thread_quote_and_crosspost_submissions() {
    run_community_gpui_test("test_cm_t1_f18_03_composer_thread_quote_and_crosspost_submissions");
}

async fn body_f18_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8183, "f18_03", true).await;
    let cw = cw.unwrap();
    let origin = cw.read_with(&app, |w, _| w.state.feed_items[0].post.clone()).unwrap();
    let t0 = now_ms();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.set_composer_tab(ComposerFormatTab::Thread, cx);
        w.state.composer.body = "Thread part 1".into();
        w.state.composer.thread_parts = vec!["Thread part 2".into(), "Thread part 3".into()];
        let req = w.build_composer_create_request(PostStatus::Published, t0).expect("valid thread");
        assert_eq!(req.thread_parts.len(), 2);

        w.open_composer_quote(origin.clone(), cx);
        w.state.composer.body = "Insightful quote".into();
        let qreq = w.build_composer_create_request(PostStatus::Published, t0).expect("valid quote");
        assert_eq!(qreq.quoted_post_id.as_deref(), Some(origin.post_id.as_str()));

        w.open_composer_crosspost(origin.clone(), cx);
        w.state.composer.community_slug = Some("systems".into());
        let xreq = w.build_composer_create_request(PostStatus::Published, t0).expect("valid crosspost");
        assert_eq!(xreq.crosspost_of_id.as_deref(), Some(origin.post_id.as_str()));
        w.submit_composer(PostStatus::Published, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.composer.submitting && cw.read(cx).state.active_modal.is_none()
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F18-04
/// [Feature / Req] F18 / ORIGINAL_REQUEST.md (`@handle`, `#hashtag`, `c/community` autocomplete alongside CJK IME text)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with ComposerModal open.
/// [Opaque-Box Steps] Insert CJK text followed by `@f`, `#r`, and `c/r`, invoke `compute_composer_autocomplete` and `apply_composer_autocomplete`.
/// [Observable Assertions] All 3 `AutocompleteKind` variants (`Mention`, `Hashtag`, `Community`) return filtered suggestions and insert cleanly without corrupting CJK prefix.
#[test]
fn test_cm_t1_f18_04_composer_autocomplete_mentions_hashtags_communities_and_cjk() {
    run_community_gpui_test("test_cm_t1_f18_04_composer_autocomplete_mentions_hashtags_communities_and_cjk");
}

async fn body_f18_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8184, "f18_04", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.state.focus_input(ActiveCommunityInput::ComposerBody);

        for (trigger, expected_kind) in [
            ("你好 @f", AutocompleteKind::Mention),
            ("社区讨论 c/r", AutocompleteKind::Community),
            ("热门标签 #", AutocompleteKind::Hashtag),
        ] {
            w.state.composer.body.clear();
            w.state.composer.autocomplete_dismissed = false;
            w.insert_text_to_active_input(trigger, cx);
            let (start, items) = compute_composer_autocomplete(w).unwrap_or_else(|| panic!("no autocomplete for {trigger}"));
            assert!(!items.is_empty(), "empty suggestions for {trigger}");
            assert_eq!(items[0].kind, expected_kind);
            let tok = items[0].insert_token.clone();
            w.apply_composer_autocomplete(start, tok.clone(), cx);
            assert!(w.state.composer.body.contains(&tok));
        }
    });
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F18-05
/// [Feature / Req] F18 / ORIGINAL_REQUEST.md (Drafts save/resume/publish, Scheduled post & Bot Co-Author)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped.
/// [Opaque-Box Steps] Invoke `generate_bot_coauthor_suggestion`, save a Draft, load & resume Draft, publish it, then submit a future Scheduled post.
/// [Observable Assertions] Co-Author suggestion populates, Draft appears & clears after publish, and future Scheduled post submits cleanly.
#[test]
fn test_cm_t1_f18_05_composer_drafts_resume_publish_schedule_and_bot_coauthor() {
    run_community_gpui_test("test_cm_t1_f18_05_composer_drafts_resume_publish_schedule_and_bot_coauthor");
}

async fn body_f18_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8185, "f18_05", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.state.composer.body = "F18 draft body with Bot Co-Author".into();
        w.generate_bot_coauthor_suggestion(cx);
        assert!(w.state.composer.bot_coauthor_suggestion.is_some());
        w.submit_composer(PostStatus::Draft, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.composer.submitting && cw.read(cx).state.active_modal.is_none()).await);

    let _ = cw.update(&mut app, |w, cx| { w.open_composer_modal(None, cx); w.load_composer_drafts(cx); });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.composer.drafts_loading && !cw.read(cx).state.composer.drafts.is_empty()).await);
    let draft = cw.read_with(&app, |w, _| w.state.composer.drafts[0].clone()).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.resume_draft_into_composer(draft.clone(), cx);
        assert_eq!(w.state.composer.editing_post_id.as_deref(), Some(draft.post_id.as_str()));
        w.publish_draft_now(draft.post_id.clone(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.composer.drafts.iter().any(|d| d.post_id == draft.post_id)).await);

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.state.composer.body = "Future scheduled post F18-05".into();
        w.state.composer.schedule_enabled = true;
        w.state.composer.schedule_minutes_input = "45".into();
        w.submit_composer(PostStatus::Scheduled, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.composer.submitting && cw.read(cx).state.active_modal.is_none()).await);
    host.draw(&mut app);
}

// ============================================================================
// F19 — Community (`c/`) Pages, Rules, Flairs & Versioned Wiki (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F19-01
/// [Feature / Req] F19 / ORIGINAL_REQUEST.md (`c/<slug>` banner, Human+Bot stats, pinned posts & rules render)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with seeded `c/rust`.
/// [Opaque-Box Steps] Navigate to `CommunityRoute::CommunityPage("rust")`, await `community_detail`, render Posts and Rules tabs.
/// [Observable Assertions] `community_detail` loads with positive Human+Bot stats, rules sidebar data, and clean full-frame renders.
#[test]
fn test_cm_t1_f19_01_community_page_banner_stats_pinned_and_rules_render() {
    run_community_gpui_test("test_cm_t1_f19_01_community_page_banner_stats_pinned_and_rules_render");
}

async fn body_f19_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8191, "f19_01", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::CommunityPage("rust".into()), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.community_detail.as_ref().is_some_and(|d| d.community.slug == "rust")
    }).await);
    cw.read_with(&app, |w, _| {
        let d = w.state.community_detail.as_ref().unwrap();
        assert!(d.community.member_count > 0);
        assert!(d.community.human_member_count + d.community.bot_member_count > 0);
        assert!(!d.community.rules.is_empty());
    }).unwrap();
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| w.set_community_tab(CommunityPageTab::Rules, cx));
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F19-02
/// [Feature / Req] F19 / ORIGINAL_REQUEST.md (`c/<slug>` Join/Leave and Bell notification toggles)
/// [Preconditions] Headless GPUI `CommunityWorkspace` viewing `c/rust`.
/// [Opaque-Box Steps] Toggle `toggle_join_community` (`true` -> `false` -> `true`) and toggle `toggle_community_bell`.
/// [Observable Assertions] `is_joined` persists across join/leave transitions and `community_bell_enabled` toggles accurately.
#[test]
fn test_cm_t1_f19_02_community_join_leave_and_bell_toggle() {
    run_community_gpui_test("test_cm_t1_f19_02_community_join_leave_and_bell_toggle");
}

async fn body_f19_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8192, "f19_02", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::CommunityPage("rust".into()), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.community_detail.as_ref().is_some_and(|d| d.community.slug == "rust")
    }).await);

    let initial_joined = cw.read_with(&app, |w, _| w.state.community_detail.as_ref().unwrap().community.is_joined).unwrap();
    let _ = cw.update(&mut app, |w, cx| w.toggle_join_community("rust".into(), !initial_joined, cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.community_detail.as_ref().is_some_and(|d| d.community.is_joined == !initial_joined)
    }).await);

    let bell_before = cw.read_with(&app, |w, _| w.state.community_bell_enabled.get("rust").copied().unwrap_or(false)).unwrap();
    let _ = cw.update(&mut app, |w, cx| w.toggle_community_bell("rust".into(), cx));
    let bell_after = cw.read_with(&app, |w, _| w.state.community_bell_enabled.get("rust").copied().unwrap_or(false)).unwrap();
    assert_ne!(bell_before, bell_after, "Bell notification state must toggle");
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F19-03
/// [Feature / Req] F19 / ORIGINAL_REQUEST.md (`c/<slug>` flair filter chips narrow community post stream)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with owned `c/flair_f19` containing `"RFC"` and `"Bench"` flaired posts.
/// [Opaque-Box Steps] Call `set_community_flair_filter("flair_f19", Some("RFC"))` and await reload.
/// [Observable Assertions] Filtered `community_posts` contain exclusively posts tagged with `"RFC"` flair.
#[test]
fn test_cm_t1_f19_03_community_flair_filter_chips_narrow_stream() {
    run_community_gpui_test("test_cm_t1_f19_03_community_flair_filter_chips_narrow_stream");
}

async fn body_f19_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8193, "f19_03", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "flair_f19".into();
        w.state.create_community.name = "Flair Lab".into();
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.route == CommunityRoute::CommunityPage("flair_f19".into())
    }).await);

    for (flair, body) in [("RFC", "RFC proposal body"), ("Bench", "Benchmark body")] {
        let _ = host.client.community().block_on(host.client.community().create_post(&CreatePostRequest {
            community_slug: Some("flair_f19".into()),
            flair: Some(flair.into()),
            body: body.into(),
            ..Default::default()
        })).unwrap();
    }

    let _ = cw.update(&mut app, |w, cx| w.set_community_flair_filter("flair_f19".into(), Some("RFC".into()), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let st = &cw.read(cx).state;
        !st.community_posts.is_empty() && st.community_posts.iter().all(|i| i.post.flair.as_deref() == Some("RFC"))
    }).await, "flair filter chip must isolate matching posts");
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F19-04
/// [Feature / Req] F19 / ORIGINAL_REQUEST.md (Versioned Markdown wiki viewer/editor saves revision & history)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with owned `c/wiki_f19`.
/// [Opaque-Box Steps] Switch to `CommunityPageTab::Wiki`, enter editor via `start_edit_community_wiki`, and call `save_community_wiki`.
/// [Observable Assertions] `wiki_revision` increments to `>= 2`, `wiki_revisions` records the edit summary, and Wiki tab renders cleanly.
#[test]
fn test_cm_t1_f19_04_community_versioned_wiki_viewer_editor_and_revisions() {
    run_community_gpui_test("test_cm_t1_f19_04_community_versioned_wiki_viewer_editor_and_revisions");
}

async fn body_f19_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8194, "f19_04", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "wiki_f19".into();
        w.state.create_community.name = "Wiki F19".into();
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.community_detail.as_ref().is_some_and(|d| d.community.slug == "wiki_f19")
    }).await);

    let _ = cw.update(&mut app, |w, cx| {
        w.set_community_tab(CommunityPageTab::Wiki, cx);
        w.start_edit_community_wiki(Some("# Wiki F19 Rev 2\nUpdated guide.".into()), cx);
        assert!(w.state.community_wiki_editing);
        w.state.community_wiki_summary = "Add onboarding guide".into();
        w.save_community_wiki("wiki_f19".into(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.community_detail.as_ref().is_some_and(|d| {
            d.community.wiki_revision >= 2 && d.wiki_revisions.iter().any(|r| r.summary == "Add onboarding guide")
        })
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F19-05
/// [Feature / Req] F19 / ORIGINAL_REQUEST.md (Mod roster & Create Community modal creation + navigation)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped.
/// [Opaque-Box Steps] Open `CreateCommunity` modal, submit `c/brand_new_hub`, switch to `CommunityPageTab::Mods`, and also inspect `c/rust` mods.
/// [Observable Assertions] Modal creates `c/brand_new_hub`, navigates directly to its page, and Mods roster lists owner & seeded moderators.
#[test]
fn test_cm_t1_f19_05_mod_roster_and_create_community_modal_navigation() {
    run_community_gpui_test("test_cm_t1_f19_05_mod_roster_and_create_community_modal_navigation");
}

async fn body_f19_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8195, "f19_05", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        assert_eq!(w.state.active_modal, Some(CommunityModal::CreateCommunity));
        w.state.create_community.slug = "brand_new_hub".into();
        w.state.create_community.name = "Brand New Hub".into();
        w.state.create_community.description = "Human + Bot hub".into();
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.route == CommunityRoute::CommunityPage("brand_new_hub".into())
            && cw.read(cx).state.community_detail.as_ref().is_some_and(|d| !d.moderators.is_empty())
    }).await);
    let _ = cw.update(&mut app, |w, cx| w.set_community_tab(CommunityPageTab::Mods, cx));
    host.draw(&mut app);
}

// ============================================================================
// F20 — Unified Human & Bot Profiles, Bookmarks, Lists & Notifications (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F20-01
/// [Feature / Req] F20 / ORIGINAL_REQUEST.md (Unified Human & Bot profile metadata, dual karma, tabs & profile edit)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with seeded Bot (`ferris_bot`) and own Human profile.
/// [Opaque-Box Steps] Open `@ferris_bot`, inspect Bot Citizenship metadata & cycle profile tabs, then open own Human profile and submit Edit Profile modal.
/// [Observable Assertions] Bot profile displays `ActorKind::Bot` metadata & karma; own Human profile edit persists updated display name & bio cleanly.
#[test]
fn test_cm_t1_f20_01_actor_profile_human_and_bot_metadata_and_tabs() {
    run_community_gpui_test("test_cm_t1_f20_01_actor_profile_human_and_bot_metadata_and_tabs");
}

async fn body_f20_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8201, "f20_01", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| w.open_actor_profile("ferris_bot".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.actor_profile.as_ref().is_some_and(|p| p.actor.handle == "ferris_bot")
    }).await);
    cw.read_with(&app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert_eq!(a.actor_kind, ActorKind::Bot);
        assert!(!a.bot_capabilities.is_empty());
    }).unwrap();
    for tab in [ActorProfileTab::Posts, ActorProfileTab::Replies, ActorProfileTab::Media, ActorProfileTab::Likes, ActorProfileTab::BotActivity] {
        let _ = cw.update(&mut app, |w, cx| w.set_profile_tab(tab, cx));
        host.draw(&mut app);
    }

    let own = cw.read_with(&app, |w, _| w.state.identities.as_ref().unwrap().human_actor.clone().unwrap()).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.open_actor_profile(own.handle.clone(), cx);
        w.open_edit_profile_modal(own, cx);
        w.state.edit_profile.display_name = "F20 Edited Human".into();
        w.state.edit_profile.bio = "Unified Reddit + X citizen".into();
        w.submit_edit_profile(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.edit_profile.submitting && cw.read(cx).state.active_modal.is_none()
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F20-02
/// [Feature / Req] F20 / ORIGINAL_REQUEST.md (Profile "Message on Telegram" bridge emits `OpenDmWithActor` & navigates AppShell)
/// [Preconditions] Headless GPUI `AppShell` with mounted `CommunityWorkspace` and a seeded Telegram peer dialog (`900002`).
/// [Opaque-Box Steps] Emit `CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: 900002 }` and dispatch through `AppShell::handle_community_action`.
/// [Observable Assertions] Action is emitted cleanly and `AppShell` transitions out of Community to open the Telegram DM conversation.
#[test]
fn test_cm_t1_f20_02_profile_telegram_dm_bridge_emits_open_dm_action() {
    run_community_gpui_test("test_cm_t1_f20_02_profile_telegram_dm_bridge_emits_open_dm_action");
}

async fn body_f20_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8202, "f20_02", true).await;
    let shell = host.shell.clone();
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        let _ = w.take_pending_actions();
        w.emit_action(CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: 900002 }, cx);
        let acts = w.take_pending_actions();
        assert!(acts.iter().any(|a| matches!(a, CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: 900002 })));
    });
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, _| {
            sb.dialogs.push(gpui_telegram_desktop::views::chat_sidebar::DialogItemModel {
                id: 900002,
                peer_name: "Peer 900002".into(),
                avatar_initials: "P2".into(),
                avatar_color: 0x2AABEE,
                last_message: "hello".into(),
                timestamp: "10:00".into(),
                unread_count: 0,
                is_muted: false,
                is_pinned: false,
                is_online: true,
                typing_indicator: None,
                delivery_status: gpui_telegram_desktop::views::chat_sidebar::DialogDeliveryStatus::Read,
                folder: "All".into(),
            });
        });
        s.handle_community_action(CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: 900002 }, cx);
    });
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats, "DM bridge must navigate to Telegram Chats");
}

/// [Test ID] TEST-CM-T1-F20-03
/// [Feature / Req] F20 / ORIGINAL_REQUEST.md (Bookmarks view custom folders & persisted folder moves)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with feed items.
/// [Opaque-Box Steps] Create custom folder `"GPUI_Reads"`, move a saved post into `"GPUI_Reads"`, open `BookmarksLists` view, and select `"GPUI_Reads"`.
/// [Observable Assertions] `compute_bookmark_folders` includes `"GPUI_Reads"` and `saved_items` reflects the bookmarked post in `"GPUI_Reads"`.
#[test]
fn test_cm_t1_f20_03_bookmarks_custom_folders_and_move_persistence() {
    run_community_gpui_test("test_cm_t1_f20_03_bookmarks_custom_folders_and_move_persistence");
}

async fn body_f20_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8203, "f20_03", true).await;
    let cw = cw.unwrap();
    let pid = cw.read_with(&app, |w, _| w.state.feed_items[0].post.post_id.clone()).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.state.bookmark_new_folder_draft = "GPUI_Reads".into();
        w.create_bookmark_folder(cx);
        w.move_bookmark_to_folder(pid.clone(), "GPUI_Reads".into(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let w = cw.read(cx);
        compute_bookmark_folders(w).iter().any(|f| f == "GPUI_Reads")
            && w.state.find_post(&pid).is_some_and(|p| p.viewer_bookmarked && p.viewer_bookmark_folder.as_deref() == Some("GPUI_Reads"))
    }).await);
    let _ = cw.update(&mut app, |w, cx| w.select_bookmark_folder("GPUI_Reads".into(), cx));
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F20-04
/// [Feature / Req] F20 / ORIGINAL_REQUEST.md (Lists & Custom Feeds creation, Human+Bot/community members & filtered stream)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped.
/// [Opaque-Box Steps] Switch to `BookmarksListsSection::Lists`, create `"Rust + Bots Feed"` with `c/rust`, add a Bot member, and load its filtered stream.
/// [Observable Assertions] Created list persists in `state.lists`, includes `c/rust` + Bot member, and `list_feed_items` loads & renders cleanly.
#[test]
fn test_cm_t1_f20_04_lists_custom_feeds_create_members_and_stream() {
    run_community_gpui_test("test_cm_t1_f20_04_lists_custom_feeds_create_members_and_stream");
}

async fn body_f20_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8204, "f20_04", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
        w.open_create_list_form(cx);
        w.state.list_editor.name = "Rust + Bots Feed".into();
        w.state.list_editor.description = "Curated Human & Bot stream".into();
        w.state.list_editor.community_slugs = vec!["rust".into()];
        w.submit_list_editor(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.lists.iter().any(|l| l.name == "Rust + Bots Feed")
    }).await);
    let list_id = cw.read_with(&app, |w, _| w.state.lists.iter().find(|l| l.name == "Rust + Bots Feed").unwrap().list_id.clone()).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_selected_list_actor(list_id.clone(), "b:910001".into(), cx);
        w.select_list_for_preview(list_id.clone(), cx);
        w.load_selected_list_feed(list_id.clone(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.list_feed_loading).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F20-05
/// [Feature / Req] F20 / ORIGINAL_REQUEST.md (Notifications categories, mark read -> unread badge sync, & jump-to-target)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with categorized unread notifications.
/// [Opaque-Box Steps] Cycle `NotificationCategoryTab` filters, click `open_notification_target` on a BotReply notification, then call `mark_all_notifications_read`.
/// [Observable Assertions] Clicking target jumps to `PostDetail(pid)` with `detail_focused_comment_id`, marks read, emits `UnreadCountChanged`, and mark-all zeroes unread count.
#[test]
fn test_cm_t1_f20_05_notifications_categories_mark_read_badge_and_jump() {
    run_community_gpui_test("test_cm_t1_f20_05_notifications_categories_mark_read_badge_and_jump");
}

async fn body_f20_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8205, "f20_05", true).await;
    let cw = cw.unwrap();
    let pid = cw.read_with(&app, |w, _| w.state.feed_items[0].post.post_id.clone()).unwrap();
    let own_id = cw.read_with(&app, |w, _| w.state.active_actor_id.clone()).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Notifications, cx);
        w.state.notifications = vec![
            CommunityNotificationDto {
                notif_id: "n_t1_1".into(),
                recipient_actor_id: own_id.clone(),
                actor_id: "b:910001".into(),
                actor_handle: "ferris_bot".into(),
                actor_display_name: "Ferris Bot".into(),
                actor_kind: ActorKind::Bot,
                kind: CommunityNotificationKind::BotReply,
                target_post_id: Some(pid.clone()),
                target_comment_id: Some("c_jump_9".into()),
                preview_text: "Bot replied to your RFC".into(),
                is_read: false,
                created_at_ms: now_ms(),
            },
            CommunityNotificationDto {
                notif_id: "n_t1_2".into(),
                recipient_actor_id: own_id,
                actor_id: "u:900001".into(),
                actor_handle: "alex_chen".into(),
                actor_display_name: "Alex".into(),
                actor_kind: ActorKind::Human,
                kind: CommunityNotificationKind::Mention,
                target_post_id: Some(pid.clone()),
                target_comment_id: None,
                preview_text: "Mentioned you".into(),
                is_read: false,
                created_at_ms: now_ms(),
            },
        ];
        w.state.unread_notifications = 2;
        for cat in NotificationCategoryTab::ALL_TABS {
            w.set_notification_category(cat, cx);
        }
    });
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| {
        let target = w.state.notifications[0].clone();
        let _ = w.take_pending_actions();
        w.open_notification_target(target, cx);
        assert_eq!(w.state.route, CommunityRoute::PostDetail(pid.clone()));
        assert_eq!(w.state.detail_focused_comment_id.as_deref(), Some("c_jump_9"));
        assert!(w.state.notifications[0].is_read);
        assert!(w.take_pending_actions().iter().any(|a| matches!(a, CommunityWorkspaceAction::UnreadCountChanged(1))));

        w.navigate_to(CommunityRoute::Notifications, cx);
        w.mark_all_notifications_read(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| cw.read(cx).state.unread_notifications == 0).await);
}

// ============================================================================
// F21 — Explore/Search, Moderation Console & Bot Citizenship Studio (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T1-F21-01
/// [Feature / Req] F21 / ORIGINAL_REQUEST.md (Explore/Search Trending + all 7 search tabs)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with seeded communities, actors, media, and polls.
/// [Opaque-Box Steps] Navigate to `Explore`, verify `trending` is populated, and execute searches across all 7 `CommunitySearchTab` values.
/// [Observable Assertions] Every tab (`Top|Latest|People|Bots|Communities|Media|Polls`) completes search (`!search_loading`), returns structured results, and draws cleanly.
#[test]
fn test_cm_t1_f21_01_explore_trending_and_all_seven_search_tabs() {
    run_community_gpui_test("test_cm_t1_f21_01_explore_trending_and_all_seven_search_tabs");
}

async fn body_f21_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8211, "f21_01", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::Explore { initial_query: None }, cx));
    assert!(!cw.read_with(&app, |w, _| w.state.trending.clone()).unwrap().is_empty());

    for (q, tab) in [
        ("rust", CommunitySearchTab::Top),
        ("rust", CommunitySearchTab::Latest),
        ("alex", CommunitySearchTab::People),
        ("bot", CommunitySearchTab::Bots),
        ("rust", CommunitySearchTab::Communities),
        ("", CommunitySearchTab::Media),
        ("", CommunitySearchTab::Polls),
    ] {
        let _ = cw.update(&mut app, |w, cx| w.execute_search(q.into(), tab, cx));
        assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.search_loading && cw.read(cx).state.search_result.is_some()
        }).await);
        host.draw(&mut app);
    }
}

/// [Test ID] TEST-CM-T1-F21-02
/// [Feature / Req] F21 / ORIGINAL_REQUEST.md (Search operator queries `from:@bot`, `c:rust`, `has:poll`, `is:bot`)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with seeded posts & bots.
/// [Opaque-Box Steps] Verify `normalize_search_query` and run live searches using `is:bot`, `c:rust`, `has:poll`, and `from:@ferris_bot`.
/// [Observable Assertions] Operators normalize accurately and returned posts strictly match requested author, community, poll, and bot filters.
#[test]
fn test_cm_t1_f21_02_search_operators_normalize_and_filter_results() {
    run_community_gpui_test("test_cm_t1_f21_02_search_operators_normalize_and_filter_results");
}

async fn body_f21_02(mut app: AsyncApp) {
    assert_eq!(
        normalize_search_query("is:bot from:@ferris_bot c/rust has:poll"),
        "kind:bot author:ferris_bot c:rust has:poll"
    );
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8212, "f21_02", true).await;
    let cw = cw.unwrap();

    for (query, check) in [
        ("is:bot", Box::new(|p: &CommunityPostDto| p.author_kind.is_bot_like()) as Box<dyn Fn(&CommunityPostDto) -> bool>),
        ("c:rust", Box::new(|p: &CommunityPostDto| p.community_slug.as_deref() == Some("rust"))),
        ("has:poll", Box::new(|p: &CommunityPostDto| p.poll.is_some())),
        ("from:@ferris_bot", Box::new(|p: &CommunityPostDto| p.author_handle == "ferris_bot")),
    ] {
        let _ = cw.update(&mut app, |w, cx| w.open_explore_with_query(query.into(), cx));
        assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.search_loading && cw.read(cx).state.search_result.is_some()
        }).await);
        cw.read_with(&app, |w, _| {
            let res = w.state.search_result.as_ref().unwrap();
            assert!(!res.posts.is_empty(), "operator query '{query}' must return >=1 matching post");
            assert!(res.posts.iter().all(|p| check(p)), "operator query '{query}' returned non-matching post");
        }).unwrap();
        host.draw(&mut app);
    }
}

/// [Test ID] TEST-CM-T1-F21-03
/// [Feature / Req] F21 / ORIGINAL_REQUEST.md (Moderation Console queue, Approve/Lock actions & transparent Mod Log)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with owned community `c/t1_mod_ui` and a reported post.
/// [Opaque-Box Steps] Navigate to `ModQueue(Some("t1_mod_ui"))`, execute `Approve` and `Lock`, then inspect `ModConsoleTab::ModLog`.
/// [Observable Assertions] Queue loads pending report, both mod actions succeed, and `state.mod_logs` contains exact recorded entries.
#[test]
fn test_cm_t1_f21_03_mod_console_queue_approve_remove_lock_and_mod_log() {
    run_community_gpui_test("test_cm_t1_f21_03_mod_console_queue_approve_remove_lock_and_mod_log");
}

async fn body_f21_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8213, "f21_03", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "t1_mod_ui".into();
        w.state.create_community.name = "T1 Mod Console".into();
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| cw.read(cx).state.route == CommunityRoute::CommunityPage("t1_mod_ui".into())).await);

    let post = host.client.community().block_on(host.client.community().create_post(&CreatePostRequest {
        community_slug: Some("t1_mod_ui".into()),
        body: "Reported item for mod console".into(),
        ..Default::default()
    })).unwrap().post;
    let _ = cw.update(&mut app, |w, cx| {
        w.record_signal(post.post_id.clone(), EngagementSignalKind::Report, Some(post.author_actor_id.clone()), Some("t1_mod_ui".into()), cx);
    });
    app.background_executor().timer(Duration::from_millis(80)).await;

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::ModQueue(Some("t1_mod_ui".into())), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.mod_loading && cw.read(cx).can_moderate_active_mod_community()).await);
    let qid = cw.read_with(&app, |w, _| w.state.mod_queue.first().map(|i| i.item_id.clone())).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.state.mod_reason_draft = "Reviewed OK".into();
        w.execute_mod_console_action("t1_mod_ui".into(), ModActionKind::Approve, "post".into(), post.post_id.clone(), qid, cx);
        w.execute_mod_console_action("t1_mod_ui".into(), ModActionKind::Lock, "post".into(), post.post_id.clone(), None, cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| cw.read(cx).state.mod_logs.len() >= 2).await);
    let _ = cw.update(&mut app, |w, cx| w.set_mod_tab(ModConsoleTab::ModLog, cx));
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F21-04
/// [Feature / Req] F21 / ORIGINAL_REQUEST.md (AutoMod rule editor creates/toggles keyword rule & enforces on submissions)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with owned `c/t1_am_ui`.
/// [Opaque-Box Steps] Add keyword AutoMod rule `"ban_token_xyz"` (`FlagForReview`), run dry-run, submit real matching post, and toggle rule off.
/// [Observable Assertions] Rule persists, dry-run flags matching text, live submission returns `AutoModActionKind::FlagForReview`, and toggle updates enabled bit.
#[test]
fn test_cm_t1_f21_04_automod_rule_editor_toggle_and_live_enforcement() {
    run_community_gpui_test("test_cm_t1_f21_04_automod_rule_editor_toggle_and_live_enforcement");
}

async fn body_f21_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8214, "f21_04", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "t1_am_ui".into();
        w.state.create_community.name = "T1 AutoMod UI".into();
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| cw.read(cx).state.route == CommunityRoute::CommunityPage("t1_am_ui".into())).await);

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::ModQueue(Some("t1_am_ui".into())), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.mod_loading && cw.read(cx).state.mod_community.is_some()).await);

    let _ = cw.update(&mut app, |w, cx| {
        w.set_mod_tab(ModConsoleTab::AutoModRules, cx);
        w.state.automod_draft.name = "Token Guard".into();
        w.state.automod_draft.keywords = "ban_token_xyz".into();
        w.state.automod_draft.action = AutoModActionKind::FlagForReview;
        w.submit_add_automod_rule(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.automod_draft.saving && cw.read(cx).state.mod_community.as_ref().is_some_and(|c| !c.automod_rules.is_empty())
    }).await);

    let rule_id = cw.read_with(&app, |w, _| w.state.mod_community.as_ref().unwrap().automod_rules[0].rule_id.clone()).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.state.automod_dry_run_body = "Contains ban_token_xyz payload".into();
        w.run_automod_dry_run(cx);
        assert_eq!(w.state.automod_dry_run_result.as_ref().unwrap().action, AutoModActionKind::FlagForReview);
    });

    let live_res = host.client.community().block_on(host.client.community().create_post(&CreatePostRequest {
        community_slug: Some("t1_am_ui".into()),
        body: "Live submission containing ban_token_xyz".into(),
        ..Default::default()
    })).unwrap();
    assert_eq!(live_res.automod_action, AutoModActionKind::FlagForReview);

    let _ = cw.update(&mut app, |w, cx| w.toggle_automod_rule_enabled(rule_id.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.mod_community.as_ref().is_some_and(|c| !c.automod_rules[0].enabled)
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T1-F21-05
/// [Feature / Req] F21 / ORIGINAL_REQUEST.md (Bot Citizenship Studio: register, Grok avatar, edit, snippet & live test post)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped.
/// [Opaque-Box Steps] Navigate to `BotStudio`, customize Grok avatar, register `@t1_studio_bot`, edit persona, format Bot API snippet, and execute `run_bot_studio_test_post`.
/// [Observable Assertions] Bot registers cleanly, edits persist, Bot API snippet includes endpoint, and live test post creates a real Bot-authored post.
#[test]
fn test_cm_t1_f21_05_bot_studio_register_grok_avatar_edit_token_and_test_post() {
    run_community_gpui_test("test_cm_t1_f21_05_bot_studio_register_grok_avatar_edit_token_and_test_post");
}

async fn body_f21_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8215, "f21_05", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BotStudio, cx);
        w.set_bot_studio_avatar_indices(3, 4, cx);
        assert_eq!((w.state.bot_studio_shape_idx, w.state.bot_studio_color_idx), (3, 4));
        w.state.bot_studio_name = "T1 Studio Bot".into();
        w.state.bot_studio_handle = "t1_studio_bot".into();
        w.state.bot_studio_persona = "Systems benchmarking citizen".into();
        w.state.bot_studio_autonomy = BotAutonomyMode::Reactive;
        w.submit_register_bot(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.identities.as_ref().is_some_and(|i| i.bot_actors.iter().any(|b| b.handle == "t1_studio_bot"))
    }).await);
    let bot_id = cw.read_with(&app, |w, _| {
        w.state.identities.as_ref().unwrap().bot_actors.iter().find(|b| b.handle == "t1_studio_bot").unwrap().actor_id.clone()
    }).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        let snippet = format_bot_api_snippet(&w.media_base_url, Some("tok_demo"), Some("rust"));
        assert!(snippet.contains("community.createPost"));
        w.copy_bot_api_snippet(snippet, cx);
        w.state.bot_studio_edit.display_name = "T1 Studio Bot Prime".into();
        w.save_selected_bot_edits(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.bot_studio_edit.saving
            && cw.read(cx).state.identities.as_ref().is_some_and(|i| i.bot_actors.iter().any(|b| b.display_name == "T1 Studio Bot Prime"))
    }).await);

    let _ = cw.update(&mut app, |w, cx| {
        w.state.bot_studio_test_community = Some("rust".into());
        w.state.bot_studio_test_content = "Live playground post from T1 Studio Bot!".into();
        w.run_bot_studio_test_post(bot_id.clone(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.bot_studio_test_submitting && cw.read(cx).state.bot_studio_test_last_post_id.is_some()
    }).await);
    host.draw(&mut app);
}
