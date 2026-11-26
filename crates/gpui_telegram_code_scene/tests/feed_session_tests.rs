//! Comprehensive tests for pure-Rust `FeedSessionController` (`CS-F07`).

use std::collections::HashSet;

use gpui_telegram_code_scene::reco::{EVAL_BASE_TS_MS, count_scatter_violations, normalize_tags};
use gpui_telegram_code_scene::{
    CodeSceneRecommender, FeedSessionController, GameSessionTelemetry, SceneInputState, VmBudgets,
    VmInstance, compile_and_verify, find_seed_by_id,
};
use gpui_telegram_proto::{
    AssetKind, AssetManifest, CODE_SCENE_FORMAT_CS1, CodeSceneAssetDto, CodeSceneFeedTab,
    SceneBehaviorEventKind,
};

#[test]
fn test_preload_window_boundaries_and_empty_feed_safety() {
    let mut empty_reco = CodeSceneRecommender::new_empty();
    let mut ctrl = FeedSessionController::new(101, "sess_empty").with_clock_ms(EVAL_BASE_TS_MS);
    assert_eq!(ctrl.load_initial_feed(&mut empty_reco), 0);
    assert!(ctrl.preload_window_indices().is_empty());
    assert!(ctrl.preload_window_items().is_empty());
    assert!(ctrl.current_item().is_none());
    assert!(ctrl.swipe_next(&mut empty_reco).is_none());
    assert!(ctrl.swipe_prev(&mut empty_reco).is_none());
    assert!(
        ctrl.record_interaction(&mut empty_reco, SceneBehaviorEventKind::Like, None)
            .is_none()
    );

    // 1-item feed
    let mut one_reco = CodeSceneRecommender::new_empty();
    one_reco.register_or_update_asset(find_seed_by_id("seed_game_flappy_bird").unwrap(), None);
    let mut ctrl1 = FeedSessionController::new(102, "sess_one").with_clock_ms(EVAL_BASE_TS_MS);
    assert_eq!(ctrl1.load_initial_feed(&mut one_reco), 1);
    assert_eq!(ctrl1.preload_window_indices(), vec![0]);

    // Standard 18-seed feed: index 0 -> [0, 1], middle -> [i-1, i, i+1], last -> [n-2, n-1]
    let mut reco = CodeSceneRecommender::new_with_seeds();
    reco.set_now_ms(Some(EVAL_BASE_TS_MS));
    let mut ctrl2 = FeedSessionController::new(103, "sess_full")
        .with_page_size(6)
        .with_clock_ms(EVAL_BASE_TS_MS);
    assert_eq!(ctrl2.load_initial_feed(&mut reco), 6);
    assert_eq!(ctrl2.preload_window_indices(), vec![0, 1]);
    assert_eq!(ctrl2.preload_window_items().len(), 2);

    ctrl2.on_card_dwell_tick(5_000, 0);
    ctrl2.swipe_next(&mut reco);
    assert_eq!(ctrl2.current_index(), 1);
    assert_eq!(ctrl2.preload_window_indices(), vec![0, 1, 2]);
    assert_eq!(ctrl2.preload_window_items().len(), 3);

    // Swipe all the way to the end across prefetched pages -> zero duplicate IDs
    for _ in 0..25 {
        ctrl2.on_card_dwell_tick(4_000, 0);
        ctrl2.swipe_next(&mut reco);
    }
    let all_ids: Vec<&str> = ctrl2.items().iter().map(|i| i.asset.scene_id.as_str()).collect();
    let unique: HashSet<&str> = all_ids.iter().copied().collect();
    assert_eq!(all_ids.len(), unique.len(), "prefetch introduced duplicates");
    assert_eq!(all_ids.len(), 18);
    let last = ctrl2.items().len() - 1;
    assert_eq!(ctrl2.current_index(), last);
    assert_eq!(ctrl2.preload_window_indices(), vec![last - 1, last]);
}

