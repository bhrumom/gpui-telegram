//! Comprehensive integration tests for `CS-F05` & `CS-F06` (`CodeSceneRecommender`).

use std::collections::HashSet;

use gpui_telegram_code_scene::reco::{
    CHANNEL_COLD_START, CHANNEL_JUST_PUBLISHED, CHANNEL_SWING_CF, EMBEDDING_DIM, EVAL_BASE_TS_MS,
    REWARD_MAX, REWARD_MIN, TIER0_COLD_START, TIER1_SEED, TIER2_RISING, TIER3_VIRAL,
    build_eval_corpus, cosine_similarity, count_scatter_violations, freshness_score,
    normalize_tag, normalize_tags, traffic_tier_for_plays,
};
use gpui_telegram_code_scene::{
    CodeSceneRecommender, OFFICIAL_ARCADE_FORGE_BOT_ID, RewardOutcome, UserInterestProfile,
    compile_and_verify, find_seed_by_id, official_seed_assets,
};
use gpui_telegram_proto::{
    AssetKind, AssetManifest, CODE_SCENE_FORMAT_CS1, CodeSceneAssetDto, CodeSceneFeedRequest,
    CodeSceneFeedTab, ListCodeSceneAssetsQuery, SceneBehaviorEventDto, SceneBehaviorEventKind,
};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn test_recommender_types_are_send_sync_and_tiers_and_freshness() {
    assert_send_sync::<CodeSceneRecommender>();
    assert_send_sync::<UserInterestProfile>();
    assert_send_sync::<RewardOutcome>();

    assert_eq!(traffic_tier_for_plays(0), TIER0_COLD_START);
    assert_eq!(traffic_tier_for_plays(19), TIER0_COLD_START);
    assert_eq!(traffic_tier_for_plays(20), TIER1_SEED);
    assert_eq!(traffic_tier_for_plays(99), TIER1_SEED);
    assert_eq!(traffic_tier_for_plays(100), TIER2_RISING);
    assert_eq!(traffic_tier_for_plays(499), TIER2_RISING);
    assert_eq!(traffic_tier_for_plays(500), TIER3_VIRAL);

    // Exact freshness formula: 1 / (age_hours + 2.0)^1.25
    let f0 = freshness_score(EVAL_BASE_TS_MS, EVAL_BASE_TS_MS);
    let expected0 = 1.0 / 2.0_f64.powf(1.25);
    assert!((f0 - expected0).abs() < 1e-12);
    let f24 = freshness_score(EVAL_BASE_TS_MS - 24 * 3_600_000, EVAL_BASE_TS_MS);
    let expected24 = 1.0 / 26.0_f64.powf(1.25);
    assert!((f24 - expected24).abs() < 1e-12);
    assert!(f0 > f24);
    // Future timestamps clamp cleanly without Inf/NaN
    let f_future = freshness_score(EVAL_BASE_TS_MS + 999_999_999, EVAL_BASE_TS_MS);
    assert!((f_future - expected0).abs() < 1e-12);
}

#[test]
fn test_all_20_behavior_event_kinds_exhaustive_matrix_and_undo_semantics() {
    let mut engine = CodeSceneRecommender::new_with_seeds();
    engine.set_now_ms(Some(EVAL_BASE_TS_MS));
    let target_scene = "seed_video_cyber_dawn";
    let initial_likes = engine.get_asset(target_scene).unwrap().likes_count;
    let initial_bookmarks = engine.get_asset(target_scene).unwrap().bookmarks_count;

    // Exercise all 20 variants in SceneBehaviorEventKind::ALL
    for (idx, &kind) in SceneBehaviorEventKind::ALL.iter().enumerate() {
        let uid = 200 + idx as i64;
        let ev = SceneBehaviorEventDto {
            user_id: uid,
            session_id: "matrix".to_string(),
            scene_id: target_scene.to_string(),
            kind,
            dwell_ms: if kind == SceneBehaviorEventKind::FastSkip { 600 } else { 12_000 },
            duration_ms: 16_000,
            completion_ratio: 1.2,
            loop_count: 1,
            game_inputs: 14,
            game_play_ms: 10_000,
            game_restarts: 1,
            game_score: Some(50.0),
            reason: if kind == SceneBehaviorEventKind::HideTag {
                Some("cyberpunk".to_string())
            } else {
                None
            },
            timestamp_ms: EVAL_BASE_TS_MS + (idx as i64) * 100,
            ..Default::default()
        };
        let out = engine.record_behavior(&ev);
        assert!(out.reward.is_finite());
        assert!(
            (REWARD_MIN..=REWARD_MAX).contains(&out.reward),
            "kind {:?} reward {} out of bounds",
            kind, out.reward
        );
        assert_eq!(out.hard_negative_applied, kind.is_hard_negative());
    }

    // Undo idempotency & zero underflow on clean user
    let mut engine2 = CodeSceneRecommender::new_with_seeds();
    engine2.set_now_ms(Some(EVAL_BASE_TS_MS));
    let u = 999_i64;
    // Unlike without prior like -> no counter decrement, never blocklists
    let u_no_prior = engine2.record_behavior(&SceneBehaviorEventDto {
        user_id: u,
        scene_id: target_scene.to_string(),
        kind: SceneBehaviorEventKind::Unlike,
        ..Default::default()
    });
    assert!(!u_no_prior.undo_reverted_prior);
    assert_eq!(engine2.get_asset(target_scene).unwrap().likes_count, initial_likes);
    assert!(
        !engine2
            .export_user_profile(u)
            .unwrap()
            .disliked_scene_ids
            .contains(target_scene)
    );

    // Like twice -> idempotent +1, then Unlike -> restores exact initial_likes
    engine2.record_behavior(&SceneBehaviorEventDto {
        user_id: u,
        scene_id: target_scene.to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    engine2.record_behavior(&SceneBehaviorEventDto {
        user_id: u,
        scene_id: target_scene.to_string(),
        kind: SceneBehaviorEventKind::DoubleTapLike,
        ..Default::default()
    });
    assert_eq!(engine2.get_asset(target_scene).unwrap().likes_count, initial_likes + 1);
    let u_rev = engine2.record_behavior(&SceneBehaviorEventDto {
        user_id: u,
        scene_id: target_scene.to_string(),
        kind: SceneBehaviorEventKind::Unlike,
        ..Default::default()
    });
    assert!(u_rev.undo_reverted_prior);
    assert_eq!(engine2.get_asset(target_scene).unwrap().likes_count, initial_likes);

    // Bookmark -> Unbookmark restores bookmarks_count and never blocklists
    engine2.record_behavior(&SceneBehaviorEventDto {
        user_id: u,
        scene_id: target_scene.to_string(),
        kind: SceneBehaviorEventKind::Bookmark,
        ..Default::default()
    });
    assert_eq!(
        engine2.get_asset(target_scene).unwrap().bookmarks_count,
        initial_bookmarks + 1
    );
    let ub = engine2.record_behavior(&SceneBehaviorEventDto {
        user_id: u,
        scene_id: target_scene.to_string(),
        kind: SceneBehaviorEventKind::Unbookmark,
        ..Default::default()
    });
    assert!(ub.undo_reverted_prior);
    assert_eq!(
        engine2.get_asset(target_scene).unwrap().bookmarks_count,
        initial_bookmarks
    );

    // Fast-skip dwell thresholds & Game-vs-Video reward parity
    let fs_hard = engine2.record_behavior(&SceneBehaviorEventDto {
        user_id: u,
        scene_id: "seed_video_code_poetry".to_string(),
        kind: SceneBehaviorEventKind::Dwell,
        dwell_ms: 800,
        ..Default::default()
    });
    assert!(fs_hard.fast_skip_detected && fs_hard.reward <= -0.85);

    let fs_soft = engine2.record_behavior(&SceneBehaviorEventDto {
        user_id: u,
        scene_id: "seed_video_code_poetry".to_string(),
        kind: SceneBehaviorEventKind::Dwell,
        dwell_ms: 2_100,
        ..Default::default()
    });
    assert!(fs_soft.fast_skip_detected && (fs_soft.reward + 0.35).abs() < 1e-6);

    let vid_comp = engine2.record_behavior(&SceneBehaviorEventDto {
        user_id: u,
        scene_id: "seed_video_solar_orbit".to_string(),
        kind: SceneBehaviorEventKind::Dwell,
        dwell_ms: 20_000,
        duration_ms: 20_000,
        completion_ratio: 1.0,
        loop_count: 1,
        ..Default::default()
    });
    let game_eng = engine2.record_behavior(&SceneBehaviorEventDto {
        user_id: u,
        scene_id: "seed_game_flappy_bird".to_string(),
        kind: SceneBehaviorEventKind::GameInteraction,
        dwell_ms: 15_000,
        game_inputs: 22,
        game_play_ms: 15_000,
        game_restarts: 1,
        game_score: Some(40.0),
        ..Default::default()
    });
    assert!(vid_comp.reward > 1.5 && game_eng.reward > 1.5);
    assert!((vid_comp.reward - game_eng.reward).abs() < 1.5);
}

#[test]
fn test_blocklists_zero_leak_across_all_tabs_and_mid_pagination() {
    let mut engine = CodeSceneRecommender::new_with_seeds();
    engine.set_now_ms(Some(EVAL_BASE_TS_MS));
    let uid = 301_i64;

    // Fetch Page 1 first
    let p1 = engine.recommend_feed(&CodeSceneFeedRequest {
        user_id: uid,
        tab: CodeSceneFeedTab::ForYou,
        limit: 6,
        ..Default::default()
    });
    assert_eq!(p1.items.len(), 6);
    assert!(p1.next_cursor.is_some());

    // Mid-pagination: apply NotInterested, HideAuthor, HideTag targeting remaining unseen seeds
    let blocked_scene = "seed_game_flappy_bird";
    let blocked_author = 777_101_i64; // neon_shader_bot
    let blocked_tag = "fractal";

    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid,
        scene_id: blocked_scene.to_string(),
        kind: SceneBehaviorEventKind::NotInterested,
        ..Default::default()
    });
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid,
        scene_id: "seed_anim_aurora_plasma".to_string(),
        author_user_id: Some(blocked_author),
        kind: SceneBehaviorEventKind::HideAuthor,
        ..Default::default()
    });
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid,
        scene_id: "seed_anim_fractal_sakura".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        reason: Some(format!("#{blocked_tag}")),
        ..Default::default()
    });

    // FollowAuthor after HideAuthor stays sticky-blocked
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid,
        author_user_id: Some(blocked_author),
        kind: SceneBehaviorEventKind::FollowAuthor,
        ..Default::default()
    });
    assert!(
        !engine
            .export_user_profile(uid)
            .unwrap()
            .followed_creators
            .contains(&blocked_author)
    );

    // Continue pagination from Page 1 cursor + check ALL 6 tabs
    let mut cursor = p1.next_cursor;
    while let Some(c) = cursor {
        let resp = engine.recommend_feed(&CodeSceneFeedRequest {
            user_id: uid,
            tab: CodeSceneFeedTab::ForYou,
            cursor: Some(c),
            limit: 6,
            ..Default::default()
        });
        for item in &resp.items {
            assert_ne!(item.asset.scene_id, blocked_scene);
            assert_ne!(item.asset.author_user_id, blocked_author);
            assert!(!normalize_tags(&item.asset.tags).iter().any(|t| t == blocked_tag));
        }
        cursor = resp.next_cursor;
    }

    for tab in CodeSceneFeedTab::ALL {
        let resp = engine.recommend_feed(&CodeSceneFeedRequest {
            user_id: uid,
            tab,
            limit: 50,
            ..Default::default()
        });
        for item in &resp.items {
            assert_ne!(item.asset.scene_id, blocked_scene);
            assert_ne!(item.asset.author_user_id, blocked_author);
            assert!(!normalize_tags(&item.asset.tags).iter().any(|t| t == blocked_tag));
        }
    }
}

