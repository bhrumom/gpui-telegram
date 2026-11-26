//! Tier 2 Opaque-Box E2E Suite — Category-Partition, BVA & Negative Rejection (`100` Tests: `F02`–`F21` × `5`).
//! Implements `TEST_INFRA.md §6` (`TEST-CM-T2-F02-01` .. `TEST-CM-T2-F21-05`).

#[allow(dead_code)]
mod community_e2e_helpers;

use std::collections::HashSet;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use community_e2e_helpers::{
    CommunityAuthMode, CommunityHttpDriver, TestTelegramClient, closed_local_url,
    define_community_gpui_harness, execute_bot_hermes_tool, now_ms, open_offline_community_host,
    safe_temp_idx_dir, test_auth_response,
};
use e2e_suite::common::{EphemeralTestServer, connect_ws, recv_ws_frame_matching};
use e2e_suite::gpui_browser_harness::pump_until;
use gpui::AsyncApp;
use gpui_telegram_agent::ApprovalMode;
use gpui_telegram_agent::hermes::toolsets::{HermesToolContext, HermesToolExecutor, ToolsetPreset};
use gpui_telegram_client::{ClientDomainEvent, CommunityRuntimeMode};
use gpui_telegram_community::identity::{MAX_TRUST_SCORE, MIN_TRUST_SCORE};
use gpui_telegram_community::model::{
    MAX_ARTICLE_BODY_CHARS, MAX_COMMENT_BODY_CHARS, MAX_MICRO_BODY_CHARS,
};
use gpui_telegram_community::{
    AutoModEngine, BotGuardRejection, BotLoopGuard, BotWakeSelector, CommentTreeBuilder,
    CommunityRecommender, CommunitySearchIndexer, CommunityStore, DeterministicBotResponder,
    KarmaKind, PostCandidate, TrendingEngine, ViewerContext, apply_karma_delta, apply_poll_vote,
    build_starter_dataset, comment_depth_from_path, compute_trust_score, default_prefs_for_preset,
    extract_entities, normalize_community_slug, normalize_handle, reddit_controversial,
};
use gpui_telegram_core::TelegramError;
use gpui_telegram_desktop::components::ui::sanitize_paste_text;
use gpui_telegram_desktop::views::community::bot_studio_view::{
    clamp_grok_avatar_indices, resolve_grok_avatar_pair, validate_bot_studio_registration,
};
use gpui_telegram_desktop::views::community::mod_queue_view::validate_automod_regex_syntax;
use gpui_telegram_desktop::views::community::shared_widgets::format_badge_count;
use gpui_telegram_desktop::views::community::{
    ActiveCommunityInput, BookmarksListsSection, CommunityLayoutMode, CommunityModal,
    CommunityPageTab, CommunityPopover, CommunityRoute, CommunityWorkspaceAction,
    ComposerFormatTab, ModConsoleTab, sanitize_algo_prefs,
};
use gpui_telegram_desktop::views::nav_rail::NavDestination;
use gpui_telegram_desktop::views::shell::{
    ActiveWorkspacePanel, nav_destination_for_shortcut_digit,
};
use gpui_telegram_proto::community_contracts::{
    ActorFilter, ActorKind, AlgoPreferencesDto, AlgoPreset, AutoModActionKind, AutoModRuleDto,
    BotAutonomyMode, CommentSort, CommunityActorDto, CommunityCommentDto, CommunityNotificationDto,
    CommunityNotificationKind, CommunityPostDto, CommunitySearchRequestDto, CommunitySearchTab,
    CreateCommentRequest, CreateCommunityRequest, CreatePostRequest, FeedItemDto, FeedRequestDto,
    FeedSort, FeedTab, ModActionKind, ModActionRequest, PostStatus, RegisterBotActorRequest,
    RegisterHumanActorRequest, SubCommunityDto,
};
use gpui_telegram_proto::miniapp_contracts::{GROK_AVATAR_COLOR_IDS, GROK_AVATAR_SHAPES};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use gpui_telegram_server::routes::community::run_community_background_tick;
use reqwest::Method;
use serde_json::json;
use tempfile::TempDir;

define_community_gpui_harness! {
    child_env: "CM_T2_CHILD_ENV",
    target_env: "CM_T2_CHILD_TARGET_ENV",
    after_ctors_env: "CM_T2_CHILD_AFTER_CTORS_ENV",
    targets: {
        "test_cm_t2_f15_01_plain_cmd_c_does_not_open_community_cmd_shift_c_toggles" => body_t2_f15_01,
        "test_cm_t2_f15_02_progressive_escape_cascade_ime_modal_lightbox_to_chats" => body_t2_f15_02,
        "test_cm_t2_f15_03_cjk_ime_80ms_commit_fence_and_backspace_during_preedit" => body_t2_f15_03,
        "test_cm_t2_f15_04_logout_and_account_switch_tear_down_community_workspace" => body_t2_f15_04,
        "test_cm_t2_f15_05_open_dm_self_or_invalid_guard_and_unread_badge_0_999_plus" => body_t2_f15_05,
        "test_cm_t2_f16_01_narrow_viewport_640x480_and_cmd_b_sidebar_stress" => body_t2_f16_01,
        "test_cm_t2_f16_02_empty_feed_state_saved_and_empty_list_renders_cleanly" => body_t2_f16_02,
        "test_cm_t2_f16_03_empty_and_whitespace_quick_post_blocked_with_validation" => body_t2_f16_03,
        "test_cm_t2_f16_04_high_volume_110_card_feed_render_latency_budget" => body_t2_f16_04,
        "test_cm_t2_f16_05_identity_switcher_with_zero_bots_renders_human_safely" => body_t2_f16_05,
        "test_cm_t2_f17_01_zero_comment_post_renders_empty_state_and_allows_first_reply" => body_t2_f17_01,
        "test_cm_t2_f17_02_deep_depth_10_comment_tree_renders_safely_narrow_and_wide" => body_t2_f17_02,
        "test_cm_t2_f17_03_locked_post_reply_disabled_and_emits_error_toast" => body_t2_f17_03,
        "test_cm_t2_f17_04_collapse_and_reexpand_branch_preserves_siblings_and_count" => body_t2_f17_04,
        "test_cm_t2_f17_05_summon_bot_when_pending_or_in_allow_bots_false_blocked" => body_t2_f17_05,
        "test_cm_t2_f18_01_composer_empty_poll_invalid_link_and_empty_thread_validation" => body_t2_f18_01,
        "test_cm_t2_f18_02_multiline_paste_preserves_newlines_and_strips_control_chars" => body_t2_f18_02,
        "test_cm_t2_f18_03_composer_past_or_zero_schedule_rejected" => body_t2_f18_03,
        "test_cm_t2_f18_04_switching_composer_format_tabs_preserves_shared_state" => body_t2_f18_04,
        "test_cm_t2_f18_05_cjk_ime_inside_composer_title_body_escape_and_80ms_fence" => body_t2_f18_05,
        "test_cm_t2_f19_01_nonexistent_community_page_renders_graceful_not_found" => body_t2_f19_01,
        "test_cm_t2_f19_02_create_community_modal_rejects_invalid_and_duplicate_slug" => body_t2_f19_02,
        "test_cm_t2_f19_03_non_moderator_wiki_read_only_and_edit_rejected" => body_t2_f19_03,
        "test_cm_t2_f19_04_flair_filter_zero_matches_renders_empty_and_clears_back" => body_t2_f19_04,
        "test_cm_t2_f19_05_allow_bots_false_community_blocks_bot_act_as_composer" => body_t2_f19_05,
        "test_cm_t2_f20_01_nonexistent_actor_profile_renders_graceful_not_found" => body_t2_f20_01,
        "test_cm_t2_f20_02_bookmark_idempotency_and_delete_folder_preserves_posts" => body_t2_f20_02,
        "test_cm_t2_f20_03_empty_list_name_rejected_and_emptying_list_renders_safely" => body_t2_f20_03,
        "test_cm_t2_f20_04_notifications_zero_unread_mark_all_read_clean_noop" => body_t2_f20_04,
        "test_cm_t2_f20_05_notification_jump_to_deleted_post_graceful_toast_fallback" => body_t2_f20_05,
        "test_cm_t2_f21_01_search_empty_punctuation_and_zero_match_renders_cleanly" => body_t2_f21_01,
        "test_cm_t2_f21_02_non_mod_opening_mod_console_gets_unauthorized_guard" => body_t2_f21_02,
        "test_cm_t2_f21_03_automod_editor_rejects_empty_name_and_invalid_regex" => body_t2_f21_03,
        "test_cm_t2_f21_04_bot_studio_rejects_invalid_username_and_out_of_range_limits" => body_t2_f21_04,
        "test_cm_t2_f21_05_bot_studio_grok_avatar_8x11_clamping_and_token_copy_feedback" => body_t2_f21_05,
    }
}

fn clone_post_as(
    base: &CommunityPostDto,
    post_id: &str,
    author_id: &str,
    kind: ActorKind,
    comm_slug: &str,
    body: &str,
    upvotes: i64,
    downvotes: i64,
    created_at_ms: i64,
) -> CommunityPostDto {
    let mut p = base.clone();
    p.post_id = post_id.to_string();
    p.author_actor_id = author_id.to_string();
    p.author_handle = author_id.replace(':', "_");
    p.author_kind = kind;
    p.community_id = Some(format!("comm_{comm_slug}"));
    p.community_slug = Some(comm_slug.to_string());
    p.title = Some(format!("Title {post_id}"));
    p.body = body.to_string();
    p.upvotes = upvotes;
    p.downvotes = downvotes;
    p.score = upvotes - downvotes;
    p.status = PostStatus::Published;
    p.is_removed = false;
    p.is_nsfw = false;
    p.created_at_ms = created_at_ms;
    p.updated_at_ms = created_at_ms;
    p
}

fn make_automod_rule(id: &str, action: AutoModActionKind) -> AutoModRuleDto {
    AutoModRuleDto {
        rule_id: id.to_string(),
        community_id: "comm_t2".to_string(),
        name: format!("Rule {id}"),
        enabled: true,
        priority: 10,
        keywords: Vec::new(),
        regex_pattern: None,
        min_author_karma: None,
        min_account_age_secs: None,
        max_links: None,
        block_duplicate_spam: false,
        apply_to_bots: true,
        apply_to_humans: true,
        action,
        assign_flair: None,
        auto_reply_template: None,
        created_at_ms: 0,
    }
}

// ============================================================================
// F02 — Actor Model Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F02-01
/// [Feature / Req] F02 / TEST_INFRA.md §6 (Duplicate & malformed handle rejection)
/// [Preconditions] Ephemeral live server with 2 Humans registered (Alice & Bob).
/// [Opaque-Box Steps] Claim `@unique_citizen_01` for Alice, attempt case-insensitive duplicate Human and Bot registration for Bob, and test empty/too-short/too-long/punctuation/SQLi handles.
/// [Observable Assertions] Duplicate handles return 409 Conflict; all malformed handles fail `normalize_handle` and return 400 Bad Request over REST.
#[tokio::test]
async fn test_cm_t2_f02_01_duplicate_and_malformed_handles_rejected() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f02_01", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f02_01", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    let (st, _) = driver
        .req(
            Method::POST,
            "/api/v1/community/identities/human",
            &a_auth,
            Some(json!({ "handle": "unique_citizen_01", "display_name": "Alice Unique" })),
        )
        .await
        .unwrap();
    assert_eq!(st, 201);

    // Case-insensitive duplicate Human handle -> 409 Conflict
    let (st_dup_h, val_dup_h) = driver
        .req(
            Method::POST,
            "/api/v1/community/identities/human",
            &b_auth,
            Some(json!({ "handle": "UNIQUE_CITIZEN_01", "display_name": "Bob Clash" })),
        )
        .await
        .unwrap();
    assert_eq!(st_dup_h, 409, "duplicate human handle must return 409: {val_dup_h}");

    // Case-insensitive duplicate Bot handle -> 409 Conflict
    let (st_dup_b, val_dup_b) = driver
        .req(
            Method::POST,
            "/api/v1/community/identities/bot",
            &b_auth,
            Some(json!({
                "handle": "Unique_Citizen_01",
                "display_name": "Clash Bot",
                "bot_autonomy_mode": "manual",
                "bot_capabilities": []
            })),
        )
        .await
        .unwrap();
    assert_eq!(st_dup_b, 409, "duplicate bot handle must return 409: {val_dup_b}");

    let long_handle = "a".repeat(33);
    for bad in ["", "a", "___", "bad handle", "sql';--", "<script>", long_handle.as_str()] {
        assert!(normalize_handle(bad).is_err(), "normalize_handle must reject '{bad}'");
        let (st_bad, val_bad) = driver
            .req(
                Method::POST,
                "/api/v1/community/identities/human",
                &b_auth,
                Some(json!({ "handle": bad, "display_name": "Bad Handle" })),
            )
            .await
            .unwrap();
        assert_eq!(st_bad, 400, "malformed handle '{bad}' must return 400: {val_bad}");
    }
}

/// [Test ID] TEST-CM-T2-F02-02
/// [Feature / Req] F02 / TEST_INFRA.md §6 (Self-follow rejection & double follow/unfollow idempotency)
/// [Preconditions] Ephemeral live server with 2 onboarded Humans (Alice & Bob).
/// [Opaque-Box Steps] Attempt Alice->Alice self-follow, follow Bob twice consecutively, then unfollow Bob twice consecutively.
/// [Observable Assertions] Self-follow returns 400; repeated follow leaves `followers_count == 1`; repeated unfollow leaves `followers_count == 0` with zero underflow.
#[tokio::test]
async fn test_cm_t2_f02_02_self_follow_rejected_and_double_follow_idempotent() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f02_02", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f02_02", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &b_auth, None).await.unwrap().0, 200);

    let (st_self, val_self) = driver
        .req(Method::POST, &format!("/api/v1/community/actors/{}/follow", alice.actor_id), &a_auth, None)
        .await
        .unwrap();
    assert_eq!(st_self, 400, "self-follow must return 400: {val_self}");

    for _ in 0..2 {
        let (st, val) = driver
            .req(Method::POST, &format!("/api/v1/community/actors/{}/follow", bob.actor_id), &a_auth, None)
            .await
            .unwrap();
        assert_eq!(st, 200, "follow failed: {val}");
        assert_eq!(val["is_following"], true);
        assert_eq!(val["followers_count"], 1, "double follow must remain idempotent");
    }

    for _ in 0..2 {
        let (st, val) = driver
            .req(Method::DELETE, &format!("/api/v1/community/actors/{}/follow", bob.actor_id), &a_auth, None)
            .await
            .unwrap();
        assert_eq!(st, 200, "unfollow failed: {val}");
        assert_eq!(val["is_following"], false);
        assert_eq!(val["followers_count"], 0, "double unfollow must never underflow");
    }
}

/// [Test ID] TEST-CM-T2-F02-03
/// [Feature / Req] F02 / TEST_INFRA.md §6 (Extreme bio length, CJK+emoji display names & website boundaries)
/// [Preconditions] Ephemeral live server with Human Alice registered.
/// [Opaque-Box Steps] Reject empty and >64-char display_name on registration, then PATCH profile with empty bio, CJK+emoji display name, malformed website, and 4 KiB bio.
/// [Observable Assertions] Out-of-range display names fail with 400; valid CJK/emoji names and extreme/empty bios persist without 500 or UTF-8 corruption.
#[tokio::test]
async fn test_cm_t2_f02_03_extreme_bio_cjk_emoji_display_name_and_website_boundaries() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f02_03", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    for bad_dn in ["".to_string(), "   ".to_string(), "极".repeat(65)] {
        let (st, val) = driver
            .req(
                Method::POST,
                "/api/v1/community/identities/human",
                &auth,
                Some(json!({ "handle": "alice_t2_f02_03", "display_name": bad_dn })),
            )
            .await
            .unwrap();
        assert_eq!(st, 400, "invalid display_name must return 400: {val}");
    }

    let cjk_emoji_name = "🦀 社区架构师 Alice 🚀";
    let (st1, v1) = driver
        .req(
            Method::PATCH,
            &format!("/api/v1/community/actors/{}", alice.actor_id),
            &auth,
            Some(json!({
                "display_name": cjk_emoji_name,
                "bio": "",
                "website": "not-a-valid-url://bad host"
            })),
        )
        .await
        .unwrap();
    assert_eq!(st1, 200, "CJK/emoji profile update failed: {v1}");
    assert_eq!(v1["display_name"], cjk_emoji_name);
    assert_eq!(v1["bio"], "");

    let long_bio = "系统设计边界测试_".repeat(400);
    let (st2, v2) = driver
        .req(
            Method::PATCH,
            &format!("/api/v1/community/actors/{}", alice.actor_id),
            &auth,
            Some(json!({ "bio": long_bio, "website": "" })),
        )
        .await
        .unwrap();
    assert_eq!(st2, 200, "long bio update failed: {v2}");
    assert_eq!(v2["bio"].as_str().unwrap(), long_bio);
    assert!(v2["website"].is_null(), "empty website patch should clear optional field");
}

/// [Test ID] TEST-CM-T2-F02-04
/// [Feature / Req] F02 / TEST_INFRA.md §6 (Non-owner PATCH on another user's Bot returns 403)
/// [Preconditions] Ephemeral live server with Alice owning `owned_t2_f02_bot` and Bob owning `bob_t2_f02_bot`.
/// [Opaque-Box Steps] Attempt PATCH `/actors/{alice_bot}` using Bob's Human token and Bob's DirectBot token, then read Alice's Bot profile.
/// [Observable Assertions] Both unauthorized PATCH calls return 403 Forbidden and Alice's Bot metadata remains untouched.
#[tokio::test]
async fn test_cm_t2_f02_04_non_owner_patch_bot_actor_forbidden() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f02_04", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f02_04", "Pass1234!").await.unwrap();
    let alice_bot = driver
        .register_community_bot(&alice.token, "owned_t2_f02_bot", "Alice Bot Original", "manual")
        .await
        .unwrap();
    let bob_bot = driver
        .register_community_bot(&bob.token, "bob_t2_f02_bot", "Bob Bot", "manual")
        .await
        .unwrap();

    for attacker in [
        CommunityAuthMode::Human(bob.token.clone()),
        CommunityAuthMode::DirectBot(bob_bot.bot_token.clone()),
    ] {
        let (st, val) = driver
            .req(
                Method::PATCH,
                &format!("/api/v1/community/actors/{}", alice_bot.actor_id),
                &attacker,
                Some(json!({ "display_name": "Hijacked Bot", "bio": "Pwned" })),
            )
            .await
            .unwrap();
        assert_eq!(st, 403, "non-owner PATCH on bot must return 403: {val}");
    }

    let (st, prof) = driver
        .req(
            Method::GET,
            &format!("/api/v1/community/actors/{}", alice_bot.actor_id),
            &CommunityAuthMode::Human(alice.token),
            None,
        )
        .await
        .unwrap();
    assert_eq!(st, 200);
    assert_eq!(prof["actor"]["display_name"], "Alice Bot Original");
}

/// [Test ID] TEST-CM-T2-F02-05
/// [Feature / Req] F02 / TEST_INFRA.md §6 (Trust score bounds under extreme positive/negative karma swings)
/// [Preconditions] Pure-Rust actor DTO derived from starter dataset.
/// [Opaque-Box Steps] Apply extreme negative, zero, and extreme positive karma deltas plus clock-skew timestamps and compute `compute_trust_score`.
/// [Observable Assertions] Every score is finite, clamped strictly within `[MIN_TRUST_SCORE, MAX_TRUST_SCORE]`, and monotonically rises from negative to positive karma.
#[test]
fn test_cm_t2_f02_05_trust_score_strictly_bounded_under_extreme_karma_swings() {
    let t0 = 1_760_000_000_000_i64;
    let base = build_starter_dataset(t0).actors[0].clone();

    let mut neg = base.clone();
    neg.post_karma = 0;
    neg.comment_karma = 0;
    neg.awardee_karma = 0;
    apply_karma_delta(&mut neg, KarmaKind::Post, i64::MIN / 2);
    apply_karma_delta(&mut neg, KarmaKind::Comment, i64::MIN / 2);

    let mut zero = base.clone();
    zero.post_karma = 0;
    zero.comment_karma = 0;
    zero.awardee_karma = 0;
    zero.followers_count = 0;
    zero.created_at_ms = t0 + 999_999; // future clock skew

    let mut huge = base.clone();
    huge.post_karma = 0;
    huge.comment_karma = 0;
    huge.awardee_karma = 0;
    apply_karma_delta(&mut huge, KarmaKind::Post, i64::MAX / 2);
    apply_karma_delta(&mut huge, KarmaKind::Awardee, i64::MAX / 2);
    huge.followers_count = u64::MAX / 2;
    huge.created_at_ms = 0;

    let s_neg = compute_trust_score(&neg, t0);
    let s_zero = compute_trust_score(&zero, t0);
    let s_huge = compute_trust_score(&huge, t0);

    for s in [s_neg, s_zero, s_huge] {
        assert!(s.is_finite(), "trust_score must be finite, got {s}");
        assert!(
            (MIN_TRUST_SCORE..=MAX_TRUST_SCORE).contains(&s),
            "trust_score {s} out of [{MIN_TRUST_SCORE}, {MAX_TRUST_SCORE}]"
        );
    }
    assert!(s_neg <= s_zero && s_zero <= s_huge, "trust scores must be monotonic: {s_neg} <= {s_zero} <= {s_huge}");
}

// ============================================================================
// F03 — Communities (`c/<slug>`) Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F03-01
/// [Feature / Req] F03 / TEST_INFRA.md §6 (Invalid & duplicate community slug rejection)
/// [Preconditions] Ephemeral live server with Human Alice registered.
/// [Opaque-Box Steps] Attempt creating communities with empty, 1-char, 33-char, punctuation, hyphen, and SQLi slugs; then create valid `c/t2_unique_lab` and retry duplicate `T2_UNIQUE_LAB`.
/// [Observable Assertions] All malformed slugs fail `normalize_community_slug` and return 400; valid slug returns 201; case-insensitive duplicate returns 409.
#[tokio::test]
async fn test_cm_t2_f03_01_invalid_and_duplicate_community_slugs_rejected() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f03_01", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let too_long = "z".repeat(33);
    for bad in ["", "a", "c/", "bad slug", "rust-hyphen", "sql';DROP", "emoji_🦀", too_long.as_str()] {
        assert!(normalize_community_slug(bad).is_err(), "normalize_community_slug must reject '{bad}'");
        let (st, val) = driver
            .req(
                Method::POST,
                "/api/v1/community/communities",
                &auth,
                Some(json!({ "slug": bad, "name": "Invalid Comm", "allow_bots": true })),
            )
            .await
            .unwrap();
        assert_eq!(st, 400, "invalid slug '{bad}' must return 400: {val}");
    }

    let (st_ok, _) = driver
        .req(
            Method::POST,
            "/api/v1/community/communities",
            &auth,
            Some(json!({ "slug": "t2_unique_lab", "name": "Unique Lab", "allow_bots": true })),
        )
        .await
        .unwrap();
    assert_eq!(st_ok, 201);

    let (st_dup, val_dup) = driver
        .req(
            Method::POST,
            "/api/v1/community/communities",
            &auth,
            Some(json!({ "slug": "T2_UNIQUE_LAB", "name": "Dup Lab", "allow_bots": true })),
        )
        .await
        .unwrap();
    assert_eq!(st_dup, 409, "duplicate slug must return 409: {val_dup}");
}

/// [Test ID] TEST-CM-T2-F03-02
/// [Feature / Req] F03 / TEST_INFRA.md §6 (`allow_bots == false` rejects Bot posts/comments with 403 while permitting Humans)
/// [Preconditions] Ephemeral live server with Alice, owned Bot, and community `c/humans_only_f03` (`allow_bots: false`).
/// [Opaque-Box Steps] Attempt Bot post via DirectBot and ActAsBot, Human post, Bot comment on Human post, and Human comment.
/// [Observable Assertions] Every Bot post/comment in `c/humans_only_f03` returns 403 Forbidden; Human post/comment return 201 Created.
#[tokio::test]
async fn test_cm_t2_f03_02_allow_bots_false_rejects_bot_posts_and_comments_allows_humans() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f03_02", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "blocked_t2_f03_bot", "Blocked Bot", "manual").await.unwrap();
    let h_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_direct = CommunityAuthMode::DirectBot(bot.bot_token.clone());
    let b_act_as = CommunityAuthMode::ActAsBot { owner_token: alice.token.clone(), actor_id: bot.actor_id.clone() };

    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/communities", &h_auth, Some(json!({
            "slug": "humans_only_f03", "name": "Humans Only", "allow_bots": false
        }))).await.unwrap().0,
        201
    );

    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/posts", &b_direct, Some(json!({
            "community_slug": "humans_only_f03", "body": "Direct bot attempt"
        }))).await.unwrap().0,
        403
    );
    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/posts", &b_act_as, Some(json!({
            "community_slug": "humans_only_f03", "body": "Act-as bot attempt"
        }))).await.unwrap().0,
        403
    );

    let (st_hp, hp) = driver
        .req(Method::POST, "/api/v1/community/posts", &h_auth, Some(json!({
            "community_slug": "humans_only_f03", "body": "Human post allowed"
        })))
        .await
        .unwrap();
    assert_eq!(st_hp, 201);
    let pid = hp["post"]["post_id"].as_str().unwrap();

    assert_eq!(
        driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &b_direct, Some(json!({
            "body": "Bot comment attempt"
        }))).await.unwrap().0,
        403
    );
    assert_eq!(
        driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &h_auth, Some(json!({
            "body": "Human comment allowed"
        }))).await.unwrap().0,
        201
    );
}

/// [Test ID] TEST-CM-T2-F03-03
/// [Feature / Req] F03 / TEST_INFRA.md §6 (Banned and muted members rejected with 403 on post/comment/rejoin)
/// [Preconditions] Ephemeral live server with owner Alice in `c/strict_f03`, plus regular members Bob (muted) and Charlie (banned).
/// [Opaque-Box Steps] Mute Bob and ban Charlie via mod-actions, then attempt posting, commenting, and ban-evading join/leave.
/// [Observable Assertions] Muted and banned actors receive 403 on posts and comments; banned actor also receives 403 on join/leave.
#[tokio::test]
async fn test_cm_t2_f03_03_banned_and_muted_members_rejected_with_403() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f03_03", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f03_03", "Pass1234!").await.unwrap();
    let charlie = driver.register_human("charlie_t2_f03_03", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let c_auth = CommunityAuthMode::Human(charlie.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "strict_f03", "name": "Strict Lab" }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/strict_f03/join", &b_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/strict_f03/join", &c_auth, None).await.unwrap().0, 200);

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "community_slug": "strict_f03", "body": "Owner thread" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap();

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/strict_f03/mod-actions", &a_auth, Some(json!({ "action": "mute_actor", "target_kind": "actor", "target_id": bob.actor_id }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/strict_f03/mod-actions", &a_auth, Some(json!({ "action": "ban_actor", "target_kind": "actor", "target_id": charlie.actor_id }))).await.unwrap().0, 200);

    for restricted in [&b_auth, &c_auth] {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", restricted, Some(json!({ "community_slug": "strict_f03", "body": "Blocked post" }))).await.unwrap().0, 403);
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), restricted, Some(json!({ "body": "Blocked reply" }))).await.unwrap().0, 403);
    }
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/strict_f03/join", &c_auth, None).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::DELETE, "/api/v1/community/communities/strict_f03/join", &c_auth, None).await.unwrap().0, 403);
}

