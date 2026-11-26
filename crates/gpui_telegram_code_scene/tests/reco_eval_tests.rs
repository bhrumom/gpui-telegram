//! Quantitative Offline RecSys Evaluation Tests (`CS-F07`).

use gpui_telegram_code_scene::reco::{
    EvalPersona, assert_eval_passes, build_eval_corpus, run_full_eval,
};

#[test]
fn test_quantitative_offline_recsys_eval_passes_all_gates() {
    let corpus = build_eval_corpus();
    assert_eq!(corpus.len(), 90, "expected 18 real seeds + 72 synthetic assets");

    let report = run_full_eval();
    eprintln!(
        "=== RecSys Offline Eval Report ===\n\
         corpus_size={} total_swipes={}\n\
         mean_ndcg@10: reco={:.4} vs popularity={:.4} vs random={:.4}\n\
         mean_hr@5:    reco={:.4} vs popularity={:.4} vs random={:.4}\n\
         pivot: pre_A={} refreshes={} post_B={} post_A={} post_B_ndcg@10={:.4}\n\
         cold_start: publisher_slot={:?} other_user_slot={:?}\n\
         blocklist: ni_leaks={} author_leaks={} tag_leaks={} audited={}\n\
         official_18_seeds: scatter_violations={} ild={:.4}\n\
         cold_user_distinct_kinds_top10={}",
        report.corpus_size,
        report.total_swipes_simulated,
        report.comparison.recommender_mean_ndcg_at_10,
        report.comparison.popularity_mean_ndcg_at_10,
        report.comparison.random_mean_ndcg_at_10,
        report.comparison.recommender_mean_hit_rate_at_5,
        report.comparison.popularity_mean_hit_rate_at_5,
        report.comparison.random_mean_hit_rate_at_5,
        report.pivot.pre_pivot_topic_a_in_top5,
        report.pivot.post_pivot_refreshes_needed,
        report.pivot.post_pivot_topic_b_in_top5,
        report.pivot.post_pivot_topic_a_in_top5,
        report.pivot.post_pivot_topic_b_ndcg_at_10,
        report.cold_start.publisher_slot_index,
        report.cold_start.other_user_slot_index,
        report.blocklist.not_interested_leaks,
        report.blocklist.hide_author_leaks,
        report.blocklist.hide_tag_leaks,
        report.blocklist.total_items_audited,
        report.official_18_seeds_scatter_violations,
        report.official_18_seeds_ild,
        report.cold_start_new_user_distinct_kinds_in_top10,
    );
    for p in &report.personas {
        eprintln!(
            "  persona={:?} swipes={} ndcg@10={:.4} hr@5={:.4} bin@5={:.2} ild={:.4} violations={}",
            p.persona,
            p.swipes_simulated,
            p.ndcg_at_10,
            p.hit_rate_at_5,
            p.binary_hit_at_5,
            p.mean_ild,
            p.scatter_violations
        );
    }

    assert_eval_passes(&report);

    // Extra explicit check that all 5 personas ran cleanly
    assert_eq!(report.personas.len(), 5);
    assert!(report.personas.iter().any(|p| p.persona == EvalPersona::GameLover));
    assert!(report.personas.iter().any(|p| p.persona == EvalPersona::ShaderArtFan));
    assert!(report.personas.iter().any(|p| p.persona == EvalPersona::ShortVideoViewer));
    assert!(report.personas.iter().any(|p| p.persona == EvalPersona::ColdStartNewUser));
    assert!(report.personas.iter().any(|p| p.persona == EvalPersona::MidSessionTopicSwitcher));
}

#[test]
fn test_quantitative_eval_is_bit_deterministic() {
    let r1 = serde_json::to_string(&run_full_eval()).unwrap();
    let r2 = serde_json::to_string(&run_full_eval()).unwrap();
    assert_eq!(r1, r2, "offline eval must be 100% deterministic across runs");
}
