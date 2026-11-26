//! Stage 2: Bayesian-Smoothed Multi-Task eCPM Online Scorer & UCB1 Exploration (`CS-F06`).

use std::collections::BTreeMap;

use gpui_telegram_proto::ScoreBreakdown;

use crate::reco::embedding::cosine_similarity;
use crate::reco::profile::{RecommenderConfig, UserInterestProfile};
use crate::reco::recall::{CatalogEntry, RecallCandidate};

/// Prior means for the 8 multi-task objectives smoothed with `alpha = 8.0`.
const PRIOR_DWELL: f64 = 0.45;
const PRIOR_FINISH: f64 = 0.32;
const PRIOR_LIKE: f64 = 0.14;
const PRIOR_BOOKMARK: f64 = 0.05;
const PRIOR_COMMENT: f64 = 0.02;
const PRIOR_SHARE: f64 = 0.03;
const PRIOR_REMIX: f64 = 0.015;
const PRIOR_SKIP: f64 = 0.15;

/// Output of Stage 2 scoring for one candidate asset.
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    pub recall: RecallCandidate,
    pub quality_ecpm: f64,
    pub negative_penalty: f64,
    pub base_score: f64,
    pub exploration_bonus: f64,
    /// `base_score + exploration_bonus` before Stage 3 MMR normalization.
    pub exp_score: f64,
    pub traffic_tier: &'static str,
    pub effective_plays: u64,
    pub zero_impression_brand_new: bool,
    pub breakdown: ScoreBreakdown,
}

/// Scores all Stage 1 candidates with Bayesian multi-task eCPM + adaptive personalization + UCB1.
pub fn score_candidates(
    candidates: &[RecallCandidate],
    catalog: &BTreeMap<String, CatalogEntry>,
    profile: &UserInterestProfile,
    total_catalog_plays: u64,
    has_query: bool,
    cfg: &RecommenderConfig,
) -> Vec<ScoredCandidate> {
    let mut out = Vec::with_capacity(candidates.len());
    for cand in candidates {
        if let Some(entry) = catalog.get(&cand.scene_id) {
            out.push(score_one_candidate(
                cand,
                entry,
                profile,
                total_catalog_plays,
                has_query,
                cfg,
            ));
        }
    }
    out
}