/// [Test ID] TEST-CM-T2-F03-04
/// [Feature / Req] F03 / TEST_INFRA.md §6 (Non-moderator pin, wiki edit, and settings update return 403)
/// [Preconditions] Ephemeral live server with Alice owning `c/mod_guard_f03` (1 post) and Bob joined as regular member.
/// [Opaque-Box Steps] Bob attempts pinning Alice's post, editing wiki via PUT `/wiki`, and mutating rules via PATCH `/communities/{slug}`.
/// [Observable Assertions] All 3 unauthorized actions return 403 Forbidden; wiki revision stays 1 and `pinned_post_ids` stays empty.
#[tokio::test]
async fn test_cm_t2_f03_04_non_moderator_pin_wiki_and_settings_update_forbidden() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f03_04", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f03_04", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "mod_guard_f03", "name": "Mod Guard" }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/mod_guard_f03/join", &b_auth, None).await.unwrap().0, 200);
    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "community_slug": "mod_guard_f03", "body": "Unpinned post" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap();

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/mod_guard_f03/mod-actions", &b_auth, Some(json!({ "action": "pin", "target_kind": "post", "target_id": pid }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::PUT, "/api/v1/community/communities/mod_guard_f03/wiki", &b_auth, Some(json!({ "markdown": "# Tampered", "summary": "Illegal" }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::PATCH, "/api/v1/community/communities/mod_guard_f03", &b_auth, Some(json!({ "description": "Hijacked" }))).await.unwrap().0, 403);

    let (st, detail) = driver.req(Method::GET, "/api/v1/community/communities/mod_guard_f03", &a_auth, None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(detail["community"]["wiki_revision"], 1);
    assert!(detail["community"]["pinned_post_ids"].as_array().unwrap().is_empty());
}

/// [Test ID] TEST-CM-T2-F03-05
/// [Feature / Req] F03 / TEST_INFRA.md §6 (Pin idempotency/multi-pin boundaries & empty/large wiki updates)
/// [Preconditions] Ephemeral live server with Alice owning `c/pin_wiki_f03` and 3 published posts.
/// [Opaque-Box Steps] Pin post 1 twice, pin posts 2 and 3, unpin post 2; update wiki with empty body and 32 KiB Markdown body.
/// [Observable Assertions] Duplicate pin creates zero duplicate IDs, unpin removes cleanly, and empty + 32 KiB wiki revisions increment cleanly to revision 3.
#[tokio::test]
async fn test_cm_t2_f03_05_pin_boundaries_idempotency_and_empty_or_large_wiki_body() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f03_05", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &auth, Some(json!({ "slug": "pin_wiki_f03", "name": "Pin Wiki Lab" }))).await.unwrap().0, 201);
    let mut pids = Vec::new();
    for i in 1..=3 {
        let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "community_slug": "pin_wiki_f03", "body": format!("Announcement {i}") }))).await.unwrap();
        pids.push(pv["post"]["post_id"].as_str().unwrap().to_string());
    }

    for pid in [&pids[0], &pids[0], &pids[1], &pids[2]] {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/pin_wiki_f03/mod-actions", &auth, Some(json!({ "action": "pin", "target_kind": "post", "target_id": pid }))).await.unwrap().0, 200);
    }
    let (_, det1) = driver.req(Method::GET, "/api/v1/community/communities/pin_wiki_f03", &auth, None).await.unwrap();
    let pinned1 = det1["community"]["pinned_post_ids"].as_array().unwrap();
    assert_eq!(pinned1.len(), 3, "re-pinning post 1 must not create duplicate entries: {pinned1:?}");

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/pin_wiki_f03/mod-actions", &auth, Some(json!({ "action": "unpin", "target_kind": "post", "target_id": pids[1] }))).await.unwrap().0, 200);
    let (_, det2) = driver.req(Method::GET, "/api/v1/community/communities/pin_wiki_f03", &auth, None).await.unwrap();
    assert_eq!(det2["community"]["pinned_post_ids"].as_array().unwrap().len(), 2);

    let (st_empty, rev2) = driver.req(Method::PUT, "/api/v1/community/communities/pin_wiki_f03/wiki", &auth, Some(json!({ "markdown": "", "summary": "Clear wiki" }))).await.unwrap();
    assert_eq!(st_empty, 200);
    assert_eq!(rev2["revision"], 2);

    let large_md = format!("# Large Wiki\n{}", "Section body with markdown **bold**.\n".repeat(900));
    let (st_big, rev3) = driver.req(Method::PUT, "/api/v1/community/communities/pin_wiki_f03/wiki", &auth, Some(json!({ "markdown": large_md, "summary": "32KB guide" }))).await.unwrap();
    assert_eq!(st_big, 200);
    assert_eq!(rev3["revision"], 3);
    assert_eq!(rev3["markdown"].as_str().unwrap(), large_md);
}

// ============================================================================
// F04 — Post Formats Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F04-01
/// [Feature / Req] F04 / TEST_INFRA.md §6 (Empty & oversized post bodies and titles rejected; exact max succeeds)
/// [Preconditions] Ephemeral live server with Human Alice registered.
/// [Opaque-Box Steps] Submit empty/whitespace Micro, untitled Article, 301-char title, `MAX_MICRO_BODY_CHARS + 1`, `MAX_ARTICLE_BODY_CHARS + 1`, and exact `MAX_MICRO_BODY_CHARS` body.
/// [Observable Assertions] All invalid/oversized payloads return 400 Bad Request; exact `MAX_MICRO_BODY_CHARS` boundary succeeds with 201 Created.
#[tokio::test]
async fn test_cm_t2_f04_01_empty_and_oversized_post_bodies_and_titles_rejected() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f04_01", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let bad_payloads = [
        json!({ "format": "micro", "body": "" }),
        json!({ "format": "micro", "body": "   \n\t  " }),
        json!({ "format": "article", "title": "", "body": "Article without title" }),
        json!({ "format": "article", "title": "T".repeat(301), "body": "Valid body" }),
        json!({ "format": "micro", "body": "m".repeat(MAX_MICRO_BODY_CHARS + 1) }),
        json!({ "format": "article", "title": "Huge", "body": "a".repeat(MAX_ARTICLE_BODY_CHARS + 1) }),
    ];
    for payload in bad_payloads {
        let (st, val) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(payload)).await.unwrap();
        assert_eq!(st, 400, "invalid post payload must return 400: {val}");
    }

    let exact_micro = "x".repeat(MAX_MICRO_BODY_CHARS);
    let (st_ok, ok_val) = driver
        .req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "format": "micro", "body": exact_micro })))
        .await
        .unwrap();
    assert_eq!(st_ok, 201, "exact MAX_MICRO_BODY_CHARS boundary must succeed: {ok_val}");
    assert_eq!(ok_val["post"]["body"].as_str().unwrap().chars().count(), MAX_MICRO_BODY_CHARS);
}

/// [Test ID] TEST-CM-T2-F04-02
/// [Feature / Req] F04 / TEST_INFRA.md §6 (Media gallery 0, 1, 4, and 5 attachment boundaries)
/// [Preconditions] Ephemeral live server with Alice having uploaded 5 distinct PNG fixtures.
/// [Opaque-Box Steps] Submit `format: "media"` posts with 0, 1, 4, and 5 media attachments.
/// [Observable Assertions] 0 items and 5 items return 400 Bad Request; 1 item and 4 items succeed with 201 Created and exact attachment counts.
#[tokio::test]
async fn test_cm_t2_f04_02_media_gallery_0_1_4_5_item_boundaries() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f04_02", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let mut mids = Vec::new();
    for i in 0..5u8 {
        let (mid, _) = driver.upload_png(&alice.token, &format!("gal_{i}.png"), (i * 40, 80, 160, 255), None).await.unwrap();
        mids.push(json!({ "media_id": mid, "kind": "image" }));
    }

    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "format": "media", "body": "Zero media", "media": [] }))).await.unwrap().0,
        400
    );
    let (st1, v1) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "format": "media", "body": "One media", "media": &mids[..1] }))).await.unwrap();
    assert_eq!(st1, 201);
    assert_eq!(v1["post"]["media"].as_array().unwrap().len(), 1);

    let (st4, v4) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "format": "media", "body": "Four media", "media": &mids[..4] }))).await.unwrap();
    assert_eq!(st4, 201);
    assert_eq!(v4["post"]["media"].as_array().unwrap().len(), 4);

    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "format": "media", "body": "Five media", "media": &mids[..5] }))).await.unwrap().0,
        400
    );
}

/// [Test ID] TEST-CM-T2-F04-03
/// [Feature / Req] F04 / TEST_INFRA.md §6 (Poll option count, single-choice, unknown option, duplicate vote & expiry boundaries)
/// [Preconditions] Ephemeral live server with Alice and Bob registered.
/// [Opaque-Box Steps] Reject 1-option, 7-option, duplicate-option, and <300s polls; create valid single-choice poll; reject empty/unknown/multi-option votes; verify idempotent repeat vote; close expired poll and reject late vote.
/// [Observable Assertions] Invalid poll specs & votes return 400; duplicate vote never double-counts; expired poll vote returns 410 Gone and fails `apply_poll_vote`.
#[tokio::test]
async fn test_cm_t2_f04_03_poll_option_count_single_choice_oob_and_expiry_boundaries() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f04_03", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f04_03", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    for bad_poll in [
        json!({ "question": "Too few?", "options": ["Only"], "multi_select": false, "duration_secs": 3600 }),
        json!({ "question": "Too many?", "options": ["1","2","3","4","5","6","7"], "multi_select": false, "duration_secs": 3600 }),
        json!({ "question": "Dup?", "options": ["Yes", "yes"], "multi_select": false, "duration_secs": 3600 }),
        json!({ "question": "Short?", "options": ["A", "B"], "multi_select": false, "duration_secs": 60 }),
    ] {
        let (st, val) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "format": "poll", "body": "Poll test", "poll": bad_poll }))).await.unwrap();
        assert_eq!(st, 400, "invalid poll must return 400: {val}");
    }

    let (st, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "format": "poll",
            "body": "Valid single-choice poll",
            "poll": { "question": "Pick one", "options": ["Alpha", "Beta"], "multi_select": false, "duration_secs": 3600 }
        })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    for bad_vote in [json!({ "option_ids": [] }), json!({ "option_ids": ["opt_99"] }), json!({ "option_ids": ["opt_0", "opt_1"] })] {
        let (vst, vval) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/poll-vote"), &a_auth, Some(bad_vote)).await.unwrap();
        assert_eq!(vst, 400, "invalid poll vote must return 400: {vval}");
    }

    let (st_v1, v1) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/poll-vote"), &a_auth, Some(json!({ "option_ids": ["opt_0"] }))).await.unwrap();
    let (st_v2, v2) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/poll-vote"), &a_auth, Some(json!({ "option_ids": ["opt_0"] }))).await.unwrap();
    assert_eq!(st_v1, 200);
    assert_eq!(st_v2, 200);
    assert_eq!(v1["total_votes"], 1);
    assert_eq!(v2["total_votes"], 1, "re-voting same option must not double-count");

    let mut poll_dto: gpui_telegram_proto::community_contracts::PollDto = serde_json::from_value(v2).unwrap();
    let expired_at_ms = poll_dto.expires_at_ms + 1;
    assert!(apply_poll_vote(&mut poll_dto, ActorKind::Human, &["opt_1".into()], &[], expired_at_ms).is_err());

    // Expire poll in DB, run background ticker, and verify REST returns 410 Gone
    let state = server.state().expect("active state");
    state.db.pool().with_transaction(|tx| {
        tx.execute("UPDATE community_posts SET poll_closes_at_ms = 1 WHERE post_id = ?1", rusqlite::params![&pid])
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        Ok(())
    }).unwrap();
    run_community_background_tick(state).await;

    let (st_late, late_val) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/poll-vote"), &b_auth, Some(json!({ "option_ids": ["opt_1"] }))).await.unwrap();
    assert_eq!(st_late, 410, "voting on closed poll must return 410 Gone: {late_val}");
}

/// [Test ID] TEST-CM-T2-F04-04
/// [Feature / Req] F04 / TEST_INFRA.md §6 (Quote/repost/crosspost missing or deleted post returns 404; self-repost toggle idempotent)
/// [Preconditions] Ephemeral live server with Alice and 1 published post.
/// [Opaque-Box Steps] Quote/repost/crosspost nonexistent ID; toggle self-repost on/off/on; delete post; retry quote/repost/crosspost on deleted ID.
/// [Observable Assertions] Nonexistent and deleted origin references return 404; self-repost count cleanly tracks `1 -> 0 -> 1`.
#[tokio::test]
async fn test_cm_t2_f04_04_quote_repost_crosspost_missing_or_deleted_and_repost_idempotency() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f04_04", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    for payload in [
        json!({ "format": "quote", "body": "Missing quote", "quoted_post_id": "post_missing_999" }),
        json!({ "format": "repost", "body": "", "reposted_post_id": "post_missing_999" }),
        json!({ "format": "crosspost", "body": "Missing xpost", "crosspost_of_id": "post_missing_999" }),
    ] {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(payload)).await.unwrap().0, 404);
    }

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "body": "Origin post" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let (s1, r1) = driver.req_empty_body(Method::POST, &format!("/api/v1/community/posts/{pid}/repost"), &auth).await.unwrap();
    let (s2, r2) = driver.req_empty_body(Method::POST, &format!("/api/v1/community/posts/{pid}/repost"), &auth).await.unwrap();
    let (s3, r3) = driver.req_empty_body(Method::POST, &format!("/api/v1/community/posts/{pid}/repost"), &auth).await.unwrap();
    assert_eq!((s1, s2, s3), (200, 200, 200));
    assert_eq!((r1["reposted"].as_bool(), r1["reposts_count"].as_u64()), (Some(true), Some(1)));
    assert_eq!((r2["reposted"].as_bool(), r2["reposts_count"].as_u64()), (Some(false), Some(0)));
    assert_eq!((r3["reposted"].as_bool(), r3["reposts_count"].as_u64()), (Some(true), Some(1)));

    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/posts/{pid}"), &auth, None).await.unwrap().0, 200);
    for payload in [
        json!({ "format": "quote", "body": "Deleted quote", "quoted_post_id": pid }),
        json!({ "format": "repost", "body": "", "reposted_post_id": pid }),
        json!({ "format": "crosspost", "body": "Deleted xpost", "crosspost_of_id": pid }),
    ] {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(payload)).await.unwrap().0, 404);
    }
}

/// [Test ID] TEST-CM-T2-F04-05
/// [Feature / Req] F04 / TEST_INFRA.md §6 (Non-author PATCH/DELETE rejected 403; deleted post cannot be edited or commented on)
/// [Preconditions] Ephemeral live server with Alice's post and non-author Bob.
/// [Opaque-Box Steps] Bob attempts PATCH and DELETE on Alice's post; Alice deletes post; Alice attempts PATCH and POST `/comments` on deleted post.
/// [Observable Assertions] Bob receives 403 on both; deleted post rejects PATCH with 410 Gone and comments with 404 Not Found.
#[tokio::test]
async fn test_cm_t2_f04_05_non_author_edit_delete_forbidden_and_deleted_post_immutable() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f04_05", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f04_05", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "body": "Alice owned post" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    assert_eq!(driver.req(Method::PATCH, &format!("/api/v1/community/posts/{pid}"), &b_auth, Some(json!({ "body": "Tampered" }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/posts/{pid}"), &b_auth, None).await.unwrap().0, 403);

    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/posts/{pid}"), &a_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::PATCH, &format!("/api/v1/community/posts/{pid}"), &a_auth, Some(json!({ "body": "Edit after delete" }))).await.unwrap().0, 410);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &a_auth, Some(json!({ "body": "Reply after delete" }))).await.unwrap().0, 404);
}

// ============================================================================
// F05 — Comment Forest & Thread Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F05-01
/// [Feature / Req] F05 / TEST_INFRA.md §6 (Deep nesting depth 14+ & cycle safety with valid materialized paths)
/// [Preconditions] Ephemeral live server with Alice's post + pure-Rust `CommentTreeBuilder`.
/// [Opaque-Box Steps] Chain 14 nested replies over REST (`depth 0..=13`), walk returned tree verifying path/depth, then build a 20-node chain and a 2-node cycle in `CommentTreeBuilder::build`.
/// [Observable Assertions] All 14 REST comments persist with exact `depth` and `comment_depth_from_path(&path) == depth`; synthetic cycle terminates safely without stack overflow or dropped nodes.
#[tokio::test]
async fn test_cm_t2_f05_01_deep_depth_14_comment_tree_builds_valid_paths_no_drops() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f05_01", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "body": "Deep nesting root" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let mut parent: Option<String> = None;
    let mut created_ids = Vec::new();
    for lvl in 0..14u32 {
        let mut body = json!({ "body": format!("Nested reply level {lvl}") });
        if let Some(ref p) = parent {
            body["parent_comment_id"] = json!(p);
        }
        let (st, cv) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &auth, Some(body)).await.unwrap();
        assert_eq!(st, 201, "level {lvl} comment failed: {cv}");
        let cid = cv["comment"]["comment_id"].as_str().unwrap().to_string();
        assert_eq!(cv["comment"]["depth"].as_u64(), Some(u64::from(lvl)));
        parent = Some(cid.clone());
        created_ids.push(cid);
    }

    let (st, list_val) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}/comments?limit=100"), &auth, None).await.unwrap();
    assert_eq!(st, 200);
    let roots: Vec<CommunityCommentDto> = serde_json::from_value(list_val["comments"].clone()).unwrap();
    assert_eq!(roots.len(), 1);
    let mut cur = &roots[0];
    for (lvl, expected_id) in created_ids.iter().enumerate() {
        assert_eq!(&cur.comment_id, expected_id);
        assert_eq!(cur.depth, lvl as u32);
        assert_eq!(comment_depth_from_path(&cur.path), cur.depth);
        if lvl + 1 < created_ids.len() {
            assert_eq!(cur.children.len(), 1, "level {lvl} must retain child");
            cur = &cur.children[0];
        }
    }

    // Cycle safety in pure-Rust CommentTreeBuilder
    let base_c = build_starter_dataset(now_ms()).comments[0].clone();
    let mut c1 = base_c.clone();
    c1.comment_id = "cyc_a".into();
    c1.parent_comment_id = Some("cyc_b".into());
    let mut c2 = base_c;
    c2.comment_id = "cyc_b".into();
    c2.parent_comment_id = Some("cyc_a".into());
    let built = CommentTreeBuilder::build(vec![c1, c2], CommentSort::Best);
    let total_nodes: usize = built.iter().map(|n| 1 + n.children.len()).sum();
    assert_eq!(total_nodes, 2, "cycle must break cleanly preserving both nodes");
}

/// [Test ID] TEST-CM-T2-F05-02
/// [Feature / Req] F05 / TEST_INFRA.md §6 (Locked/deleted post replies rejected; empty/oversized comments rejected)
/// [Preconditions] Ephemeral live server with Alice owning `c/lock_f05`, 2 posts, and Bob registered.
/// [Opaque-Box Steps] Reject empty, whitespace, and oversized comments (400); lock post 1 and attempt reply (403); delete post 2 and attempt reply (404 / store Err).
/// [Observable Assertions] Invalid comment bodies return 400; locked post rejects replies with 403; deleted post rejects replies cleanly.
#[tokio::test]
async fn test_cm_t2_f05_02_locked_and_deleted_post_replies_and_empty_comment_rejected() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f05_02", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f05_02", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "lock_f05", "name": "Lock F05" }))).await.unwrap().0, 201);
    let (_, p1) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "community_slug": "lock_f05", "body": "To be locked" }))).await.unwrap();
    let pid1 = p1["post"]["post_id"].as_str().unwrap().to_string();
    let (_, p2) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "community_slug": "lock_f05", "body": "To be deleted" }))).await.unwrap();
    let pid2 = p2["post"]["post_id"].as_str().unwrap().to_string();

    for bad in ["".to_string(), "   \n ".to_string(), "c".repeat(MAX_COMMENT_BODY_CHARS + 1)] {
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid1}/comments"), &a_auth, Some(json!({ "body": bad }))).await.unwrap().0, 400);
    }

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/lock_f05/mod-actions", &a_auth, Some(json!({ "action": "lock", "target_kind": "post", "target_id": pid1 }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid1}/comments"), &b_auth, Some(json!({ "body": "Locked reply" }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid1}/comments"), &a_auth, Some(json!({ "body": "Owner locked reply" }))).await.unwrap().0, 403);

    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/posts/{pid2}"), &a_auth, None).await.unwrap().0, 200);
    let (st_del, _) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid2}/comments"), &b_auth, Some(json!({ "body": "Deleted post reply" }))).await.unwrap();
    assert!(matches!(st_del, 403 | 404 | 410), "commenting on deleted post must be rejected, got {st_del}");
}

/// [Test ID] TEST-CM-T2-F05-03
/// [Feature / Req] F05 / TEST_INFRA.md §6 (Deleting parent comment preserves child subtree via tombstone)
/// [Preconditions] Ephemeral live server with Alice's post and a 3-level comment branch (`parent -> child -> grandchild`).
/// [Opaque-Box Steps] Delete the parent comment via `DELETE /comments/{parent_id}`, then fetch `GET /posts/{pid}/comments`.
/// [Observable Assertions] Parent remains in tree as `[deleted]` tombstone (`is_deleted == true`) and both child and grandchild stay attached beneath it.
#[tokio::test]
async fn test_cm_t2_f05_03_deleting_parent_comment_preserves_child_subtree_with_tombstone() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f05_03", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f05_03", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "body": "Tombstone thread" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let (_, c1) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &a_auth, Some(json!({ "body": "Parent text" }))).await.unwrap();
    let c1_id = c1["comment"]["comment_id"].as_str().unwrap().to_string();
    let (_, c2) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &b_auth, Some(json!({ "parent_comment_id": c1_id, "body": "Child text" }))).await.unwrap();
    let c2_id = c2["comment"]["comment_id"].as_str().unwrap().to_string();
    let (_, c3) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &a_auth, Some(json!({ "parent_comment_id": c2_id, "body": "Grandchild text" }))).await.unwrap();
    let c3_id = c3["comment"]["comment_id"].as_str().unwrap().to_string();

    assert_eq!(driver.req(Method::DELETE, &format!("/api/v1/community/comments/{c1_id}"), &a_auth, None).await.unwrap().0, 200);

    let (st, tree_val) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}/comments"), &a_auth, None).await.unwrap();
    assert_eq!(st, 200);
    let roots: Vec<CommunityCommentDto> = serde_json::from_value(tree_val["comments"].clone()).unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].comment_id, c1_id);
    assert!(roots[0].is_deleted, "deleted parent with children must be tombstoned");
    assert_eq!(roots[0].body, "[deleted]");
    assert_eq!(roots[0].children.len(), 1, "child must remain attached to tombstoned parent");
    assert_eq!(roots[0].children[0].comment_id, c2_id);
    assert_eq!(roots[0].children[0].children[0].comment_id, c3_id);
}

/// [Test ID] TEST-CM-T2-F05-04
/// [Feature / Req] F05 / TEST_INFRA.md §6 (`Controversial` sort zero-vote & equal-vote tie-breaking without NaN/Inf)
/// [Preconditions] Pure-Rust `reddit_controversial` and `CommentTreeBuilder`.
/// [Opaque-Box Steps] Evaluate `reddit_controversial` on `(0,0)`, `(10,0)`, `(0,10)`, `(-5,-5)`, `(50,50)`, `(50,5)`, and sort tied comments twice with `CommentSort::Controversial`.
/// [Observable Assertions] Zero NaN/Inf scores; 50/50 tied controversial comments outrank consensus 100/1 and 0/0 comments with 100% deterministic tie-breaking.
#[test]
fn test_cm_t2_f05_04_controversial_sort_tie_breaking_zero_and_balanced_votes_nan_free() {
    for (u, d) in [(0, 0), (10, 0), (0, 10), (-5, -5), (50, 50), (50, 5), (100, 1)] {
        let s = reddit_controversial(u, d);
        assert!(s.is_finite() && !s.is_nan(), "reddit_controversial({u},{d}) must be finite, got {s}");
    }
    assert_eq!(reddit_controversial(0, 0), 0.0);
    assert!(reddit_controversial(50, 50) > reddit_controversial(50, 5));

    let base = build_starter_dataset(1_760_000_000_000).comments[0].clone();
    let mk = |id: &str, up: i64, down: i64, ts: i64| {
        let mut c = base.clone();
        c.comment_id = id.into();
        c.parent_comment_id = None;
        c.is_stickied = false;
        c.upvotes = up;
        c.downvotes = down;
        c.score = up - down;
        c.created_at_ms = ts;
        c
    };
    let flat = vec![
        mk("z_b", 0, 0, 2000),
        mk("z_a", 0, 0, 1000),
        mk("cons", 100, 1, 1500),
        mk("tie_2", 50, 50, 1200),
        mk("tie_1", 50, 50, 1100),
    ];
    let run1: Vec<String> = CommentTreeBuilder::build(flat.clone(), CommentSort::Controversial).into_iter().map(|c| c.comment_id).collect();
    let run2: Vec<String> = CommentTreeBuilder::build(flat, CommentSort::Controversial).into_iter().map(|c| c.comment_id).collect();
    assert_eq!(run1, run2, "controversial sort tie-breaking must be deterministic");
    assert_eq!(&run1[..2], &["tie_1".to_string(), "tie_2".to_string()]);
}

/// [Test ID] TEST-CM-T2-F05-05
/// [Feature / Req] F05 / TEST_INFRA.md §6 (Comment pagination limit=0, limit>max, invalid/stale cursor boundaries)
/// [Preconditions] Ephemeral live server with Alice's post containing 7 top-level comments.
/// [Opaque-Box Steps] Request `limit=0`, `limit=99999`, `cursor=not_a_number`, `cursor=999999`, and walk valid `limit=3` cursors to completion.
/// [Observable Assertions] Boundary limits clamp safely, malformed/stale cursors never panic or 500, and cursor pagination yields exact 7 unique IDs.
#[tokio::test]
async fn test_cm_t2_f05_05_comment_pagination_limit_and_cursor_boundaries() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f05_05", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "body": "Pagination post" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();
    for i in 0..7 {
        assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/comments"), &auth, Some(json!({ "body": format!("Top comment {i}") }))).await.unwrap().0, 201);
    }

    let (st0, v0) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}/comments?limit=0"), &auth, None).await.unwrap();
    assert_eq!(st0, 200);
    assert_eq!(v0["comments"].as_array().unwrap().len(), 1);
    assert_eq!(v0["has_more"], true);

    let (st_hi, v_hi) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}/comments?limit=99999"), &auth, None).await.unwrap();
    assert_eq!(st_hi, 200);
    assert_eq!(v_hi["comments"].as_array().unwrap().len(), 7);
    assert_eq!(v_hi["has_more"], false);

    let (st_bad, v_bad) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}/comments?cursor=not_a_number&limit=3"), &auth, None).await.unwrap();
    assert_eq!(st_bad, 200);
    assert_eq!(v_bad["comments"].as_array().unwrap().len(), 3);

    let (st_stale, v_stale) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}/comments?cursor=999999&limit=3"), &auth, None).await.unwrap();
    assert_eq!(st_stale, 200);
    assert!(v_stale["comments"].as_array().unwrap().is_empty());
    assert_eq!(v_stale["has_more"], false);

    let mut seen = HashSet::new();
    let mut cursor: Option<String> = None;
    for _ in 0..5 {
        let q = match cursor {
            Some(ref c) => format!("/api/v1/community/posts/{pid}/comments?limit=3&cursor={c}"),
            None => format!("/api/v1/community/posts/{pid}/comments?limit=3"),
        };
        let (st, page) = driver.req(Method::GET, &q, &auth, None).await.unwrap();
        assert_eq!(st, 200);
        for item in page["comments"].as_array().unwrap() {
            let cid = item["comment_id"].as_str().unwrap().to_string();
            assert!(seen.insert(cid), "paginated comments must not duplicate IDs");
        }
        if page["has_more"] != true {
            break;
        }
        cursor = page["next_cursor"].as_str().map(str::to_string);
    }
    assert_eq!(seen.len(), 7);
}

