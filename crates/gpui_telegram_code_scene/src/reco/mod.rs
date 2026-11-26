//! 5-Stage Douyin-Grade Recommendation Engine (`CS-F05`, `CS-F06`, `CS-F07`).

pub mod embedding;
pub mod engine;
pub mod eval;
pub mod profile;
pub mod recall;
pub mod reranker;
pub mod scorer;
pub mod telemetry;

pub use embedding::{
    EMBEDDING_DIM, cosine_similarity, effective_primary_tag, effective_query_vector, embed_asset,
    embed_query, extract_code_feature_tokens, item_similarity, jaccard_similarity, l2_normalize,
    normalize_tag, normalize_tags,
};
pub use engine::CodeSceneRecommender;
pub use eval::{
    BaselineComparison, ColdStartEvalMetrics, EVAL_BASE_TS_MS, EvalPersona,
    NegativeBlocklistEvalMetrics, PersonaEvalMetrics, PivotEvalMetrics, RecoEvalReport,
    assert_eval_passes, build_eval_corpus, compute_hit_rate_at_5, compute_ild, compute_ndcg_at_k,
    count_scatter_violations, oracle_relevance, run_full_eval,
};
pub use profile::{RecommenderConfig, SeenEntry, UserInterestProfile};
pub use recall::{
    CHANNEL_COLD_START, CHANNEL_CREATOR_GRAPH, CHANNEL_EMBEDDING, CHANNEL_HOT,
    CHANNEL_JUST_PUBLISHED, CHANNEL_SWING_CF, CHANNEL_TAG_KIND, TIER0_COLD_START, TIER1_SEED,
    TIER2_RISING, TIER3_VIRAL, freshness_score, traffic_tier_for_plays, wilson_lower_bound,
};
pub use telemetry::{
    FAST_SKIP_HARD_MS, FAST_SKIP_SOFT_MS, ItemEngagementStats, REWARD_MAX, REWARD_MIN,
    RewardOutcome,
};