#[test]
fn test_embeddings_code_tokens_bilingual_queries_and_nan_safety() {
    let mut engine = CodeSceneRecommender::new_with_seeds();
    engine.set_now_ms(Some(EVAL_BASE_TS_MS));

    // All 18 seeds have unit-norm 64-dim embeddings and non-empty code feature tokens
    for seed in official_seed_assets() {
        let emb = engine.item_embedding(&seed.scene_id).unwrap();
        assert_eq!(emb.len(), EMBEDDING_DIM);
        let norm_sq: f64 = emb.iter().map(|&x| (x as f64) * (x as f64)).sum();
        assert!((norm_sq - 1.0).abs() < 1e-4, "seed {} norm_sq={}", seed.scene_id, norm_sq);
        let toks = engine.item_feature_tokens(&seed.scene_id).unwrap();
        assert!(!toks.is_empty());
        assert!(engine.item_traffic_tier(&seed.scene_id).is_some());
    }

    // Zero-norm & NaN safety
    assert_eq!(cosine_similarity(&[], &[]), 0.0);
    assert_eq!(cosine_similarity(&[0.0; 64], &[1.0; 64]), 0.0);
    assert_eq!(cosine_similarity(&[f32::NAN; 64], &[1.0; 64]), 0.0);

    // Same-modality similar scenes beat unrelated scenes
    let sim_games = cosine_similarity(
        engine.item_embedding("seed_game_flappy_bird").unwrap(),
        engine.item_embedding("seed_game_brick_breaker").unwrap(),
    );
    let sim_cross = cosine_similarity(
        engine.item_embedding("seed_game_flappy_bird").unwrap(),
        engine.item_embedding("seed_video_code_poetry").unwrap(),
    );
    assert!(sim_games > sim_cross, "sim_games={sim_games} vs sim_cross={sim_cross}");

    // Bilingual Chinese & English search queries
    let cases = [
        ("霓虹飞鸟", "seed_game_flappy_bird"),
        ("赛博", "seed_video_cyber_dawn"),
        ("分形着色器", "seed_anim_mandelbrot_abyss"),
        ("flappy", "seed_game_flappy_bird"),
    ];
    for (q, expected_id) in cases {
        let resp = engine.recommend_feed(&CodeSceneFeedRequest {
            user_id: 401,
            tab: CodeSceneFeedTab::ForYou,
            query: Some(q.to_string()),
            limit: 6,
            ..Default::default()
        });
        assert!(
            resp.items.iter().any(|i| i.asset.scene_id == expected_id),
            "query '{}' did not retrieve {} (got {:?})",
            q,
            expected_id,
            resp.items.iter().map(|i| &i.asset.scene_id).collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_multi_user_swing_cf_and_creator_just_published_fast_track() {
    let mut engine = CodeSceneRecommender::new_with_seeds();
    engine.set_now_ms(Some(EVAL_BASE_TS_MS));

    // Users A (501) and B (502) co-engage on X (seed_anim_voronoi_reef) and Y (seed_video_code_poetry)
    for uid in [501_i64, 502_i64] {
        for sid in ["seed_anim_voronoi_reef", "seed_video_code_poetry"] {
            engine.record_behavior(&SceneBehaviorEventDto {
                user_id: uid,
                scene_id: sid.to_string(),
                kind: SceneBehaviorEventKind::Like,
                timestamp_ms: EVAL_BASE_TS_MS,
                ..Default::default()
            });
        }
    }
    // User C (503) likes X -> Y gets real Swing CF recall signal!
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: 503,
        scene_id: "seed_anim_voronoi_reef".to_string(),
        kind: SceneBehaviorEventKind::Like,
        timestamp_ms: EVAL_BASE_TS_MS + 100,
        ..Default::default()
    });
    let explained = engine
        .explain_asset_for_user(503, "seed_video_code_poetry", EVAL_BASE_TS_MS + 200)
        .unwrap();
    assert!(
        explained.explanation.breakdown.swing_cf > 0.20,
        "expected strong multi-user swing_cf, got {}",
        explained.explanation.breakdown.swing_cf
    );
    assert!(
        explained
            .explanation
            .recall_channels
            .iter()
            .any(|c| c == CHANNEL_SWING_CF)
    );

    // CreatorJustPublished fast-track for publisher (slot 0) + global cold-start slot 3 for other user
    let custom = CodeSceneAssetDto {
        scene_id: "bot_custom_new_01".to_string(),
        title: "Fresh Bot Creation".to_string(),
        description: "Brand new AI bot asset".to_string(),
        kind: AssetKind::Animation,
        tags: vec!["custom_bot".to_string(), "generative".to_string()],
        author_user_id: 888,
        author_username: "user_888".to_string(),
        creator_bot_id: Some(777_000),
        creator_bot_username: Some("fabushi_ai_bot".to_string()),
        source_code: "fn on_render(s, t, c) { clear(0x0A0A20FF); circle(180, 320, 40, 0x00F5FFFF); }".to_string(),
        code_format: CODE_SCENE_FORMAT_CS1.to_string(),
        manifest: AssetManifest::for_kind(AssetKind::Animation),
        created_at_ms: EVAL_BASE_TS_MS,
        updated_at_ms: EVAL_BASE_TS_MS,
        ..Default::default()
    };
    engine.register_or_update_asset(custom, Some(888));

    let pub_feed = engine.recommend_feed(&CodeSceneFeedRequest {
        user_id: 888,
        tab: CodeSceneFeedTab::ForYou,
        limit: 12,
        ..Default::default()
    });
    assert_eq!(pub_feed.items[0].asset.scene_id, "bot_custom_new_01");
    assert!(
        pub_feed.items[0]
            .explanation
            .recall_channels
            .iter()
            .any(|c| c == CHANNEL_JUST_PUBLISHED)
    );

    let other_feed = engine.recommend_feed(&CodeSceneFeedRequest {
        user_id: 889,
        tab: CodeSceneFeedTab::ForYou,
        limit: 12,
        ..Default::default()
    });
    let pos = other_feed
        .items
        .iter()
        .position(|i| i.asset.scene_id == "bot_custom_new_01")
        .expect("new asset must appear in unrelated user's cold-start slot");
    assert!(pos < 10, "expected slot < 10, got {pos}");
    assert_eq!(other_feed.items[pos].explanation.traffic_tier, TIER0_COLD_START);
    assert!(
        other_feed.items[pos]
            .explanation
            .recall_channels
            .iter()
            .any(|c| c == CHANNEL_COLD_START)
    );
}

#[test]
fn test_pagination_continuity_profile_roundtrip_and_degenerate_boundaries() {
    let mut engine = CodeSceneRecommender::new_with_seeds();
    engine.set_now_ms(Some(EVAL_BASE_TS_MS));

    // Paginate 3 pages of 6 items across 18 seeds -> 0 dupes, 0 scatter violations, cold-start at 3 & 10
    let mut all_items = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut cursor: Option<String> = None;
    for _ in 0..3 {
        let resp = engine.recommend_feed(&CodeSceneFeedRequest {
            user_id: 601,
            tab: CodeSceneFeedTab::ForYou,
            cursor: cursor.clone(),
            limit: 6,
            ..Default::default()
        });
        assert!(!resp.items.is_empty());
        for item in resp.items {
            assert!(seen_ids.insert(item.asset.scene_id.clone()), "duplicate across pages!");
            assert!(!item.explanation.primary_reason_zh.is_empty());
            assert!(!item.explanation.primary_reason_en.is_empty());
            assert!(!item.explanation.recall_channels.is_empty());
            assert!(item.explanation.breakdown.final_score.is_finite());
            all_items.push(item);
        }
        cursor = resp.next_cursor;
    }
    assert_eq!(all_items.len(), 18);
    assert_eq!(count_scatter_violations(&all_items[..12], true), 0);
    assert!(
        all_items[0..7]
            .iter()
            .any(|i| i.explanation.traffic_tier == TIER0_COLD_START),
        "expected tier0_cold_start seed in window 0..7"
    );

    // 3-page pagination across 90-asset corpus: 0 dupes, 0 cross-page scatter violations,
    // and exact cold-start slot placement at global indices 3, 10, 17!
    let mut big_engine = CodeSceneRecommender::new_empty();
    big_engine.set_now_ms(Some(EVAL_BASE_TS_MS));
    for a in gpui_telegram_code_scene::reco::build_eval_corpus() {
        big_engine.register_or_update_asset(a, None);
    }
    // Warm up user on games so regular MMR slots prefer Tier2/3 games while cold-start slots inject Tier0
    for sid in ["seed_game_flappy_bird", "seed_game_space_shooter", "seed_game_brick_breaker"] {
        big_engine.record_behavior(&SceneBehaviorEventDto {
            user_id: 699,
            scene_id: sid.to_string(),
            kind: SceneBehaviorEventKind::Like,
            timestamp_ms: EVAL_BASE_TS_MS - 1_000,
            ..Default::default()
        });
    }
    let mut big_items = Vec::new();
    let mut big_seen = HashSet::new();
    let mut big_cursor: Option<String> = None;
    for _ in 0..3 {
        let resp = big_engine.recommend_feed(&CodeSceneFeedRequest {
            user_id: 699,
            tab: CodeSceneFeedTab::ForYou,
            cursor: big_cursor.clone(),
            limit: 7,
            ..Default::default()
        });
        assert_eq!(resp.items.len(), 7);
        for item in resp.items {
            assert!(big_seen.insert(item.asset.scene_id.clone()));
            big_items.push(item);
        }
        big_cursor = resp.next_cursor;
    }
    assert_eq!(big_items.len(), 21);
    assert_eq!(count_scatter_violations(&big_items, true), 0);
    assert_eq!(big_items[3].explanation.traffic_tier, TIER0_COLD_START);
    assert_eq!(big_items[10].explanation.traffic_tier, TIER0_COLD_START);
    assert_eq!(big_items[17].explanation.traffic_tier, TIER0_COLD_START);

    // Profile JSON roundtrip reproduces bit-identical ranking + corrupted import sanitizes cleanly
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: 602,
        scene_id: "seed_game_flappy_bird".to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    let exported = engine.export_user_profile(602).unwrap();
    let json = serde_json::to_string(&exported).unwrap();
    let restored: UserInterestProfile = serde_json::from_str(&json).unwrap();

    let mut cloned_engine = CodeSceneRecommender::new_with_seeds();
    cloned_engine.set_now_ms(Some(EVAL_BASE_TS_MS));
    cloned_engine.import_user_profile(restored);
    let r1 = engine.recommend_feed(&CodeSceneFeedRequest { user_id: 602, limit: 12, ..Default::default() });
    let r2 = cloned_engine.recommend_feed(&CodeSceneFeedRequest { user_id: 602, limit: 12, ..Default::default() });
    assert_eq!(
        r1.items.iter().map(|i| &i.asset.scene_id).collect::<Vec<_>>(),
        r2.items.iter().map(|i| &i.asset.scene_id).collect::<Vec<_>>()
    );

    // Corrupted profile sanitization (wrong vector len, NaN weights, hidden author in followed)
    let mut bad = UserInterestProfile::new(603);
    bad.u_short = vec![f32::NAN, f32::INFINITY, 1.0];
    bad.tag_weights.insert(" #CyberPunk ".to_string(), f64::NAN);
    bad.hidden_author_ids.insert(777_102);
    bad.followed_creators.insert(777_102);
    cloned_engine.import_user_profile(bad);
    let clean = cloned_engine.export_user_profile(603).unwrap();
    assert_eq!(clean.u_short.len(), EMBEDDING_DIM);
    assert!(clean.u_short.iter().all(|x| x.is_finite()));
    assert!(!clean.followed_creators.contains(&777_102));

    // Degenerate inputs: empty catalog, malformed/oversized/OOB cursors, NaN telemetry, unknown scene_id
    let empty = CodeSceneRecommender::new_empty();
    let er = empty.recommend_feed(&CodeSceneFeedRequest::default());
    assert!(er.items.is_empty() && er.next_cursor.is_none() && er.total_catalog == 0);

    for bad_cursor in ["csc1:zzzz", "not_a_cursor", "999999", &"a".repeat(40_000)] {
        let resp = engine.recommend_feed(&CodeSceneFeedRequest {
            user_id: 601,
            cursor: Some(bad_cursor.to_string()),
            limit: 12,
            ..Default::default()
        });
        assert!(resp.items.is_empty() && resp.next_cursor.is_none(), "cursor {bad_cursor} must fail closed");
    }

    let extreme = engine.record_behavior(&SceneBehaviorEventDto {
        user_id: 604,
        scene_id: "unknown_future_scene".to_string(),
        kind: SceneBehaviorEventKind::Dwell,
        dwell_ms: u64::MAX,
        completion_ratio: f64::NAN,
        game_score: Some(f64::INFINITY),
        ..Default::default()
    });
    assert!(extreme.reward.is_finite() && (REWARD_MIN..=REWARD_MAX).contains(&extreme.reward));

    // Social toggle helpers, list_assets, update_social_counters, remove_asset
    let listed = engine.list_assets(&ListCodeSceneAssetsQuery {
        kind: Some(AssetKind::Game),
        ..Default::default()
    });
    assert_eq!(listed.total, 6);
    let lk = engine.toggle_like(605, "seed_game_neon_snake").unwrap();
    assert!(lk.liked);
    let bm = engine.toggle_bookmark(605, "seed_game_neon_snake").unwrap();
    assert!(bm.bookmarked);
    let fl = engine.toggle_follow(605, 777_000);
    assert!(fl.following && fl.followers_count >= 1);
    assert!(engine.update_social_counters("seed_game_neon_snake", Some(999), None, None, None, None, None));
    assert_eq!(engine.get_asset("seed_game_neon_snake").unwrap().likes_count, 999);
    assert!(engine.remove_asset("seed_game_neon_snake"));
    assert_eq!(engine.catalog_len(), 17);
}

#[test]
fn test_iter2_bayesian_p_finish_p_skip_and_profile_hygiene_regressions() {
    // Item 1: High-plays_count seed must NOT collapse p_finish/quality_ecpm on first Impression+Complete,
    // and live skips on a popular seed must have real Bayesian responsiveness.
    let mut engine = CodeSceneRecommender::new_with_seeds();
    engine.set_now_ms(Some(EVAL_BASE_TS_MS));
    let mut pop_asset = find_seed_by_id("seed_video_cyber_dawn").unwrap();
    pop_asset.scene_id = "pop_bayes_video".to_string();
    pop_asset.plays_count = 1_200;
    pop_asset.likes_count = 240;
    engine.register_or_update_asset(pop_asset, None);

    let before = engine
        .explain_asset_for_user(9_001, "pop_bayes_video", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .quality_ecpm;
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: 9_002,
        scene_id: "pop_bayes_video".to_string(),
        kind: SceneBehaviorEventKind::Impression,
        timestamp_ms: EVAL_BASE_TS_MS,
        ..Default::default()
    });
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: 9_002,
        scene_id: "pop_bayes_video".to_string(),
        kind: SceneBehaviorEventKind::Complete,
        dwell_ms: 16_000,
        completion_ratio: 1.0,
        timestamp_ms: EVAL_BASE_TS_MS + 100,
        ..Default::default()
    });
    let after_complete = engine
        .explain_asset_for_user(9_001, "pop_bayes_video", EVAL_BASE_TS_MS + 200)
        .unwrap()
        .explanation
        .breakdown
        .quality_ecpm;
    assert!(
        after_complete >= before - 0.02,
        "quality_ecpm collapsed on first Impression+Complete: before={before:.4}, after={after_complete:.4}"
    );

    // Repeated live skips on a popular seed measurably lower quality_ecpm via aligned live-trial p_skip
    for skip_uid in 9_100..9_112 {
        engine.record_behavior(&SceneBehaviorEventDto {
            user_id: skip_uid,
            scene_id: "pop_bayes_video".to_string(),
            kind: SceneBehaviorEventKind::Impression,
            timestamp_ms: EVAL_BASE_TS_MS + 300,
            ..Default::default()
        });
        engine.record_behavior(&SceneBehaviorEventDto {
            user_id: skip_uid,
            scene_id: "pop_bayes_video".to_string(),
            kind: SceneBehaviorEventKind::FastSkip,
            dwell_ms: 500,
            timestamp_ms: EVAL_BASE_TS_MS + 400,
            ..Default::default()
        });
    }
    let after_skips = engine
        .explain_asset_for_user(9_001, "pop_bayes_video", EVAL_BASE_TS_MS + 500)
        .unwrap()
        .explanation
        .breakdown
        .quality_ecpm;
    assert!(
        after_skips < after_complete - 0.15,
        "live skips failed to reduce quality_ecpm on popular seed: before={after_complete:.4}, after={after_skips:.4}"
    );

    // Item 2: No phantom AssetKind::Animation mutation on author/tag-only events
    let uid_phantom = 9_010;
    let fl = engine.toggle_follow(uid_phantom, OFFICIAL_ARCADE_FORGE_BOT_ID);
    assert!(fl.following);
    let prof_after_follow = engine.export_user_profile(uid_phantom).unwrap();
    assert!(
        prof_after_follow.kind_weights.is_empty(),
        "toggle_follow mutated kind_weights with phantom Animation: {:?}",
        prof_after_follow.kind_weights
    );
    assert!(prof_after_follow.kind_fatigue.is_empty());

    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_phantom,
        author_user_id: Some(777_101),
        kind: SceneBehaviorEventKind::HideAuthor,
        timestamp_ms: EVAL_BASE_TS_MS + 600,
        ..Default::default()
    });
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_phantom,
        kind: SceneBehaviorEventKind::HideTag,
        reason: Some("fractal".to_string()),
        timestamp_ms: EVAL_BASE_TS_MS + 700,
        ..Default::default()
    });
    let prof_after_hides = engine.export_user_profile(uid_phantom).unwrap();
    assert!(
        prof_after_hides.kind_weights.is_empty(),
        "author/tag-only hides mutated kind_weights: {:?}",
        prof_after_hides.kind_weights
    );
    assert!(
        prof_after_hides.kind_fatigue.is_empty(),
        "author/tag-only hides mutated kind_fatigue: {:?}",
        prof_after_hides.kind_fatigue
    );

    // Item 2 (part B): Blocked author / disliked scene rejects FollowAuthor, Like, DoubleTapLike, Bookmark
    let uid_blk = 9_011;
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_blk,
        scene_id: "seed_anim_aurora_plasma".to_string(),
        author_user_id: Some(777_101),
        kind: SceneBehaviorEventKind::HideAuthor,
        timestamp_ms: EVAL_BASE_TS_MS,
        ..Default::default()
    });
    let follow_blocked = engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_blk,
        scene_id: "seed_anim_mandelbrot_abyss".to_string(),
        author_user_id: Some(777_101),
        kind: SceneBehaviorEventKind::FollowAuthor,
        timestamp_ms: EVAL_BASE_TS_MS + 100,
        ..Default::default()
    });
    assert_eq!(follow_blocked.reward, 0.0);
    assert!(!follow_blocked.profile_updated);
    let like_blocked_author = engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_blk,
        scene_id: "seed_anim_mandelbrot_abyss".to_string(),
        kind: SceneBehaviorEventKind::Like,
        timestamp_ms: EVAL_BASE_TS_MS + 110,
        ..Default::default()
    });
    assert_eq!(like_blocked_author.reward, 0.0);
    assert!(!like_blocked_author.profile_updated);
    let bm_blocked_author = engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_blk,
        scene_id: "seed_anim_mandelbrot_abyss".to_string(),
        kind: SceneBehaviorEventKind::Bookmark,
        timestamp_ms: EVAL_BASE_TS_MS + 120,
        ..Default::default()
    });
    assert_eq!(bm_blocked_author.reward, 0.0);
    assert!(!bm_blocked_author.profile_updated);

    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_blk,
        scene_id: "seed_game_flappy_bird".to_string(),
        kind: SceneBehaviorEventKind::NotInterested,
        timestamp_ms: EVAL_BASE_TS_MS + 130,
        ..Default::default()
    });
    let dt_disliked = engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_blk,
        scene_id: "seed_game_flappy_bird".to_string(),
        kind: SceneBehaviorEventKind::DoubleTapLike,
        timestamp_ms: EVAL_BASE_TS_MS + 140,
        ..Default::default()
    });
    assert_eq!(dt_disliked.reward, 0.0);
    assert!(!dt_disliked.profile_updated);
    let tl_disliked = engine.toggle_like(uid_blk, "seed_game_flappy_bird").unwrap();
    assert!(!tl_disliked.liked);
    let tb_disliked = engine.toggle_bookmark(uid_blk, "seed_game_flappy_bird").unwrap();
    assert!(!tb_disliked.bookmarked);

    let prof_blk = engine.export_user_profile(uid_blk).unwrap();
    assert_eq!(prof_blk.total_positives, 0);
    assert!(!prof_blk.followed_creators.contains(&777_101));
    assert!(prof_blk.positive_scene_history.is_empty());
    assert!(prof_blk.liked_scene_ids.is_empty());
    assert!(prof_blk.bookmarked_scene_ids.is_empty());

    // Item 3: Idempotent repeated Like/DoubleTapLike/Bookmark/FollowAuthor (zero inflation!)
    let uid_idem = 9_012;
    let scene_idem = "seed_video_cyber_dawn";
    let author_idem = engine.get_asset(scene_idem).unwrap().author_user_id;
    let base_likes = engine.get_asset(scene_idem).unwrap().likes_count;
    let base_bms = engine.get_asset(scene_idem).unwrap().bookmarks_count;

    let first_like = engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_idem,
        scene_id: scene_idem.to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    assert!(first_like.reward > 0.0 && first_like.profile_updated);
    for _ in 0..4 {
        let dup = engine.record_behavior(&SceneBehaviorEventDto {
            user_id: uid_idem,
            scene_id: scene_idem.to_string(),
            kind: SceneBehaviorEventKind::Like,
            ..Default::default()
        });
        assert_eq!(dup.reward, 0.0);
        assert!(!dup.profile_updated);
    }
    for _ in 0..5 {
        let dup_dt = engine.record_behavior(&SceneBehaviorEventDto {
            user_id: uid_idem,
            scene_id: scene_idem.to_string(),
            kind: SceneBehaviorEventKind::DoubleTapLike,
            ..Default::default()
        });
        assert_eq!(dup_dt.reward, 0.0);
        assert!(!dup_dt.profile_updated);
    }
    let first_bm = engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_idem,
        scene_id: scene_idem.to_string(),
        kind: SceneBehaviorEventKind::Bookmark,
        ..Default::default()
    });
    assert!(first_bm.reward > 0.0 && first_bm.profile_updated);
    for _ in 0..4 {
        let dup = engine.record_behavior(&SceneBehaviorEventDto {
            user_id: uid_idem,
            scene_id: scene_idem.to_string(),
            kind: SceneBehaviorEventKind::Bookmark,
            ..Default::default()
        });
        assert_eq!(dup.reward, 0.0);
        assert!(!dup.profile_updated);
    }
    let first_fl = engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid_idem,
        scene_id: scene_idem.to_string(),
        author_user_id: Some(author_idem),
        kind: SceneBehaviorEventKind::FollowAuthor,
        ..Default::default()
    });
    assert!(first_fl.reward > 0.0 && first_fl.profile_updated);
    let cw_after_first = engine
        .export_user_profile(uid_idem)
        .unwrap()
        .creator_weights[&author_idem];
    for _ in 0..4 {
        let dup = engine.record_behavior(&SceneBehaviorEventDto {
            user_id: uid_idem,
            scene_id: scene_idem.to_string(),
            author_user_id: Some(author_idem),
            kind: SceneBehaviorEventKind::FollowAuthor,
            ..Default::default()
        });
        assert_eq!(dup.reward, 0.0);
        assert!(!dup.profile_updated);
    }
    let prof_idem = engine.export_user_profile(uid_idem).unwrap();
    assert_eq!(prof_idem.total_positives, 3);
    assert!((prof_idem.creator_weights[&author_idem] - cw_after_first).abs() < 1e-12);
    assert_eq!(engine.get_asset(scene_idem).unwrap().likes_count, base_likes + 1);
    assert_eq!(engine.get_asset(scene_idem).unwrap().bookmarks_count, base_bms + 1);

    // Item 10 (telemetry belt-and-suspenders): short Dwell (<3000ms) on already-liked scene never fast-skip-penalizes
    for short_ms in [400_u64, 2_000_u64] {
        let out = engine.record_behavior(&SceneBehaviorEventDto {
            user_id: uid_idem,
            scene_id: scene_idem.to_string(),
            kind: SceneBehaviorEventKind::Dwell,
            dwell_ms: short_ms,
            duration_ms: 15_000,
            completion_ratio: short_ms as f64 / 15_000.0,
            ..Default::default()
        });
        assert!(!out.fast_skip_detected);
        assert_eq!(out.reward, 0.0);
    }
    let prof_after_short_dwell = engine.export_user_profile(uid_idem).unwrap();
    assert_eq!(prof_after_short_dwell.total_negatives, 0);
    let u_neg_norm: f32 = prof_after_short_dwell.u_neg.iter().map(|x| x * x).sum();
    assert!(u_neg_norm <= 1e-9, "u_neg contaminated: norm_sq={u_neg_norm}");
}