// ============================================================================
// F06 — Recommendation Engine Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F06-01
/// [Feature / Req] F06 / TEST_INFRA.md §6 (Empty candidate set returns valid empty feed across all tabs & sorts without NaN)
/// [Preconditions] Default `ViewerContext` and empty `Vec::new()` candidate pool.
/// [Opaque-Box Steps] Execute `CommunityRecommender::rank_feed` across every `FeedTab` and `FeedSort` combination.
/// [Observable Assertions] Every response returns empty `items`, `total_candidates == 0`, `next_cursor == None`, and zero panics.
#[test]
fn test_cm_t2_f06_01_empty_candidate_set_returns_valid_empty_feed_no_nan() {
    let viewer = ViewerContext { actor_id: "u:42".into(), now_ms: 1_760_000_000_000, ..Default::default() };
    let tabs = [
        FeedTab::ForYou,
        FeedTab::Following,
        FeedTab::Popular,
        FeedTab::All,
        FeedTab::Community("empty_comm".into()),
        FeedTab::List("empty_list".into()),
        FeedTab::Saved,
    ];
    let sorts = [FeedSort::Algo, FeedSort::Hot, FeedSort::Best, FeedSort::Top, FeedSort::New, FeedSort::Controversial, FeedSort::Rising];
    for tab in tabs {
        for sort in sorts {
            let resp = CommunityRecommender::rank_feed(
                &viewer,
                Vec::new(),
                &FeedRequestDto { tab: tab.clone(), sort, limit: 25, now_ms: Some(viewer.now_ms), ..Default::default() },
            );
            assert!(resp.items.is_empty());
            assert_eq!(resp.total_candidates, 0);
            assert!(resp.next_cursor.is_none());
        }
    }
}

/// [Test ID] TEST-CM-T2-F06-02
/// [Feature / Req] F06 / TEST_INFRA.md §6 (Extreme, negative & NaN/Inf weights clamped safely without NaN scores)
/// [Preconditions] Starter dataset candidates + in-memory `CommunityStore` + `sanitize_algo_prefs`.
/// [Opaque-Box Steps] Run `CommunityRecommender::rank_feed` with zeroed, negative, NaN, and Inf `AlgoPreferencesDto` overrides, and persist extreme prefs via `put_algo_prefs`.
/// [Observable Assertions] All ranked scores & explanation percentages are finite; stored and sanitized weights stay strictly within `[0.0, 1.0]` (`discovery_ratio <= 0.5` in store).
#[test]
fn test_cm_t2_f06_02_extreme_and_non_finite_recommender_weights_clamped_safely() {
    let t0 = 1_760_000_000_000_i64;
    let ds = build_starter_dataset(t0);
    let cands: Vec<PostCandidate> = ds.posts.iter().cloned().map(PostCandidate::from_post).collect();
    let viewer = ViewerContext { actor_id: "u:900001".into(), now_ms: t0, ..Default::default() };

    let extreme_prefs = [
        AlgoPreferencesDto { preset: AlgoPreset::Balanced, recency_weight: 0.0, affinity_weight: 0.0, quality_weight: 0.0, discovery_ratio: 0.0, bot_content_ratio: 0.0, show_explanations: true, allow_nsfw: false },
        AlgoPreferencesDto { preset: AlgoPreset::BotHeavy, recency_weight: -5.0, affinity_weight: f64::NAN, quality_weight: f64::INFINITY, discovery_ratio: 99.0, bot_content_ratio: f64::NEG_INFINITY, show_explanations: true, allow_nsfw: false },
        AlgoPreferencesDto { preset: AlgoPreset::HumanFocused, recency_weight: 100.0, affinity_weight: 100.0, quality_weight: 100.0, discovery_ratio: -2.0, bot_content_ratio: 10.0, show_explanations: true, allow_nsfw: false },
    ];
    for prefs in &extreme_prefs {
        let resp = CommunityRecommender::rank_feed(
            &viewer,
            cands.clone(),
            &FeedRequestDto { algo_override: Some(prefs.clone()), now_ms: Some(t0), limit: 15, ..Default::default() },
        );
        assert!(!resp.items.is_empty());
        for item in &resp.items {
            assert!(item.rank_score.is_finite(), "rank_score must be finite: {}", item.rank_score);
            let exp = item
                .explanation
                .as_ref()
                .expect("explanation must be populated when show_explanations=true");
            assert!(exp.signal_breakdown.iter().all(|s| s.contribution_pct.is_finite() && s.contribution_pct >= 0.0));
        }
        let ui_san = sanitize_algo_prefs(prefs);
        assert!((0.0..=1.0).contains(&ui_san.recency_weight) && (0.0..=1.0).contains(&ui_san.affinity_weight) && (0.0..=1.0).contains(&ui_san.bot_content_ratio));
    }

    let store = CommunityStore::open_in_memory().unwrap();
    store.seed_if_empty(t0).unwrap();
    let saved = store.put_algo_prefs("u:900001", &extreme_prefs[1], t0).unwrap();
    assert!((0.0..=1.0).contains(&saved.recency_weight));
    assert!((0.0..=1.0).contains(&saved.affinity_weight));
    assert!((0.0..=1.0).contains(&saved.quality_weight));
    assert!((0.0..=0.5).contains(&saved.discovery_ratio));
    assert!((0.0..=1.0).contains(&saved.bot_content_ratio));
}

/// [Test ID] TEST-CM-T2-F06-03
/// [Feature / Req] F06 / TEST_INFRA.md §6 (`allow_nsfw == false` & blocked/muted/hidden content never leak across any tab/sort)
/// [Preconditions] Mixed candidate pool containing 1 clean post and 6 gated posts (NSFW, blocked author, muted author, muted community, muted keyword, hidden post).
/// [Opaque-Box Steps] Rank across all 7 `FeedSort` modes with `allow_nsfw == false`, then toggle `allow_nsfw == true`.
/// [Observable Assertions] Only the clean post appears under `allow_nsfw == false` across every sort; enabling `allow_nsfw` admits only the NSFW post while keeping blocked/muted/hidden excluded.
#[test]
fn test_cm_t2_f06_03_allow_nsfw_false_and_muted_blocked_strictly_excluded_all_tabs_sorts() {
    let t0 = 1_760_000_000_000_i64;
    let base = build_starter_dataset(t0).posts[0].clone();
    let clean = clone_post_as(&base, "p_clean", "u:10", ActorKind::Human, "rust", "Clean systems post", 10, 0, t0);
    let mut nsfw = clone_post_as(&base, "p_nsfw", "u:11", ActorKind::Human, "rust", "Adult tagged post", 90, 0, t0);
    nsfw.is_nsfw = true;
    let blocked = clone_post_as(&base, "p_blk", "u:bad", ActorKind::Human, "rust", "Blocked author post", 95, 0, t0);
    let muted_a = clone_post_as(&base, "p_muta", "b:noisy", ActorKind::Bot, "rust", "Muted bot post", 95, 0, t0);
    let muted_c = clone_post_as(&base, "p_mutc", "u:12", ActorKind::Human, "politics", "Muted comm post", 95, 0, t0);
    let muted_kw = clone_post_as(&base, "p_kw", "u:13", ActorKind::Human, "rust", "Contains forbidden_spoiler_kw inside", 95, 0, t0);
    let hidden = clone_post_as(&base, "p_hid", "u:14", ActorKind::Human, "rust", "Hidden post", 95, 0, t0);

    let cands: Vec<PostCandidate> = [clean, nsfw, blocked, muted_a, muted_c, muted_kw, hidden]
        .into_iter()
        .map(PostCandidate::from_post)
        .collect();
    let mut viewer = ViewerContext { actor_id: "u:viewer".into(), now_ms: t0, ..Default::default() };
    viewer.algo_prefs.allow_nsfw = false;
    viewer.blocked_actor_ids.insert("u:bad".into());
    viewer.muted_actor_ids.insert("b:noisy".into());
    viewer.muted_communities.insert("politics".into());
    viewer.muted_keywords.push("forbidden_spoiler_kw".into());
    viewer.hidden_post_ids.insert("p_hid".into());

    for sort in [FeedSort::Algo, FeedSort::Hot, FeedSort::Best, FeedSort::Top, FeedSort::New, FeedSort::Controversial, FeedSort::Rising] {
        let resp = CommunityRecommender::rank_feed(
            &viewer,
            cands.clone(),
            &FeedRequestDto { tab: FeedTab::ForYou, sort, limit: 25, now_ms: Some(t0), ..Default::default() },
        );
        let ids: Vec<&str> = resp.items.iter().map(|i| i.post.post_id.as_str()).collect();
        assert_eq!(ids, vec!["p_clean"], "negative gating leaked under sort {sort:?}: {ids:?}");
    }

    viewer.algo_prefs.allow_nsfw = true;
    let resp_nsfw = CommunityRecommender::rank_feed(
        &viewer,
        cands,
        &FeedRequestDto { tab: FeedTab::ForYou, sort: FeedSort::Algo, limit: 25, now_ms: Some(t0), ..Default::default() },
    );
    let ids: HashSet<&str> = resp_nsfw.items.iter().map(|i| i.post.post_id.as_str()).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains("p_clean") && ids.contains("p_nsfw"));
}

/// [Test ID] TEST-CM-T2-F06-04
/// [Feature / Req] F06 / TEST_INFRA.md §6 (Impression fatigue & 50-post single-author MMR diversity cap)
/// [Preconditions] 50 high-score posts by one flooding author plus 12 diverse Human/Bot posts.
/// [Opaque-Box Steps] Apply impression fatigue to selected posts and rank `ForYou` (`limit: 10`).
/// [Observable Assertions] Flooding author is capped at <= 2 slots in top-10, no two consecutive items share the same author, and fatigued seen post scores lower than an identical unseen post.
#[test]
fn test_cm_t2_f06_04_impression_fatigue_and_50_post_single_author_mmr_cap() {
    let t0 = 1_760_000_000_000_i64;
    let base = build_starter_dataset(t0).posts[0].clone();
    let mut cands = Vec::new();
    for i in 0..50 {
        let p = clone_post_as(&base, &format!("flood_{i}"), "u:flooder", ActorKind::Human, "rust", &format!("Flood post #{i} about kernels"), 200, 1, t0 - i * 1_000);
        cands.push(PostCandidate::from_post(p));
    }
    for i in 0..12 {
        let (aid, kind) = if i % 2 == 0 { (format!("u:div_{i}"), ActorKind::Human) } else { (format!("b:div_{i}"), ActorKind::Bot) };
        let comm = ["rust", "ai_agents", "systems", "design_ui"][i % 4];
        let p = clone_post_as(&base, &format!("div_{i}"), &aid, kind, comm, &format!("Diverse perspective #{i} on {comm}"), 80, 1, t0 - (i as i64) * 5_000);
        cands.push(PostCandidate::from_post(p));
    }

    let mut viewer = ViewerContext { actor_id: "u:v64".into(), now_ms: t0, ..Default::default() };
    let resp = CommunityRecommender::rank_feed(
        &viewer,
        cands,
        &FeedRequestDto { tab: FeedTab::ForYou, sort: FeedSort::Algo, limit: 10, now_ms: Some(t0), ..Default::default() },
    );
    assert_eq!(resp.items.len(), 10);
    let flood_slots = resp.items.iter().filter(|it| it.post.author_actor_id == "u:flooder").count();
    assert!(flood_slots <= 2, "single flooding author must be capped at <= 2 slots, got {flood_slots}");
    for pair in resp.items.windows(2) {
        assert_ne!(pair[0].post.author_actor_id, pair[1].post.author_actor_id, "MMR must prevent consecutive same-author posts");
    }

    // Verify impression fatigue directly lowers score of seen post vs unseen twin
    let p_unseen = PostCandidate::from_post(clone_post_as(&base, "fat_a", "u:tw1", ActorKind::Human, "rust", "Identical topic text", 50, 0, t0));
    let p_seen = PostCandidate::from_post(clone_post_as(&base, "fat_b", "u:tw2", ActorKind::Human, "rust", "Identical topic text", 50, 0, t0));
    viewer.seen_impressions.insert("fat_b".into(), 6);
    let fat_resp = CommunityRecommender::rank_feed(
        &viewer,
        vec![p_seen, p_unseen],
        &FeedRequestDto { tab: FeedTab::ForYou, sort: FeedSort::Algo, limit: 5, now_ms: Some(t0), ..Default::default() },
    );
    assert_eq!(fat_resp.items[0].post.post_id, "fat_a", "unseen post must outrank heavily fatigued seen post");
    assert!(fat_resp.items[0].rank_score > fat_resp.items[1].rank_score);
}

/// [Test ID] TEST-CM-T2-F06-05
/// [Feature / Req] F06 / TEST_INFRA.md §6 (Recommender determinism across repeated runs & explanation % sum == 100%)
/// [Preconditions] Fixed timestamp starter dataset + `ViewerContext` with `show_explanations == true`.
/// [Opaque-Box Steps] Execute `CommunityRecommender::rank_feed` 5 times on identical inputs and inspect serialized output and `FeedExplanationDto`.
/// [Observable Assertions] All 5 runs are byte-identical; every item includes `FeedExplanationDto` whose `signal_breakdown` sums to `100.0%` (±0.5%).
#[test]
fn test_cm_t2_f06_05_recommender_determinism_and_explanation_percentage_sum() {
    let t0 = 1_760_000_000_000_i64;
    let ds = build_starter_dataset(t0);
    let cands: Vec<PostCandidate> = ds.posts.iter().cloned().map(PostCandidate::from_post).collect();
    let mut viewer = ViewerContext { actor_id: "u:900001".into(), now_ms: t0, ..Default::default() };
    viewer.algo_prefs = default_prefs_for_preset(AlgoPreset::Balanced);
    viewer.algo_prefs.show_explanations = true;
    let req = FeedRequestDto { tab: FeedTab::ForYou, sort: FeedSort::Algo, limit: 15, now_ms: Some(t0), ..Default::default() };

    let baseline = CommunityRecommender::rank_feed(&viewer, cands.clone(), &req);
    assert!(!baseline.items.is_empty());
    let baseline_json = serde_json::to_string(&baseline).unwrap();
    for _ in 0..4 {
        let again = CommunityRecommender::rank_feed(&viewer, cands.clone(), &req);
        assert_eq!(serde_json::to_string(&again).unwrap(), baseline_json, "recommender must be strictly deterministic");
    }
    for item in &baseline.items {
        let exp = item.explanation.as_ref().expect("show_explanations=true must attach explanation");
        assert!(!exp.primary_reason.is_empty());
        let sum: f64 = exp.signal_breakdown.iter().map(|s| s.contribution_pct).sum();
        assert!((sum - 100.0).abs() <= 0.5, "signal_breakdown must sum to ~100%, got {sum}");
    }
}

// ============================================================================
// F07 — Trending & Entity Extraction Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F07-01
/// [Feature / Req] F07 / TEST_INFRA.md §6 (Malformed edge entities, email non-mentions & trailing URL punctuation)
/// [Preconditions] Pure-Rust `extract_entities`.
/// [Opaque-Box Steps] Feed bare `#`, `@`, `c/`, 1-char `@a`/`c/x`, underscore-only `@___`/`#___`, email addresses, and punctuation-wrapped URLs.
/// [Observable Assertions] Emails and bare/too-short/underscore-only tokens produce zero bogus mentions/hashtags/communities; URLs strip trailing punctuation cleanly.
#[test]
fn test_cm_t2_f07_01_malformed_edge_entities_and_email_url_punctuation() {
    let raw = "# alone @ alone c/ alone @a c/x @___ #___ email@example.com alice.bob+tag@sub.domain.org (https://rust-lang.org/docs/guide.), [https://example.org/a?x=1&y=2];";
    let ent = extract_entities(raw);
    assert!(ent.mentions.is_empty(), "emails & invalid handles must not extract mentions: {:?}", ent.mentions);
    assert!(ent.hashtags.is_empty(), "bare/underscore hashtags must not extract: {:?}", ent.hashtags);
    assert!(ent.communities.is_empty(), "bare/1-char community slugs must not extract: {:?}", ent.communities);
    assert_eq!(
        ent.urls,
        vec![
            "https://rust-lang.org/docs/guide".to_string(),
            "https://example.org/a?x=1&y=2".to_string()
        ]
    );
}

/// [Test ID] TEST-CM-T2-F07-02
/// [Feature / Req] F07 / TEST_INFRA.md §6 (Unicode CJK hashtags & emoji-adjacent mentions without UTF-8 boundary panic)
/// [Preconditions] Pure-Rust `extract_entities`.
/// [Opaque-Box Steps] Parse dense mixed emoji, CJK hashtags, mentions, `c/<slug>`, and CJK-bracketed URLs.
/// [Observable Assertions] Extracts exact lowercase ASCII & CJK entities without panicking on multi-byte UTF-8 char boundaries.
#[test]
fn test_cm_t2_f07_02_unicode_cjk_hashtags_and_emoji_adjacent_mentions_utf8_safe() {
    let text = "🔥#开源社区 #Rust2026 🚀@Ferris_Bot🎉和@alex_chen！请访问 c/Rust（https://example.org/docs）";
    let ent = extract_entities(text);
    assert_eq!(ent.hashtags, vec!["开源社区".to_string(), "rust2026".to_string()]);
    assert_eq!(ent.mentions, vec!["ferris_bot".to_string(), "alex_chen".to_string()]);
    assert_eq!(ent.communities, vec!["rust".to_string()]);
    assert_eq!(ent.urls, vec!["https://example.org/docs".to_string()]);
}

/// [Test ID] TEST-CM-T2-F07-03
/// [Feature / Req] F07 / TEST_INFRA.md §6 (Deleted, removed, Draft, and Scheduled posts strictly excluded from Trending)
/// [Preconditions] Pure-Rust `TrendingEngine::compute` with 5 posts bearing distinct hashtags across statuses.
/// [Opaque-Box Steps] Compute trending topics across 1 live published post and 4 non-public/removed posts.
/// [Observable Assertions] Only `#alive_tag` appears in trending output; `#deleted_tag`, `#removed_tag`, `#draft_tag`, and `#scheduled_tag` are absent.
#[test]
fn test_cm_t2_f07_03_deleted_removed_and_unpublished_posts_excluded_from_trending() {
    let t0 = 1_760_000_000_000_i64;
    let base = build_starter_dataset(t0).posts[0].clone();

    let alive = clone_post_as(&base, "tr_alive", "u:1", ActorKind::Human, "rust", "Live post #alive_tag", 20, 0, t0 - 60_000);
    let mut deleted = clone_post_as(&base, "tr_del", "u:2", ActorKind::Human, "rust", "Gone #deleted_tag", 500, 0, t0 - 60_000);
    deleted.status = PostStatus::Deleted;
    let mut removed = clone_post_as(&base, "tr_rem", "u:3", ActorKind::Human, "rust", "Removed #removed_tag", 500, 0, t0 - 60_000);
    removed.is_removed = true;
    let mut draft = clone_post_as(&base, "tr_drf", "u:4", ActorKind::Human, "rust", "Draft #draft_tag", 500, 0, t0 - 60_000);
    draft.status = PostStatus::Draft;
    let mut sched = clone_post_as(&base, "tr_sch", "u:5", ActorKind::Human, "rust", "Future #scheduled_tag", 500, 0, t0 - 60_000);
    sched.status = PostStatus::Scheduled;

    let topics = TrendingEngine::compute(&[alive, deleted, removed, draft, sched], t0, 20);
    let tags: HashSet<String> = topics.into_iter().map(|t| t.slug_or_tag).collect();
    assert!(tags.contains("alive_tag"), "live published hashtag must trend: {tags:?}");
    for forbidden in ["deleted_tag", "removed_tag", "draft_tag", "scheduled_tag"] {
        assert!(!tags.contains(forbidden), "non-public/removed tag '{forbidden}' leaked into trending: {tags:?}");
    }
}

/// [Test ID] TEST-CM-T2-F07-04
/// [Feature / Req] F07 / TEST_INFRA.md §6 (Trending `limit == 0`, empty corpus, and stale/future window boundaries)
/// [Preconditions] Pure-Rust `TrendingEngine::compute`.
/// [Opaque-Box Steps] Call `TrendingEngine::compute` with empty posts, `limit == 0`, >24h-old posts, and far-future timestamps.
/// [Observable Assertions] Every boundary returns an empty vector cleanly with zero division-by-zero or panic.
#[test]
fn test_cm_t2_f07_04_trending_limit_zero_and_empty_or_stale_window_no_div_zero() {
    let t0 = 1_760_000_000_000_i64;
    let base = build_starter_dataset(t0).posts[0].clone();
    let fresh = clone_post_as(&base, "tr_f", "u:1", ActorKind::Human, "rust", "Fresh #fresh_tag", 10, 0, t0 - 1_000);
    let stale = clone_post_as(&base, "tr_old", "u:2", ActorKind::Human, "rust", "Stale #old_tag", 100, 0, t0 - 90_000_000);
    let future = clone_post_as(&base, "tr_fut", "u:3", ActorKind::Human, "rust", "Future #fut_tag", 100, 0, t0 + 300_000);

    assert!(TrendingEngine::compute(&[], t0, 10).is_empty());
    assert!(TrendingEngine::compute(&[fresh], t0, 0).is_empty());
    assert!(TrendingEngine::compute(&[stale, future], t0, 10).is_empty());
}

/// [Test ID] TEST-CM-T2-F07-05
/// [Feature / Req] F07 / TEST_INFRA.md §6 (Single-author spam burst dampened vs. organic multi-author hashtag trend)
/// [Preconditions] Pure-Rust `TrendingEngine::compute` with 10 single-author `#spam_solo` posts and 10 distinct-author `#organic_multi` posts.
/// [Opaque-Box Steps] Compute trending topics for both equal-volume bursts in the same 4h recency bin.
/// [Observable Assertions] `#organic_multi` achieves a strictly higher `velocity_score` and ranks strictly above `#spam_solo`.
#[test]
fn test_cm_t2_f07_05_single_author_spam_burst_dampened_vs_organic_multi_author_trend() {
    let t0 = 1_760_000_000_000_i64;
    let base = build_starter_dataset(t0).posts[0].clone();
    let mut posts = Vec::new();
    for i in 0..10 {
        posts.push(clone_post_as(&base, &format!("spam_{i}"), "u:spammer_one", ActorKind::Human, "rust", &format!("Spam burst {i} #spam_solo"), 2, 0, t0 - (i as i64) * 5_000));
        posts.push(clone_post_as(&base, &format!("org_{i}"), &format!("u:organic_{i}"), ActorKind::Human, "rust", &format!("Organic post {i} #organic_multi"), 2, 0, t0 - (i as i64) * 5_000));
    }

    let topics = TrendingEngine::compute(&posts, t0, 20);
    let org_pos = topics.iter().position(|t| t.slug_or_tag == "organic_multi").expect("organic_multi missing");
    let spam_pos = topics.iter().position(|t| t.slug_or_tag == "spam_solo").expect("spam_solo missing");
    assert!(org_pos < spam_pos, "multi-author trend must outrank single-author spam burst: {org_pos} vs {spam_pos}");
    assert!(topics[org_pos].velocity_score > topics[spam_pos].velocity_score);
}

// ============================================================================
// F08 — Moderation & AutoMod Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F08-01
/// [Feature / Req] F08 / TEST_INFRA.md §6 (Malformed AutoMod regex rejected by validator & never crashes evaluator/server)
/// [Preconditions] Pure-Rust `validate_automod_regex_syntax` & `AutoModEngine` + Ephemeral live server.
/// [Opaque-Box Steps] Reject malformed regex syntax via validator, evaluate malformed regex alongside a valid keyword rule in `AutoModEngine`, and submit clean & flagged posts to a live community with that ruleset.
/// [Observable Assertions] Validator rejects all malformed patterns; `AutoModEngine` and live server never panic/500 and still enforce valid sibling rules.
#[tokio::test]
async fn test_cm_t2_f08_01_invalid_automod_regex_rejected_and_evaluator_never_crashes() {
    for bad_pat in ["[unclosed", "(abc", "*dangling", "a{5,2}", "trailing\\"] {
        assert!(validate_automod_regex_syntax(bad_pat).is_err(), "validator must reject '{bad_pat}'");
    }

    let actor = build_starter_dataset(now_ms()).actors[0].clone();
    let mut bad_rule = make_automod_rule("r_bad_re", AutoModActionKind::Remove);
    bad_rule.regex_pattern = Some("[unclosed(".into());
    let mut valid_kw = make_automod_rule("r_valid_kw", AutoModActionKind::FlagForReview);
    valid_kw.keywords = vec!["banned_phrase_f08".into()];

    let clean_dec = AutoModEngine::evaluate(&[bad_rule.clone(), valid_kw.clone()], &actor, Some("Clean"), "All good");
    assert_eq!(clean_dec.action, AutoModActionKind::Approve);
    let flagged_dec = AutoModEngine::evaluate(&[bad_rule.clone(), valid_kw.clone()], &actor, Some("Test"), "Has banned_phrase_f08 inside");
    assert_eq!(flagged_dec.action, AutoModActionKind::FlagForReview);

    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f08_01", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());
    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/communities", &auth, Some(json!({
            "slug": "am_regex_f08",
            "name": "AutoMod Regex Guard",
            "automod_rules": [bad_rule, valid_kw]
        }))).await.unwrap().0,
        201
    );
    let (st_ok, p_ok) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "community_slug": "am_regex_f08", "body": "Safe discussion" }))).await.unwrap();
    assert_eq!(st_ok, 201);
    assert_eq!(p_ok["automod_action"], "approve");

    let (st_fl, p_fl) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "community_slug": "am_regex_f08", "body": "Trigger banned_phrase_f08 now" }))).await.unwrap();
    assert_eq!(st_fl, 201);
    assert_eq!(p_fl["automod_action"], "flag_for_review");
}

/// [Test ID] TEST-CM-T2-F08-02
/// [Feature / Req] F08 / TEST_INFRA.md §6 (AutoMod karma floor exact BVA: `floor - 1`, `floor`, `floor + 1`)
/// [Preconditions] Pure-Rust `AutoModEngine` with `min_author_karma = Some(50)` (`FlagForReview`).
/// [Opaque-Box Steps] Evaluate actors with total karma `49`, `50`, `51`, `-10`, and split `(post=25, comment=25)`.
/// [Observable Assertions] `49` and `-10` trigger `FlagForReview`; `50`, `51`, and split `25+25` pass with `Approve`.
#[test]
fn test_cm_t2_f08_02_automod_karma_floor_exact_bva_minus_one_eq_plus_one() {
    let base = build_starter_dataset(now_ms()).actors[0].clone();
    let mut rule = make_automod_rule("r_karma_50", AutoModActionKind::FlagForReview);
    rule.min_author_karma = Some(50);

    for (post_k, cmt_k, expected) in [
        (-10, 0, AutoModActionKind::FlagForReview),
        (49, 0, AutoModActionKind::FlagForReview),
        (50, 0, AutoModActionKind::Approve),
        (51, 0, AutoModActionKind::Approve),
        (25, 25, AutoModActionKind::Approve),
    ] {
        let mut a = base.clone();
        a.post_karma = post_k;
        a.comment_karma = cmt_k;
        let dec = AutoModEngine::evaluate(std::slice::from_ref(&rule), &a, None, "Hello world");
        assert_eq!(dec.action, expected, "karma ({post_k}+{cmt_k}) expected {expected:?}, got {:?}", dec.action);
    }
}