#[test]
fn test_swipe_away_telemetry_state_machine_and_vm_game_telemetry_integration() {
    let mut reco = CodeSceneRecommender::new_with_seeds();
    reco.set_now_ms(Some(EVAL_BASE_TS_MS));
    let mut ctrl = FeedSessionController::new(201, "sess_telem").with_clock_ms(EVAL_BASE_TS_MS);
    ctrl.load_initial_feed(&mut reco);
    let _ = ctrl.drain_emitted_telemetry(); // consume initial Impression

    // 1. Short dwell (< 1500ms) with no positive -> emits FastSkip
    ctrl.on_card_dwell_tick(600, 0);
    ctrl.swipe_next(&mut reco);
    let evs = ctrl.drain_emitted_telemetry();
    assert_eq!(evs[0].kind, SceneBehaviorEventKind::FastSkip);
    assert_eq!(evs[1].kind, SceneBehaviorEventKind::Impression);

    // 2. Quick positive before 3000ms suppresses contradictory FastSkip & short Dwell
    let neg_before = reco.export_user_profile(201).unwrap().total_negatives;
    ctrl.on_card_dwell_tick(400, 0);
    ctrl.record_interaction(&mut reco, SceneBehaviorEventKind::DoubleTapLike, None);
    ctrl.swipe_next(&mut reco);
    let evs = ctrl.drain_emitted_telemetry();
    assert!(evs.iter().any(|e| e.kind == SceneBehaviorEventKind::DoubleTapLike));
    assert!(!evs.iter().any(|e| e.kind == SceneBehaviorEventKind::FastSkip));
    assert!(!evs.iter().any(|e| e.kind == SceneBehaviorEventKind::Dwell));
    assert_eq!(reco.export_user_profile(201).unwrap().total_negatives, neg_before);

    // 3. Long dwell / loop -> emits Complete
    ctrl.on_card_dwell_tick(18_000, 1);
    ctrl.swipe_next(&mut reco);
    let evs = ctrl.drain_emitted_telemetry();
    assert_eq!(evs[0].kind, SceneBehaviorEventKind::Complete);

    // 4. Short active gameplay (< 1500ms) with real VmInstance::game_telemetry() -> GameInteraction (NOT FastSkip!)
    let flappy = find_seed_by_id("seed_game_flappy_bird").unwrap();
    let verified = compile_and_verify(&flappy.source_code, &flappy.manifest).unwrap();
    let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();
    let mut input = SceneInputState::default();
    for step in 0..10 {
        input.action = step % 2 == 0;
        let frame = vm.step_frame(1.0 / 60.0, &input).unwrap();
        if step == 0 {
            ctrl.open_scene_by_id(&mut reco, "seed_game_flappy_bird");
            let _ = ctrl.drain_emitted_telemetry();
        }
        ctrl.on_frame_output(17, &frame, Some(vm.game_telemetry()));
    }
    let flushed = ctrl.flush_active_telemetry(&mut reco).unwrap();
    assert_eq!(flushed.kind, SceneBehaviorEventKind::GameInteraction);
    assert!(flushed.game_inputs > 0);

    // Explicit fallback check with synthetic short GameSessionTelemetry
    ctrl.open_scene_by_id(&mut reco, "seed_game_neon_snake");
    let _ = ctrl.drain_emitted_telemetry();
    ctrl.record_game_telemetry(GameSessionTelemetry {
        input_count: 5,
        play_ms: 900,
        restarts: 0,
        best_score: 20,
        current_score: 20,
        game_over_count: 0,
    });
    let flushed2 = ctrl.flush_active_telemetry(&mut reco).unwrap();
    assert_eq!(flushed2.kind, SceneBehaviorEventKind::GameInteraction);
    assert_eq!(flushed2.game_inputs, 5);
}