#[test]
fn test_iter2_unicode_tags_numeric_cursor_verified_parity_seam_scatter_and_swing_degree() {
    // Item 4: Unicode tag normalization across catalog, tag_filter, list_assets, and HideTag
    let mut engine = CodeSceneRecommender::new_with_seeds();
    engine.set_now_ms(Some(EVAL_BASE_TS_MS));
    let mut unicode_asset = find_seed_by_id("seed_anim_aurora_plasma").unwrap();
    unicode_asset.scene_id = "unicode_tag_scene".to_string();
    unicode_asset.tags = vec!["#ÜBER".to_string(), "#КОД".to_string(), "#CAFÉ".to_string()];
    engine.register_or_update_asset(unicode_asset, None);

    let filtered = engine.recommend_feed(&CodeSceneFeedRequest {
        user_id: 9_020,
        tab: CodeSceneFeedTab::ForYou,
        tag_filter: Some("  #Über  ".to_string()),
        limit: 12,
        ..Default::default()
    });
    assert!(
        filtered.items.iter().any(|i| i.asset.scene_id == "unicode_tag_scene"),
        "Unicode tag_filter missed #ÜBER asset"
    );
    let listed = engine.list_assets(&ListCodeSceneAssetsQuery {
        tag: Some("  #код  ".to_string()),
        ..Default::default()
    });
    assert!(listed.assets.iter().any(|a| a.scene_id == "unicode_tag_scene"));

    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: 9_021,
        kind: SceneBehaviorEventKind::HideTag,
        reason: Some("  #Über  ".to_string()),
        ..Default::default()
    });
    for tab in CodeSceneFeedTab::ALL {
        let resp = engine.recommend_feed(&CodeSceneFeedRequest {
            user_id: 9_021,
            tab,
            limit: 50,
            ..Default::default()
        });
        assert!(
            resp.items.iter().all(|i| i.asset.scene_id != "unicode_tag_scene"),
            "Unicode HideTag leaked on tab {tab:?}"
        );
    }

    // Item 5: NumericOffset cursor records skipped prefix in next_cursor (zero Page-3 duplicates)
    // and sizes recall quota for large numeric offsets
    let p1 = engine.recommend_feed(&CodeSceneFeedRequest {
        user_id: 9_022,
        cursor: None,
        limit: 5,
        ..Default::default()
    });
    let p2 = engine.recommend_feed(&CodeSceneFeedRequest {
        user_id: 9_022,
        cursor: Some("5".to_string()),
        limit: 5,
        ..Default::default()
    });
    let p3 = engine.recommend_feed(&CodeSceneFeedRequest {
        user_id: 9_022,
        cursor: p2.next_cursor.clone(),
        limit: 5,
        ..Default::default()
    });
    let mut seen_3p = HashSet::new();
    for item in p1.items.iter().chain(p2.items.iter()).chain(p3.items.iter()) {
        assert!(
            seen_3p.insert(item.asset.scene_id.clone()),
            "duplicate {} across NumericOffset -> csc1 continuation!",
            item.asset.scene_id
        );
    }
    assert_eq!(seen_3p.len(), p1.items.len() + p2.items.len() + p3.items.len());

    let mut big = CodeSceneRecommender::new_empty();
    big.set_now_ms(Some(EVAL_BASE_TS_MS));
    for a in gpui_telegram_code_scene::reco::build_eval_corpus() {
        big.register_or_update_asset(a, None);
    }
    let off60 = big.recommend_feed(&CodeSceneFeedRequest {
        user_id: 9_023,
        cursor: Some("60".to_string()),
        limit: 12,
        ..Default::default()
    });
    assert_eq!(off60.items.len(), 12, "NumericOffset 60 on 90-item corpus was starved by quota");
    let off72 = big.recommend_feed(&CodeSceneFeedRequest {
        user_id: 9_023,
        cursor: off60.next_cursor.clone(),
        limit: 12,
        ..Default::default()
    });
    assert_eq!(off72.items.len(), 12);
    let s60: HashSet<&str> = off60.items.iter().map(|i| i.asset.scene_id.as_str()).collect();
    assert!(off72.items.iter().all(|i| !s60.contains(i.asset.scene_id.as_str())));

    // Item 6: register_verified_asset produces identical code_tokens & bit-identical embeddings as register_or_update_asset
    for sid in ["seed_game_flappy_bird", "seed_video_cyber_dawn"] {
        let seed = find_seed_by_id(sid).unwrap();
        let verified = compile_and_verify(&seed.source_code, &seed.manifest).unwrap();
        let mut eng_raw = CodeSceneRecommender::new_empty();
        let mut eng_ver = CodeSceneRecommender::new_empty();
        eng_raw.set_now_ms(Some(EVAL_BASE_TS_MS));
        eng_ver.set_now_ms(Some(EVAL_BASE_TS_MS));
        eng_raw.register_or_update_asset(seed.clone(), None);
        eng_ver.register_verified_asset(seed, &verified, None);
        assert_eq!(eng_raw.item_feature_tokens(sid), eng_ver.item_feature_tokens(sid));
        assert_eq!(eng_raw.item_embedding(sid), eng_ver.item_embedding(sid));
    }

    // Item 7: Dual CreatorJustPublished by same author with limit=3 has 0 Page-2 seam Author-Window-3 violations
    let mut ft_eng = CodeSceneRecommender::new_with_seeds();
    ft_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    for suffix in ["a", "b"] {
        let mut a = find_seed_by_id("seed_anim_aurora_plasma").unwrap();
        a.scene_id = format!("dual_ft_{suffix}");
        a.author_user_id = 9_999;
        a.plays_count = 0;
        a.created_at_ms = EVAL_BASE_TS_MS;
        ft_eng.register_or_update_asset(a, Some(9_999));
    }
    let fp1 = ft_eng.recommend_feed(&CodeSceneFeedRequest {
        user_id: 9_999,
        limit: 3,
        ..Default::default()
    });
    let fp2 = ft_eng.recommend_feed(&CodeSceneFeedRequest {
        user_id: 9_999,
        cursor: fp1.next_cursor.clone(),
        limit: 6,
        ..Default::default()
    });
    assert_eq!(fp1.items[0].asset.author_user_id, 9_999);
    let mut concat = fp1.items;
    concat.extend(fp2.items);
    assert_eq!(
        count_scatter_violations(&concat, true),
        0,
        "dual CreatorJustPublished violated cross-page seam scatter!"
    );

    // Item 8: Single-tag 10-asset catalog across 5 distinct authors preserves Author Window 3 (0 author violations)
    let mut tag_eng = CodeSceneRecommender::new_empty();
    tag_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    let kinds = [AssetKind::Video, AssetKind::Animation, AssetKind::Game];
    for idx in 0..10usize {
        let a = CodeSceneAssetDto {
            scene_id: format!("same_tag_{idx:02}"),
            title: format!("Cyberpunk Scene {idx}"),
            description: "Shared single primary tag".to_string(),
            kind: kinds[idx % 3],
            tags: vec!["cyberpunk".to_string()],
            author_user_id: 5_000 + (idx % 5) as i64,
            author_username: format!("author_{}", idx % 5),
            source_code: "fn on_render(s,t,c){clear(0x000000FF);}".to_string(),
            code_format: CODE_SCENE_FORMAT_CS1.to_string(),
            manifest: AssetManifest::for_kind(kinds[idx % 3]),
            plays_count: 100 + idx as u64 * 10,
            likes_count: 20 + idx as u64 * 2,
            created_at_ms: EVAL_BASE_TS_MS,
            updated_at_ms: EVAL_BASE_TS_MS,
            ..Default::default()
        };
        tag_eng.register_or_update_asset(a, None);
    }
    let st_resp = tag_eng.recommend_feed(&CodeSceneFeedRequest {
        user_id: 9_024,
        limit: 10,
        ..Default::default()
    });
    assert_eq!(st_resp.items.len(), 10);
    for k in 0..st_resp.items.len() {
        let start = k.saturating_sub(3);
        let auth = st_resp.items[k].asset.author_user_id;
        assert!(
            !st_resp.items[start..k]
                .iter()
                .any(|p| p.asset.author_user_id == auth),
            "Author Window 3 violated at index {k} on single-tag 5-author catalog"
        );
    }

    // Item 9a: HideTag with reason="   ###   " falls back to effective_primary_tag (NOT "game")
    let mut ht_eng = CodeSceneRecommender::new_with_seeds();
    ht_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    ht_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 9_025,
        scene_id: "seed_game_flappy_bird".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        tags: vec!["game".to_string(), "flappy".to_string(), "arcade".to_string()],
        reason: Some("   ###   ".to_string()),
        ..Default::default()
    });
    let ht_prof = ht_eng.export_user_profile(9_025).unwrap();
    assert!(ht_prof.hidden_tags.contains("flappy"));
    assert!(!ht_prof.hidden_tags.contains("game"));

    // Item 9b: Swing CF sorts co-users by ascending degree before truncating to 24 so low-ID hubs cannot starve focused users
    let build_swing_test_engine = |include_focused: bool| {
        let mut eng = CodeSceneRecommender::new_empty();
        eng.set_now_ms(Some(EVAL_BASE_TS_MS));
        for i in 0..80usize {
            eng.register_or_update_asset(
                CodeSceneAssetDto {
                    scene_id: format!("sw_{i:02}"),
                    title: format!("Item {i}"),
                    kind: AssetKind::Animation,
                    tags: vec![format!("uniq_{i}")],
                    author_user_id: 7_000 + i as i64,
                    source_code: "fn on_render(s,t,c){clear(0x000000FF);}".to_string(),
                    code_format: CODE_SCENE_FORMAT_CS1.to_string(),
                    manifest: AssetManifest::for_kind(AssetKind::Animation),
                    created_at_ms: EVAL_BASE_TS_MS,
                    ..Default::default()
                },
                None,
            );
        }
        // 24 low-ID hub users (degree = 80 each -> overlap = 80)
        for hub_uid in 1..=24_i64 {
            for i in 0..80usize {
                eng.record_behavior(&SceneBehaviorEventDto {
                    user_id: hub_uid,
                    scene_id: format!("sw_{i:02}"),
                    kind: SceneBehaviorEventKind::Like,
                    ..Default::default()
                });
            }
        }
        // 4 high-ID focused users (degree = 2 each -> ONLY sw_00 and sw_01)
        if include_focused {
            for foc_uid in 50_001..=50_004_i64 {
                for sid in ["sw_00", "sw_01"] {
                    eng.record_behavior(&SceneBehaviorEventDto {
                        user_id: foc_uid,
                        scene_id: sid.to_string(),
                        kind: SceneBehaviorEventKind::Like,
                        ..Default::default()
                    });
                }
            }
        }
        eng.record_behavior(&SceneBehaviorEventDto {
            user_id: 99_999,
            scene_id: "sw_00".to_string(),
            kind: SceneBehaviorEventKind::Like,
            ..Default::default()
        });
        eng.explain_asset_for_user(99_999, "sw_01", EVAL_BASE_TS_MS)
            .unwrap()
            .explanation
            .breakdown
            .swing_cf
    };
    let s_hubs_only = build_swing_test_engine(false);
    let s_with_focused = build_swing_test_engine(true);
    assert!(
        s_with_focused > s_hubs_only,
        "low-ID hubs starved high-ID focused users in Swing CF: with_focused={s_with_focused:.6} <= hubs_only={s_hubs_only:.6}"
    );
}