/// [Test ID] TEST-CM-T2-F08-03
/// [Feature / Req] F08 / TEST_INFRA.md §6 (Unprivileged mod-queue & banning owner rejected with 403)
/// [Preconditions] Ephemeral live server with Owner Alice in `c/priv_f08`, regular member Bob, and promoted mod Charlie.
/// [Opaque-Box Steps] Bob accesses `/mod-queue` and runs `mod-actions`; Charlie (`mod`) attempts banning Owner Alice and promoting Bob.
/// [Observable Assertions] All 4 privilege-escalation attempts return 403 Forbidden.
#[tokio::test]
async fn test_cm_t2_f08_03_unprivileged_mod_queue_and_ban_owner_forbidden() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f08_03", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f08_03", "Pass1234!").await.unwrap();
    let charlie = driver.register_human("charlie_t2_f08_03", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let c_auth = CommunityAuthMode::Human(charlie.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "priv_f08", "name": "Priv F08" }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/priv_f08/join", &b_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/priv_f08/join", &c_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/priv_f08/mod-actions", &a_auth, Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": charlie.actor_id }))).await.unwrap().0, 200);

    assert_eq!(driver.req(Method::GET, "/api/v1/community/communities/priv_f08/mod-queue", &b_auth, None).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/priv_f08/mod-actions", &b_auth, Some(json!({ "action": "ban_actor", "target_kind": "actor", "target_id": charlie.actor_id }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/priv_f08/mod-actions", &c_auth, Some(json!({ "action": "ban_actor", "target_kind": "actor", "target_id": alice.actor_id }))).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/priv_f08/mod-actions", &c_auth, Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": bob.actor_id }))).await.unwrap().0, 403);
}

/// [Test ID] TEST-CM-T2-F08-04
/// [Feature / Req] F08 / TEST_INFRA.md §6 (Duplicate report, empty mute target, self-block & nonexistent mute idempotency)
/// [Preconditions] Ephemeral live server with Alice's post and onboarded Bob.
/// [Opaque-Box Steps] Submit duplicate reports, empty mute `target_value`, duplicate mutes on nonexistent actor, and self-block followed by removal.
/// [Observable Assertions] Empty mute returns 400; duplicate reports and duplicate mutes handle cleanly with idempotent storage; self-block never crashes feed.
#[tokio::test]
async fn test_cm_t2_f08_04_duplicate_report_self_block_and_mute_idempotency() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f08_04", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f08_04", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "community_slug": "rust", "body": "Reportable post" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    for _ in 0..2 {
        let (st, rv) = driver.req(Method::POST, "/api/v1/community/signals", &b_auth, Some(json!({
            "post_id": pid, "kind": "report", "reason": "Spam check", "community_slug": "rust"
        }))).await.unwrap();
        assert_eq!(st, 200);
        assert_eq!(rv["recorded"], true);
    }

    assert_eq!(driver.req(Method::POST, "/api/v1/community/mutes-blocks", &b_auth, Some(json!({
        "target_kind": "actor", "target_value": "   ", "action": "mute"
    }))).await.unwrap().0, 400);

    for _ in 0..2 {
        assert_eq!(driver.req(Method::POST, "/api/v1/community/mutes-blocks", &b_auth, Some(json!({
            "target_kind": "actor", "target_value": "u:999888777", "action": "mute"
        }))).await.unwrap().0, 200);
    }
    let (_, list1) = driver.req(Method::GET, "/api/v1/community/mutes-blocks", &b_auth, None).await.unwrap();
    assert_eq!(list1["items"].as_array().unwrap().len(), 1, "repeated mute must be idempotent");

    assert_eq!(driver.req(Method::POST, "/api/v1/community/mutes-blocks", &b_auth, Some(json!({
        "target_kind": "actor", "target_value": bob.actor_id, "action": "block"
    }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::GET, "/api/v1/community/feed?limit=5", &b_auth, None).await.unwrap().0, 200);
    let (st_del, dv) = driver.req(Method::DELETE, "/api/v1/community/mutes-blocks", &b_auth, Some(json!({
        "target_kind": "actor", "target_value": bob.actor_id, "action": "block"
    }))).await.unwrap();
    assert_eq!(st_del, 200);
    assert_eq!(dv["deleted"], true);
}

/// [Test ID] TEST-CM-T2-F08-05
/// [Feature / Req] F08 / TEST_INFRA.md §6 (`bot_mod` can moderate regular members while non-mod Bot receives 403)
/// [Preconditions] Ephemeral live server with Owner Alice in `c/botmod_f08`, promoted `mod_t2_f08_bot` (`bot_mod`), regular `reg_t2_f08_bot`, and member Bob.
/// [Opaque-Box Steps] Regular Bot attempts queue/lock; `bot_mod` inspects queue, locks Bob's post, bans Bob, and attempts banning Owner Alice.
/// [Observable Assertions] Regular Bot gets 403; `bot_mod` succeeds (200) on queue, lock, and banning regular member Bob, while banning Owner Alice returns 403.
#[tokio::test]
async fn test_cm_t2_f08_05_bot_mod_can_moderate_members_while_regular_bot_gets_403() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f08_05", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f08_05", "Pass1234!").await.unwrap();
    let mod_bot = driver.register_community_bot(&alice.token, "mod_t2_f08_bot", "Mod Bot", "manual").await.unwrap();
    let reg_bot = driver.register_community_bot(&alice.token, "reg_t2_f08_bot", "Reg Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());
    let mb_auth = CommunityAuthMode::DirectBot(mod_bot.bot_token.clone());
    let rb_auth = CommunityAuthMode::DirectBot(reg_bot.bot_token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "botmod_f08", "name": "BotMod Lab", "allow_bots": true }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/botmod_f08/join", &b_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/botmod_f08/join", &rb_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/botmod_f08/mod-actions", &a_auth, Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": mod_bot.actor_id }))).await.unwrap().0, 200);

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({ "community_slug": "botmod_f08", "body": "Bob heated thread" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    assert_eq!(driver.req(Method::GET, "/api/v1/community/communities/botmod_f08/mod-queue", &rb_auth, None).await.unwrap().0, 403);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/botmod_f08/mod-actions", &rb_auth, Some(json!({ "action": "lock", "target_kind": "post", "target_id": pid }))).await.unwrap().0, 403);

    assert_eq!(driver.req(Method::GET, "/api/v1/community/communities/botmod_f08/mod-queue", &mb_auth, None).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/botmod_f08/mod-actions", &mb_auth, Some(json!({ "action": "lock", "target_kind": "post", "target_id": pid }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/botmod_f08/mod-actions", &mb_auth, Some(json!({ "action": "ban_actor", "target_kind": "actor", "target_id": bob.actor_id }))).await.unwrap().0, 200);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/botmod_f08/mod-actions", &mb_auth, Some(json!({ "action": "ban_actor", "target_kind": "actor", "target_id": alice.actor_id }))).await.unwrap().0, 403);
}

// ============================================================================
// F09 — Bot Runtime & Seeder Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F09-01
/// [Feature / Req] F09 / TEST_INFRA.md §6 (Anti-loop hard ceiling: hop 5 allowed, hop 6 strictly blocked)
/// [Preconditions] Pure-Rust `BotLoopGuard::new(5, 0)` + Ephemeral live server with 2 Manual Bots.
/// [Opaque-Box Steps] Acquire alternating bot turns 1..=6 on `BotLoopGuard` and verify human reset; then post 5 alternating Bot API replies on a live post and attempt a 6th consecutive Bot reply.
/// [Observable Assertions] Hops 1..=5 succeed; hop 6 returns `BotGuardRejection::LoopBudgetExceeded` and HTTP 400 `bot_loop_budget_exceeded` over Bot API; self-trigger returns `SelfTriggerBlocked`.
#[tokio::test]
async fn test_cm_t2_f09_01_anti_loop_hard_ceiling_hop_5_allowed_hop_6_blocked() {
    let guard = BotLoopGuard::new(5, 0);
    assert_eq!(
        guard.try_acquire_turn("t1", "b:1", "b:1", ActorKind::Bot, false, 1_000),
        Err(BotGuardRejection::SelfTriggerBlocked)
    );
    for hop in 1..=5u32 {
        let (bot, trig) = if hop % 2 == 1 { ("b:1", "b:2") } else { ("b:2", "b:1") };
        assert_eq!(guard.try_acquire_turn("t1", bot, trig, ActorKind::Bot, false, 2_000 + i64::from(hop)), Ok(hop));
    }
    assert_eq!(
        guard.try_acquire_turn("t1", "b:2", "b:1", ActorKind::Bot, false, 3_000),
        Err(BotGuardRejection::LoopBudgetExceeded { hops: 5, max_hops: 5 })
    );
    guard.reset_on_human_turn("t1");
    assert_eq!(guard.current_hops("t1"), 0);
    assert_eq!(guard.try_acquire_turn("t1", "b:1", "b:2", ActorKind::Bot, false, 4_000), Ok(1));

    // Live server 5-hop enforcement over Bot API
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f09_01", "Pass1234!").await.unwrap();
    let b1 = driver.register_community_bot(&alice.token, "hop1_t2_f09_bot", "Hop1 Bot", "manual").await.unwrap();
    let b2 = driver.register_community_bot(&alice.token, "hop2_t2_f09_bot", "Hop2 Bot", "manual").await.unwrap();

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &CommunityAuthMode::Human(alice.token), Some(json!({ "body": "Anti-loop thread" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let mut parent: Option<String> = None;
    for hop in 1..=5 {
        let tok = if hop % 2 == 1 { &b1.bot_token } else { &b2.bot_token };
        let mut params = json!({ "post_id": pid, "body": format!("Bot hop {hop}") });
        if let Some(ref p) = parent {
            params["parent_comment_id"] = json!(p);
        }
        let (st, resp) = driver.bot_api(tok, "community.createComment", true, Some(params)).await.unwrap();
        assert_eq!(st, 200, "hop {hop} must succeed: {resp}");
        assert_eq!(resp["ok"], true);
        parent = Some(resp["result"]["comment"]["comment_id"].as_str().unwrap().to_string());
    }

    let (st6, resp6) = driver
        .bot_api(&b2.bot_token, "community.createComment", true, Some(json!({
            "post_id": pid, "parent_comment_id": parent, "body": "Bot hop 6 must be blocked"
        })))
        .await
        .unwrap();
    assert_eq!(st6, 400, "hop 6 must be rejected with 400: {resp6}");
    assert_eq!(resp6["ok"], false);
    assert!(resp6["description"].as_str().unwrap_or("").contains("bot_loop_budget_exceeded"));
}

/// [Test ID] TEST-CM-T2-F09-02
/// [Feature / Req] F09 / TEST_INFRA.md §6 (Cooldown guard suppresses inside window and resumes at exact boundary)
/// [Preconditions] `BotLoopGuard::new(5, 30_000)` + in-memory `CommunityStore`.
/// [Opaque-Box Steps] Acquire a bot turn at `t0`, attempt again at `t0 + 10_000`, `t0 + 29_999`, and `t0 + 30_000`; repeat with `CommunityStore::check_and_record_bot_cooldown`.
/// [Observable Assertions] Calls strictly `< 30_000ms` fail with exact `remaining_ms` (`false` in store); call at `t0 + 30_000` succeeds.
#[test]
fn test_cm_t2_f09_02_cooldown_guard_suppresses_inside_window_and_resumes_after() {
    let t0 = 1_760_000_000_000_i64;
    let guard = BotLoopGuard::new(5, 30_000);
    assert_eq!(guard.try_acquire_turn("post_cd", "b:1", "u:1", ActorKind::Human, false, t0), Ok(0));
    assert_eq!(
        guard.try_acquire_turn("post_cd", "b:1", "u:1", ActorKind::Human, false, t0 + 10_000),
        Err(BotGuardRejection::CooldownActive { remaining_ms: 20_000 })
    );
    assert_eq!(
        guard.try_acquire_turn("post_cd", "b:1", "u:1", ActorKind::Human, false, t0 + 29_999),
        Err(BotGuardRejection::CooldownActive { remaining_ms: 1 })
    );
    assert_eq!(guard.try_acquire_turn("post_cd", "b:1", "u:1", ActorKind::Human, false, t0 + 30_000), Ok(0));

    let store = CommunityStore::open_in_memory().unwrap();
    store.seed_if_empty(t0).unwrap();
    assert!(store.check_and_record_bot_cooldown("b:910001", "thread:cd", 30_000, 10, t0).unwrap());
    assert!(!store.check_and_record_bot_cooldown("b:910001", "thread:cd", 30_000, 10, t0 + 15_000).unwrap());
    assert!(store.check_and_record_bot_cooldown("b:910001", "thread:cd", 30_000, 10, t0 + 30_000).unwrap());
}

/// [Test ID] TEST-CM-T2-F09-03
/// [Feature / Req] F09 / TEST_INFRA.md §6 (`Manual` & `MentionOnly` autonomy silence guards & low-signal chatter filter)
/// [Preconditions] 3 bots (`Manual`, `MentionOnly`, `Reactive`) with Rust expertise + 1 Human trigger actor.
/// [Opaque-Box Steps] Invoke `BotWakeSelector::select_bots_for_event` on unmentioned Rust post, explicit `@manual_bot @mention_bot` mention, and low-signal `"ok"` chatter.
/// [Observable Assertions] `Manual` never wakes even on `@mention`; `MentionOnly` wakes strictly on explicit `@mention`; low-signal chatter triggers zero proactive wakes.
#[test]
fn test_cm_t2_f09_03_manual_and_mention_only_autonomy_silence_guards() {
    let t0 = 1_760_000_000_000_i64;
    let ds = build_starter_dataset(t0);
    let human = ds.actors.iter().find(|a| a.actor_kind == ActorKind::Human).unwrap().clone();
    let base_bot = ds.actors.iter().find(|a| a.actor_kind == ActorKind::Bot).unwrap().clone();
    let post = ds.posts[0].clone();

    let mk_bot = |id: &str, handle: &str, mode: BotAutonomyMode| -> CommunityActorDto {
        let mut b = base_bot.clone();
        b.actor_id = id.into();
        b.handle = handle.into();
        b.bot_autonomy_mode = mode;
        b.bot_capabilities = vec!["rust".into(), "systems".into()];
        b.bot_persona = Some("Rust async systems specialist".into());
        b
    };
    let bots = vec![
        mk_bot("b:701", "manual_t2_bot", BotAutonomyMode::Manual),
        mk_bot("b:702", "mention_t2_bot", BotAutonomyMode::MentionOnly),
        mk_bot("b:703", "reactive_t2_bot", BotAutonomyMode::Reactive),
    ];

    let unmentioned = BotWakeSelector::select_bots_for_event(&bots, &post, "Deep dive into rust systems lock contention", &human, None, 3);
    assert!(!unmentioned.iter().any(|b| b.handle == "manual_t2_bot"));
    assert!(!unmentioned.iter().any(|b| b.handle == "mention_t2_bot"));
    assert!(unmentioned.iter().any(|b| b.handle == "reactive_t2_bot"));

    let mentioned = BotWakeSelector::select_bots_for_event(&bots, &post, "Asking @manual_t2_bot and @mention_t2_bot about rust", &human, None, 3);
    assert!(!mentioned.iter().any(|b| b.handle == "manual_t2_bot"), "Manual bot must remain strictly silent even when @mentioned");
    assert!(mentioned.iter().any(|b| b.handle == "mention_t2_bot"), "MentionOnly bot must wake on explicit @mention");

    let chatter = BotWakeSelector::select_bots_for_event(&bots, &post, "ok", &human, None, 3);
    assert!(chatter.is_empty(), "low-signal chatter must not wake proactive bots");
}

/// [Test ID] TEST-CM-T2-F09-04
/// [Feature / Req] F09 / TEST_INFRA.md §6 (Banned bot & `allow_bots == false` community produce zero bot replies)
/// [Preconditions] Pure-Rust `BotWakeSelector` + Ephemeral live server with `c/nobots_f09` (`allow_bots: false`) and `c/banbot_f09` (bot banned).
/// [Opaque-Box Steps] Verify `BotWakeSelector` returns empty when `allow_bots == false`; summon reactive bot in `c/nobots_f09` and in `c/banbot_f09` where the bot is banned.
/// [Observable Assertions] Selector yields empty; both live summons return 403 Forbidden and zero unauthorized bot comments exist.
#[tokio::test]
async fn test_cm_t2_f09_04_banned_bot_and_allow_bots_false_produce_no_reply() {
    let ds = build_starter_dataset(now_ms());
    let human = ds.actors.iter().find(|a| a.actor_kind == ActorKind::Human).unwrap().clone();
    let bot_dto = ds.actors.iter().find(|a| a.actor_kind == ActorKind::Bot).unwrap().clone();
    let mut no_bot_comm: SubCommunityDto = ds.communities[0].clone();
    no_bot_comm.allow_bots = false;
    assert!(
        BotWakeSelector::select_bots_for_event(&[bot_dto.clone()], &ds.posts[0], &format!("Hey @{} rust systems?", bot_dto.handle), &human, Some(&no_bot_comm), 3).is_empty(),
        "BotWakeSelector must return empty when allow_bots == false"
    );

    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f09_04", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "watch_t2_f09_bot", "Watch Bot", "reactive").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &auth, Some(json!({ "slug": "nobots_f09", "name": "NoBots", "allow_bots": false }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &auth, Some(json!({ "slug": "banbot_f09", "name": "BanBot", "allow_bots": true }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/banbot_f09/mod-actions", &auth, Some(json!({ "action": "ban_actor", "target_kind": "actor", "target_id": bot.actor_id }))).await.unwrap().0, 200);

    let (_, p_no) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "community_slug": "nobots_f09", "body": "Testing @watch_t2_f09_bot" }))).await.unwrap();
    let pid_no = p_no["post"]["post_id"].as_str().unwrap();
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid_no}/summon-bot"), &auth, Some(json!({ "bot_actor_id": bot.actor_id }))).await.unwrap().0, 403);

    let (_, p_ban) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "community_slug": "banbot_f09", "body": "Testing banned @watch_t2_f09_bot" }))).await.unwrap();
    let pid_ban = p_ban["post"]["post_id"].as_str().unwrap();
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid_ban}/summon-bot"), &auth, Some(json!({ "bot_actor_id": bot.actor_id }))).await.unwrap().0, 403);
}

/// [Test ID] TEST-CM-T2-F09-05
/// [Feature / Req] F09 / TEST_INFRA.md §6 (`DeterministicBotResponder` handles empty, 10KB & CJK/emoji prompts safely)
/// [Preconditions] Seeded Bot (`ferris_bot`) and Human (`alex_chen`) actors.
/// [Opaque-Box Steps] Call `DeterministicBotResponder::generate_reply` with Human actor, all-empty prompt, 10 KiB body, and pure CJK+emoji trigger.
/// [Observable Assertions] Human actor & empty prompt return `None`; 10 KiB and CJK+emoji prompts return non-empty UTF-8 replies without panic.
#[test]
fn test_cm_t2_f09_05_deterministic_bot_responder_empty_10kb_and_cjk_emoji_safe() {
    let ds = build_starter_dataset(now_ms());
    let human = ds.actors.iter().find(|a| a.actor_kind == ActorKind::Human).unwrap().clone();
    let bot = ds.actors.iter().find(|a| a.actor_kind == ActorKind::Bot).unwrap().clone();
    let mut empty_post = ds.posts[0].clone();
    empty_post.title = None;
    empty_post.body = String::new();

    assert!(DeterministicBotResponder::generate_reply(&human, &ds.posts[0], &[], "Hello?").is_none());
    assert!(DeterministicBotResponder::generate_reply(&bot, &empty_post, &[], "   ").is_none());

    let mut big_post = ds.posts[0].clone();
    big_post.body = "Rust异步系统边界压测🚀 ".repeat(400);
    let r_big = DeterministicBotResponder::generate_reply(&bot, &big_post, &[], "How does lock contention behave?").expect("10KB reply");
    assert!(!r_big.trim().is_empty());

    let r_cjk = DeterministicBotResponder::generate_reply(&bot, &empty_post, &[], "🔥🎉 社区推荐算法和内存布局怎么权衡？🚀").expect("CJK+emoji reply");
    assert!(!r_cjk.trim().is_empty());
}

// ============================================================================
// F10 — SQLite V42 & Tantivy Search Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F10-01
/// [Feature / Req] F10 / TEST_INFRA.md §6 (Concurrent 20-thread SQLite WAL voting: zero `SQLITE_BUSY`, exact total)
/// [Preconditions] Disk-backed SQLite WAL `CommunityStore` with 1 author, 20 distinct voters, and 1 post (initial score `1`).
/// [Opaque-Box Steps] Open 20 independent WAL connections, synchronize on a 20-thread `Barrier`, and execute 15 upvotes (+1) and 5 downvotes (-1) simultaneously.
/// [Observable Assertions] All 20 concurrent transactions succeed with zero `SQLITE_BUSY`; final `upvotes == 16`, `downvotes == 5`, `score == 11`, and author `post_karma == 10`.
#[test]
fn test_cm_t2_f10_01_concurrent_20_thread_sqlite_voting_zero_busy_exact_total() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("concurrent_f10_01.db");
    let store = CommunityStore::open(&db_path).unwrap();
    let t0 = now_ms();

    let author = store
        .create_human_actor(100, &RegisterHumanActorRequest {
            handle: "author_f10_01".into(), display_name: "Author".into(), bio: None, location: None, website: None,
            avatar_media_id: None, banner_media_id: None, avatar_shape: None, avatar_color: None,
        }, t0)
        .unwrap();
    let mut voter_ids = Vec::with_capacity(20);
    for i in 0..20i64 {
        let v = store
            .create_human_actor(200 + i, &RegisterHumanActorRequest {
                handle: format!("voter_f10_{i}"), display_name: format!("Voter {i}"), bio: None, location: None, website: None,
                avatar_media_id: None, banner_media_id: None, avatar_shape: None, avatar_color: None,
            }, t0)
            .unwrap();
        voter_ids.push(v.actor_id);
    }
    let post = store
        .create_post(&author.actor_id, &CreatePostRequest { body: "Concurrent WAL target".into(), ..Default::default() }, t0)
        .unwrap()
        .post;

    let conns: Vec<CommunityStore> = (0..20).map(|_| CommunityStore::open(&db_path).unwrap()).collect();
    let barrier = Arc::new(Barrier::new(20));
    let mut handles = Vec::with_capacity(20);
    for (idx, (conn, vid)) in conns.into_iter().zip(voter_ids).enumerate() {
        let bar = Arc::clone(&barrier);
        let pid = post.post_id.clone();
        let dir_vote: i8 = if idx < 15 { 1 } else { -1 };
        handles.push(std::thread::spawn(move || {
            bar.wait();
            conn.vote_post(&vid, &pid, dir_vote, t0 + (idx as i64) + 1)
        }));
    }
    for h in handles {
        h.join().expect("thread join").expect("concurrent WAL vote must not hit SQLITE_BUSY");
    }

    let refreshed = store.get_post(&post.post_id, None).unwrap().unwrap();
    assert_eq!(refreshed.upvotes, 16);
    assert_eq!(refreshed.downvotes, 5);
    assert_eq!(refreshed.score, 11);
    let author_after = store.get_actor(&author.actor_id, None).unwrap().unwrap();
    assert_eq!(author_after.post_karma, 10);
}

/// [Test ID] TEST-CM-T2-F10-02
/// [Feature / Req] F10 / TEST_INFRA.md §6 (`CommunityStore` persistence across close and reopen on same TempDir)
/// [Preconditions] Isolated `TempDir` SQLite file `persist_f10_02.db`.
/// [Opaque-Box Steps] Create actors, community, wiki rev 2, post, 2-level comments, and upvote; drop store; reopen same path and read back all entities.
/// [Observable Assertions] Karma, wiki revision 2, post score, and nested comment tree survive close/reopen byte-exact.
#[test]
fn test_cm_t2_f10_02_store_persistence_across_close_and_reopen_same_tempdir() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("persist_f10_02.db");
    let t0 = now_ms();

    let (pid, c1_id, c2_id) = {
        let s1 = CommunityStore::open(&db_path).unwrap();
        let alice = s1.create_human_actor(301, &RegisterHumanActorRequest {
            handle: "alice_f10_02".into(), display_name: "Alice".into(), bio: None, location: None, website: None,
            avatar_media_id: None, banner_media_id: None, avatar_shape: None, avatar_color: None,
        }, t0).unwrap();
        let bot = s1.create_bot_actor(301, 401, &RegisterBotActorRequest {
            bot_user_id: Some(401), actor_kind: Some(ActorKind::Bot), handle: "persist_f10_bot".into(),
            display_name: "Persist Bot".into(), bio: None, avatar_media_id: None, banner_media_id: None,
            avatar_shape: None, avatar_color: None, bot_badge: None, bot_model: None, bot_capabilities: vec![],
            bot_autonomy_mode: BotAutonomyMode::Manual, bot_persona: None, cooldown_secs: None, max_posts_per_hour: None,
        }, t0).unwrap();
        s1.create_community(&alice.actor_id, &CreateCommunityRequest {
            slug: "persist_hub".into(), name: "Persist Hub".into(), description: "Durable".into(),
            category: "tech".into(), banner_media_id: None, icon_media_id: None, icon_emoji: None,
            accent_color: None, rules: vec![], post_flairs: vec![], user_flairs: vec![],
            wiki_markdown: "# Rev 1".into(), automod_rules: vec![], allow_bots: true,
            require_bot_disclosure: true, is_nsfw: false,
        }, t0).unwrap();
        s1.update_wiki("persist_hub", &alice.actor_id, "# Rev 2\nDurable guide", "Rev 2 edit", t0 + 10).unwrap();
        let post = s1.create_post(&alice.actor_id, &CreatePostRequest {
            community_slug: Some("persist_hub".into()), body: "Durable post".into(), ..Default::default()
        }, t0 + 20).unwrap().post;
        s1.vote_post(&bot.actor_id, &post.post_id, 1, t0 + 30).unwrap();
        let c1 = s1.create_comment(&bot.actor_id, &post.post_id, &CreateCommentRequest { body: "Bot root".into(), ..Default::default() }, t0 + 40).unwrap().comment;
        let c2 = s1.create_comment(&alice.actor_id, &post.post_id, &CreateCommentRequest { parent_comment_id: Some(c1.comment_id.clone()), body: "Human reply".into(), ..Default::default() }, t0 + 50).unwrap().comment;
        (post.post_id, c1.comment_id, c2.comment_id)
    };

    let s2 = CommunityStore::open(&db_path).unwrap();
    let comm = s2.get_community("persist_hub", Some("u:301")).unwrap().unwrap();
    assert_eq!(comm.wiki_revision, 2);
    assert_eq!(comm.wiki_markdown, "# Rev 2\nDurable guide");
    let post = s2.get_post(&pid, Some("b:401")).unwrap().unwrap();
    assert_eq!(post.score, 2);
    assert_eq!(s2.get_actor("u:301", None).unwrap().unwrap().post_karma, 1);
    let tree = s2.list_comment_tree(&pid, None, CommentSort::Old, None, 50).unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].comment_id, c1_id);
    assert_eq!(tree[0].children[0].comment_id, c2_id);
}