/// Computes Bayesian-smoothed 8-task quality eCPM + personalization + UCB1 for a single asset.
pub fn score_one_candidate(
    cand: &RecallCandidate,
    entry: &CatalogEntry,
    profile: &UserInterestProfile,
    total_catalog_plays: u64,
    has_query: bool,
    cfg: &RecommenderConfig,
) -> ScoredCandidate {
    let alpha = cfg.bayes_alpha.max(1.0);
    let eff_plays = entry.effective_plays();
    let n = eff_plays as f64;

    // 1. Bayesian empirical smoothing across all 8 multi-task objectives
    let live_trials = entry
        .stats
        .impressions
        .max(entry.stats.finish_count.saturating_add(entry.stats.skip_count))
        .max(entry.stats.dwell_events);
    let hist_dwell_plays = entry.asset.plays_count.saturating_sub(entry.stats.dwell_events);
    let hist_dwell_proxy = if entry.asset.plays_count > 0 {
        ((entry.asset.likes_count as f64)
            * 2.4
            * (hist_dwell_plays as f64 / entry.asset.plays_count as f64))
            .min(hist_dwell_plays as f64)
    } else {
        0.0
    };
    let p_dwell = bayesian_rate(
        hist_dwell_proxy + entry.stats.dwell_score_sum,
        hist_dwell_plays as f64 + entry.stats.dwell_events as f64,
        PRIOR_DWELL,
        alpha,
    );

    let hist_plays = entry.asset.plays_count.saturating_sub(live_trials);
    let hist_finish_proxy = if entry.asset.plays_count > 0 {
        ((entry.asset.likes_count as f64)
            * 2.1
            * (hist_plays as f64 / entry.asset.plays_count as f64))
            .min(hist_plays as f64)
    } else {
        0.0
    };
    let p_finish = bayesian_rate(
        hist_finish_proxy + entry.stats.finish_count as f64,
        hist_plays as f64 + live_trials as f64,
        PRIOR_FINISH,
        alpha,
    );
    let p_like = bayesian_rate(entry.asset.likes_count as f64, n, PRIOR_LIKE, alpha);
    let p_bookmark = bayesian_rate(entry.asset.bookmarks_count as f64, n, PRIOR_BOOKMARK, alpha);
    let p_comment = bayesian_rate(entry.asset.comments_count as f64, n, PRIOR_COMMENT, alpha);
    let p_share = bayesian_rate(entry.asset.shares_count as f64, n, PRIOR_SHARE, alpha);
    let p_remix = bayesian_rate(entry.asset.remixes_count as f64, n, PRIOR_REMIX, alpha);
    let p_skip = bayesian_rate(
        entry.stats.skip_count as f64,
        live_trials as f64,
        PRIOR_SKIP,
        alpha,
    );

    let quality_ecpm = (0.85 * p_dwell
        + 1.35 * p_finish
        + 1.95 * p_like
        + 2.35 * p_bookmark
        + 1.90 * p_comment
        + 2.55 * p_share
        + 2.95 * p_remix
        - 1.35 * p_skip)
        .clamp(0.0, 2.6);

    // 2. Fatigue & negative repulsion penalty
    let neg_sim = cosine_similarity(&profile.u_neg, &entry.embedding).max(0.0);
    let mut tag_fat_sum = 0.0_f64;
    let mut tag_fat_cnt = 0usize;
    for tag in &entry.normalized_tags {
        if let Some(&f) = profile.tag_fatigue.get(tag) {
            tag_fat_sum += f;
            tag_fat_cnt += 1;
        }
    }
    let mean_tag_fatigue = if tag_fat_cnt > 0 {
        tag_fat_sum / (tag_fat_cnt as f64)
    } else {
        0.0
    };
    let kind_fatigue = profile
        .kind_fatigue
        .get(&entry.asset.kind)
        .copied()
        .unwrap_or(0.0);
    let creator_fatigue = profile
        .creator_fatigue
        .get(&entry.asset.author_user_id)
        .copied()
        .unwrap_or(0.0);
    let fatigue_term = 0.55 * mean_tag_fatigue + 0.50 * kind_fatigue + 0.30 * creator_fatigue;

    let negative_penalty = (1.80 * neg_sim
        + 1.50 * fatigue_term
        + if cand.in_seen_cooldown { 0.45 } else { 0.0 })
    .clamp(0.0, 4.5);

    // 3. Adaptive fusion: cold-start users rely on quality_ecpm, warm users lead with personalization
    let profile_maturity =
        (((profile.total_positives + profile.total_negatives) as f64) / 5.0).clamp(0.0, 1.0);
    let ecpm_weight = 0.78 - 0.42 * profile_maturity;
    let fast_track_boost = if cand.is_fast_track { 10.0 } else { 0.0 };

    let base_score = 2.45 * cand.embedding_sim
        + 1.35 * cand.swing_cf
        + 1.85 * cand.tag_kind_affinity
        + 1.05 * cand.creator_affinity
        + ecpm_weight * quality_ecpm
        + 0.55 * cand.freshness
        + 1.90 * cand.query_match_score
        + fast_track_boost
        - negative_penalty;

    // 4. UCB1 exploration bonus (capped at +0.85, damped during search queries)
    let total_impr = (total_catalog_plays
        .saturating_add(profile.total_impressions)
        .saturating_add(10)) as f64;
    let raw_ucb = 0.42 * (total_impr.ln() / (n + 1.0)).max(0.0).sqrt();
    let ucb_scaled = if has_query { raw_ucb * 0.25 } else { raw_ucb };
    let exploration_bonus = ucb_scaled.clamp(0.0, cfg.ucb_cap);

    let exp_score = base_score + exploration_bonus;
    let zero_impression_brand_new =
        entry.asset.plays_count == 0 && entry.stats.impressions == 0;

    let breakdown = ScoreBreakdown {
        embedding_sim: cand.embedding_sim,
        swing_cf: cand.swing_cf,
        tag_kind_affinity: cand.tag_kind_affinity,
        creator_affinity: cand.creator_affinity,
        quality_ecpm,
        freshness: cand.freshness,
        exploration_bonus,
        mmr_penalty: 0.0,
        negative_penalty,
        final_score: exp_score,
    }
    .sanitized();

    ScoredCandidate {
        recall: cand.clone(),
        quality_ecpm,
        negative_penalty,
        base_score,
        exploration_bonus,
        exp_score,
        traffic_tier: entry.traffic_tier(),
        effective_plays: eff_plays,
        zero_impression_brand_new,
        breakdown,
    }
}

fn bayesian_rate(observed: f64, trials: f64, prior: f64, alpha: f64) -> f64 {
    let obs = if observed.is_finite() {
        observed.max(0.0)
    } else {
        0.0
    };
    let n = if trials.is_finite() {
        trials.max(0.0)
    } else {
        0.0
    };
    let clamped_obs = obs.min(n);
    let post = (clamped_obs + alpha * prior) / (n + alpha);
    if post.is_finite() {
        post.clamp(0.0, 1.0)
    } else {
        prior
    }
}