#[test]
fn test_iter3_hide_tag_fallback_sticky_blocklist_warm_numeric_cursor_and_swing_damping() {
    // Fix 1 (R1-F1 / C1-P2.6c): HideTag with reason: None or whitespace falls through to
    // effective_primary_tag (NEVER banning "game"), while explicit reason is honored.
    let mut ht_eng = CodeSceneRecommender::new_with_seeds();
    ht_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    ht_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 9_101,
        scene_id: "seed_game_orbit_catcher".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        tags: vec![
            "game".to_string(),
            "physics".to_string(),
            "orbit".to_string(),
            "space".to_string(),
        ],
        reason: None,
        ..Default::default()
    });
    let p_none = ht_eng.export_user_profile(9_101).unwrap();
    assert!(p_none.hidden_tags.contains("physics"));
    assert!(!p_none.hidden_tags.contains("game"));
    let games_after_none = ht_eng.recommend_feed(&CodeSceneFeedRequest {
        user_id: 9_101,
        tab: CodeSceneFeedTab::Games,
        limit: 20,
        ..Default::default()
    });
    let expected_non_physics_games = official_seed_assets()
        .iter()
        .filter(|a| {
            a.kind == AssetKind::Game
                && !normalize_tags(&a.tags).iter().any(|t| t == "physics")
        })
        .count();
    assert_eq!(
        games_after_none.items.len(),
        expected_non_physics_games,
        "HideTag(reason=None) should hide only 'physics' games, not the 'game' modality"
    );

    ht_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 9_102,
        scene_id: "seed_game_flappy_bird".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        tags: vec!["game".to_string(), "flappy".to_string(), "arcade".to_string()],
        reason: Some("   ".to_string()),
        ..Default::default()
    });
    let p_ws = ht_eng.export_user_profile(9_102).unwrap();
    assert!(p_ws.hidden_tags.contains("flappy"));
    assert!(!p_ws.hidden_tags.contains("game"));

    ht_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 9_103,
        scene_id: "seed_game_flappy_bird".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        tags: vec!["game".to_string(), "flappy".to_string()],
        reason: Some("  #Arcade  ".to_string()),
        ..Default::default()
    });
    let p_explicit = ht_eng.export_user_profile(9_103).unwrap();
    assert!(p_explicit.hidden_tags.contains("arcade"));
    assert!(!p_explicit.hidden_tags.contains("flappy"));

    // Fix 2a (C1-P2.3b): HideAuthor sticky-blocks ALL positive engagement kinds on hidden author's scenes
    let mut blk_eng = CodeSceneRecommender::new_with_seeds();
    blk_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    let hidden_author = 777_101_i64; // authors both seed_anim_aurora_plasma and seed_anim_mandelbrot_abyss
    let other_scene = "seed_anim_mandelbrot_abyss";
    blk_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 9_104,
        scene_id: "seed_anim_aurora_plasma".to_string(),
        author_user_id: Some(hidden_author),
        kind: SceneBehaviorEventKind::HideAuthor,
        timestamp_ms: EVAL_BASE_TS_MS,
        ..Default::default()
    });
    let snap_asset = blk_eng.get_asset(other_scene).unwrap();
    let snap_rev = blk_eng.export_user_profile(9_104).unwrap().revision;
    let post_block_events = [
        SceneBehaviorEventDto {
            user_id: 9_104,
            scene_id: other_scene.to_string(),
            kind: SceneBehaviorEventKind::Comment,
            ..Default::default()
        },
        SceneBehaviorEventDto {
            user_id: 9_104,
            scene_id: other_scene.to_string(),
            kind: SceneBehaviorEventKind::Share,
            ..Default::default()
        },
        SceneBehaviorEventDto {
            user_id: 9_104,
            scene_id: other_scene.to_string(),
            kind: SceneBehaviorEventKind::InspectCode,
            ..Default::default()
        },
        SceneBehaviorEventDto {
            user_id: 9_104,
            scene_id: other_scene.to_string(),
            kind: SceneBehaviorEventKind::Remix,
            ..Default::default()
        },
        SceneBehaviorEventDto {
            user_id: 9_104,
            scene_id: other_scene.to_string(),
            kind: SceneBehaviorEventKind::Complete,
            dwell_ms: 14_000,
            completion_ratio: 1.0,
            ..Default::default()
        },
        SceneBehaviorEventDto {
            user_id: 9_104,
            scene_id: other_scene.to_string(),
            kind: SceneBehaviorEventKind::Replay,
            dwell_ms: 18_000,
            loop_count: 2,
            ..Default::default()
        },
        SceneBehaviorEventDto {
            user_id: 9_104,
            scene_id: other_scene.to_string(),
            kind: SceneBehaviorEventKind::GameInteraction,
            dwell_ms: 12_000,
            game_inputs: 20,
            game_play_ms: 12_000,
            game_restarts: 1,
            game_score: Some(80.0),
            ..Default::default()
        },
        SceneBehaviorEventDto {
            user_id: 9_104,
            scene_id: other_scene.to_string(),
            kind: SceneBehaviorEventKind::Dwell,
            dwell_ms: 15_000,
            duration_ms: 15_000,
            completion_ratio: 1.0,
            ..Default::default()
        },
    ];
    for ev in &post_block_events {
        let out = blk_eng.record_behavior(ev);
        assert_eq!(out.reward, 0.0, "{:?} awarded reward on hidden author", ev.kind);
        assert!(
            !out.profile_updated,
            "{:?} updated profile on hidden author",
            ev.kind
        );
    }
    let after_prof = blk_eng.export_user_profile(9_104).unwrap();
    assert_eq!(after_prof.total_positives, 0);
    assert!(after_prof.positive_scene_history.is_empty());
    assert_eq!(after_prof.revision, snap_rev);
    let after_asset = blk_eng.get_asset(other_scene).unwrap();
    assert_eq!(after_asset.plays_count, snap_asset.plays_count);
    assert_eq!(after_asset.comments_count, snap_asset.comments_count);
    assert_eq!(after_asset.shares_count, snap_asset.shares_count);
    assert_eq!(after_asset.remixes_count, snap_asset.remixes_count);

    // Fix 2b (C1-P5.6): Out-of-order positive events after NotInterested never poison Swing CF
    let mut cf_blk = CodeSceneRecommender::new_empty();
    cf_blk.set_now_ms(Some(EVAL_BASE_TS_MS));
    for (sid, tag) in [("ni_a", "alpha_only"), ("ni_b", "beta_only")] {
        cf_blk.register_or_update_asset(
            CodeSceneAssetDto {
                scene_id: sid.to_string(),
                title: sid.to_string(),
                kind: AssetKind::Animation,
                tags: vec![tag.to_string()],
                author_user_id: 6_100,
                source_code: "fn on_render(s,t,c){clear(0x000000FF);}".to_string(),
                code_format: CODE_SCENE_FORMAT_CS1.to_string(),
                manifest: AssetManifest::for_kind(AssetKind::Animation),
                created_at_ms: EVAL_BASE_TS_MS,
                ..Default::default()
            },
            None,
        );
    }
    cf_blk.record_behavior(&SceneBehaviorEventDto {
        user_id: 6_202,
        scene_id: "ni_a".to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    let clean_swing = cf_blk
        .explain_asset_for_user(6_202, "ni_b", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;
    cf_blk.record_behavior(&SceneBehaviorEventDto {
        user_id: 6_201,
        scene_id: "ni_b".to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    cf_blk.record_behavior(&SceneBehaviorEventDto {
        user_id: 6_201,
        scene_id: "ni_a".to_string(),
        kind: SceneBehaviorEventKind::NotInterested,
        ..Default::default()
    });
    for kind in [
        SceneBehaviorEventKind::Comment,
        SceneBehaviorEventKind::Share,
        SceneBehaviorEventKind::Complete,
        SceneBehaviorEventKind::Replay,
        SceneBehaviorEventKind::Remix,
        SceneBehaviorEventKind::InspectCode,
        SceneBehaviorEventKind::GameInteraction,
        SceneBehaviorEventKind::Dwell,
    ] {
        let out = cf_blk.record_behavior(&SceneBehaviorEventDto {
            user_id: 6_201,
            scene_id: "ni_a".to_string(),
            kind,
            dwell_ms: 15_000,
            completion_ratio: 1.0,
            game_inputs: 15,
            game_play_ms: 10_000,
            ..Default::default()
        });
        assert_eq!(out.reward, 0.0, "NotInterested target accepted {kind:?}");
        assert!(!out.profile_updated);
    }
    let post_swing = cf_blk
        .explain_asset_for_user(6_202, "ni_b", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;
    assert_eq!(
        post_swing.to_bits(),
        clean_swing.to_bits(),
        "post-NotInterested events poisoned Swing CF: {clean_swing} -> {post_swing}"
    );

    // Fix 3 (C1-P3.1c / C1-P5.8): Warm-user NumericOffset matches Fresh seen cooldown (0 dupes)
    let mut warm_eng = CodeSceneRecommender::new_with_seeds();
    warm_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    let warm_uid = 9_105_i64;
    for sid in [
        "seed_game_flappy_bird",
        "seed_game_neon_snake",
        "seed_game_brick_breaker",
    ] {
        warm_eng.record_behavior(&SceneBehaviorEventDto {
            user_id: warm_uid,
            scene_id: sid.to_string(),
            kind: SceneBehaviorEventKind::Impression,
            timestamp_ms: EVAL_BASE_TS_MS,
            ..Default::default()
        });
        warm_eng.record_behavior(&SceneBehaviorEventDto {
            user_id: warm_uid,
            scene_id: sid.to_string(),
            kind: SceneBehaviorEventKind::Like,
            timestamp_ms: EVAL_BASE_TS_MS + 100,
            ..Default::default()
        });
    }
    let p_fresh = warm_eng.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id: warm_uid,
            cursor: None,
            limit: 5,
            ..Default::default()
        },
        EVAL_BASE_TS_MS + 1_000,
    );
    let p_zero = warm_eng.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id: warm_uid,
            cursor: Some("0".to_string()),
            limit: 5,
            ..Default::default()
        },
        EVAL_BASE_TS_MS + 1_000,
    );
    let ids_fresh: Vec<&str> = p_fresh.items.iter().map(|i| i.asset.scene_id.as_str()).collect();
    let ids_zero: Vec<&str> = p_zero.items.iter().map(|i| i.asset.scene_id.as_str()).collect();
    assert_eq!(ids_fresh, ids_zero, "warm cursor=Some(\"0\") diverged from cursor=None");

    let p_off5 = warm_eng.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id: warm_uid,
            cursor: Some("5".to_string()),
            limit: 5,
            ..Default::default()
        },
        EVAL_BASE_TS_MS + 1_000,
    );
    assert_eq!(p_off5.items.len(), 5);
    let set_fresh: HashSet<&str> = ids_fresh.into_iter().collect();
    for item in &p_off5.items {
        assert!(
            !set_fresh.contains(item.asset.scene_id.as_str()),
            "warm NumericOffset(\"5\") duplicated Page-1 item {}",
            item.asset.scene_id
        );
    }

    // Fix 4 (C1-P3.5b): Non-saturating Alibaba Swing CF damping under 30 dispersed degree-40 hubs
    let mut disp_eng = CodeSceneRecommender::new_empty();
    disp_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    let mut all_ids = vec!["hub_A".to_string(), "hub_B".to_string(), "hub_C".to_string()];
    for i in 0..120usize {
        all_ids.push(format!("pool_{i:03}"));
    }
    for (idx, sid) in all_ids.iter().enumerate() {
        disp_eng.register_or_update_asset(
            CodeSceneAssetDto {
                scene_id: sid.clone(),
                title: format!("Disjoint {idx}"),
                kind: AssetKind::Animation,
                tags: vec![format!("disj_tag_{idx}")],
                author_user_id: 8_000 + idx as i64,
                source_code: "fn on_render(s,t,c){clear(0x000000FF);}".to_string(),
                code_format: CODE_SCENE_FORMAT_CS1.to_string(),
                manifest: AssetManifest::for_kind(AssetKind::Animation),
                created_at_ms: EVAL_BASE_TS_MS,
                ..Default::default()
            },
            None,
        );
    }
    // 30 low-ID dispersed degree-40 hubs (each likes A, B, C + 37 rotating pool scenes)
    for h in 0..30usize {
        let uid = (h + 1) as i64;
        for sid in ["hub_A", "hub_B", "hub_C"] {
            disp_eng.record_behavior(&SceneBehaviorEventDto {
                user_id: uid,
                scene_id: sid.to_string(),
                kind: SceneBehaviorEventKind::Like,
                ..Default::default()
            });
        }
        for k in 0..37usize {
            let pid = format!("pool_{:03}", (h * 4 + k) % 120);
            disp_eng.record_behavior(&SceneBehaviorEventDto {
                user_id: uid,
                scene_id: pid,
                kind: SceneBehaviorEventKind::Like,
                ..Default::default()
            });
        }
    }
    // 4 high-ID focused users who ONLY like hub_A and hub_B
    for uid in 1_000..=1_003_i64 {
        for sid in ["hub_A", "hub_B"] {
            disp_eng.record_behavior(&SceneBehaviorEventDto {
                user_id: uid,
                scene_id: sid.to_string(),
                kind: SceneBehaviorEventKind::Like,
                ..Default::default()
            });
        }
    }
    disp_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 99_001,
        scene_id: "hub_A".to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    let sw_ab = disp_eng
        .explain_asset_for_user(99_001, "hub_B", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;
    let sw_ac = disp_eng
        .explain_asset_for_user(99_001, "hub_C", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;
    assert!(
        sw_ab - sw_ac >= 0.15,
        "dispersed hub-flood Swing CF margin too small or saturated: A->B={sw_ab:.6}, A->C={sw_ac:.6}, delta={:.6}",
        sw_ab - sw_ac
    );

    // 2 focused users (deg=2) >> 2 hub users (deg=20)
    let mut pair_eng = CodeSceneRecommender::new_empty();
    pair_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    for i in 0..25usize {
        pair_eng.register_or_update_asset(
            CodeSceneAssetDto {
                scene_id: format!("p_{i:02}"),
                title: format!("P{i}"),
                kind: AssetKind::Video,
                tags: vec![format!("ptag_{i}")],
                author_user_id: 8_500 + i as i64,
                source_code: "fn on_render(s,t,c){clear(0x000000FF);}".to_string(),
                code_format: CODE_SCENE_FORMAT_CS1.to_string(),
                manifest: AssetManifest::for_kind(AssetKind::Video),
                created_at_ms: EVAL_BASE_TS_MS,
                ..Default::default()
            },
            None,
        );
    }
    // 2 focused co-users on p_00 & p_01 (deg=2)
    for uid in [10_i64, 11_i64] {
        for sid in ["p_00", "p_01"] {
            pair_eng.record_behavior(&SceneBehaviorEventDto {
                user_id: uid,
                scene_id: sid.to_string(),
                kind: SceneBehaviorEventKind::Like,
                ..Default::default()
            });
        }
    }
    // 2 hub co-users on p_00 & p_02 + 18 more (deg=20)
    for uid in [20_i64, 21_i64] {
        pair_eng.record_behavior(&SceneBehaviorEventDto {
            user_id: uid,
            scene_id: "p_00".to_string(),
            kind: SceneBehaviorEventKind::Like,
            ..Default::default()
        });
        for i in 2..21usize {
            pair_eng.record_behavior(&SceneBehaviorEventDto {
                user_id: uid,
                scene_id: format!("p_{i:02}"),
                kind: SceneBehaviorEventKind::Like,
                ..Default::default()
            });
        }
    }
    pair_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 99_002,
        scene_id: "p_00".to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    let s_2foc = pair_eng
        .explain_asset_for_user(99_002, "p_01", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;
    let s_2hub = pair_eng
        .explain_asset_for_user(99_002, "p_02", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;
    assert!(
        s_2foc > s_2hub + 0.15,
        "2 focused users should strongly beat 2 hub users: 2foc={s_2foc:.6}, 2hub={s_2hub:.6}"
    );
}

#[test]
fn test_iter4_sticky_hide_tag_follow_guard_prior_positive_purge_and_two_phase_numeric_cursor() {
    let mk_asset = |sid: &str, author: i64, tags: &[&str], plays: u64| CodeSceneAssetDto {
        scene_id: sid.to_string(),
        title: sid.to_string(),
        kind: AssetKind::Game,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        author_user_id: author,
        source_code: "fn on_render(s,t,c){clear(0x000000FF);}".to_string(),
        code_format: CODE_SCENE_FORMAT_CS1.to_string(),
        manifest: AssetManifest::for_kind(AssetKind::Game),
        plays_count: plays,
        likes_count: 10,
        bookmarks_count: 2,
        comments_count: 1,
        shares_count: 1,
        remixes_count: 1,
        created_at_ms: EVAL_BASE_TS_MS,
        ..Default::default()
    };

    // 1a (C1-Def1): HideTag sticky-blocks ALL 12 positive kinds + toggles + zero Swing CF poisoning
    let mut ht_eng = CodeSceneRecommender::new_empty();
    ht_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    ht_eng.register_or_update_asset(mk_asset("ht_bridge", 501, &["alpha_only"], 50), None);
    ht_eng.register_or_update_asset(
        mk_asset("ht_trigger", 502, &["game", "blocked_tag", "beta_only"], 50),
        None,
    );
    ht_eng.register_or_update_asset(
        mk_asset("ht_other", 503, &["blocked_tag", "gamma_only"], 50),
        None,
    );

    ht_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 2,
        scene_id: "ht_other".to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    let clean_ht_swing = ht_eng
        .explain_asset_for_user(2, "ht_bridge", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;

    ht_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 1,
        scene_id: "ht_bridge".to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    ht_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 1,
        scene_id: "ht_trigger".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        reason: Some("blocked_tag".to_string()),
        ..Default::default()
    });

    assert!(!ht_eng.toggle_like(1, "ht_trigger").unwrap().liked);
    assert!(!ht_eng.toggle_bookmark(1, "ht_trigger").unwrap().bookmarked);

    let snap_other = ht_eng.get_asset("ht_other").unwrap();
    let snap_pos = ht_eng.export_user_profile(1).unwrap().total_positives;
    let all_12_positives = [
        SceneBehaviorEventKind::Dwell,
        SceneBehaviorEventKind::Complete,
        SceneBehaviorEventKind::Replay,
        SceneBehaviorEventKind::GameInteraction,
        SceneBehaviorEventKind::Like,
        SceneBehaviorEventKind::DoubleTapLike,
        SceneBehaviorEventKind::Bookmark,
        SceneBehaviorEventKind::Comment,
        SceneBehaviorEventKind::Share,
        SceneBehaviorEventKind::FollowAuthor,
        SceneBehaviorEventKind::InspectCode,
        SceneBehaviorEventKind::Remix,
    ];
    for kind in all_12_positives {
        let out = ht_eng.record_behavior(&SceneBehaviorEventDto {
            user_id: 1,
            scene_id: "ht_other".to_string(),
            kind,
            dwell_ms: 12_000,
            duration_ms: 12_000,
            completion_ratio: 1.0,
            loop_count: 2,
            game_inputs: 18,
            game_play_ms: 12_000,
            game_restarts: 1,
            game_score: Some(50.0),
            ..Default::default()
        });
        assert_eq!(out.reward, 0.0, "HideTag target awarded reward on {kind:?}");
        assert!(!out.profile_updated, "HideTag target updated profile on {kind:?}");
    }
    let after_other = ht_eng.get_asset("ht_other").unwrap();
    assert_eq!(after_other.plays_count, snap_other.plays_count);
    assert_eq!(after_other.likes_count, snap_other.likes_count);
    assert_eq!(after_other.bookmarks_count, snap_other.bookmarks_count);
    assert_eq!(after_other.comments_count, snap_other.comments_count);
    assert_eq!(after_other.shares_count, snap_other.shares_count);
    assert_eq!(after_other.remixes_count, snap_other.remixes_count);
    let prof_1 = ht_eng.export_user_profile(1).unwrap();
    assert_eq!(prof_1.total_positives, snap_pos);
    assert!(!prof_1.positive_scene_history.iter().any(|s| s == "ht_trigger" || s == "ht_other"));
    let post_ht_swing = ht_eng
        .explain_asset_for_user(2, "ht_bridge", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;
    assert_eq!(post_ht_swing.to_bits(), clean_ht_swing.to_bits());

    // 1b (C1-Def1 + R1 Note): NotInterested blocks FollowAuthor on that scene; creator-sheet
    // toggle_follow still works; catalog-first resolve_author defeats spoofed author_user_id.
    let mut ni_eng = CodeSceneRecommender::new_empty();
    ni_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    ni_eng.register_or_update_asset(mk_asset("sb_target", 502, &["alpha"], 50), None);
    ni_eng.register_or_update_asset(mk_asset("sb_sibling", 502, &["beta"], 50), None);
    ni_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 10,
        scene_id: "sb_target".to_string(),
        kind: SceneBehaviorEventKind::NotInterested,
        ..Default::default()
    });
    let before_ni_prof = ni_eng.export_user_profile(10).unwrap();
    let follow_disliked = ni_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 10,
        scene_id: "sb_target".to_string(),
        kind: SceneBehaviorEventKind::FollowAuthor,
        ..Default::default()
    });
    assert_eq!(follow_disliked.reward, 0.0);
    assert!(!follow_disliked.profile_updated);
    let after_ni_prof = ni_eng.export_user_profile(10).unwrap();
    assert_eq!(after_ni_prof.total_positives, before_ni_prof.total_positives);
    assert_eq!(after_ni_prof.u_short, before_ni_prof.u_short);
    assert!(!after_ni_prof.followed_creators.contains(&502));
    // Creator-sheet toggle_follow (empty scene_id) still works when author is not hidden
    assert!(ni_eng.toggle_follow(10, 502).following);

    // Spoofed client author_user_id cannot bypass HideAuthor on a known catalog scene
    ni_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 11,
        scene_id: "sb_target".to_string(),
        kind: SceneBehaviorEventKind::HideAuthor,
        ..Default::default()
    });
    let spoof_follow = ni_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 11,
        scene_id: "sb_sibling".to_string(),
        author_user_id: Some(999_999),
        kind: SceneBehaviorEventKind::FollowAuthor,
        ..Default::default()
    });
    assert_eq!(spoof_follow.reward, 0.0);
    assert!(!spoof_follow.profile_updated);
    assert!(!ni_eng.export_user_profile(11).unwrap().followed_creators.contains(&999_999));

    // 1c (C1-Def1): Prior positives purged from positive_scene_history & Swing CF on HideAuthor/HideTag + import
    let mut purge_eng = CodeSceneRecommender::new_empty();
    purge_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    purge_eng.register_or_update_asset(mk_asset("p_bridge", 601, &["only_a"], 50), None);
    purge_eng.register_or_update_asset(mk_asset("p_sibling", 602, &["only_b"], 50), None);
    purge_eng.register_or_update_asset(mk_asset("p_target_auth", 602, &["only_c"], 50), None);
    purge_eng.register_or_update_asset(mk_asset("p_other_tag", 603, &["special_tag"], 50), None);

    purge_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 202,
        scene_id: "p_sibling".to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    let clean_sib_swing = purge_eng
        .explain_asset_for_user(202, "p_bridge", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;
    purge_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 203,
        scene_id: "p_other_tag".to_string(),
        kind: SceneBehaviorEventKind::Like,
        ..Default::default()
    });
    let clean_tag_swing = purge_eng
        .explain_asset_for_user(203, "p_bridge", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;

    for sid in ["p_bridge", "p_sibling", "p_other_tag"] {
        purge_eng.record_behavior(&SceneBehaviorEventDto {
            user_id: 201,
            scene_id: sid.to_string(),
            kind: SceneBehaviorEventKind::Like,
            ..Default::default()
        });
    }
    purge_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 201,
        scene_id: "p_target_auth".to_string(),
        kind: SceneBehaviorEventKind::HideAuthor,
        ..Default::default()
    });
    purge_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 201,
        scene_id: "p_other_tag".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        reason: Some("special_tag".to_string()),
        ..Default::default()
    });
    let purged_prof = purge_eng.export_user_profile(201).unwrap();
    assert!(!purged_prof.positive_scene_history.iter().any(|s| s == "p_sibling"));
    assert!(!purged_prof.positive_scene_history.iter().any(|s| s == "p_other_tag"));
    assert!(purged_prof.positive_scene_history.iter().any(|s| s == "p_bridge"));
    let post_sib_swing = purge_eng
        .explain_asset_for_user(202, "p_bridge", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;
    let post_tag_swing = purge_eng
        .explain_asset_for_user(203, "p_bridge", EVAL_BASE_TS_MS)
        .unwrap()
        .explanation
        .breakdown
        .swing_cf;
    assert_eq!(post_sib_swing.to_bits(), clean_sib_swing.to_bits());
    assert_eq!(post_tag_swing.to_bits(), clean_tag_swing.to_bits());

    // Round-tripping profile via import_user_profile must NOT re-insert blocked liked scenes into Swing CF
    let mut reimported = purge_eng.clone();
    reimported.import_user_profile(purged_prof);
    assert_eq!(
        reimported
            .explain_asset_for_user(202, "p_bridge", EVAL_BASE_TS_MS)
            .unwrap()
            .explanation
            .breakdown
            .swing_cf
            .to_bits(),
        clean_sib_swing.to_bits()
    );
    assert_eq!(
        reimported
            .explain_asset_for_user(203, "p_bridge", EVAL_BASE_TS_MS)
            .unwrap()
            .explanation
            .breakdown
            .swing_cf
            .to_bits(),
        clean_tag_swing.to_bits()
    );

    // 2 (C1-Def2): Two-phase seen-cooldown ranking gives ZERO duplicates across warm NumericOffset pages
    let mut warm18 = CodeSceneRecommender::new_with_seeds();
    warm18.set_now_ms(Some(EVAL_BASE_TS_MS));
    let all_seed_ids: Vec<String> = official_seed_assets()
        .into_iter()
        .map(|a| a.scene_id)
        .collect();
    for sid in &all_seed_ids[..3] {
        warm18.record_behavior(&SceneBehaviorEventDto {
            user_id: 301,
            scene_id: sid.clone(),
            kind: SceneBehaviorEventKind::Impression,
            timestamp_ms: EVAL_BASE_TS_MS,
            ..Default::default()
        });
        warm18.record_behavior(&SceneBehaviorEventDto {
            user_id: 301,
            scene_id: sid.clone(),
            kind: SceneBehaviorEventKind::Like,
            timestamp_ms: EVAL_BASE_TS_MS + 100,
            ..Default::default()
        });
    }
    // 3 seen on 18 seeds, limit = 12 (None + "12") -> 12 + 6 = 18 unique, 0 dupes
    let p12_a = warm18.recommend_feed_at(
        &CodeSceneFeedRequest { user_id: 301, cursor: None, limit: 12, ..Default::default() },
        EVAL_BASE_TS_MS + 1_000,
    );
    let p12_0 = warm18.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id: 301,
            cursor: Some("0".to_string()),
            limit: 12,
            ..Default::default()
        },
        EVAL_BASE_TS_MS + 1_000,
    );
    assert_eq!(
        p12_a.items.iter().map(|i| &i.asset.scene_id).collect::<Vec<_>>(),
        p12_0.items.iter().map(|i| &i.asset.scene_id).collect::<Vec<_>>()
    );
    let p12_b = warm18.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id: 301,
            cursor: Some("12".to_string()),
            limit: 12,
            ..Default::default()
        },
        EVAL_BASE_TS_MS + 1_000,
    );
    assert_eq!((p12_a.items.len(), p12_b.items.len()), (12, 6));
    assert!(p12_b.next_cursor.is_none());
    let uniq12: HashSet<&str> = p12_a
        .items
        .iter()
        .chain(p12_b.items.iter())
        .map(|i| i.asset.scene_id.as_str())
        .collect();
    assert_eq!(uniq12.len(), 18, "limit=12 warm NumericOffset duplicated items across cooldown boundary");

    // 3 seen on 18 seeds, limit = 5 (None, "5", "10", "15") -> 5 + 5 + 5 + 3 = 18 unique, 0 dupes
    let mut collected5 = Vec::new();
    for cur in [None, Some("5"), Some("10"), Some("15")] {
        let resp = warm18.recommend_feed_at(
            &CodeSceneFeedRequest {
                user_id: 301,
                cursor: cur.map(str::to_string),
                limit: 5,
                ..Default::default()
            },
            EVAL_BASE_TS_MS + 1_000,
        );
        collected5.extend(resp.items);
    }
    assert_eq!(collected5.len(), 18);
    let uniq5: HashSet<&str> = collected5.iter().map(|i| i.asset.scene_id.as_str()).collect();
    assert_eq!(uniq5.len(), 18, "limit=5 warm NumericOffset duplicated items across cooldown boundary");

    // 10 seen on 18 seeds, limit = 5 (None + "5") -> 10 unique, 0 dupes
    for sid in &all_seed_ids[..10] {
        warm18.record_behavior(&SceneBehaviorEventDto {
            user_id: 302,
            scene_id: sid.clone(),
            kind: SceneBehaviorEventKind::Impression,
            timestamp_ms: EVAL_BASE_TS_MS,
            ..Default::default()
        });
        warm18.record_behavior(&SceneBehaviorEventDto {
            user_id: 302,
            scene_id: sid.clone(),
            kind: SceneBehaviorEventKind::Like,
            timestamp_ms: EVAL_BASE_TS_MS + 100,
            ..Default::default()
        });
    }
    let w10_a = warm18.recommend_feed_at(
        &CodeSceneFeedRequest { user_id: 302, cursor: None, limit: 5, ..Default::default() },
        EVAL_BASE_TS_MS + 1_000,
    );
    let w10_b = warm18.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id: 302,
            cursor: Some("5".to_string()),
            limit: 5,
            ..Default::default()
        },
        EVAL_BASE_TS_MS + 1_000,
    );
    assert_eq!((w10_a.items.len(), w10_b.items.len()), (5, 5));
    let uniq10: HashSet<&str> = w10_a
        .items
        .iter()
        .chain(w10_b.items.iter())
        .map(|i| i.asset.scene_id.as_str())
        .collect();
    assert_eq!(uniq10.len(), 10, "10-seen warm NumericOffset duplicated Page-1 items");

    // 90-asset corpus full pagination across top-60 -> overflow & fresh -> cooldown seams (0 dupes)
    let mut warm90 = CodeSceneRecommender::new_empty();
    warm90.set_now_ms(Some(EVAL_BASE_TS_MS));
    let corpus90 = build_eval_corpus();
    for asset in &corpus90 {
        warm90.register_or_update_asset(asset.clone(), None);
    }
    for asset in corpus90.iter().take(15) {
        warm90.record_behavior(&SceneBehaviorEventDto {
            user_id: 303,
            scene_id: asset.scene_id.clone(),
            kind: SceneBehaviorEventKind::Impression,
            timestamp_ms: EVAL_BASE_TS_MS,
            ..Default::default()
        });
        warm90.record_behavior(&SceneBehaviorEventDto {
            user_id: 303,
            scene_id: asset.scene_id.clone(),
            kind: SceneBehaviorEventKind::Like,
            timestamp_ms: EVAL_BASE_TS_MS + 100,
            ..Default::default()
        });
    }
    let mut all90 = Vec::new();
    for off in [0usize, 12, 24, 36, 48, 60, 72, 84] {
        let resp = warm90.recommend_feed_at(
            &CodeSceneFeedRequest {
                user_id: 303,
                cursor: if off == 0 { None } else { Some(off.to_string()) },
                limit: 12,
                ..Default::default()
            },
            EVAL_BASE_TS_MS + 1_000,
        );
        all90.extend(resp.items);
    }
    assert_eq!(all90.len(), 90);
    let uniq90: HashSet<&str> = all90.iter().map(|i| i.asset.scene_id.as_str()).collect();
    assert_eq!(uniq90.len(), 90, "90-asset warm NumericOffset pagination had duplicates");

    // 4 (C1-Def4): 3rd-order HideTag normalization ("  #  #  ") & catalog non-generic tag priority
    assert_eq!(normalize_tag("  #  #  "), "");
    assert_eq!(normalize_tag("  #Cyberpunk#  "), "cyberpunk");
    let mut norm_eng = CodeSceneRecommender::new_with_seeds();
    norm_eng.set_now_ms(Some(EVAL_BASE_TS_MS));
    norm_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 401,
        scene_id: "seed_game_orbit_catcher".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        reason: Some("  #  #  ".to_string()),
        ..Default::default()
    });
    let p_hashes = norm_eng.export_user_profile(401).unwrap();
    assert!(p_hashes.hidden_tags.contains("physics"));
    assert!(!p_hashes.hidden_tags.contains("#"));
    assert!(!p_hashes.hidden_tags.contains("game"));

    norm_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 402,
        scene_id: "seed_game_orbit_catcher".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        tags: vec!["game".to_string()],
        reason: None,
        ..Default::default()
    });
    let p_trunc = norm_eng.export_user_profile(402).unwrap();
    assert!(p_trunc.hidden_tags.contains("physics"));
    assert!(!p_trunc.hidden_tags.contains("game"));

    norm_eng.record_behavior(&SceneBehaviorEventDto {
        user_id: 403,
        scene_id: "seed_game_orbit_catcher".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        reason: Some("game".to_string()),
        ..Default::default()
    });
    assert!(norm_eng.export_user_profile(403).unwrap().hidden_tags.contains("game"));
}