/// [Test ID] TEST-CM-T2-F10-03
/// [Feature / Req] F10 / TEST_INFRA.md §6 (Tantivy search handles empty, unbalanced parens, boolean ops & SQLi without panic/500)
/// [Preconditions] In-RAM `CommunitySearchIndexer` with starter docs + Ephemeral live server.
/// [Opaque-Box Steps] Run malformed queries directly against `CommunitySearchIndexer::search` and via REST `GET /api/v1/community/search`.
/// [Observable Assertions] Every malformed query returns `Ok(_)` locally and HTTP 200 over REST with zero panics or 500 errors.
#[tokio::test]
async fn test_cm_t2_f10_03_tantivy_search_malformed_queries_no_panic_or_500() {
    let idx = CommunitySearchIndexer::in_ram().unwrap();
    for p in &build_starter_dataset(now_ms()).posts {
        idx.index_post(p).unwrap();
    }
    idx.commit().unwrap();

    let nasty = [
        "",
        "   ",
        "(((",
        ")))",
        "AND OR NOT",
        "\"unclosed quote",
        "'; DROP TABLE community_posts; --",
        "c: author: has:invalid :::",
        "🦀🚀",
    ];
    for q in nasty {
        assert!(
            idx.search(&CommunitySearchRequestDto { query: q.into(), ..Default::default() }).is_ok(),
            "Tantivy search must never fail on malformed query '{q}'"
        );
    }

    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f10_03", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token);
    for raw_encoded in ["", "%28%28%28", "AND%20OR%20NOT", "%22unclosed", "%27%3B%20DROP%20TABLE"] {
        let (st, _) = driver.req(Method::GET, &format!("/api/v1/community/search?q={raw_encoded}"), &auth, None).await.unwrap();
        assert_eq!(st, 200, "malformed search query '{raw_encoded}' must return 200, not 500");
    }
}

/// [Test ID] TEST-CM-T2-F10-04
/// [Feature / Req] F10 / TEST_INFRA.md §6 (Disk `CommunitySearchIndexer` reopen on safe `community.idx` dir preserves documents)
/// [Preconditions] `TempDir` + `safe_temp_idx_dir(&temp)` verified free of forbidden directory substrings.
/// [Opaque-Box Steps] Open indexer, index 3 posts, commit, drop indexer, reopen same path, verify `doc_count() == 3` and search hit.
/// [Observable Assertions] Path passes hygiene check; reopened indexer preserves all 3 documents and returns immediate search hits.
#[test]
fn test_cm_t2_f10_04_search_indexer_reopen_safe_dir_preserves_docs_hygiene() {
    let temp = TempDir::new().unwrap();
    let idx_dir = safe_temp_idx_dir(&temp);
    let path_str = idx_dir.to_string_lossy().to_ascii_lowercase();
    for forbidden in ["tantivy", "search_index", "fts_index"] {
        assert!(!path_str.contains(forbidden), "index path violated hygiene: {path_str}");
    }
    let ds = build_starter_dataset(now_ms());
    {
        let idx1 = CommunitySearchIndexer::open_or_create(&idx_dir).unwrap();
        for p in &ds.posts[..3] {
            idx1.index_post(p).unwrap();
        }
        idx1.commit().unwrap();
        assert_eq!(idx1.doc_count(), 3);
    }
    let idx2 = CommunitySearchIndexer::open_or_create(&idx_dir).unwrap();
    assert_eq!(idx2.doc_count(), 3, "reopened disk index must preserve committed docs");
    let hits = idx2.search(&CommunitySearchRequestDto { actor_kind: None, community_slug: ds.posts[0].community_slug.clone(), limit: 10, ..Default::default() }).unwrap();
    assert!(hits.contains(&ds.posts[0].post_id));
}

/// [Test ID] TEST-CM-T2-F10-05
/// [Feature / Req] F10 / TEST_INFRA.md §6 (SQLite STRICT table type, CHECK & foreign-key constraints enforced)
/// [Preconditions] Fresh `CommunityStore` in `TempDir` with `PRAGMA foreign_keys = ON` and `STRICT` tables.
/// [Opaque-Box Steps] Execute raw SQL inserts violating STRICT integer column type, `actor_kind` CHECK, post author FK, and self-follow CHECK.
/// [Observable Assertions] Every invalid SQLite insert fails cleanly with a database constraint error.
#[test]
fn test_cm_t2_f10_05_sqlite_strict_table_constraints_reject_bad_types_and_fks() {
    let dir = TempDir::new().unwrap();
    let store = CommunityStore::open(dir.path().join("strict_f10_05.db")).unwrap();

    store.with_conn(|conn| {
        // 1. STRICT type mismatch: TEXT into INTEGER post_karma
        assert!(conn.execute(
            "INSERT INTO community_actors (actor_id, actor_kind, handle, display_name, post_karma, created_at_ms, updated_at_ms) VALUES ('u:bad1', 'human', 'bad1', 'Bad', 'not_int', 1, 1)",
            []
        ).is_err(), "STRICT table must reject TEXT in INTEGER column");

        // 2. CHECK constraint violation on actor_kind
        assert!(conn.execute(
            "INSERT INTO community_actors (actor_id, actor_kind, handle, display_name, created_at_ms, updated_at_ms) VALUES ('u:bad2', 'alien', 'bad2', 'Bad', 1, 1)",
            []
        ).is_err(), "CHECK constraint must reject invalid actor_kind");

        // 3. Foreign key violation on nonexistent author_actor_id
        assert!(conn.execute(
            "INSERT INTO community_posts (post_id, author_actor_id, body_markdown, created_at_ms, updated_at_ms) VALUES ('p:orphan', 'u:ghost_999', 'Orphan', 1, 1)",
            []
        ).is_err(), "FK constraint must reject nonexistent author_actor_id");

        // 4. Self-follow CHECK constraint violation
        conn.execute(
            "INSERT INTO community_actors (actor_id, actor_kind, handle, display_name, created_at_ms, updated_at_ms) VALUES ('u:ok1', 'human', 'ok1', 'OK', 1, 1)",
            []
        ).unwrap();
        assert!(conn.execute(
            "INSERT INTO community_follows (follower_actor_id, target_actor_id, created_at_ms) VALUES ('u:ok1', 'u:ok1', 1)",
            []
        ).is_err(), "CHECK constraint must reject self-follow");
        Ok(())
    }).unwrap();
}

// ============================================================================
// F11 — REST API, ActorAuth & Public Media Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F11-01
/// [Feature / Req] F11 / TEST_INFRA.md §6 (Unauthenticated, forged Bearer & forged Bot tokens return 401)
/// [Preconditions] Ephemeral live server booted.
/// [Opaque-Box Steps] Call GET `/feed` and POST `/posts` with no auth, forged `Bearer`, forged `Bot`, and empty `Bot` token.
/// [Observable Assertions] Every unauthorized request returns 401 Unauthorized.
#[tokio::test]
async fn test_cm_t2_f11_01_unauthenticated_and_forged_bearer_or_bot_token_401() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());

    for bad_auth in [
        CommunityAuthMode::None,
        CommunityAuthMode::Human("forged_invalid_bearer_token".into()),
        CommunityAuthMode::DirectBot("999999:forged_invalid_bot_secret".into()),
        CommunityAuthMode::DirectBot("".into()),
    ] {
        assert_eq!(driver.req(Method::GET, "/api/v1/community/feed?limit=5", &bad_auth, None).await.unwrap().0, 401);
        assert_eq!(driver.req(Method::POST, "/api/v1/community/posts", &bad_auth, Some(json!({ "body": "Unauth post" }))).await.unwrap().0, 401);
    }
}

/// [Test ID] TEST-CM-T2-F11-02
/// [Feature / Req] F11 / TEST_INFRA.md §6 (`Act-As` security boundary: non-owner & cross-actor spoofing return 403 with zero writes)
/// [Preconditions] Ephemeral live server with Alice owning `sec_t2_f11_bot` and non-owner Bob.
/// [Opaque-Box Steps] Bob sends POST `/posts` acting as Alice's Bot and as Alice's Human; Alice's Bot sends POST `/posts` with `X-Community-Actor-Id` set to Alice's Human.
/// [Observable Assertions] All 3 spoofing attempts return 403 Forbidden and Alice's Bot `posts_count` remains 0.
#[tokio::test]
async fn test_cm_t2_f11_02_act_as_security_boundary_non_owner_returns_403_zero_writes() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f11_02", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f11_02", "Pass1234!").await.unwrap();
    let alice_bot = driver.register_community_bot(&alice.token, "sec_t2_f11_bot", "Sec Bot", "manual").await.unwrap();

    for spoof in [
        CommunityAuthMode::ActAsBot { owner_token: bob.token.clone(), actor_id: alice_bot.actor_id.clone() },
        CommunityAuthMode::ActAsBot { owner_token: bob.token.clone(), actor_id: alice.actor_id.clone() },
        CommunityAuthMode::DirectBotWithActorHeader { bot_token: alice_bot.bot_token.clone(), actor_id: alice.actor_id.clone() },
    ] {
        let (st, val) = driver.req(Method::POST, "/api/v1/community/posts", &spoof, Some(json!({ "body": "Spoofed write" }))).await.unwrap();
        assert_eq!(st, 403, "cross-actor spoofing must return 403: {val}");
    }

    let (_, prof) = driver.req(Method::GET, &format!("/api/v1/community/actors/{}", alice_bot.actor_id), &CommunityAuthMode::Human(alice.token), None).await.unwrap();
    assert_eq!(prof["actor"]["posts_count"], 0, "spoofed writes must never land");
}

/// [Test ID] TEST-CM-T2-F11-03
/// [Feature / Req] F11 / TEST_INFRA.md §6 (Forged unknown & SQLi `X-Community-Actor-Id` headers rejected cleanly)
/// [Preconditions] Ephemeral live server with Alice registered.
/// [Opaque-Box Steps] Send GET `/identities` and POST `/posts` with unknown, path-traversal, and SQLi `X-Community-Actor-Id` headers.
/// [Observable Assertions] Every forged header returns 403 or 404 (never 200 or 500).
#[tokio::test]
async fn test_cm_t2_f11_03_forged_unknown_or_sqli_actor_id_header_rejected() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f11_03", "Pass1234!").await.unwrap();

    for forged in ["b:999999999", "u:999999999", "b:1' OR '1'='1", "../admin"] {
        let mode = CommunityAuthMode::ActAsBot { owner_token: alice.token.clone(), actor_id: forged.into() };
        let (st_g, _) = driver.req(Method::GET, "/api/v1/community/identities", &mode, None).await.unwrap();
        let (st_p, _) = driver.req(Method::POST, "/api/v1/community/posts", &mode, Some(json!({ "body": "Forged" }))).await.unwrap();
        assert!(matches!(st_g, 403 | 404), "forged GET actor header '{forged}' returned {st_g}");
        assert!(matches!(st_p, 403 | 404), "forged POST actor header '{forged}' returned {st_p}");
    }
}

/// [Test ID] TEST-CM-T2-F11-04
/// [Feature / Req] F11 / TEST_INFRA.md §6 (Private chat & unattached media stay 403 to outsiders; media hijack blocked)
/// [Preconditions] Ephemeral live server with Alice (having a private chat without Bob) and Bob.
/// [Opaque-Box Steps] Alice uploads unattached, private-chat, and public-post PNGs; Bob tries downloading all 3 and hijacking unattached media; Alice tries attaching private-chat media to a public post.
/// [Observable Assertions] Unattached and private-chat media remain strictly 403 to Bob, hijack & private-chat attachment fail 403, while attached public media returns 200 byte-exact.
#[tokio::test]
async fn test_cm_t2_f11_04_private_and_unattached_media_403_isolation_and_hijack_defense() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f11_04", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f11_04", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    let b_auth = CommunityAuthMode::Human(bob.token.clone());

    let (st_chat, chat_val) = driver.req(Method::POST, "/api/v1/chats", &a_auth, Some(json!({ "title": "Alice Secret", "chat_type": "group" }))).await.unwrap();
    assert!(st_chat == 200 || st_chat == 201, "create private chat failed: {chat_val}");
    let private_chat_id = chat_val["chat_id"].as_i64().or_else(|| chat_val["id"].as_i64()).unwrap();

    let (mid_unattached, _) = driver.upload_png(&alice.token, "unattached.png", (10, 20, 30, 255), None).await.unwrap();
    let (mid_private, _) = driver.upload_png(&alice.token, "private.png", (40, 50, 60, 255), Some(private_chat_id)).await.unwrap();
    let (mid_public, pub_bytes) = driver.upload_png(&alice.token, "public.png", (70, 80, 90, 255), None).await.unwrap();

    assert_eq!(driver.get_media(&bob.token, &mid_unattached).await.unwrap().0, 403);
    assert_eq!(driver.get_media(&bob.token, &mid_private).await.unwrap().0, 403);

    // Media hijack by Bob blocked (403)
    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/posts", &b_auth, Some(json!({
            "format": "media", "body": "Hijack", "media": [{ "media_id": mid_unattached, "kind": "image" }]
        }))).await.unwrap().0,
        403
    );
    // Private chat media cannot be leaked into public post even by Alice (403)
    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "format": "media", "body": "Leak private", "media": [{ "media_id": mid_private, "kind": "image" }]
        }))).await.unwrap().0,
        403
    );

    // Public post attachment succeeds -> Bob reads public media byte-exact while private/unattached stay 403
    assert_eq!(
        driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "format": "media", "body": "Public image", "media": [{ "media_id": mid_public, "kind": "image" }]
        }))).await.unwrap().0,
        201
    );
    let (st_pub, got_pub) = driver.get_media(&bob.token, &mid_public).await.unwrap();
    assert_eq!(st_pub, 200);
    assert_eq!(got_pub, pub_bytes);
    assert_eq!(driver.get_media(&bob.token, &mid_unattached).await.unwrap().0, 403);
    assert_eq!(driver.get_media(&bob.token, &mid_private).await.unwrap().0, 403);
}

/// [Test ID] TEST-CM-T2-F11-05
/// [Feature / Req] F11 / TEST_INFRA.md §6 (`POST /posts/{id}/summon-bot` rejects nonexistent, Human, or Manual bot cleanly)
/// [Preconditions] Ephemeral live server with Alice, Bob (Human), a Manual bot, and 1 post.
/// [Opaque-Box Steps] Call `/summon-bot` with nonexistent actor ID, Bob's Human actor ID, and Manual bot actor ID.
/// [Observable Assertions] Nonexistent returns 404, Human returns 400, Manual bot returns 403, and post remains uncorrupted with 0 comments.
#[tokio::test]
async fn test_cm_t2_f11_05_summon_nonexistent_human_or_manual_bot_rejected_cleanly() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f11_05", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f11_05", "Pass1234!").await.unwrap();
    let manual_bot = driver.register_community_bot(&alice.token, "manual_t2_f11_bot", "Manual Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());
    assert_eq!(driver.req(Method::GET, "/api/v1/community/identities", &CommunityAuthMode::Human(bob.token), None).await.unwrap().0, 200);

    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "body": "Summon guard post" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/summon-bot"), &a_auth, Some(json!({ "bot_actor_id": "b:99999999" }))).await.unwrap().0, 404);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/summon-bot"), &a_auth, Some(json!({ "bot_actor_id": bob.actor_id }))).await.unwrap().0, 400);
    assert_eq!(driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/summon-bot"), &a_auth, Some(json!({ "bot_actor_id": manual_bot.actor_id }))).await.unwrap().0, 403);

    let (st, det) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &a_auth, None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(det["post"]["comments_count"], 0);
}

// ============================================================================
// F12 — Bot API, Webhooks & Hermes Tools Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F12-01
/// [Feature / Req] F12 / TEST_INFRA.md §6 (Invalid Bot API token returns 401 structured JSON on split & concat routes)
/// [Preconditions] Ephemeral live server booted.
/// [Opaque-Box Steps] Call `community.createPost` via `/bot/{bad_token}/...` and `community.getFeed` via `/bot{bad_token}/...`.
/// [Observable Assertions] Both route shapes return HTTP 401 with `{"ok": false, "error_code": 401}`.
#[tokio::test]
async fn test_cm_t2_f12_01_bot_api_invalid_token_returns_401_structured_error() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());

    let (st1, v1) = driver.bot_api("999999:bad_secret_token", "community.createPost", true, Some(json!({ "body": "Hi" }))).await.unwrap();
    assert_eq!(st1, 401, "split invalid bot token must return 401: {v1}");
    assert_eq!(v1["ok"], false);
    assert_eq!(v1["error_code"], 401);

    let (st2, v2) = driver.bot_api("999999:bad_secret_token", "community.getFeed", false, Some(json!({ "limit": 5 }))).await.unwrap();
    assert_eq!(st2, 401, "concat invalid bot token must return 401: {v2}");
    assert_eq!(v2["ok"], false);
    assert_eq!(v2["error_code"], 401);
}

/// [Test ID] TEST-CM-T2-F12-02
/// [Feature / Req] F12 / TEST_INFRA.md §6 (Unknown Bot API method & missing required params return structured 404/400 JSON)
/// [Preconditions] Ephemeral live server with registered Bot citizen.
/// [Opaque-Box Steps] Invoke `community.nonExistent`, `community.getPost` without `post_id`, and `community.createComment` without `body`.
/// [Observable Assertions] Unknown method returns HTTP 404 (`ok: false`); missing params return HTTP 400 (`ok: false`).
#[tokio::test]
async fn test_cm_t2_f12_02_bot_api_unknown_method_and_missing_params_structured_error() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f12_02", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "err_t2_f12_bot", "Err Bot", "manual").await.unwrap();

    let (st_unk, v_unk) = driver.bot_api(&bot.bot_token, "community.nonExistent", true, Some(json!({}))).await.unwrap();
    assert_eq!(st_unk, 404, "unknown method must return 404: {v_unk}");
    assert_eq!(v_unk["ok"], false);
    assert_eq!(v_unk["error_code"], 404);

    let (st_gp, v_gp) = driver.bot_api(&bot.bot_token, "community.getPost", false, Some(json!({}))).await.unwrap();
    assert_eq!(st_gp, 400, "getPost missing post_id must return 400: {v_gp}");
    assert_eq!(v_gp["ok"], false);

    let (st_cc, v_cc) = driver.bot_api(&bot.bot_token, "community.createComment", true, Some(json!({ "post_id": "p_1" }))).await.unwrap();
    assert_eq!(st_cc, 400, "createComment missing body must return 400: {v_cc}");
    assert_eq!(v_cc["ok"], false);
}

/// [Test ID] TEST-CM-T2-F12-03
/// [Feature / Req] F12 / TEST_INFRA.md §6 (Unreachable webhook URL fails non-blockingly and `getUpdates` retains enqueued event)
/// [Preconditions] Ephemeral live server with Alice, Bot citizen, and `setWebhook` pointed at an unreachable closed loopback port.
/// [Opaque-Box Steps] Configure unreachable webhook URL, post mentioning `@deadhook_t2_bot` while timing HTTP latency, then poll `getUpdates`.
/// [Observable Assertions] Post creation returns 201 in `< 2s` without blocking; `getUpdates` returns the enqueued `community_mention`.
#[tokio::test]
async fn test_cm_t2_f12_03_unreachable_webhook_non_blocking_and_get_updates_fallback() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f12_03", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "deadhook_t2_bot", "DeadHook Bot", "manual").await.unwrap();

    let dead_url = format!("{}/webhook", closed_local_url());
    let (st_wh, wh) = driver.bot_api(&bot.bot_token, "setWebhook", true, Some(json!({ "url": dead_url }))).await.unwrap();
    assert_eq!(st_wh, 200, "setWebhook failed: {wh}");
    assert_eq!(wh["ok"], true);

    let t_start = Instant::now();
    let (st_post, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &CommunityAuthMode::Human(alice.token), Some(json!({
            "body": "Non-blocking mention for @deadhook_t2_bot"
        })))
        .await
        .unwrap();
    let elapsed = t_start.elapsed();
    assert_eq!(st_post, 201);
    assert!(elapsed < Duration::from_secs(2), "unreachable webhook must not block post response ({elapsed:?})");
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let (st_upd, upd) = driver.bot_api_get(&bot.bot_token, "getUpdates", true, "").await.unwrap();
    assert_eq!(st_upd, 200);
    assert_eq!(upd["ok"], true);
    assert!(
        upd["result"].as_array().unwrap().iter().any(|u| u.get("community_mention").is_some_and(|m| m["post_id"] == pid)),
        "enqueued mention must remain retrievable via getUpdates: {upd}"
    );
}

/// [Test ID] TEST-CM-T2-F12-04
/// [Feature / Req] F12 / TEST_INFRA.md §6 (Hermes community tool rejected under `Minimal` preset & unattached `PlatformImBridge`)
/// [Preconditions] Ephemeral live server + registered Bot + standalone `HermesToolContext` without bridge.
/// [Opaque-Box Steps] Execute `community_browse_feed` under `ToolsetPreset::Minimal`, and execute `community_browse_feed` against a `HermesToolContext::new(...)` with `platform_im_bridge == None`.
/// [Observable Assertions] Both invocations return `success == false` with clear diagnostic errors.
#[tokio::test]
async fn test_cm_t2_f12_04_hermes_tool_minimal_preset_and_unattached_bridge_rejected() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f12_04", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "min_t2_f12_bot", "Minimal Bot", "manual").await.unwrap();

    let res_min = execute_bot_hermes_tool(
        &server,
        bot.bot_user_id,
        "community_browse_feed",
        json!({ "limit": 5 }),
        ToolsetPreset::Minimal,
        ApprovalMode::Prompt,
    )
    .await;
    assert!(!res_min.success, "Minimal preset must reject PlatformIm community tool");
    assert!(res_min.error.is_some());

    let dir = TempDir::new().unwrap();
    let ctx = HermesToolContext::new(dir.path());
    assert!(ctx.platform_im_bridge.is_none());
    let res_unattached = HermesToolExecutor::execute("community_browse_feed", json!({ "limit": 5 }), &ctx).await;
    assert!(!res_unattached.success, "unattached PlatformImBridge must return structured error");
    assert!(
        res_unattached.error.as_deref().unwrap_or("").contains("PlatformImBridge"),
        "expected PlatformImBridge diagnostic, got {:?}",
        res_unattached.error
    );
}

/// [Test ID] TEST-CM-T2-F12-05
/// [Feature / Req] F12 / TEST_INFRA.md §6 (High-risk `community_mod_action` denied under `ApprovalMode::Deny` with zero mutation)
/// [Preconditions] Ephemeral live server with Owner Alice in `c/deny_f12`, Bot promoted to `bot_mod`, and unlocked post `pid`.
/// [Opaque-Box Steps] Execute `community_mod_action` (`lock`) via `execute_bot_hermes_tool` with `ApprovalMode::Deny`, then GET `/posts/{pid}`.
/// [Observable Assertions] Tool returns `success == false` citing `Deny` policy, and post remains `is_locked == false`.
#[tokio::test]
async fn test_cm_t2_f12_05_hermes_high_risk_mod_tool_denied_under_approval_mode_deny() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f12_05", "Pass1234!").await.unwrap();
    let bot = driver.register_community_bot(&alice.token, "deny_t2_f12_bot", "Deny Bot", "manual").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token.clone());

    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities", &a_auth, Some(json!({ "slug": "deny_f12", "name": "Deny F12", "allow_bots": true }))).await.unwrap().0, 201);
    assert_eq!(driver.req(Method::POST, "/api/v1/community/communities/deny_f12/mod-actions", &a_auth, Some(json!({ "action": "promote_mod", "target_kind": "actor", "target_id": bot.actor_id }))).await.unwrap().0, 200);
    let (_, pv) = driver.req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({ "community_slug": "deny_f12", "body": "Protected unlocked post" }))).await.unwrap();
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let res = execute_bot_hermes_tool(
        &server,
        bot.bot_user_id,
        "community_mod_action",
        json!({ "community_slug": "deny_f12", "action": "lock", "target_kind": "post", "target_id": pid }),
        ToolsetPreset::HermesTelegram,
        ApprovalMode::Deny,
    )
    .await;
    assert!(!res.success, "ApprovalMode::Deny must block high-risk community_mod_action");
    assert!(res.error.as_deref().unwrap_or("").contains("Deny"), "error should mention Deny: {:?}", res.error);

    let (st, post_after) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &a_auth, None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(post_after["post"]["is_locked"], false, "denied tool must leave post unlocked");
}

// ============================================================================
// F13 — WS Fan-Out & 5s Scheduler Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F13-01
/// [Feature / Req] F13 / TEST_INFRA.md §6 (Unauthenticated WS rejected; dropped WS client never blocks live fan-out)
/// [Preconditions] Ephemeral live server with Alice and Bob registered.
/// [Opaque-Box Steps] Attempt WS connect with forged token; connect Alice & Bob WS streams; abruptly drop Alice WS; Alice posts via REST.
/// [Observable Assertions] Forged WS fails handshake; Bob's WS promptly receives `UpdateEvent::CommunityEvent { event_kind: "post_created", .. }` within 3s.
#[tokio::test]
async fn test_cm_t2_f13_01_unauth_ws_rejected_and_disconnected_client_does_not_block_fanout() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f13_01", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f13_01", "Pass1234!").await.unwrap();

    assert!(connect_ws(&server, "forged_invalid_ws_token").await.is_err(), "forged WS token must be rejected");

    let ws_alice = connect_ws(&server, &alice.token).await.expect("connect ws_alice");
    let mut ws_bob = connect_ws(&server, &bob.token).await.expect("connect ws_bob");
    drop(ws_alice);
    tokio::time::sleep(Duration::from_millis(60)).await;

    let (st, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &CommunityAuthMode::Human(alice.token), Some(json!({ "body": "Fanout resilience post" })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let frame = recv_ws_frame_matching(&mut ws_bob, Duration::from_secs(3), |f| {
        matches!(
            f,
            WsServerFrame::Update(UpdateEvent::CommunityEvent { event_kind, post_id, .. })
                if event_kind == "post_created" && post_id.as_deref() == Some(pid.as_str())
        )
    })
    .await
    .expect("healthy WS client must receive post_created fan-out");
    assert!(matches!(frame, WsServerFrame::Update(UpdateEvent::CommunityEvent { .. })));
    drop(ws_bob);
}

/// [Test ID] TEST-CM-T2-F13-02
/// [Feature / Req] F13 / TEST_INFRA.md §6 (Future `Scheduled` post is NOT prematurely published by background ticker)
/// [Preconditions] Ephemeral live server with Alice and Bob registered.
/// [Opaque-Box Steps] Create a Scheduled post 1 hour in the future, run `run_community_background_tick` twice, then query as author and outsider.
/// [Observable Assertions] Post stays `status == "scheduled"` for Alice and remains strictly hidden (`404` & absent from feed) for outsider Bob.
#[tokio::test]
async fn test_cm_t2_f13_02_future_scheduled_post_not_prematurely_published_by_ticker() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f13_02", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f13_02", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token);
    let b_auth = CommunityAuthMode::Human(bob.token);

    let future_ts = now_ms() + 3_600_000;
    let (st, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "body": "Future scheduled announcement",
            "status": "scheduled",
            "scheduled_at_ms": future_ts
        })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let state = server.state().expect("server state");
    run_community_background_tick(state).await;
    run_community_background_tick(state).await;

    let (st_a, pa) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &a_auth, None).await.unwrap();
    assert_eq!(st_a, 200);
    assert_eq!(pa["post"]["status"], "scheduled", "future post must not publish early");

    assert_eq!(driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &b_auth, None).await.unwrap().0, 404);
    let (_, feed) = driver.req(Method::GET, "/api/v1/community/feed?tab=all&limit=50", &b_auth, None).await.unwrap();
    assert!(!feed["items"].as_array().unwrap().iter().any(|i| i["post"]["post_id"] == pid));
}