#[test]
fn test_strict_prefix_immutability_and_live_tail_reranking_across_seam() {
    let mut reco = CodeSceneRecommender::new_with_seeds();
    reco.set_now_ms(Some(EVAL_BASE_TS_MS));
    let mut ctrl = FeedSessionController::new(301, "sess_prefix")
        .with_page_size(12)
        .with_clock_ms(EVAL_BASE_TS_MS);
    ctrl.load_initial_feed(&mut reco);

    // Advance to index 3 so we have a multi-card viewed prefix [0..=3]
    for _ in 0..3 {
        ctrl.on_card_dwell_tick(5_000, 0);
        ctrl.swipe_next(&mut reco);
    }
    assert_eq!(ctrl.current_index(), 3);

    let non_hard_kinds = [
        SceneBehaviorEventKind::Like,
        SceneBehaviorEventKind::DoubleTapLike,
        SceneBehaviorEventKind::Bookmark,
        SceneBehaviorEventKind::Comment,
        SceneBehaviorEventKind::Share,
        SceneBehaviorEventKind::FollowAuthor,
        SceneBehaviorEventKind::InspectCode,
        SceneBehaviorEventKind::Remix,
        SceneBehaviorEventKind::Unlike,
        SceneBehaviorEventKind::Unbookmark,
        SceneBehaviorEventKind::UnfollowAuthor,
    ];

    let locked_prefix_before: Vec<String> = ctrl.items()[0..=3]
        .iter()
        .map(|i| i.asset.scene_id.clone())
        .collect();
    let prior_history_before = ctrl.items()[0..3].to_vec();

    for kind in non_hard_kinds {
        ctrl.record_interaction(&mut reco, kind, None);
        assert_eq!(ctrl.current_index(), 3, "current_index mutated on {kind:?}");
        let locked_prefix_after: Vec<String> = ctrl.items()[0..=3]
            .iter()
            .map(|i| i.asset.scene_id.clone())
            .collect();
        assert_eq!(
            locked_prefix_before, locked_prefix_after,
            "items[0..=current_index] scene order mutated on {kind:?}"
        );
        assert_eq!(
            prior_history_before,
            ctrl.items()[0..3],
            "prior history items[0..current_index] mutated on {kind:?}"
        );
        // Zero scatter violations across the seam [0..=11]
        assert_eq!(
            count_scatter_violations(ctrl.items(), true),
            0,
            "seam scatter violated after {kind:?}"
        );
    }

    // swipe_prev never reorders items
    let snapshot: Vec<String> = ctrl.items().iter().map(|i| i.asset.scene_id.clone()).collect();
    ctrl.swipe_prev(&mut reco);
    assert_eq!(ctrl.current_index(), 2);
    assert_eq!(
        snapshot,
        ctrl.items().iter().map(|i| i.asset.scene_id.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn test_active_card_hard_negatives_and_filters_and_deep_link() {
    let mut reco = CodeSceneRecommender::new_with_seeds();
    reco.set_now_ms(Some(EVAL_BASE_TS_MS));
    let mut ctrl = FeedSessionController::new(401, "sess_hardneg")
        .with_page_size(8)
        .with_clock_ms(EVAL_BASE_TS_MS);
    ctrl.load_initial_feed(&mut reco);

    // Move to index 2 so items[0..2] is non-empty viewed prefix
    ctrl.on_card_dwell_tick(4_000, 0);
    ctrl.swipe_next(&mut reco);
    ctrl.on_card_dwell_tick(4_000, 0);
    ctrl.swipe_next(&mut reco);
    assert_eq!(ctrl.current_index(), 2);

    let prefix_before = ctrl.items()[0..2].to_vec();
    let dismissed_scene = ctrl.current_item().unwrap().asset.scene_id.clone();

    // Active-card NotInterested dismisses current card, preserves items[0..2], and purges it from tail
    ctrl.record_interaction(&mut reco, SceneBehaviorEventKind::NotInterested, None);
    assert_eq!(ctrl.items()[0..2], prefix_before);
    assert_ne!(ctrl.current_item().unwrap().asset.scene_id, dismissed_scene);
    assert!(
        ctrl.items()[2..]
            .iter()
            .all(|i| i.asset.scene_id != dismissed_scene)
    );

    // Active-card HideAuthor purges all items by that author from remaining tail & prefetched pages
    let hidden_author = ctrl.current_item().unwrap().asset.author_user_id;
    ctrl.record_interaction(&mut reco, SceneBehaviorEventKind::HideAuthor, None);
    assert!(
        ctrl.items()[2..]
            .iter()
            .all(|i| i.asset.author_user_id != hidden_author)
    );
    let _ = ctrl.prefetch_if_needed(&reco);
    assert!(
        ctrl.items()[2..]
            .iter()
            .all(|i| i.asset.author_user_id != hidden_author)
    );

    // Active-card HideTag purges that tag from tail
    ctrl.record_interaction(
        &mut reco,
        SceneBehaviorEventKind::HideTag,
        Some("cyberpunk".to_string()),
    );
    assert!(
        ctrl.items()[2..]
            .iter()
            .all(|i| !normalize_tags(&i.asset.tags).iter().any(|t| t == "cyberpunk"))
    );

    // Tab switching, tag filtering, query filtering, and deep-link open_scene_by_id
    let mut ctrl2 = FeedSessionController::new(402, "sess_nav").with_clock_ms(EVAL_BASE_TS_MS);
    // Cold empty open_scene_by_id
    assert!(ctrl2.open_scene_by_id(&mut reco, "nonexistent_scene").is_none());
    let opened = ctrl2.open_scene_by_id(&mut reco, "seed_game_flappy_bird").unwrap();
    assert_eq!(opened.asset.scene_id, "seed_game_flappy_bird");
    assert_eq!(ctrl2.current_index(), 0);

    // Warm mid-feed open_scene_by_id preserves back-swipe history
    let opened2 = ctrl2.open_scene_by_id(&mut reco, "seed_anim_aurora_plasma").unwrap();
    assert_eq!(opened2.asset.scene_id, "seed_anim_aurora_plasma");
    assert_eq!(ctrl2.current_index(), 1);
    assert_eq!(ctrl2.swipe_prev(&mut reco).unwrap().asset.scene_id, "seed_game_flappy_bird");

    // Tab, tag, and query filters
    let games_n = ctrl2.switch_tab(&mut reco, CodeSceneFeedTab::Games);
    assert_eq!(games_n, 6);
    assert!(ctrl2.items().iter().all(|i| i.asset.kind == gpui_telegram_proto::AssetKind::Game));

    let tag_n = ctrl2.set_tag_filter(&mut reco, Some("#Snake".to_string()));
    assert_eq!(tag_n, 1);
    assert_eq!(ctrl2.tag_filter(), Some("snake"));

    let _ = ctrl2.set_tag_filter(&mut reco, None);
    let _ = ctrl2.switch_tab(&mut reco, CodeSceneFeedTab::ForYou);
    let q_n = ctrl2.set_query(&mut reco, Some("霓虹飞鸟".to_string()));
    assert!(q_n >= 1);
    assert!(
        ctrl2
            .items()
            .iter()
            .any(|i| i.asset.scene_id == "seed_game_flappy_bird")
    );
}

#[test]
fn test_iter2_quick_positive_cold_deeplink_exhausted_hide_clock_overflow_and_dwell_rerank() {
    // Item 10: Quick positive (< 1500ms DoubleTapLike AND 2000ms soft-zone Bookmark) + swipe_next
    // must leave total_negatives == 0, u_neg norm_sq == 0.0, and zero fatigue on liked/bookmarked tags.
    let mut reco = CodeSceneRecommender::new_with_seeds();
    reco.set_now_ms(Some(EVAL_BASE_TS_MS));
    let mut ctrl = FeedSessionController::new(9_101, "sess_qpos")
        .with_page_size(8)
        .with_clock_ms(EVAL_BASE_TS_MS);
    ctrl.load_initial_feed(&mut reco);
    let _ = ctrl.drain_emitted_telemetry();

    let first_tags = normalize_tags(&ctrl.current_item().unwrap().asset.tags);
    ctrl.on_card_dwell_tick(400, 0);
    ctrl.record_interaction(&mut reco, SceneBehaviorEventKind::DoubleTapLike, None);
    ctrl.swipe_next(&mut reco);

    let second_tags = normalize_tags(&ctrl.current_item().unwrap().asset.tags);
    ctrl.on_card_dwell_tick(2_000, 0);
    ctrl.record_interaction(&mut reco, SceneBehaviorEventKind::Bookmark, None);
    ctrl.swipe_next(&mut reco);

    let evs = ctrl.drain_emitted_telemetry();
    assert!(!evs.iter().any(|e| e.kind == SceneBehaviorEventKind::FastSkip));
    assert!(!evs.iter().any(|e| e.kind == SceneBehaviorEventKind::Dwell));

    let prof = reco.export_user_profile(9_101).unwrap();
    assert_eq!(prof.total_negatives, 0, "quick positive swipe triggered fast-skip negative");
    let u_neg_norm_sq: f32 = prof.u_neg.iter().map(|x| x * x).sum();
    assert!(u_neg_norm_sq <= 1e-9, "u_neg was polluted: {u_neg_norm_sq}");
    for tag in first_tags.iter().chain(second_tags.iter()) {
        // Card 3's Impression may add a gentle 0.06 exposure cooldown if it shares a tag,
        // whereas a fast-skip would inflict a +0.35 fatigue bump.
        let fat = prof.tag_fatigue.get(tag).copied().unwrap_or(0.0);
        assert!(
            fat <= 0.12,
            "liked/bookmarked tag {tag} suffered fast-skip fatigue bump: {fat}"
        );
    }

    // Item 11: Cold deep-link open_scene_by_id must immediately populate an unseen tail & cursor
    let mut cold_ctrl = FeedSessionController::new(9_102, "sess_cold_dl")
        .with_page_size(8)
        .with_clock_ms(EVAL_BASE_TS_MS);
    let opened = cold_ctrl
        .open_scene_by_id(&mut reco, "seed_game_flappy_bird")
        .unwrap();
    assert_eq!(opened.asset.scene_id, "seed_game_flappy_bird");
    assert_eq!(cold_ctrl.current_index(), 0);
    assert_eq!(cold_ctrl.preload_window_indices(), vec![0, 1]);
    assert!(cold_ctrl.items().len() >= 2, "cold open_scene_by_id froze feed at 1 item");
    assert!(cold_ctrl.next_cursor().is_some());
    let unique_dl: HashSet<&str> = cold_ctrl
        .items()
        .iter()
        .map(|i| i.asset.scene_id.as_str())
        .collect();
    assert_eq!(unique_dl.len(), cold_ctrl.items().len());
    assert_eq!(count_scatter_violations(cold_ctrl.items(), true), 0);

    cold_ctrl.on_card_dwell_tick(4_500, 0);
    let next_id = cold_ctrl
        .swipe_next(&mut reco)
        .unwrap()
        .asset
        .scene_id
        .clone();
    assert_eq!(cold_ctrl.current_index(), 1);
    assert_ne!(next_id, "seed_game_flappy_bird");

    // Item 12: Exhausted-feed final-card HideTag must NEVER land on or emit Impression for a blocked card
    let mut tiny_reco = CodeSceneRecommender::new_empty();
    tiny_reco.set_now_ms(Some(EVAL_BASE_TS_MS));
    for idx in 0..4usize {
        tiny_reco.register_or_update_asset(
            CodeSceneAssetDto {
                scene_id: format!("cp_{idx}"),
                title: format!("Cyberpunk {idx}"),
                kind: AssetKind::Animation,
                tags: vec!["cyberpunk".to_string()],
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
    let mut ex_ctrl = FeedSessionController::new(9_103, "sess_exhaust")
        .with_page_size(4)
        .with_clock_ms(EVAL_BASE_TS_MS);
    assert_eq!(ex_ctrl.load_initial_feed(&mut tiny_reco), 4);
    for _ in 0..3 {
        ex_ctrl.on_card_dwell_tick(4_000, 0);
        ex_ctrl.swipe_next(&mut tiny_reco);
    }
    assert_eq!(ex_ctrl.current_index(), 3);
    let _ = ex_ctrl.drain_emitted_telemetry();

    ex_ctrl.record_interaction(
        &mut tiny_reco,
        SceneBehaviorEventKind::HideTag,
        Some("cyberpunk".to_string()),
    );
    assert!(
        ex_ctrl.items().is_empty() && ex_ctrl.current_item().is_none(),
        "exhausted HideTag landed on a newly blocked prior card!"
    );
    let after_hide_evs = ex_ctrl.drain_emitted_telemetry();
    assert!(
        !after_hide_evs
            .iter()
            .any(|e| e.kind == SceneBehaviorEventKind::Impression),
        "emitted Impression on a newly blocked card!"
    );

    // Item 9 (FeedSessionController): HideTag with reason="   ###   " hides effective_primary_tag, NOT "game"
    let mut ht_ctrl = FeedSessionController::new(9_104, "sess_ht")
        .with_page_size(8)
        .with_clock_ms(EVAL_BASE_TS_MS);
    ht_ctrl.open_scene_by_id(&mut reco, "seed_game_flappy_bird");
    ht_ctrl.record_interaction(
        &mut reco,
        SceneBehaviorEventKind::HideTag,
        Some("   ###   ".to_string()),
    );
    let ht_prof = reco.export_user_profile(9_104).unwrap();
    let flappy_seed = find_seed_by_id("seed_game_flappy_bird").unwrap();
    let expected_primary = gpui_telegram_code_scene::reco::effective_primary_tag(&normalize_tags(
        &flappy_seed.tags,
    ))
    .unwrap();
    assert!(ht_prof.hidden_tags.contains(&expected_primary));
    assert!(!ht_prof.hidden_tags.contains("game"));
    assert!(
        ht_ctrl.switch_tab(&mut reco, CodeSceneFeedTab::Games) >= 1,
        "HideTag with whitespace/hash reason falsely blocked all games"
    );

    // Item 13: Clock overflow guard on delta_ms > i64::MAX (never wraps negative!)
    let mut clk_ctrl = FeedSessionController::new(9_105, "sess_clk").with_clock_ms(EVAL_BASE_TS_MS);
    clk_ctrl.load_initial_feed(&mut reco);
    clk_ctrl.on_card_dwell_tick((i64::MAX as u64) + 1, 1);
    assert_eq!(clk_ctrl.clock_ms(), i64::MAX);
}

#[test]
fn test_iter4_hard_negative_purges_blocked_prefix_for_swipe_prev_and_preload() {
    let mk_item = |sid: &str, author: i64, tags: &[&str], plays: u64| CodeSceneAssetDto {
        scene_id: sid.to_string(),
        title: sid.to_string(),
        kind: AssetKind::Animation,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        author_user_id: author,
        source_code: "fn on_render(s,t,c){clear(0x000000FF);}".to_string(),
        code_format: CODE_SCENE_FORMAT_CS1.to_string(),
        manifest: AssetManifest::for_kind(AssetKind::Animation),
        plays_count: plays,
        likes_count: plays / 4,
        created_at_ms: EVAL_BASE_TS_MS,
        ..Default::default()
    };

    // 1. HideAuthor purges earlier cards by the hidden author in BOTH exhausted and non-exhausted feeds
    for with_tail in [false, true] {
        let mut reco = CodeSceneRecommender::new_empty();
        reco.set_now_ms(Some(EVAL_BASE_TS_MS));
        reco.register_or_update_asset(mk_item("sp_a0", 700, &["tag_a0"], 500), None);
        reco.register_or_update_asset(mk_item("sp_1", 701, &["tag_s1"], 400), None);
        reco.register_or_update_asset(mk_item("sp_2", 702, &["tag_s2"], 350), None);
        reco.register_or_update_asset(mk_item("sp_3", 703, &["tag_s3"], 300), None);
        reco.register_or_update_asset(mk_item("sp_a4", 700, &["tag_a4"], 200), None);
        if with_tail {
            reco.register_or_update_asset(mk_item("sp_s5", 705, &["tag_s5"], 100), None);
        }

        let mut ctrl = FeedSessionController::new(9_201, "sess_sp_auth")
            .with_page_size(6)
            .with_clock_ms(EVAL_BASE_TS_MS);
        ctrl.load_initial_feed(&mut reco);

        // Find the second occurrence of author 700 in the loaded feed
        let auth700_positions: Vec<usize> = ctrl
            .items()
            .iter()
            .enumerate()
            .filter(|(_, it)| it.asset.author_user_id == 700)
            .map(|(idx, _)| idx)
            .collect();
        assert_eq!(auth700_positions.len(), 2);
        let target_pos = auth700_positions[1];

        while ctrl.current_index() < target_pos {
            ctrl.on_card_dwell_tick(4_000, 0);
            ctrl.swipe_next(&mut reco);
        }
        assert_eq!(ctrl.current_item().unwrap().asset.author_user_id, 700);
        let safe_before: Vec<String> = ctrl.items()[..target_pos]
            .iter()
            .filter(|it| it.asset.author_user_id != 700)
            .map(|it| it.asset.scene_id.clone())
            .collect();
        let _ = ctrl.drain_emitted_telemetry();

        ctrl.record_interaction(&mut reco, SceneBehaviorEventKind::HideAuthor, None);
        assert!(
            ctrl.items().iter().all(|it| it.asset.author_user_id != 700),
            "HideAuthor (with_tail={with_tail}) left hidden author in session items"
        );
        assert!(
            ctrl.preload_window_items()
                .iter()
                .all(|(_, it)| it.asset.author_user_id != 700),
            "HideAuthor (with_tail={with_tail}) left hidden author in preload window"
        );
        let kept_prefix: Vec<String> = ctrl.items()[..safe_before.len()]
            .iter()
            .map(|it| it.asset.scene_id.clone())
            .collect();
        assert_eq!(kept_prefix, safe_before, "non-blocked viewed prefix order was disturbed");

        // Swipe all the way back to index 0 -> must NEVER land on or emit Impression for author 700
        for _ in 0..10 {
            if let Some(cur) = ctrl.swipe_prev(&mut reco) {
                assert_ne!(cur.asset.author_user_id, 700);
            }
        }
        let emitted = ctrl.drain_emitted_telemetry();
        assert!(
            !emitted.iter().any(|e| {
                e.kind == SceneBehaviorEventKind::Impression && e.author_user_id == Some(700)
            }),
            "swipe_prev emitted Impression for hidden author 700 (with_tail={with_tail})"
        );
    }

    // 2. HideTag purges earlier cards matching the hidden tag so swipe_prev never lands on them
    let mut tag_reco = CodeSceneRecommender::new_empty();
    tag_reco.set_now_ms(Some(EVAL_BASE_TS_MS));
    tag_reco.register_or_update_asset(mk_item("t0", 801, &["doom"], 500), None);
    tag_reco.register_or_update_asset(mk_item("t1", 802, &["calm"], 400), None);
    tag_reco.register_or_update_asset(mk_item("t2", 803, &["zen"], 350), None);
    tag_reco.register_or_update_asset(mk_item("t3", 804, &["doom"], 250), None);
    tag_reco.register_or_update_asset(mk_item("t4", 805, &["aurora"], 150), None);

    let mut tag_ctrl = FeedSessionController::new(9_202, "sess_sp_tag")
        .with_page_size(6)
        .with_clock_ms(EVAL_BASE_TS_MS);
    tag_ctrl.load_initial_feed(&mut tag_reco);
    let doom_positions: Vec<usize> = tag_ctrl
        .items()
        .iter()
        .enumerate()
        .filter(|(_, it)| normalize_tags(&it.asset.tags).iter().any(|t| t == "doom"))
        .map(|(idx, _)| idx)
        .collect();
    assert_eq!(doom_positions.len(), 2);
    while tag_ctrl.current_index() < doom_positions[1] {
        tag_ctrl.on_card_dwell_tick(4_000, 0);
        tag_ctrl.swipe_next(&mut tag_reco);
    }
    let _ = tag_ctrl.drain_emitted_telemetry();

    tag_ctrl.record_interaction(&mut tag_reco, SceneBehaviorEventKind::HideTag, Some("doom".to_string()));
    assert!(
        tag_ctrl
            .items()
            .iter()
            .all(|it| !normalize_tags(&it.asset.tags).iter().any(|t| t == "doom"))
    );
    for _ in 0..10 {
        if let Some(cur) = tag_ctrl.swipe_prev(&mut tag_reco) {
            assert!(!normalize_tags(&cur.asset.tags).iter().any(|t| t == "doom"));
        }
    }
    let tag_emitted = tag_ctrl.drain_emitted_telemetry();
    assert!(
        !tag_emitted.iter().any(|e| {
            e.kind == SceneBehaviorEventKind::Impression
                && normalize_tags(&e.tags).iter().any(|t| t == "doom")
        }),
        "swipe_prev emitted Impression for hidden tag 'doom'"
    );

    // 3. Spaced hashes "  #  #  " in FeedSessionController HideTag falls back to "physics", NOT "#"
    let mut seed_reco = CodeSceneRecommender::new_with_seeds();
    seed_reco.set_now_ms(Some(EVAL_BASE_TS_MS));
    let mut hash_ctrl = FeedSessionController::new(9_203, "sess_spaced_hash")
        .with_page_size(8)
        .with_clock_ms(EVAL_BASE_TS_MS);
    hash_ctrl.open_scene_by_id(&mut seed_reco, "seed_game_orbit_catcher");
    hash_ctrl.record_interaction(
        &mut seed_reco,
        SceneBehaviorEventKind::HideTag,
        Some("  #  #  ".to_string()),
    );
    let hp = seed_reco.export_user_profile(9_203).unwrap();
    assert!(hp.hidden_tags.contains("physics"));
    assert!(!hp.hidden_tags.contains("#"));
    assert!(!hp.hidden_tags.contains("game"));
}