/// [Test ID] TEST-CM-T2-F13-03
/// [Feature / Req] F13 / TEST_INFRA.md §6 (Active unexpired poll stays open across multiple background ticker cycles)
/// [Preconditions] Ephemeral live server with Alice and Bob registered.
/// [Opaque-Box Steps] Create a poll expiring in 1 hour, run `run_community_background_tick` 3 times, then have Bob vote on the poll.
/// [Observable Assertions] Poll remains `is_closed == false` and Bob's vote returns 200 with `total_votes == 1`.
#[tokio::test]
async fn test_cm_t2_f13_03_active_poll_remains_open_across_multiple_ticker_cycles() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f13_03", "Pass1234!").await.unwrap();
    let bob = driver.register_human("bob_t2_f13_03", "Pass1234!").await.unwrap();
    let a_auth = CommunityAuthMode::Human(alice.token);
    let b_auth = CommunityAuthMode::Human(bob.token);

    let (st, pv) = driver
        .req(Method::POST, "/api/v1/community/posts", &a_auth, Some(json!({
            "format": "poll",
            "body": "Active poll",
            "poll": { "question": "Still open?", "options": ["Yes", "No"], "multi_select": false, "duration_secs": 3600 }
        })))
        .await
        .unwrap();
    assert_eq!(st, 201);
    let pid = pv["post"]["post_id"].as_str().unwrap().to_string();

    let state = server.state().expect("server state");
    for _ in 0..3 {
        run_community_background_tick(state).await;
    }

    let (_, det) = driver.req(Method::GET, &format!("/api/v1/community/posts/{pid}"), &b_auth, None).await.unwrap();
    assert_eq!(det["post"]["poll"]["is_closed"], false);

    let (vst, vval) = driver.req(Method::POST, &format!("/api/v1/community/posts/{pid}/poll-vote"), &b_auth, Some(json!({ "option_ids": ["opt_0"] }))).await.unwrap();
    assert_eq!(vst, 200, "unexpired poll must remain votable: {vval}");
    assert_eq!(vval["total_votes"], 1);
}

/// [Test ID] TEST-CM-T2-F13-04
/// [Feature / Req] F13 / TEST_INFRA.md §6 (Server restart drains background ticker cleanly & preserves persisted state)
/// [Preconditions] Ephemeral live server with Alice having created 1 published post and 1 future scheduled post.
/// [Opaque-Box Steps] Drop HTTP driver, call `server.restart().await` measuring drain latency, reconnect new driver, and fetch both posts.
/// [Observable Assertions] Restart completes cleanly within `< 5s` (zero ticker hang) and both published & scheduled posts survive intact.
#[tokio::test]
async fn test_cm_t2_f13_04_server_restart_drains_ticker_cleanly_and_preserves_state() {
    let mut server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f13_04", "Pass1234!").await.unwrap();
    let auth = CommunityAuthMode::Human(alice.token);

    let (_, p_pub) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({ "body": "Survives restart" }))).await.unwrap();
    let pid_pub = p_pub["post"]["post_id"].as_str().unwrap().to_string();
    let (_, p_sch) = driver.req(Method::POST, "/api/v1/community/posts", &auth, Some(json!({
        "body": "Scheduled survives restart", "status": "scheduled", "scheduled_at_ms": now_ms() + 3_600_000
    }))).await.unwrap();
    let pid_sch = p_sch["post"]["post_id"].as_str().unwrap().to_string();
    drop(driver);

    let t0 = Instant::now();
    server.restart().await.expect("restart server cleanly");
    assert!(t0.elapsed() < Duration::from_secs(5), "weak-state ticker must drain swiftly on restart");

    let driver2 = CommunityHttpDriver::new(server.base_url());
    let (s1, v1) = driver2.req(Method::GET, &format!("/api/v1/community/posts/{pid_pub}"), &auth, None).await.unwrap();
    let (s2, v2) = driver2.req(Method::GET, &format!("/api/v1/community/posts/{pid_sch}"), &auth, None).await.unwrap();
    assert_eq!((s1, s2), (200, 200));
    assert_eq!(v1["post"]["status"], "published");
    assert_eq!(v2["post"]["status"], "scheduled");
}

/// [Test ID] TEST-CM-T2-F13-05
/// [Feature / Req] F13 / TEST_INFRA.md §6 (`UpdateEvent::CommunityEvent` serde edge cases: empty, large UTF-8 & forward-compat)
/// [Preconditions] Pure-Rust `UpdateEvent::CommunityEvent` and `WsServerFrame`.
/// [Opaque-Box Steps] Round-trip an all-`None`/empty payload event, a 16 KiB CJK+emoji payload event, and a raw JSON payload with an unknown future `event_kind`.
/// [Observable Assertions] Every edge-case frame serializes/deserializes cleanly and preserves exact values without panic.
#[test]
fn test_cm_t2_f13_05_update_event_community_event_serde_edge_cases() {
    let empty_ev = WsServerFrame::Update(UpdateEvent::CommunityEvent {
        event_kind: "ping_empty".into(),
        community_id: None,
        post_id: None,
        comment_id: None,
        actor_id: None,
        payload_json: String::new(),
        timestamp_ms: 0,
    });
    let s1 = serde_json::to_string(&empty_ev).unwrap();
    assert_eq!(serde_json::from_str::<WsServerFrame>(&s1).unwrap(), empty_ev);

    let big_payload = json!({ "summary": "社区实时推送🚀".repeat(600) }).to_string();
    assert!(big_payload.len() > 10_000);
    let big_ev = WsServerFrame::Update(UpdateEvent::CommunityEvent {
        event_kind: "post_created".into(),
        community_id: Some("comm_rust".into()),
        post_id: Some("post_big".into()),
        comment_id: None,
        actor_id: Some("b:910001".into()),
        payload_json: big_payload,
        timestamp_ms: 1_760_000_000_123,
    });
    let s2 = serde_json::to_string(&big_ev).unwrap();
    assert_eq!(serde_json::from_str::<WsServerFrame>(&s2).unwrap(), big_ev);

    let raw_future = r#"{"type":"community_event","data":{"event_kind":"future_v99_poll_boost","payload_json":"{}","timestamp_ms":42}}"#;
    let parsed: UpdateEvent = serde_json::from_str(raw_future).expect("forward-compat unknown event_kind must parse");
    assert!(matches!(parsed, UpdateEvent::CommunityEvent { ref event_kind, timestamp_ms: 42, .. } if event_kind == "future_v99_poll_boost"));
}

// ============================================================================
// F14 — Client API & Dual Online/Offline Boundaries (5 Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F14-01
/// [Feature / Req] F14 / TEST_INFRA.md §6 (Account switch on same TempDir isolates drafts, bookmarks & algo prefs)
/// [Preconditions] Unreachable URL (`closed_local_url()`) + single `TestTelegramClient` TempDir shared across Account A and Account B.
/// [Opaque-Box Steps] Bind Account A (`7411`), create Draft, bookmark `seed_post_01`, save `BotHeavy`/`allow_nsfw=true` algo prefs; switch to Account B (`7412`).
/// [Observable Assertions] Account B cannot read Account A's draft, has zero leaked bookmarks in `Saved`, and sees fresh default `Balanced`/`allow_nsfw=false` algo prefs.
#[tokio::test]
async fn test_cm_t2_f14_01_account_switch_same_tempdir_isolates_drafts_bookmarks_algo_prefs() {
    let url = closed_local_url();
    let tc = TestTelegramClient::open(&url).unwrap();
    tc.bind_auth(7411, "alice_f14_01", "tok_alice_f14_01");

    let draft = tc
        .community()
        .create_post(&CreatePostRequest {
            body: "Alice private offline draft".into(),
            status: PostStatus::Draft,
            ..Default::default()
        })
        .await
        .unwrap();
    let draft_id = draft.post.post_id.clone();
    tc.community()
        .bookmark_post(
            "seed_post_01",
            &gpui_telegram_proto::community_contracts::BookmarkRequest { folder: Some("AliceVault".into()), remove: None },
        )
        .await
        .unwrap();
    let mut custom_prefs = default_prefs_for_preset(AlgoPreset::BotHeavy);
    custom_prefs.allow_nsfw = true;
    tc.community().update_algo_prefs(&custom_prefs).await.unwrap();

    // Switch account on the exact same TempDir
    tc.bind_auth(7412, "bob_f14_01", "tok_bob_f14_01");

    assert!(tc.community().get_post(&draft_id, None).await.is_err(), "Bob must not read Alice's offline draft");
    let bob_saved = tc
        .community()
        .get_feed(&FeedRequestDto { tab: FeedTab::Saved, limit: 25, ..Default::default() })
        .await
        .unwrap();
    assert!(bob_saved.items.is_empty(), "Alice's bookmarks must not leak to Bob after account switch");
    let bob_prefs = tc.community().get_algo_prefs().await.unwrap();
    assert_eq!(bob_prefs.preset, AlgoPreset::Balanced);
    assert!(!bob_prefs.allow_nsfw);

    tc.shutdown().await;
}

/// [Test ID] TEST-CM-T2-F14-02
/// [Feature / Req] F14 / TEST_INFRA.md §6 (Mid-session server disconnect transparently degrades to `LocalFallback`)
/// [Preconditions] Ephemeral live server + `TestTelegramClient` initially verified in `CommunityRuntimeMode::Online`.
/// [Opaque-Box Steps] Fetch online feed, stop server (`server.stop().await`), then invoke `get_feed` and `create_post`.
/// [Observable Assertions] Runtime switches cleanly from `Online` to `LocalFallback` and serves/persists local community data without panicking.
#[tokio::test]
async fn test_cm_t2_f14_02_mid_session_server_disconnect_falls_back_to_local_runtime() {
    let mut server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let tc = TestTelegramClient::open(&server.base_url()).unwrap();
    let _alice = tc.register_and_bind(&driver, "alice_t2_f14_02", "Pass1234!").await.unwrap();

    let _ = tc.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc.community().runtime_mode(), CommunityRuntimeMode::Online);

    drop(driver);
    server.stop().await;

    let offline_feed = tc.community().get_feed(&FeedRequestDto::default()).await.unwrap();
    assert_eq!(tc.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(!offline_feed.items.is_empty());

    let local_post = tc
        .community()
        .create_post(&CreatePostRequest { body: "Created after server disconnect".into(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(local_post.post.body, "Created after server disconnect");

    tc.shutdown().await;
}

/// [Test ID] TEST-CM-T2-F14-03
/// [Feature / Req] F14 / TEST_INFRA.md §6 (Server `403 Forbidden` surfaces error instead of silently falling back offline)
/// [Preconditions] Ephemeral live server with Alice owning a Bot and Bob's `TestTelegramClient` online.
/// [Opaque-Box Steps] Verify Bob online, set Bob's active actor to Alice's Bot ID, and call `create_post`.
/// [Observable Assertions] Call fails with `Err(TelegramError::PermissionDenied(_))` and client stays `CommunityRuntimeMode::Online`.
#[tokio::test]
async fn test_cm_t2_f14_03_server_403_not_masked_by_offline_fallback() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let alice = driver.register_human("alice_t2_f14_03", "Pass1234!").await.unwrap();
    let alice_bot = driver.register_community_bot(&alice.token, "priv_t2_f14_bot", "Priv Bot", "manual").await.unwrap();

    let tc_bob = TestTelegramClient::open(&server.base_url()).unwrap();
    let _bob = tc_bob.register_and_bind(&driver, "bob_t2_f14_03", "Pass1234!").await.unwrap();
    let _ = tc_bob.community().get_identities().await.unwrap();
    assert_eq!(tc_bob.community().runtime_mode(), CommunityRuntimeMode::Online);

    tc_bob.community().set_active_actor_id(Some(alice_bot.actor_id.clone()));
    let err = tc_bob
        .community()
        .create_post(&CreatePostRequest { body: "Unauthorized act-as post".into(), ..Default::default() })
        .await
        .expect_err("403 Forbidden must NEVER be masked by local fallback");
    assert!(matches!(err, TelegramError::PermissionDenied(_)), "expected PermissionDenied, got {err:?}");
    assert_eq!(tc_bob.community().runtime_mode(), CommunityRuntimeMode::Online);

    tc_bob.shutdown().await;
}

/// [Test ID] TEST-CM-T2-F14-04
/// [Feature / Req] F14 / TEST_INFRA.md §6 (Malformed `CommunityEvent` WS frames dropped cleanly without poisoning dispatch)
/// [Preconditions] `TestTelegramClient` bound to authenticated user.
/// [Opaque-Box Steps] Inject empty-`event_kind` and invalid-JSON `CommunityEvent` frames into `process_ws_frame`, then inject a valid `post_created` frame.
/// [Observable Assertions] Both malformed frames produce zero emitted events; the subsequent valid frame emits `ClientDomainEvent::CommunityEvent` intact.
#[tokio::test]
async fn test_cm_t2_f14_04_malformed_community_ws_frame_resilience() {
    let tc = TestTelegramClient::open(&closed_local_url()).unwrap();
    tc.bind_auth(7414, "alice_f14_04", "tok_f14_04");

    let bad1 = WsServerFrame::Update(UpdateEvent::CommunityEvent {
        event_kind: "   ".into(),
        community_id: None,
        post_id: Some("p_bad1".into()),
        comment_id: None,
        actor_id: None,
        payload_json: "{}".into(),
        timestamp_ms: now_ms(),
    });
    let bad2 = WsServerFrame::Update(UpdateEvent::CommunityEvent {
        event_kind: "post_created".into(),
        community_id: None,
        post_id: Some("p_bad2".into()),
        comment_id: None,
        actor_id: None,
        payload_json: "{not valid json".into(),
        timestamp_ms: now_ms(),
    });
    assert!(tc.client().process_ws_frame(bad1).is_empty(), "empty event_kind must be dropped");
    assert!(tc.client().process_ws_frame(bad2).is_empty(), "corrupt payload_json must be dropped");

    let good = WsServerFrame::Update(UpdateEvent::CommunityEvent {
        event_kind: "post_created".into(),
        community_id: Some("comm_rust".into()),
        post_id: Some("p_good_14".into()),
        comment_id: None,
        actor_id: Some("u:7414".into()),
        payload_json: r#"{"ok":true}"#.into(),
        timestamp_ms: now_ms(),
    });
    let emitted = tc.client().process_ws_frame(good);
    assert!(
        emitted.iter().any(|e| matches!(e, ClientDomainEvent::CommunityEvent { event_kind, post_id, .. } if event_kind == "post_created" && post_id.as_deref() == Some("p_good_14"))),
        "valid frame after malformed frames must still emit cleanly: {emitted:?}"
    );

    tc.shutdown().await;
}

/// [Test ID] TEST-CM-T2-F14-05
/// [Feature / Req] F14 / TEST_INFRA.md §6 (Rapid concurrent client async calls & clean nested-runtime-safe shutdown)
/// [Preconditions] Ephemeral live server + online `TestTelegramClient` bound to Alice.
/// [Opaque-Box Steps] Spawn 12 concurrent tasks executing mixed `get_feed`, `get_trending`, `list_communities`, and `create_post`, await all, and invoke `tc.shutdown().await`.
/// [Observable Assertions] All 12 concurrent tasks succeed without contention error and shutdown exits cleanly with zero drop panic.
#[tokio::test]
async fn test_cm_t2_f14_05_rapid_concurrent_client_calls_and_safe_shutdown() {
    let server = EphemeralTestServer::start().await.expect("start server");
    let driver = CommunityHttpDriver::new(server.base_url());
    let tc = TestTelegramClient::open(&server.base_url()).unwrap();
    let _alice = tc.register_and_bind(&driver, "alice_t2_f14_05", "Pass1234!").await.unwrap();

    let mut handles = Vec::with_capacity(12);
    for i in 0..12 {
        let comm = tc.community().clone();
        handles.push(tokio::spawn(async move {
            match i % 4 {
                0 => { comm.get_feed(&FeedRequestDto { limit: 10, ..Default::default() }).await.map(|_| ()) }
                1 => { comm.get_trending(Some(5)).await.map(|_| ()) }
                2 => { comm.list_communities(None, Some(10)).await.map(|_| ()) }
                _ => { comm.create_post(&CreatePostRequest { body: format!("Concurrent client post #{i}"), ..Default::default() }).await.map(|_| ()) }
            }
        }));
    }
    for h in handles {
        h.await.expect("task join").expect("concurrent CommunityClientFacade call must succeed");
    }

    tc.shutdown().await;
}

// ============================================================================
// F15 — Left Sidebar & AppShell Integration Boundaries (5 GPUI Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F15-01
/// [Feature / Req] F15 / TEST_INFRA.md §6 (Plain `Cmd+C` never opens/toggles Community; `Cmd+Shift+C` toggles; `Cmd+0` unassigned)
/// [Preconditions] Headless GPUI `AppShell` authenticated on `ActiveWorkspacePanel::Chats`.
/// [Opaque-Box Steps] Verify `nav_destination_for_shortcut_digit("0") == None`, press `cmd-c` on Chats, press `cmd-shift-c`, press `cmd-c` inside Community, then press `cmd-shift-c` again.
/// [Observable Assertions] Plain `Cmd+C` leaves active panel unchanged in both Chats and Community; `Cmd+Shift+C` cleanly toggles Chats <-> Community.
#[test]
fn test_cm_t2_f15_01_plain_cmd_c_does_not_open_community_cmd_shift_c_toggles() {
    run_community_gpui_test("test_cm_t2_f15_01_plain_cmd_c_does_not_open_community_cmd_shift_c_toggles");
}

async fn body_t2_f15_01(mut app: AsyncApp) {
    assert_eq!(nav_destination_for_shortcut_digit("0"), None);
    assert_eq!(nav_destination_for_shortcut_digit("1"), Some(NavDestination::Chats));
    assert_eq!(nav_destination_for_shortcut_digit("9"), Some(NavDestination::Browser));

    let (host, _) = open_offline_community_host(&mut app, 1440.0, 900.0, 8251, "t2_f15_01", false).await;
    let shell = host.shell.clone();

    host.key(&mut app, "cmd-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats);

    host.key(&mut app, "cmd-shift-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);
    host.draw(&mut app);

    host.key(&mut app, "cmd-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    host.key(&mut app, "cmd-shift-c");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats);
}

/// [Test ID] TEST-CM-T2-F15-02
/// [Feature / Req] F15 / TEST_INFRA.md §6 (Progressive Escape cascade: IME preedit -> Composer modal -> MediaLightbox -> exit to Chats)
/// [Preconditions] Headless GPUI `AppShell` with mounted `CommunityWorkspace`.
/// [Opaque-Box Steps] Open ComposerModal with active CJK IME preedit, press Escape twice, open MediaLightbox, press Escape twice more.
/// [Observable Assertions] Each Escape dismisses strictly one layer in Z-order until final Escape exits `Community` back to `Chats`.
#[test]
fn test_cm_t2_f15_02_progressive_escape_cascade_ime_modal_lightbox_to_chats() {
    run_community_gpui_test("test_cm_t2_f15_02_progressive_escape_cascade_ime_modal_lightbox_to_chats");
}

async fn body_t2_f15_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8252, "t2_f15_02", true).await;
    let shell = host.shell.clone();
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.state.focus_input(ActiveCommunityInput::ComposerTitle);
    });
    let _ = shell.update(&mut app, |s, cx| s.update_ime_marked_text("中文草稿", cx));
    assert!(shell.read_with(&app, |s, cx| s.has_active_ime_preedit(cx)).unwrap());

    // Step 1: Escape cancels IME preedit only
    host.key(&mut app, "escape");
    assert!(!shell.read_with(&app, |s, cx| s.has_active_ime_preedit(cx)).unwrap());
    assert_eq!(cw.read_with(&app, |w, _| w.state.active_modal).unwrap(), Some(CommunityModal::Composer));
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    // Step 2: Escape closes Composer modal while staying in Community
    host.key(&mut app, "escape");
    assert_eq!(cw.read_with(&app, |w, _| w.state.active_modal).unwrap(), None);
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    // Step 3: Escape closes MediaViewer overlay while staying in Community
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_action(CommunityWorkspaceAction::OpenMediaLightbox("mid_esc_f15".into()), cx);
    });
    assert!(shell.read_with(&app, |s, _| s.overlays.media_viewer_overlay.is_some()).unwrap());
    host.key(&mut app, "escape");
    assert!(shell.read_with(&app, |s, _| s.overlays.media_viewer_overlay.is_none()).unwrap());
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);

    // Step 4: Final Escape exits Community -> Chats
    host.key(&mut app, "escape");
    assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Chats);
}

/// [Test ID] TEST-CM-T2-F15-03
/// [Feature / Req] F15 / TEST_INFRA.md §6 (CJK IME 80ms commit fence & Backspace during preedit preserves committed text)
/// [Preconditions] Headless GPUI `AppShell` with Community Quick-Post focused.
/// [Opaque-Box Steps] Insert `"你好"`, start preedit `"shijie"`, press Backspace, start preedit `"世界"`, press Enter to commit, press immediate trailing Enter (<80ms), wait 95ms, then press Enter.
/// [Observable Assertions] Backspace clears preedit only preserving `"你好"`; immediate trailing Enter is absorbed without submitting; Enter after 95ms submits cleanly.
#[test]
fn test_cm_t2_f15_03_cjk_ime_80ms_commit_fence_and_backspace_during_preedit() {
    run_community_gpui_test("test_cm_t2_f15_03_cjk_ime_80ms_commit_fence_and_backspace_during_preedit");
}

async fn body_t2_f15_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8253, "t2_f15_03", true).await;
    let shell = host.shell.clone();
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| w.open_quick_post_composer(cx));
    let _ = shell.update(&mut app, |s, cx| {
        s.insert_committed_text("你好", cx);
        s.update_ime_marked_text("shijie", cx);
        assert!(s.has_active_ime_preedit(cx));
        assert!(s.handle_non_char_key("backspace", false, cx));
        assert!(!s.has_active_ime_preedit(cx));
        assert_eq!(s.active_text_and_preedit(cx).0, "你好", "Backspace during preedit must preserve committed text");

        // Enter during active preedit commits preedit text without submitting
        s.update_ime_marked_text("世界", cx);
        assert!(s.handle_non_char_key("enter", false, cx));
        assert_eq!(s.active_text_and_preedit(cx).0, "你好世界");
    });
    assert_eq!(cw.read_with(&app, |w, _| w.state.quick_post_body.clone()).unwrap(), "你好世界");
    assert!(!cw.read_with(&app, |w, _| w.state.quick_post_submitting).unwrap());

    // IME unmark_text commit arms the 80ms trailing-Enter fence so immediate Enter is absorbed
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("！", cx);
        s.commit_ime_unmark_text(cx);
        assert_eq!(s.active_text_and_preedit(cx).0, "你好世界！");
        assert!(s.handle_non_char_key("enter", false, cx));
    });
    assert_eq!(cw.read_with(&app, |w, _| w.state.quick_post_body.clone()).unwrap(), "你好世界！");
    assert!(!cw.read_with(&app, |w, _| w.state.quick_post_submitting).unwrap());

    app.background_executor().timer(Duration::from_millis(95)).await;
    let _ = shell.update(&mut app, |s, cx| assert!(s.handle_non_char_key("enter", false, cx)));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        let w = cw.read(cx);
        !w.state.quick_post_submitting && w.state.feed_items.iter().any(|i| i.post.body.contains("你好世界！"))
    }).await);
}

/// [Test ID] TEST-CM-T2-F15-04
/// [Feature / Req] F15 / TEST_INFRA.md §6 (Logout & account switch tear down `CommunityWorkspace` and clear drafts/badges)
/// [Preconditions] Headless GPUI `AppShell` signed in as user `8254` with mounted `CommunityWorkspace`, draft text, and unread badge `17`.
/// [Opaque-Box Steps] Switch account to user `8255`, verify teardown, reopen Community, verify clean state, set badge `9`, then call `logout(cx)`.
/// [Observable Assertions] Both account switch and logout clear `community_workspace`, reset `active_panel` to `Chats`, zero unread badges, and eliminate prior user drafts.
#[test]
fn test_cm_t2_f15_04_logout_and_account_switch_tear_down_community_workspace() {
    run_community_gpui_test("test_cm_t2_f15_04_logout_and_account_switch_tear_down_community_workspace");
}

async fn body_t2_f15_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8254, "t2_f15_alice", true).await;
    let shell = host.shell.clone();
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, _| w.state.quick_post_body = "Alice secret draft".into());
    let _ = shell.update(&mut app, |s, cx| {
        s.set_community_unread_badge(17, cx);
        s.apply_authenticated_session(test_auth_response(8255, "t2_f15_bob", "tok_8255"), cx);
    });
    shell.read_with(&app, |s, cx| {
        assert!(s.community_workspace.is_none(), "account switch must tear down CommunityWorkspace");
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.apple_sidebar.read(cx).community_unread_count, 0);
    }).unwrap();

    let cw_bob = host.open_community_and_wait(&mut app).await;
    assert!(cw_bob.read_with(&app, |w, _| w.state.quick_post_body.is_empty()).unwrap(), "Bob must not inherit Alice draft");

    let _ = shell.update(&mut app, |s, cx| {
        s.set_community_unread_badge(9, cx);
        s.logout(cx);
    });
    shell.read_with(&app, |s, cx| {
        assert!(s.community_workspace.is_none(), "logout must tear down CommunityWorkspace");
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.apple_sidebar.read(cx).community_unread_count, 0);
    }).unwrap();
}

/// [Test ID] TEST-CM-T2-F15-05
/// [Feature / Req] F15 / TEST_INFRA.md §6 (`OpenDmWithActor` self/invalid ID guard & unread badge `0`/`999+` boundary)
/// [Preconditions] Headless GPUI `AppShell` signed in as `8256` with mounted `CommunityWorkspace`.
/// [Opaque-Box Steps] Dispatch `OpenDmWithActor` for `0`, `-42`, and self (`8256`); verify `format_badge_count` at `0`, `999`, `1000`, `u32::MAX` and dispatch `UnreadCountChanged(0)`/`(1500)`.
/// [Observable Assertions] Invalid/self DM bridges stay safely in Community with user toast; badge formatter clamps `>999` to `"999+"` and renders cleanly.
#[test]
fn test_cm_t2_f15_05_open_dm_self_or_invalid_guard_and_unread_badge_0_999_plus() {
    run_community_gpui_test("test_cm_t2_f15_05_open_dm_self_or_invalid_guard_and_unread_badge_0_999_plus");
}

async fn body_t2_f15_05(mut app: AsyncApp) {
    assert_eq!(format_badge_count(0), "0");
    assert_eq!(format_badge_count(999), "999");
    assert_eq!(format_badge_count(1000), "999+");
    assert_eq!(format_badge_count(u32::MAX), "999+");

    let (host, _) = open_offline_community_host(&mut app, 1440.0, 900.0, 8256, "t2_f15_05", true).await;
    let shell = host.shell.clone();

    for bad_uid in [0_i64, -42_i64, 8256_i64] {
        let _ = shell.update(&mut app, |s, cx| {
            s.handle_community_action(CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: bad_uid }, cx);
        });
        assert_eq!(shell.read_with(&app, |s, _| s.active_panel).unwrap(), ActiveWorkspacePanel::Community);
        assert!(shell.read_with(&app, |s, _| s.window_runtime.controller.current_toast.is_some()).unwrap());
    }

    for count in [0u32, 1500u32] {
        let _ = shell.update(&mut app, |s, cx| {
            s.handle_community_action(CommunityWorkspaceAction::UnreadCountChanged(count), cx);
        });
        assert_eq!(shell.read_with(&app, |s, cx| s.apple_sidebar.read(cx).community_unread_count).unwrap(), count);
        host.draw(&mut app);
    }
}

// ============================================================================
// F16 — 3-Column Shell, Feed & Cards Boundaries (5 GPUI Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F16-01
/// [Feature / Req] F16 / TEST_INFRA.md §6 (Narrow viewport `640×480` & rapid `Cmd+B` sidebar collapse/expand stress)
/// [Preconditions] Headless GPUI `AppShell` with mounted `CommunityWorkspace`.
/// [Opaque-Box Steps] Render at `640×480`, `820×720`, and `1440×900` while toggling `cmd-b` 6 times across breakpoints.
/// [Observable Assertions] `last_layout_mode` resolves to `OneColumn` at narrow widths and `ThreeColumn` at 1440px with zero layout panics.
#[test]
fn test_cm_t2_f16_01_narrow_viewport_640x480_and_cmd_b_sidebar_stress() {
    run_community_gpui_test("test_cm_t2_f16_01_narrow_viewport_640x480_and_cmd_b_sidebar_stress");
}

async fn body_t2_f16_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 640.0, 480.0, 8261, "t2_f16_01", true).await;
    let cw = cw.unwrap();

    host.draw_at(&mut app, 640.0, 480.0);
    assert_eq!(cw.read_with(&app, |w, _| w.last_layout_mode).unwrap(), CommunityLayoutMode::OneColumn);

    for i in 0..6 {
        host.key(&mut app, "cmd-b");
        let (w, h, exp) = match i % 3 {
            0 => (640.0, 480.0, CommunityLayoutMode::OneColumn),
            1 => (820.0, 720.0, CommunityLayoutMode::OneColumn),
            _ => (1440.0, 900.0, CommunityLayoutMode::ThreeColumn),
        };
        host.draw_at(&mut app, w, h);
        assert_eq!(cw.read_with(&app, |ws, _| ws.last_layout_mode).unwrap(), exp);
    }
}

/// [Test ID] TEST-CM-T2-F16-02
/// [Feature / Req] F16 / TEST_INFRA.md §6 (Empty feed states for `Saved` and empty custom `List` render cleanly)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with fresh user (0 saved bookmarks).
/// [Opaque-Box Steps] Select `FeedTab::Saved` and `FeedTab::List("empty_list_t2")`, await load completion, and draw across 3/2/1-col breakpoints.
/// [Observable Assertions] Both empty tabs settle with `feed_items.is_empty()` and render empty-state UI without panic.
#[test]
fn test_cm_t2_f16_02_empty_feed_state_saved_and_empty_list_renders_cleanly() {
    run_community_gpui_test("test_cm_t2_f16_02_empty_feed_state_saved_and_empty_list_renders_cleanly");
}

async fn body_t2_f16_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8262, "t2_f16_02", true).await;
    let cw = cw.unwrap();

    for tab in [FeedTab::Saved, FeedTab::List("empty_list_t2".into())] {
        let _ = cw.update(&mut app, |w, cx| w.select_feed_tab(tab, cx));
        assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.feed_loading).await);
        assert!(cw.read_with(&app, |w, _| w.state.feed_items.is_empty()).unwrap());
        host.draw_at(&mut app, 1440.0, 900.0);
        host.draw_at(&mut app, 1040.0, 800.0);
        host.draw_at(&mut app, 640.0, 480.0);
    }
}

/// [Test ID] TEST-CM-T2-F16-03
/// [Feature / Req] F16 / TEST_INFRA.md §6 (Empty and whitespace-only Quick-Post blocked with validation feedback)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped with feed items.
/// [Opaque-Box Steps] Submit Quick-Post with `""` and `"   \n\t  "`, rendering after each attempt.
/// [Observable Assertions] `quick_post_validation_error.is_some()`, `!quick_post_submitting`, and `feed_items.len()` remains unchanged.
#[test]
fn test_cm_t2_f16_03_empty_and_whitespace_quick_post_blocked_with_validation() {
    run_community_gpui_test("test_cm_t2_f16_03_empty_and_whitespace_quick_post_blocked_with_validation");
}

async fn body_t2_f16_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8263, "t2_f16_03", true).await;
    let cw = cw.unwrap();
    let before_len = cw.read_with(&app, |w, _| w.state.feed_items.len()).unwrap();

    for bad in ["", "   \n\t  "] {
        let _ = cw.update(&mut app, |w, cx| {
            w.state.quick_post_title.clear();
            w.state.quick_post_body = bad.into();
            w.submit_quick_post(cx);
        });
        cw.read_with(&app, |w, _| {
            assert!(w.state.quick_post_validation_error.is_some(), "empty quick-post must set validation error");
            assert!(!w.state.quick_post_submitting);
            assert_eq!(w.state.feed_items.len(), before_len);
        }).unwrap();
        host.draw(&mut app);
    }
}

/// [Test ID] TEST-CM-T2-F16-04
/// [Feature / Req] F16 / TEST_INFRA.md §6 (High-volume 110-card feed renders cleanly within UI latency budget)
/// [Preconditions] Headless GPUI `CommunityWorkspace` populated with 110 mixed Human/Bot `FeedItemDto` cards.
/// [Opaque-Box Steps] Render 5 full frames at `1440×900` and measure per-frame duration, then cycle `ActorFilter`s cleanly.
/// [Observable Assertions] Every 110-card frame completes well within non-blocking UI latency budget (`< 1500ms`) with zero panics.
#[test]
fn test_cm_t2_f16_04_high_volume_110_card_feed_render_latency_budget() {
    run_community_gpui_test("test_cm_t2_f16_04_high_volume_110_card_feed_render_latency_budget");
}

async fn body_t2_f16_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8264, "t2_f16_04", true).await;
    let cw = cw.unwrap();
    let t0 = now_ms();
    let ds = build_starter_dataset(t0);

    let items: Vec<FeedItemDto> = (0..110)
        .map(|i| {
            let mut p = ds.posts[i % ds.posts.len()].clone();
            p.post_id = format!("hv_post_{i}");
            p.body = format!("High-volume card #{i}: {}", p.body);
            FeedItemDto {
                post: p,
                explanation: None,
                rank_score: 100.0 - (i as f64) * 0.1,
                is_exploration_slot: i % 11 == 0,
            }
        })
        .collect();

    let _ = cw.update(&mut app, |w, cx| {
        w.state.feed_items = items;
        cx.notify();
    });
    assert_eq!(cw.read_with(&app, |w, _| w.state.feed_items.len()).unwrap(), 110);

    for frame_idx in 0..5 {
        let start = Instant::now();
        host.draw(&mut app);
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(1500), "110-card frame {frame_idx} exceeded UI budget: {elapsed:?}");
    }

    for filter in [ActorFilter::HumansOnly, ActorFilter::BotsOnly, ActorFilter::AllCitizens] {
        let _ = cw.update(&mut app, |w, cx| w.set_actor_filter(filter, cx));
        assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.feed_loading).await);
        host.draw(&mut app);
    }
}

/// [Test ID] TEST-CM-T2-F16-05
/// [Feature / Req] F16 / TEST_INFRA.md §6 (Identity Switcher with 0 owned Bots renders Human safely & opens Bot Studio CTA)
/// [Preconditions] Headless GPUI `CommunityWorkspace` for fresh user with zero owned Bots.
/// [Opaque-Box Steps] Open `CommunityPopover::IdentitySwitcher`, draw frame, then navigate to `CommunityRoute::BotStudio` and draw.
/// [Observable Assertions] `identities.bot_actors.is_empty()` holds, switcher renders safely without empty-list panic, and Bot Studio opens cleanly.
#[test]
fn test_cm_t2_f16_05_identity_switcher_with_zero_bots_renders_human_safely() {
    run_community_gpui_test("test_cm_t2_f16_05_identity_switcher_with_zero_bots_renders_human_safely");
}

async fn body_t2_f16_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8265, "t2_f16_05", true).await;
    let cw = cw.unwrap();

    cw.read_with(&app, |w, _| {
        let ids = w.state.identities.as_ref().expect("identities loaded");
        assert!(ids.human_actor.is_some());
        assert!(ids.bot_actors.is_empty(), "fresh user must have 0 owned bots");
    }).unwrap();

    let _ = cw.update(&mut app, |w, cx| w.toggle_popover(CommunityPopover::IdentitySwitcher, cx));
    assert_eq!(cw.read_with(&app, |w, _| w.state.active_popover.clone()).unwrap(), Some(CommunityPopover::IdentitySwitcher));
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::BotStudio, cx));
    assert_eq!(cw.read_with(&app, |w, _| w.state.route.clone()).unwrap(), CommunityRoute::BotStudio);
    host.draw(&mut app);
}

// ============================================================================
// F17 — Post Detail & Summon Bot Boundaries (5 GPUI Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F17-01
/// [Feature / Req] F17 / TEST_INFRA.md §6 (Zero-comment post renders empty comment state & allows first reply)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with a freshly created post (0 comments).
/// [Opaque-Box Steps] Open `PostDetail(pid)`, draw empty comment state, then submit first inline reply via `submit_detail_reply`.
/// [Observable Assertions] Initial detail renders cleanly with `comments.is_empty()`; first reply persists (`comments.len() == 1`) and renders cleanly.
#[test]
fn test_cm_t2_f17_01_zero_comment_post_renders_empty_state_and_allows_first_reply() {
    run_community_gpui_test("test_cm_t2_f17_01_zero_comment_post_renders_empty_state_and_allows_first_reply");
}

async fn body_t2_f17_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8271, "t2_f17_01", true).await;
    let cw = cw.unwrap();
    let pid = host
        .client
        .community()
        .block_on(host.client.community().create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Fresh zero-comment post".into(),
            ..Default::default()
        }))
        .unwrap()
        .post
        .post_id;

    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(pid.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == pid && !cw.read(cx).state.detail_loading)
    }).await);
    assert!(cw.read_with(&app, |w, _| w.state.detail_post.as_ref().unwrap().comments.is_empty()).unwrap());
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| {
        w.state.detail_reply_draft = "First reply in empty thread!".into();
        w.submit_detail_reply(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.comments.len() == 1)
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F17-02
/// [Feature / Req] F17 / TEST_INFRA.md §6 (Deeply nested depth 10 comment tree renders safely on narrow & wide viewports)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with a post containing a 10-level nested reply chain (`depth 0..=9`).
/// [Opaque-Box Steps] Open `PostDetail(pid)`, verify leaf depth `== 9`, and draw at `640×480` (`OneColumn`) and `1440×900` (`ThreeColumn`).
/// [Observable Assertions] Full 10-level chain hydrates with `depth == 9` and renders cleanly without indentation overflow or panic.
#[test]
fn test_cm_t2_f17_02_deep_depth_10_comment_tree_renders_safely_narrow_and_wide() {
    run_community_gpui_test("test_cm_t2_f17_02_deep_depth_10_comment_tree_renders_safely_narrow_and_wide");
}

async fn body_t2_f17_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8272, "t2_f17_02", true).await;
    let cw = cw.unwrap();
    let comm = host.client.community();
    let pid = comm
        .block_on(comm.create_post(&CreatePostRequest { body: "Deep UI nesting post".into(), ..Default::default() }))
        .unwrap()
        .post
        .post_id;
    let mut parent = None;
    for lvl in 0..10 {
        let c = comm
            .block_on(comm.create_comment(&pid, &CreateCommentRequest { parent_comment_id: parent.clone(), body: format!("Depth {lvl} reply"), ..Default::default() }))
            .unwrap()
            .comment;
        parent = Some(c.comment_id);
    }

    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(pid.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == pid && !d.comments.is_empty())
    }).await);
    cw.read_with(&app, |w, _| {
        let mut cur = &w.state.detail_post.as_ref().unwrap().comments[0];
        let mut max_d = cur.depth;
        while let Some(child) = cur.children.first() {
            cur = child;
            max_d = cur.depth;
        }
        assert_eq!(max_d, 9, "leaf comment must reach depth 9");
    }).unwrap();
    host.draw_at(&mut app, 640.0, 480.0);
    host.draw_at(&mut app, 1440.0, 900.0);
}

/// [Test ID] TEST-CM-T2-F17-03
/// [Feature / Req] F17 / TEST_INFRA.md §6 (Locked post disables inline reply & emits error toast)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with owned `c/lock_ui_f17` and locked post `pid`.
/// [Opaque-Box Steps] Lock `pid`, open `PostDetail(pid)`, fill `detail_reply_draft`, and call `submit_detail_reply`.
/// [Observable Assertions] Reply is blocked client-side emitting `ShowToast { is_error: true, .. }`, 0 comments added, and locked banner renders cleanly.
#[test]
fn test_cm_t2_f17_03_locked_post_reply_disabled_and_emits_error_toast() {
    run_community_gpui_test("test_cm_t2_f17_03_locked_post_reply_disabled_and_emits_error_toast");
}

async fn body_t2_f17_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8273, "t2_f17_03", true).await;
    let cw = cw.unwrap();
    let comm = host.client.community();
    let _ = comm
        .block_on(comm.create_community(&CreateCommunityRequest {
            slug: "lock_ui_f17".into(), name: "Lock UI".into(), description: String::new(),
            category: "tech".into(), banner_media_id: None, icon_media_id: None, icon_emoji: None,
            accent_color: None, rules: vec![], post_flairs: vec![], user_flairs: vec![],
            wiki_markdown: String::new(), automod_rules: vec![], allow_bots: true,
            require_bot_disclosure: true, is_nsfw: false,
        }))
        .unwrap();
    let pid = comm
        .block_on(comm.create_post(&CreatePostRequest { community_slug: Some("lock_ui_f17".into()), body: "Locked discussion".into(), ..Default::default() }))
        .unwrap()
        .post
        .post_id;
    let _ = comm
        .block_on(comm.execute_mod_action("lock_ui_f17", &ModActionRequest {
            action: ModActionKind::Lock, target_kind: "post".into(), target_id: pid.clone(), reason: Some("Closed".into()), flair: None, queue_item_id: None,
        }))
        .unwrap();

    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(pid.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == pid && d.post.is_locked)
    }).await);

    let _ = cw.update(&mut app, |w, cx| {
        let _ = w.take_pending_actions();
        w.state.detail_reply_draft = "Attempted reply on locked post".into();
        w.submit_detail_reply(cx);
        let acts = w.take_pending_actions();
        assert!(acts.iter().any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. })), "locked reply must emit error toast");
        assert!(w.state.detail_post.as_ref().unwrap().comments.is_empty());
    });
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F17-04
/// [Feature / Req] F17 / TEST_INFRA.md §6 (Collapsing & re-expanding branch preserves siblings and total comment count)
/// [Preconditions] Headless GPUI `CommunityWorkspace` viewing a post with 2 root branches (`r1 -> c1 -> c2` and `r2`).
/// [Opaque-Box Steps] Collapse `r1`, draw, verify `r2` remains uncollapsed and `total_comments == 4`, then re-expand `r1` and draw.
/// [Observable Assertions] `detail_collapsed_comments` tracks `r1` independently without affecting sibling `r2` or corrupting `total_comments`.
#[test]
fn test_cm_t2_f17_04_collapse_and_reexpand_branch_preserves_siblings_and_count() {
    run_community_gpui_test("test_cm_t2_f17_04_collapse_and_reexpand_branch_preserves_siblings_and_count");
}

async fn body_t2_f17_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8274, "t2_f17_04", true).await;
    let cw = cw.unwrap();
    let comm = host.client.community();
    let pid = comm.block_on(comm.create_post(&CreatePostRequest { body: "Collapse test post".into(), ..Default::default() })).unwrap().post.post_id;
    let r1 = comm.block_on(comm.create_comment(&pid, &CreateCommentRequest { body: "Root 1".into(), ..Default::default() })).unwrap().comment.comment_id;
    let c1 = comm.block_on(comm.create_comment(&pid, &CreateCommentRequest { parent_comment_id: Some(r1.clone()), body: "Child 1".into(), ..Default::default() })).unwrap().comment.comment_id;
    let _c2 = comm.block_on(comm.create_comment(&pid, &CreateCommentRequest { parent_comment_id: Some(c1), body: "Grandchild 1".into(), ..Default::default() })).unwrap().comment.comment_id;
    let r2 = comm.block_on(comm.create_comment(&pid, &CreateCommentRequest { body: "Root 2 sibling".into(), ..Default::default() })).unwrap().comment.comment_id;

    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(pid.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.total_comments == 4 && d.comments.len() == 2)
    }).await);

    let _ = cw.update(&mut app, |w, cx| w.toggle_comment_collapsed(r1.clone(), cx));
    cw.read_with(&app, |w, _| {
        assert!(w.state.detail_collapsed_comments.contains(&r1));
        assert!(!w.state.detail_collapsed_comments.contains(&r2));
        assert_eq!(w.state.detail_post.as_ref().unwrap().total_comments, 4);
    }).unwrap();
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| w.toggle_comment_collapsed(r1.clone(), cx));
    cw.read_with(&app, |w, _| {
        assert!(!w.state.detail_collapsed_comments.contains(&r1));
        assert_eq!(w.state.detail_post.as_ref().unwrap().total_comments, 4);
    }).unwrap();
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F17-05
/// [Feature / Req] F17 / TEST_INFRA.md §6 (Summon Bot blocked in `allow_bots == false` community and while summon is already pending)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with owned `c/nobots_ui_f17` (`allow_bots: false`) and a published post.
/// [Opaque-Box Steps] Attempt `summon_bot_with_options` on `c/nobots_ui_f17` post; also set `detail_summon_pending = true` on a `c/rust` post and invoke summon again.
/// [Observable Assertions] `allow_bots == false` immediately sets error feedback & emits error toast with 0 bot comments; duplicate pending call is ignored cleanly.
#[test]
fn test_cm_t2_f17_05_summon_bot_when_pending_or_in_allow_bots_false_blocked() {
    run_community_gpui_test("test_cm_t2_f17_05_summon_bot_when_pending_or_in_allow_bots_false_blocked");
}

async fn body_t2_f17_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8275, "t2_f17_05", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "nobots_ui_f17".into();
        w.state.create_community.name = "NoBots UI F17".into();
        w.state.create_community.allow_bots = false;
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.community_detail.as_ref().is_some_and(|d| d.community.slug == "nobots_ui_f17" && !d.community.allow_bots)
    }).await);

    let pid = host
        .client
        .community()
        .block_on(host.client.community().create_post(&CreatePostRequest {
            community_slug: Some("nobots_ui_f17".into()),
            body: "Human-only post".into(),
            ..Default::default()
        }))
        .unwrap()
        .post
        .post_id;
    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(pid.clone(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.detail_post.as_ref().is_some_and(|d| d.post.post_id == pid)
    }).await);

    let _ = cw.update(&mut app, |w, cx| {
        let _ = w.take_pending_actions();
        w.summon_bot_with_options(pid.clone(), Some("b:910001".into()), Some("Hello".into()), None, cx);
        assert!(!w.state.detail_summon_pending);
        assert!(matches!(w.state.detail_summon_feedback, Some((_, true))));
        assert!(w.take_pending_actions().iter().any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. })));

        // Re-entrancy guard when already pending prevents duplicate submission and shows clear feedback
        w.state.detail_summon_pending = true;
        w.state.detail_summon_feedback = None;
        let _ = w.take_pending_actions();
        w.summon_bot_with_options(pid, Some("b:910001".into()), Some("Dup".into()), None, cx);
        assert!(w.state.detail_summon_pending, "in-flight guard must keep pending state intact");
        assert!(
            matches!(w.state.detail_summon_feedback, Some((_, false))),
            "in-flight guard must surface clear non-error in-progress feedback"
        );
        assert!(w.take_pending_actions().iter().any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: false, .. })));
        assert_eq!(w.state.detail_post.as_ref().unwrap().total_comments, 0, "duplicate summon must not add comments");
    });
    host.draw(&mut app);
}

// ============================================================================
// F18 — Rich Composer, Drafts & Scheduling Boundaries (5 GPUI Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F18-01
/// [Feature / Req] F18 / TEST_INFRA.md §6 (Composer rejects <2 poll options, invalid link URL & empty thread segment)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with `CommunityModal::Composer` open.
/// [Opaque-Box Steps] Attempt submitting invalid Poll (<2 options), invalid Link URL (`"not_a_url"`), and Thread with blank segment (`"  "`).
/// [Observable Assertions] Every invalid format sets `composer.validation_error.is_some()`, keeps `active_modal == Some(Composer)`, and draws cleanly.
#[test]
fn test_cm_t2_f18_01_composer_empty_poll_invalid_link_and_empty_thread_validation() {
    run_community_gpui_test("test_cm_t2_f18_01_composer_empty_poll_invalid_link_and_empty_thread_validation");
}

async fn body_t2_f18_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8281, "t2_f18_01", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);

        w.set_composer_tab(ComposerFormatTab::Poll, cx);
        w.state.composer.poll_question = "Only one option?".into();
        w.state.composer.poll_options = vec!["Solo".into(), "   ".into()];
        w.submit_composer(PostStatus::Published, cx);
        assert!(w.state.composer.validation_error.is_some());
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));

        w.set_composer_tab(ComposerFormatTab::Link, cx);
        w.state.composer.validation_error = None;
        w.state.composer.link_url = "not_a_valid_url".into();
        w.submit_composer(PostStatus::Published, cx);
        assert!(w.state.composer.validation_error.is_some());
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));

        w.set_composer_tab(ComposerFormatTab::Thread, cx);
        w.state.composer.validation_error = None;
        w.state.composer.body = "Part 1".into();
        w.state.composer.thread_parts = vec!["   ".into()];
        w.submit_composer(PostStatus::Published, cx);
        assert!(w.state.composer.validation_error.is_some());
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));
    });
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F18-02
/// [Feature / Req] F18 / TEST_INFRA.md §6 (Multiline Markdown paste preserves newlines & strips control chars; single-line flattens)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with `ComposerModal` open on `Article` tab.
/// [Opaque-Box Steps] Verify `sanitize_paste_text`, insert CRLF + NUL + BEL text into multiline `ComposerBody`, and insert multiline + BEL text into single-line `ComposerTitle`.
/// [Observable Assertions] `ComposerBody` normalizes CRLF to LF and strips control bytes; `ComposerTitle` flattens newlines to spaces and strips control bytes.
#[test]
fn test_cm_t2_f18_02_multiline_paste_preserves_newlines_and_strips_control_chars() {
    run_community_gpui_test("test_cm_t2_f18_02_multiline_paste_preserves_newlines_and_strips_control_chars");
}

async fn body_t2_f18_02(mut app: AsyncApp) {
    assert_eq!(sanitize_paste_text("# Header\r\nLine 2\x00\x07\nLine 3", true), "# Header\nLine 2\nLine 3");
    assert_eq!(sanitize_paste_text("A\r\nB\x07", false), "AB");

    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8282, "t2_f18_02", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.set_composer_tab(ComposerFormatTab::Article, cx);
        w.state.focus_input(ActiveCommunityInput::ComposerBody);
        w.insert_text_to_active_input("# Heading\r\nPara 1\x00\x07\nPara 2", cx);
        assert_eq!(w.state.composer.body, "# Heading\nPara 1\nPara 2");

        w.state.focus_input(ActiveCommunityInput::ComposerTitle);
        w.insert_text_to_active_input("Title1\r\nTitle2\x07", cx);
        assert_eq!(w.state.composer.title, "Title1 Title2");
    });
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F18-03
/// [Feature / Req] F18 / TEST_INFRA.md §6 (Composer rejects past, zero, or non-numeric scheduled timestamps)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with `ComposerModal` open and body populated.
/// [Opaque-Box Steps] Attempt `PostStatus::Scheduled` submission with `schedule_minutes_input` set to `"-15"`, `"0"`, and `"abc"`.
/// [Observable Assertions] Both `build_composer_create_request` (`Err`) and `submit_composer` reject every invalid schedule keeping modal open with `validation_error`.
#[test]
fn test_cm_t2_f18_03_composer_past_or_zero_schedule_rejected() {
    run_community_gpui_test("test_cm_t2_f18_03_composer_past_or_zero_schedule_rejected");
}

async fn body_t2_f18_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8283, "t2_f18_03", true).await;
    let cw = cw.unwrap();
    let t0 = now_ms();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.state.composer.body = "Scheduled boundary test".into();
        w.state.composer.schedule_enabled = true;

        for bad_mins in ["-15", "0", "abc"] {
            w.state.composer.validation_error = None;
            w.state.composer.schedule_minutes_input = bad_mins.into();
            assert!(w.build_composer_create_request(PostStatus::Scheduled, t0).is_err(), "schedule '{bad_mins}' must fail");
            w.submit_composer(PostStatus::Scheduled, cx);
            assert!(w.state.composer.validation_error.is_some());
            assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));
        }
    });
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F18-04
/// [Feature / Req] F18 / TEST_INFRA.md §6 (Switching across all 8 Composer format tabs preserves shared draft state)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with `ComposerModal` populated with title, body, community, flair, and spoiler flag.
/// [Opaque-Box Steps] Cycle `set_composer_tab` across all 8 `ComposerFormatTab` variants and draw each tab.
/// [Observable Assertions] Shared title, body, `community_slug`, flair, `is_spoiler`, and active identity remain intact across every tab transition.
#[test]
fn test_cm_t2_f18_04_switching_composer_format_tabs_preserves_shared_state() {
    run_community_gpui_test("test_cm_t2_f18_04_switching_composer_format_tabs_preserves_shared_state");
}

async fn body_t2_f18_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8284, "t2_f18_04", true).await;
    let cw = cw.unwrap();

    let initial_actor = cw.read_with(&app, |w, _| w.state.active_actor_id.clone()).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.state.composer.title = "Preserved Title".into();
        w.state.composer.body = "Preserved Body".into();
        w.state.composer.flair = Some("RFC".into());
        w.state.composer.is_spoiler = true;
    });

    for tab in [
        ComposerFormatTab::Micro,
        ComposerFormatTab::Article,
        ComposerFormatTab::Media,
        ComposerFormatTab::Link,
        ComposerFormatTab::Poll,
        ComposerFormatTab::Thread,
        ComposerFormatTab::Crosspost,
        ComposerFormatTab::Quote,
        ComposerFormatTab::Article,
    ] {
        let _ = cw.update(&mut app, |w, cx| w.set_composer_tab(tab, cx));
        host.draw(&mut app);
        cw.read_with(&app, |w, _| {
            assert_eq!(w.state.composer.title, "Preserved Title");
            assert_eq!(w.state.composer.body, "Preserved Body");
            assert_eq!(w.state.composer.community_slug.as_deref(), Some("rust"));
            assert_eq!(w.state.composer.flair.as_deref(), Some("RFC"));
            assert!(w.state.composer.is_spoiler);
            assert_eq!(w.state.active_actor_id, initial_actor);
        }).unwrap();
    }
}

/// [Test ID] TEST-CM-T2-F18-05
/// [Feature / Req] F18 / TEST_INFRA.md §6 (CJK IME inside Composer Title/Body: Escape cancels preedit only & 80ms Enter fence)
/// [Preconditions] Headless GPUI `AppShell` with `ComposerModal` open on `Article` tab.
/// [Opaque-Box Steps] Compose preedit in `ComposerTitle` and press Escape; commit `"标题一"` and press immediate Enter (<80ms); focus `ComposerBody`, commit `"段落"`, press immediate Enter, wait 95ms, press Enter.
/// [Observable Assertions] Escape clears preedit without closing modal; 80ms fence prevents premature submit in Title and prevents stray newline in Body; Enter after 95ms inserts newline in Body.
#[test]
fn test_cm_t2_f18_05_cjk_ime_inside_composer_title_body_escape_and_80ms_fence() {
    run_community_gpui_test("test_cm_t2_f18_05_cjk_ime_inside_composer_title_body_escape_and_80ms_fence");
}

async fn body_t2_f18_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8285, "t2_f18_05", true).await;
    let shell = host.shell.clone();
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".into()), cx);
        w.set_composer_tab(ComposerFormatTab::Article, cx);
        w.state.composer.body = "正文".into();
        w.state.focus_input(ActiveCommunityInput::ComposerTitle);
    });
    let _ = shell.update(&mut app, |s, cx| s.update_ime_marked_text("测试标题", cx));
    host.key(&mut app, "escape");
    assert!(!shell.read_with(&app, |s, cx| s.has_active_ime_preedit(cx)).unwrap());
    assert_eq!(cw.read_with(&app, |w, _| w.state.active_modal).unwrap(), Some(CommunityModal::Composer));

    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("标题一", cx);
        s.commit_ime_unmark_text(cx);
        assert!(s.handle_non_char_key("enter", false, cx));
    });
    cw.read_with(&app, |w, _| {
        assert_eq!(w.state.composer.title, "标题一");
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));
        assert!(!w.state.composer.submitting);
    }).unwrap();

    let _ = cw.update(&mut app, |w, _| w.state.focus_input(ActiveCommunityInput::ComposerBody));
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("段落", cx);
        s.commit_ime_unmark_text(cx);
        assert!(s.handle_non_char_key("enter", false, cx));
    });
    assert_eq!(cw.read_with(&app, |w, _| w.state.composer.body.clone()).unwrap(), "正文段落");

    app.background_executor().timer(Duration::from_millis(95)).await;
    let _ = shell.update(&mut app, |s, cx| assert!(s.handle_non_char_key("enter", false, cx)));
    assert_eq!(cw.read_with(&app, |w, _| w.state.composer.body.clone()).unwrap(), "正文段落\n");
    host.draw(&mut app);
}

// ============================================================================
// F19 — Community (`c/`) Page & Wiki Boundaries (5 GPUI Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F19-01
/// [Feature / Req] F19 / TEST_INFRA.md §6 (Navigating to nonexistent `c/does_not_exist_999` renders graceful not-found fallback)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped.
/// [Opaque-Box Steps] Navigate to `CommunityRoute::CommunityPage("does_not_exist_999")`, await `!community_loading`, and draw full frame.
/// [Observable Assertions] `community_detail.is_none()`, `community_error.is_some()`, and view renders cleanly without crash.
#[test]
fn test_cm_t2_f19_01_nonexistent_community_page_renders_graceful_not_found() {
    run_community_gpui_test("test_cm_t2_f19_01_nonexistent_community_page_renders_graceful_not_found");
}

async fn body_t2_f19_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8291, "t2_f19_01", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::CommunityPage("does_not_exist_999".into()), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.community_loading && cw.read(cx).state.community_error.is_some()).await);
    assert!(cw.read_with(&app, |w, _| w.state.community_detail.is_none()).unwrap());
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F19-02
/// [Feature / Req] F19 / TEST_INFRA.md §6 (Create Community modal rejects invalid & duplicate slugs while preserving entered fields)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with seeded `c/rust`.
/// [Opaque-Box Steps] Open `CreateCommunity` modal, submit invalid slug `"bad slug!"`, then submit duplicate slug `"rust"`.
/// [Observable Assertions] Both attempts set `create_community.error.is_some()`, keep modal open, and preserve `name` and `description`.
#[test]
fn test_cm_t2_f19_02_create_community_modal_rejects_invalid_and_duplicate_slug() {
    run_community_gpui_test("test_cm_t2_f19_02_create_community_modal_rejects_invalid_and_duplicate_slug");
}

async fn body_t2_f19_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8292, "t2_f19_02", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.name = "Keep My Title".into();
        w.state.create_community.description = "Keep My Description".into();
        w.state.create_community.slug = "bad slug!".into();
        w.submit_create_community(cx);
        assert!(w.state.create_community.error.is_some());
        assert_eq!(w.state.active_modal, Some(CommunityModal::CreateCommunity));
        assert_eq!(w.state.create_community.name, "Keep My Title");
        assert_eq!(w.state.create_community.description, "Keep My Description");
    });
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| {
        w.state.create_community.slug = "rust".into();
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.create_community.submitting && cw.read(cx).state.create_community.error.is_some()
    }).await);
    cw.read_with(&app, |w, _| {
        assert_eq!(w.state.active_modal, Some(CommunityModal::CreateCommunity));
        assert_eq!(w.state.create_community.name, "Keep My Title");
        assert_eq!(w.state.create_community.description, "Keep My Description");
    }).unwrap();
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F19-03
/// [Feature / Req] F19 / TEST_INFRA.md §6 (Non-moderator viewing `c/rust` Wiki stays read-only & edit attempts rejected)
/// [Preconditions] Headless GPUI `CommunityWorkspace` for fresh non-mod user `8293` viewing seeded `c/rust`.
/// [Opaque-Box Steps] Switch to `CommunityPageTab::Wiki`, invoke `start_edit_community_wiki` and `save_community_wiki("rust")`.
/// [Observable Assertions] `community_wiki_editing` stays `false`, both calls emit `ShowToast { is_error: true, .. }`, and `wiki_revision` stays unchanged.
#[test]
fn test_cm_t2_f19_03_non_moderator_wiki_read_only_and_edit_rejected() {
    run_community_gpui_test("test_cm_t2_f19_03_non_moderator_wiki_read_only_and_edit_rejected");
}

async fn body_t2_f19_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8293, "t2_f19_03", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::CommunityPage("rust".into()), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.community_detail.as_ref().is_some_and(|d| d.community.slug == "rust")
    }).await);
    let orig_rev = cw.read_with(&app, |w, _| w.state.community_detail.as_ref().unwrap().community.wiki_revision).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.set_community_tab(CommunityPageTab::Wiki, cx);
        let _ = w.take_pending_actions();
        w.start_edit_community_wiki(Some("# Hijacked".into()), cx);
        assert!(!w.state.community_wiki_editing, "non-mod must not enter wiki edit mode");
        assert!(w.take_pending_actions().iter().any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. })));

        w.save_community_wiki("rust".into(), cx);
        assert!(w.take_pending_actions().iter().any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. })));
        assert_eq!(w.state.community_detail.as_ref().unwrap().community.wiki_revision, orig_rev);
    });
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F19-04
/// [Feature / Req] F19 / TEST_INFRA.md §6 (Flair filter with 0 matches renders empty state & clears cleanly back to full stream)
/// [Preconditions] Headless GPUI `CommunityWorkspace` viewing seeded `c/rust` with existing posts.
/// [Opaque-Box Steps] Apply nonexistent flair `"NoSuchFlair_XYZ"`, await reload & draw, then clear flair filter (`None`) and await reload.
/// [Observable Assertions] Unmatched flair yields empty `community_posts` without crash; clearing restores non-empty community feed.
#[test]
fn test_cm_t2_f19_04_flair_filter_zero_matches_renders_empty_and_clears_back() {
    run_community_gpui_test("test_cm_t2_f19_04_flair_filter_zero_matches_renders_empty_and_clears_back");
}

async fn body_t2_f19_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8294, "t2_f19_04", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::CommunityPage("rust".into()), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.community_loading && !cw.read(cx).state.community_posts.is_empty()
    }).await);

    let _ = cw.update(&mut app, |w, cx| w.set_community_flair_filter("rust".into(), Some("NoSuchFlair_XYZ".into()), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.community_loading && cw.read(cx).state.community_posts.is_empty()).await);
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| w.set_community_flair_filter("rust".into(), None, cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.community_loading && !cw.read(cx).state.community_posts.is_empty()).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F19-05
/// [Feature / Req] F19 / TEST_INFRA.md §6 (`allow_bots == false` community blocks Bot Act-As composer submission with clear notice)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with owned `c/human_only_f19` (`allow_bots: false`) and registered owned Bot.
/// [Opaque-Box Steps] Switch active identity to owned Bot, open `ComposerModal` targeting `c/human_only_f19`, and attempt `submit_composer`.
/// [Observable Assertions] `build_composer_create_request` returns `Err` citing `allow_bots`, `composer.validation_error.is_some()`, and modal stays open without posting.
#[test]
fn test_cm_t2_f19_05_allow_bots_false_community_blocks_bot_act_as_composer() {
    run_community_gpui_test("test_cm_t2_f19_05_allow_bots_false_community_blocks_bot_act_as_composer");
}

async fn body_t2_f19_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8295, "t2_f19_05", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.state.bot_studio_name = "Guard F19 Bot".into();
        w.state.bot_studio_handle = "guard_f19_bot".into();
        w.submit_register_bot(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.identities.as_ref().is_some_and(|i| i.bot_actors.iter().any(|b| b.handle == "guard_f19_bot"))
    }).await);
    let bot_id = cw.read_with(&app, |w, _| {
        w.state.identities.as_ref().unwrap().bot_actors.iter().find(|b| b.handle == "guard_f19_bot").unwrap().actor_id.clone()
    }).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "human_only_f19".into();
        w.state.create_community.name = "Human Sanctuary".into();
        w.state.create_community.allow_bots = false;
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.community_detail.as_ref().is_some_and(|d| d.community.slug == "human_only_f19" && !d.community.allow_bots)
    }).await);

    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(bot_id, cx);
        w.open_composer_modal(Some("human_only_f19".into()), cx);
        w.state.composer.body = "Bot attempting to post in human-only hub".into();
        let err = w.build_composer_create_request(PostStatus::Published, now_ms()).expect_err("must reject bot in allow_bots=false");
        assert!(err.contains("allow_bots"), "expected allow_bots notice, got: {err}");
        w.submit_composer(PostStatus::Published, cx);
        assert!(w.state.composer.validation_error.is_some());
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));
    });
    host.draw(&mut app);
}

// ============================================================================
// F20 — Profiles, Bookmarks, Lists & Notifications Boundaries (5 GPUI Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F20-01
/// [Feature / Req] F20 / TEST_INFRA.md §6 (Opening nonexistent `@actor` profile renders graceful not-found state)
/// [Preconditions] Headless GPUI `CommunityWorkspace` bootstrapped.
/// [Opaque-Box Steps] Call `open_actor_profile("missing_citizen_xyz_999")`, await `!profile_loading`, and render frame.
/// [Observable Assertions] `actor_profile.is_none()`, `profile_error.is_some()`, and not-found view draws without crash.
#[test]
fn test_cm_t2_f20_01_nonexistent_actor_profile_renders_graceful_not_found() {
    run_community_gpui_test("test_cm_t2_f20_01_nonexistent_actor_profile_renders_graceful_not_found");
}

async fn body_t2_f20_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8301, "t2_f20_01", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| w.open_actor_profile("missing_citizen_xyz_999".into(), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.profile_loading && cw.read(cx).state.profile_error.is_some()).await);
    assert!(cw.read_with(&app, |w, _| w.state.actor_profile.is_none()).unwrap());
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F20-02
/// [Feature / Req] F20 / TEST_INFRA.md §6 (Bookmark idempotency & deleting custom folder preserves saved posts in `"All"`)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with feed posts loaded.
/// [Opaque-Box Steps] Create `"TempFolder"`, save `pid` into `"TempFolder"`, load Bookmarks view, delete `"TempFolder"`, then remove bookmark twice.
/// [Observable Assertions] Deleting `"TempFolder"` reassigns `pid` to `"All"` preserving `viewer_bookmarked == true`; repeated removal is idempotent with zero panic.
#[test]
fn test_cm_t2_f20_02_bookmark_idempotency_and_delete_folder_preserves_posts() {
    run_community_gpui_test("test_cm_t2_f20_02_bookmark_idempotency_and_delete_folder_preserves_posts");
}

async fn body_t2_f20_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8302, "t2_f20_02", true).await;
    let cw = cw.unwrap();
    let (pid, before_bm_count) = cw
        .read_with(&app, |w, _| {
            let p = &w.state.feed_items[0].post;
            (p.post_id.clone(), p.bookmarks_count)
        })
        .unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.state.bookmark_new_folder_draft = "TempFolder".into();
        w.create_bookmark_folder(cx);
        w.move_bookmark_to_folder(pid.clone(), "TempFolder".into(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.find_post(&pid).is_some_and(|p| {
            p.viewer_bookmarked
                && p.viewer_bookmark_folder.as_deref() == Some("TempFolder")
                && p.bookmarks_count > before_bm_count
        })
    }).await);

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::BookmarksLists, cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.saved_items.iter().any(|i| i.post.post_id == pid)
    }).await);

    let _ = cw.update(&mut app, |w, cx| w.delete_bookmark_folder("TempFolder".into(), cx));
    cw.read_with(&app, |w, _| {
        assert!(!w.state.bookmark_custom_folders.iter().any(|f| f == "TempFolder"));
        let saved = w.state.saved_items.iter().find(|i| i.post.post_id == pid).expect("post must be preserved on folder delete");
        assert!(saved.post.viewer_bookmarked);
        assert_eq!(saved.post.viewer_bookmark_folder.as_deref(), Some("All"));
    }).unwrap();
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| {
        w.remove_bookmark_from_saved(pid.clone(), cx);
        w.remove_bookmark_from_saved(pid.clone(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.find_post(&pid).is_some_and(|p| !p.viewer_bookmarked)
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F20-03
/// [Feature / Req] F20 / TEST_INFRA.md §6 (Empty list name rejected; removing last member/community renders empty list safely)
/// [Preconditions] Headless GPUI `CommunityWorkspace` navigated to `BookmarksLists` -> `Lists`.
/// [Opaque-Box Steps] Submit whitespace list name, then create `"Solo List"` with `c/rust` and toggle `c/rust` off to empty the list.
/// [Observable Assertions] Whitespace name sets `list_editor.error.is_some()` keeping editor open; emptied list persists cleanly and renders without crash.
#[test]
fn test_cm_t2_f20_03_empty_list_name_rejected_and_emptying_list_renders_safely() {
    run_community_gpui_test("test_cm_t2_f20_03_empty_list_name_rejected_and_emptying_list_renders_safely");
}

async fn body_t2_f20_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8303, "t2_f20_03", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
        w.open_create_list_form(cx);
        w.state.list_editor.name = "   ".into();
        w.submit_list_editor(cx);
        assert!(w.state.list_editor.error.is_some());
        assert!(w.state.list_editor_open);

        w.state.list_editor.name = "Solo List".into();
        w.state.list_editor.community_slugs = vec!["rust".into()];
        w.submit_list_editor(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.lists.iter().any(|l| l.name == "Solo List")
    }).await);
    let list_id = cw.read_with(&app, |w, _| w.state.lists.iter().find(|l| l.name == "Solo List").unwrap().list_id.clone()).unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.select_list_for_preview(list_id.clone(), cx);
        w.toggle_selected_list_community(list_id.clone(), "rust".into(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        cw.read(cx).state.lists.iter().find(|l| l.list_id == list_id).is_some_and(|l| l.community_slugs.is_empty() && l.member_actor_ids.is_empty())
    }).await);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F20-04
/// [Feature / Req] F20 / TEST_INFRA.md §6 (Notifications view with 0 unread handles repeated `mark_all_notifications_read` cleanly)
/// [Preconditions] Headless GPUI `CommunityWorkspace` navigated to `CommunityRoute::Notifications`.
/// [Opaque-Box Steps] Call `mark_all_notifications_read` to settle at 0 unread, then invoke `mark_all_notifications_read` a second time.
/// [Observable Assertions] `unread_notifications == 0` stays zero with no underflow or error and renders cleanly.
#[test]
fn test_cm_t2_f20_04_notifications_zero_unread_mark_all_read_clean_noop() {
    run_community_gpui_test("test_cm_t2_f20_04_notifications_zero_unread_mark_all_read_clean_noop");
}

async fn body_t2_f20_04(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8304, "t2_f20_04", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Notifications, cx);
        w.mark_all_notifications_read(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| cw.read(cx).state.unread_notifications == 0).await);

    let _ = cw.update(&mut app, |w, cx| w.mark_all_notifications_read(cx));
    app.background_executor().timer(Duration::from_millis(80)).await;
    assert_eq!(cw.read_with(&app, |w, _| w.state.unread_notifications).unwrap(), 0);
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F20-05
/// [Feature / Req] F20 / TEST_INFRA.md §6 (Notification jump to deleted target post shows graceful fallback & error toast)
/// [Preconditions] Headless GPUI `AppShell` + `CommunityWorkspace` with a post created then deleted.
/// [Opaque-Box Steps] Invoke `open_notification_target` targeting the deleted `post_id` and `target_comment_id`, await `!detail_loading`, and draw.
/// [Observable Assertions] `detail_post.is_none()`, `detail_error.is_some()`, shell displays user toast without crash, and fallback renders cleanly.
#[test]
fn test_cm_t2_f20_05_notification_jump_to_deleted_post_graceful_toast_fallback() {
    run_community_gpui_test("test_cm_t2_f20_05_notification_jump_to_deleted_post_graceful_toast_fallback");
}

async fn body_t2_f20_05(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8305, "t2_f20_05", true).await;
    let shell = host.shell.clone();
    let cw = cw.unwrap();
    let comm = host.client.community();

    let pid = comm.block_on(comm.create_post(&CreatePostRequest { body: "Soon deleted post".into(), ..Default::default() })).unwrap().post.post_id;
    let _ = comm.block_on(comm.delete_post(&pid)).unwrap();
    let own_id = cw.read_with(&app, |w, _| w.state.active_actor_id.clone()).unwrap();

    let notif = CommunityNotificationDto {
        notif_id: "n_del_target".into(),
        recipient_actor_id: own_id,
        actor_id: "b:910001".into(),
        actor_handle: "ferris_bot".into(),
        actor_display_name: "Ferris Bot".into(),
        actor_kind: ActorKind::Bot,
        kind: CommunityNotificationKind::BotReply,
        target_post_id: Some(pid),
        target_comment_id: Some("c_deleted_1".into()),
        preview_text: "Bot replied on deleted post".into(),
        is_read: false,
        created_at_ms: now_ms(),
    };

    let _ = cw.update(&mut app, |w, cx| w.open_notification_target(notif, cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| {
        !cw.read(cx).state.detail_loading && cw.read(cx).state.detail_error.is_some()
    }).await);
    assert!(cw.read_with(&app, |w, _| w.state.detail_post.is_none()).unwrap());
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let ctrl = &shell.read(cx).window_runtime.controller;
            ctrl.current_toast.is_some() || !ctrl.toast_queue.is_empty()
        })
        .await
    );
    host.draw(&mut app);
}

// ============================================================================
// F21 — Explore/Search, Mod Console & Bot Studio Boundaries (5 GPUI Tests)
// ============================================================================

/// [Test ID] TEST-CM-T2-F21-01
/// [Feature / Req] F21 / TEST_INFRA.md §6 (Search empty, punctuation-only & zero-match queries render empty state cleanly)
/// [Preconditions] Headless GPUI `CommunityWorkspace` navigated to `CommunityRoute::Explore`.
/// [Opaque-Box Steps] Execute searches for `""`, `"!!! ??? ((("`, and `"zxqvbnm_nonexistent_token_99999"`, awaiting completion and drawing each.
/// [Observable Assertions] Every query settles (`!search_loading`) with `search_result.is_some()`, zero-match returns `posts.is_empty()`, and renders cleanly.
#[test]
fn test_cm_t2_f21_01_search_empty_punctuation_and_zero_match_renders_cleanly() {
    run_community_gpui_test("test_cm_t2_f21_01_search_empty_punctuation_and_zero_match_renders_cleanly");
}

async fn body_t2_f21_01(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8311, "t2_f21_01", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::Explore { initial_query: None }, cx));

    for q in ["", "!!! ??? (((", "zxqvbnm_nonexistent_token_99999"] {
        let _ = cw.update(&mut app, |w, cx| w.execute_search(q.into(), CommunitySearchTab::Top, cx));
        assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.search_loading && cw.read(cx).state.search_result.is_some()).await);
        if q.contains("nonexistent") {
            assert!(cw.read_with(&app, |w, _| w.state.search_result.as_ref().unwrap().posts.is_empty()).unwrap());
        }
        host.draw(&mut app);
    }
}

/// [Test ID] TEST-CM-T2-F21-02
/// [Feature / Req] F21 / TEST_INFRA.md §6 (Non-moderator opening Mod Console gets unauthorized guard & blocked mod actions)
/// [Preconditions] Headless GPUI `CommunityWorkspace` for fresh non-mod user `8312`.
/// [Opaque-Box Steps] Open `ModQueue(None)` and `ModQueue(Some("rust"))`, then invoke `execute_mod_console_action` targeting `seed_post_01`.
/// [Observable Assertions] `moderated_communities().is_empty()`, `!can_moderate_active_mod_community()`, and unauthorized action emits `ShowToast { is_error: true, .. }`.
#[test]
fn test_cm_t2_f21_02_non_mod_opening_mod_console_gets_unauthorized_guard() {
    run_community_gpui_test("test_cm_t2_f21_02_non_mod_opening_mod_console_gets_unauthorized_guard");
}

async fn body_t2_f21_02(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8312, "t2_f21_02", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(None), cx);
        assert!(w.moderated_communities().is_empty(), "fresh user moderates 0 communities");
        assert!(!w.can_moderate_active_mod_community());
    });
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::ModQueue(Some("rust".into())), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.mod_loading).await);
    let _ = cw.update(&mut app, |w, cx| {
        assert!(!w.can_moderate_active_mod_community());
        let _ = w.take_pending_actions();
        w.execute_mod_console_action("rust".into(), ModActionKind::Lock, "post".into(), "seed_post_01".into(), None, cx);
        assert!(w.take_pending_actions().iter().any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. })));
    });
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F21-03
/// [Feature / Req] F21 / TEST_INFRA.md §6 (AutoMod rule editor rejects empty rule name & malformed regex syntax inline)
/// [Preconditions] Headless GPUI `CommunityWorkspace` with owned `c/am_err_f21` opened on `ModConsoleTab::AutoModRules`.
/// [Opaque-Box Steps] Submit empty rule name `""`, then submit valid name `"Bad Regex"` with invalid regex `"[unclosed("`.
/// [Observable Assertions] Both attempts set `automod_draft.error.is_some()` inline, keep `automod_rules` empty, and draw cleanly.
#[test]
fn test_cm_t2_f21_03_automod_editor_rejects_empty_name_and_invalid_regex() {
    run_community_gpui_test("test_cm_t2_f21_03_automod_editor_rejects_empty_name_and_invalid_regex");
}

async fn body_t2_f21_03(mut app: AsyncApp) {
    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8313, "t2_f21_03", true).await;
    let cw = cw.unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "am_err_f21".into();
        w.state.create_community.name = "AutoMod Error F21".into();
        w.submit_create_community(cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| cw.read(cx).state.route == CommunityRoute::CommunityPage("am_err_f21".into())).await);

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::ModQueue(Some("am_err_f21".into())), cx));
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.mod_loading && cw.read(cx).state.mod_community.is_some()).await);

    let _ = cw.update(&mut app, |w, cx| {
        w.set_mod_tab(ModConsoleTab::AutoModRules, cx);
        w.state.automod_draft.name = "   ".into();
        w.submit_add_automod_rule(cx);
        assert!(w.state.automod_draft.error.is_some());

        w.state.automod_draft.error = None;
        w.state.automod_draft.name = "Bad Regex Guard".into();
        w.state.automod_draft.regex_pattern = "[unclosed(".into();
        w.submit_add_automod_rule(cx);
        assert!(w.state.automod_draft.error.is_some());
        assert!(w.state.mod_community.as_ref().unwrap().automod_rules.is_empty());
    });
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F21-04
/// [Feature / Req] F21 / TEST_INFRA.md §6 (Bot Studio rejects missing `bot` suffix, short handles, and out-of-range rate/cooldown limits)
/// [Preconditions] `validate_bot_studio_registration` helper + Headless GPUI `CommunityWorkspace` navigated to `BotStudio`.
/// [Opaque-Box Steps] Validate boundary inputs (`no_suffix`, `abot`, cooldown `999999`, max_posts `0` & `501`), then submit invalid form entries inside `BotStudio`.
/// [Observable Assertions] Every invalid input fails validation; `BotStudio` surfaces inline `bot_studio_error` without submitting.
#[test]
fn test_cm_t2_f21_04_bot_studio_rejects_invalid_username_and_out_of_range_limits() {
    run_community_gpui_test("test_cm_t2_f21_04_bot_studio_rejects_invalid_username_and_out_of_range_limits");
}

async fn body_t2_f21_04(mut app: AsyncApp) {
    assert!(validate_bot_studio_registration("", "valid_bot", "30", "12").is_err());
    assert!(validate_bot_studio_registration("Bot", "no_suffix", "30", "12").is_err());
    assert!(validate_bot_studio_registration("Bot", "abot", "30", "12").is_err());
    assert!(validate_bot_studio_registration("Bot", "valid_bot", "999999", "12").is_err());
    assert!(validate_bot_studio_registration("Bot", "valid_bot", "30", "0").is_err());
    assert!(validate_bot_studio_registration("Bot", "valid_bot", "30", "501").is_err());
    assert!(validate_bot_studio_registration("Bot", "valid_f21_bot", "0", "500").is_ok());

    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8314, "t2_f21_04", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BotStudio, cx);
        w.state.bot_studio_name = "Bad Suffix".into();
        w.state.bot_studio_handle = "missing_suffix".into();
        w.submit_register_bot(cx);
        assert!(w.state.bot_studio_error.is_some());
        assert!(!w.state.bot_studio_submitting);

        w.state.bot_studio_error = None;
        w.state.bot_studio_handle = "good_f21_bot".into();
        w.state.bot_studio_max_posts_per_hour = "0".into();
        w.submit_register_bot(cx);
        assert!(w.state.bot_studio_error.is_some());
        assert!(!w.state.bot_studio_submitting);
    });
    host.draw(&mut app);
}

/// [Test ID] TEST-CM-T2-F21-05
/// [Feature / Req] F21 / TEST_INFRA.md §6 (Bot Studio 8×11 Grok avatar out-of-range clamping & clipboard token copy feedback)
/// [Preconditions] `clamp_grok_avatar_indices` / `resolve_grok_avatar_pair` + Headless GPUI `CommunityWorkspace` on `BotStudio`.
/// [Opaque-Box Steps] Verify clamping on `(0,0)`, `(7,10)`, `(8,11)`, `(999, usize::MAX)`, call `set_bot_studio_avatar_indices(999, usize::MAX)` and `copy_bot_token` inside `BotStudio`.
/// [Observable Assertions] Indices clamp safely to `(7, 10)` matching final palette entries without panic, and `copy_bot_token` emits confirmation toast cleanly.
#[test]
fn test_cm_t2_f21_05_bot_studio_grok_avatar_8x11_clamping_and_token_copy_feedback() {
    run_community_gpui_test("test_cm_t2_f21_05_bot_studio_grok_avatar_8x11_clamping_and_token_copy_feedback");
}

async fn body_t2_f21_05(mut app: AsyncApp) {
    assert_eq!(GROK_AVATAR_SHAPES.len(), 8);
    assert_eq!(GROK_AVATAR_COLOR_IDS.len(), 11);
    assert_eq!(clamp_grok_avatar_indices(0, 0), (0, 0));
    assert_eq!(clamp_grok_avatar_indices(7, 10), (7, 10));
    assert_eq!(clamp_grok_avatar_indices(8, 11), (7, 10));
    assert_eq!(clamp_grok_avatar_indices(999, usize::MAX), (7, 10));
    assert_eq!(resolve_grok_avatar_pair(999, usize::MAX), (GROK_AVATAR_SHAPES[7], GROK_AVATAR_COLOR_IDS[10]));

    let (host, cw) = open_offline_community_host(&mut app, 1440.0, 900.0, 8315, "t2_f21_05", true).await;
    let cw = cw.unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BotStudio, cx);
        w.set_bot_studio_avatar_indices(999, usize::MAX, cx);
        assert_eq!((w.state.bot_studio_shape_idx, w.state.bot_studio_color_idx), (7, 10));

        let _ = w.take_pending_actions();
        w.copy_bot_token("123456:demo_studio_bot_token".into(), cx);
        assert!(w.take_pending_actions().iter().any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: false, .. })));
    });
    host.draw(&mut app);
}
