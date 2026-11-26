//! Stage-0 Behavioral Telemetry & Kind-Normalized Reward Shaping (`CS-F05`).
//!
//! Handles all 20 `SceneBehaviorEventKind` variants exhaustively with strict
//! `reward ∈ [-2.5, +5.0]`, finite sanitization, kind-normalized Video/Animation/Game
//! reward parity, idempotent undo semantics, persistent hard blocklists, and rapid
//! dual-timescale online interest updates.

use gpui_telegram_proto::{AssetKind, SceneBehaviorEventDto, SceneBehaviorEventKind};
use serde::{Deserialize, Serialize};

use crate::reco::embedding::{
    EMBEDDING_DIM, cosine_similarity, effective_primary_tag, first_non_generic_tag, l2_normalize,
    normalize_tag, normalize_tags,
};
use crate::reco::profile::{RecommenderConfig, UserInterestProfile};

pub const REWARD_MIN: f64 = -2.5;
pub const REWARD_MAX: f64 = 5.0;
pub const FAST_SKIP_HARD_MS: u64 = 1_500;
pub const FAST_SKIP_SOFT_MS: u64 = 3_000;
pub const MAX_CLAMPED_DWELL_MS: u64 = 600_000;
pub const MIN_REF_DURATION_MS: u64 = 5_000;

/// Structured result of recording one behavioral telemetry event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RewardOutcome {
    pub event_kind: SceneBehaviorEventKind,
    pub scene_id: String,
    pub asset_kind: AssetKind,
    /// Strictly finite instant reward clamped to `[-2.5, +5.0]`.
    pub reward: f64,
    pub profile_updated: bool,
    /// `true` for `NotInterested`, `HideAuthor`, `HideTag`.
    pub hard_negative_applied: bool,
    /// `true` for `FastSkip` or short-dwell fast-skip branches.
    pub fast_skip_detected: bool,
    /// `true` when `Unlike`/`Unbookmark`/`UnfollowAuthor` reverted an active state.
    pub undo_reverted_prior: bool,
}

/// Per-asset runtime engagement stats accumulated online for Stage 2 Bayesian smoothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemEngagementStats {
    pub impressions: u64,
    pub dwell_score_sum: f64,
    pub dwell_events: u64,
    pub finish_count: u64,
    pub skip_count: u64,
    pub not_interested_count: u64,
}

/// Resolved metadata for a telemetry event target (catalog-first, event-fallback).
pub struct ResolvedEventTarget<'a> {
    pub scene_id: &'a str,
    pub asset_kind: Option<AssetKind>,
    pub author_user_id: Option<i64>,
    pub normalized_tags: Vec<String>,
    pub catalog_duration_ms: u64,
    pub item_embedding: Option<&'a [f32]>,
}

/// Counter and index side-effects returned to `CodeSceneRecommender`.
#[derive(Debug, Clone, Default)]
pub struct TelemetrySideEffects {
    pub delta_plays: u64,
    pub delta_likes: i64,
    pub delta_bookmarks: i64,
    pub delta_comments: u64,
    pub delta_shares: u64,
    pub delta_remixes: u64,
    pub add_impression: bool,
    pub add_dwell_norm: Option<f64>,
    pub add_finish: bool,
    pub add_skip: bool,
    pub add_not_interested: bool,
    pub add_positive_coengagement: bool,
    pub remove_positive_coengagement: bool,
    pub clear_just_published_fast_track: bool,
}

/// Sanitizes and applies one `SceneBehaviorEventDto` to `profile`, computing the
/// kind-normalized reward and updating dual-timescale interest vectors, EMA weights,
/// fatigue counters, and persistent hard blocklists.
pub fn apply_behavior_event(
    raw_event: &SceneBehaviorEventDto,
    target: &ResolvedEventTarget<'_>,
    profile: &mut UserInterestProfile,
    cfg: &RecommenderConfig,
    now_ms: i64,
) -> (RewardOutcome, TelemetrySideEffects) {
    let ev = raw_event.sanitized();
    let event_ts = if ev.timestamp_ms > 0 {
        ev.timestamp_ms
    } else {
        now_ms
    };
    let mut fx = TelemetrySideEffects::default();

    let dwell_ms = ev.dwell_ms.min(MAX_CLAMPED_DWELL_MS);
    let dwell_s = (dwell_ms as f64) / 1_000.0;
    let game_play_ms = ev.game_play_ms.min(MAX_CLAMPED_DWELL_MS);
    let game_play_s = (game_play_ms as f64) / 1_000.0;
    let replays = ev.loop_count.min(20);
    let game_inputs = ev.game_inputs.min(500);
    let game_restarts = ev.game_restarts.min(50);
    let game_score_norm = ev
        .game_score
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(|s| ((1.0 + s).ln() / 5.0).clamp(0.0, 1.0))
        .unwrap_or(0.0);

    let raw_duration_ms = if ev.duration_ms > 0 {
        ev.duration_ms
    } else {
        target.catalog_duration_ms
    };
    let effective_duration_ms = raw_duration_ms.max(MIN_REF_DURATION_MS);
    let completion_ratio = if ev.completion_ratio > 0.0 {
        ev.completion_ratio.clamp(0.0, 3.0)
    } else if dwell_ms > 0 {
        ((dwell_ms as f64) / (effective_duration_ms as f64)).clamp(0.0, 3.0)
    } else {
        0.0
    };

    let effective_kind = target.asset_kind.unwrap_or(AssetKind::Animation);
    let resolved_author = resolve_author(&ev, target);
    let target_is_hard_blocked = (!target.scene_id.is_empty()
        && profile.disliked_scene_ids.contains(target.scene_id))
        || resolved_author.is_some_and(|a| profile.hidden_author_ids.contains(&a))
        || (!profile.hidden_tags.is_empty()
            && target
                .normalized_tags
                .iter()
                .any(|t| profile.hidden_tags.contains(t)));

    let mut hard_negative_applied = false;
    let mut fast_skip_detected = false;
    let mut undo_reverted_prior = false;
    let mut is_undo = false;
    let mut clear_or_decay_fatigue = false;
    let mut bump_fatigue = false;

    let reward_raw: f64 = match ev.kind {
        SceneBehaviorEventKind::Impression => {
            profile.record_seen(target.scene_id, event_ts);
            profile.total_impressions = profile.total_impressions.saturating_add(1);
            fx.add_impression = true;
            // Very gentle exposure fatigue so repeated unengaged impressions cool down
            bump_fatigue_amount(profile, target, 0.06, cfg.fatigue_cap);
            0.0
        }
        SceneBehaviorEventKind::Dwell => {
            let game_engaged = effective_kind == AssetKind::Game
                && (game_inputs >= 3 || game_play_ms >= 2_000 || game_restarts > 0);
            let already_positive = !target_is_hard_blocked
                && !target.scene_id.is_empty()
                && (profile.liked_scene_ids.contains(target.scene_id)
                    || profile.bookmarked_scene_ids.contains(target.scene_id)
                    || profile
                        .positive_scene_history
                        .iter()
                        .any(|id| id == target.scene_id));
            if dwell_ms < FAST_SKIP_HARD_MS && !game_engaged && completion_ratio < 0.25 {
                profile.record_seen(target.scene_id, event_ts);
                fx.delta_plays = 1;
                fx.clear_just_published_fast_track = true;
                if already_positive {
                    fx.add_dwell_norm = Some(0.35);
                    0.0
                } else {
                    fast_skip_detected = true;
                    bump_fatigue = true;
                    fx.add_skip = true;
                    fx.add_dwell_norm = Some(0.05);
                    let frac = 1.0 - (dwell_ms as f64 / FAST_SKIP_HARD_MS as f64).clamp(0.0, 1.0);
                    -(0.85 + 0.20 * frac)
                }
            } else if dwell_ms < FAST_SKIP_SOFT_MS && !game_engaged && completion_ratio < 0.40 {
                profile.record_seen(target.scene_id, event_ts);
                fx.delta_plays = 1;
                fx.clear_just_published_fast_track = true;
                if already_positive {
                    fx.add_dwell_norm = Some(0.45);
                    0.0
                } else {
                    fast_skip_detected = true;
                    bump_fatigue = true;
                    fx.add_skip = true;
                    fx.add_dwell_norm = Some(0.20);
                    -0.35
                }
            } else if target_is_hard_blocked {
                0.0
            } else {
                profile.record_seen(target.scene_id, event_ts);
                fx.delta_plays = 1;
                fx.clear_just_published_fast_track = true;
                clear_or_decay_fatigue = true;
                let r = match effective_kind {
                    AssetKind::Video | AssetKind::Animation => {
                        let loop_term = (1.0 + replays as f64).ln();
                        let dwell_term = (1.0 + dwell_s / 10.0).ln();
                        (1.05 * completion_ratio + 0.55 * loop_term + 0.70 * dwell_term)
                            .clamp(0.10, 3.6)
                    }
                    AssetKind::Game => {
                        let play_s = game_play_s.max(dwell_s);
                        let play_term = (1.0 + play_s / 8.0).ln();
                        let input_term = (1.0 + game_inputs as f64 / 6.0).ln();
                        let restart_term = (game_restarts.min(3) as f64) * 0.45;
                        (0.60 * play_term
                            + 0.75 * input_term
                            + restart_term
                            + 0.50 * game_score_norm)
                            .clamp(0.10, 4.0)
                    }
                };
                fx.add_dwell_norm = Some((r / 2.5).clamp(0.0, 1.0));
                let is_finished = match effective_kind {
                    AssetKind::Video | AssetKind::Animation => {
                        completion_ratio >= 0.85 || replays > 0
                    }
                    AssetKind::Game => game_inputs >= 6 || game_play_s.max(dwell_s) >= 8.0,
                };
                if is_finished {
                    fx.add_finish = true;
                    fx.add_positive_coengagement = true;
                }
                r
            }
        }
        SceneBehaviorEventKind::Complete => {
            if target_is_hard_blocked {
                0.0
            } else {
                profile.record_seen(target.scene_id, event_ts);
                fx.delta_plays = 1;
                fx.add_finish = true;
                fx.add_dwell_norm = Some(0.90);
                fx.add_positive_coengagement = true;
                fx.clear_just_published_fast_track = true;
                clear_or_decay_fatigue = true;
                let comp = completion_ratio.max(1.0);
                match effective_kind {
                    AssetKind::Video | AssetKind::Animation => {
                        let loop_term = (1.0 + replays as f64).ln();
                        let dwell_term = (1.0 + dwell_s / 10.0).ln();
                        (1.30
                            + 0.55 * (comp - 1.0).clamp(0.0, 2.0)
                            + 0.45 * loop_term
                            + 0.35 * dwell_term)
                            .clamp(1.30, 3.6)
                    }
                    AssetKind::Game => {
                        let input_term = (1.0 + game_inputs as f64 / 6.0).ln();
                        let play_term = (1.0 + game_play_s.max(dwell_s) / 8.0).ln();
                        (1.40 + 0.50 * input_term + 0.35 * play_term + 0.50 * game_score_norm)
                            .clamp(1.40, 3.8)
                    }
                }
            }
        }
        SceneBehaviorEventKind::Replay => {
            if target_is_hard_blocked {
                0.0
            } else {
                profile.record_seen(target.scene_id, event_ts);
                fx.add_finish = true;
                fx.add_dwell_norm = Some(1.0);
                fx.add_positive_coengagement = true;
                clear_or_decay_fatigue = true;
                let eff_replays = replays.max(1);
                (1.80
                    + 0.55 * (1.0 + eff_replays as f64).ln()
                    + 0.25 * (1.0 + dwell_s / 10.0).ln())
                    .clamp(1.80, 3.4)
            }
        }
        SceneBehaviorEventKind::GameInteraction => {
            if target_is_hard_blocked {
                0.0
            } else {
                profile.record_seen(target.scene_id, event_ts);
                fx.delta_plays = 1;
                fx.clear_just_published_fast_track = true;
                if game_inputs == 0
                    && game_play_ms < FAST_SKIP_HARD_MS
                    && game_restarts == 0
                    && dwell_ms < FAST_SKIP_HARD_MS
                {
                    0.0
                } else {
                    clear_or_decay_fatigue = true;
                    let play_s = game_play_s.max(dwell_s);
                    let r = (0.85 * (1.0 + game_inputs as f64 / 5.0).ln().clamp(0.0, 2.3)
                        + 0.65 * (1.0 + play_s / 7.0).ln().clamp(0.0, 2.2)
                        + 0.55 * (game_restarts.min(3) as f64)
                        + 0.55 * game_score_norm)
                        .clamp(0.25, 4.2);
                    fx.add_dwell_norm = Some((r / 2.8).clamp(0.0, 1.0));
                    if game_inputs >= 5 || play_s >= 6.0 || game_restarts >= 1 {
                        fx.add_finish = true;
                        fx.add_positive_coengagement = true;
                    }
                    r
                }
            }
        }
        SceneBehaviorEventKind::Like => {
            if target_is_hard_blocked {
                0.0
            } else if !target.scene_id.is_empty()
                && profile.liked_scene_ids.insert(target.scene_id.to_string())
            {
                clear_or_decay_fatigue = true;
                fx.delta_likes = 1;
                fx.add_positive_coengagement = true;
                2.5
            } else {
                0.0
            }
        }
        SceneBehaviorEventKind::DoubleTapLike => {
            if target_is_hard_blocked {
                0.0
            } else if !target.scene_id.is_empty()
                && profile.liked_scene_ids.insert(target.scene_id.to_string())
            {
                clear_or_decay_fatigue = true;
                fx.delta_likes = 1;
                fx.add_positive_coengagement = true;
                2.8
            } else {
                0.0
            }
        }
        SceneBehaviorEventKind::Unlike => {
            is_undo = true;
            if profile.liked_scene_ids.remove(target.scene_id) {
                undo_reverted_prior = true;
                fx.delta_likes = -1;
                if !profile.bookmarked_scene_ids.contains(target.scene_id) {
                    profile.remove_positive_history(target.scene_id);
                    fx.remove_positive_coengagement = true;
                }
                -1.2
            } else {
                -0.25
            }
        }
        SceneBehaviorEventKind::Bookmark => {
            if target_is_hard_blocked {
                0.0
            } else if !target.scene_id.is_empty()
                && profile
                    .bookmarked_scene_ids
                    .insert(target.scene_id.to_string())
            {
                clear_or_decay_fatigue = true;
                fx.delta_bookmarks = 1;
                fx.add_positive_coengagement = true;
                3.2
            } else {
                0.0
            }
        }
        SceneBehaviorEventKind::Unbookmark => {
            is_undo = true;
            if profile.bookmarked_scene_ids.remove(target.scene_id) {
                undo_reverted_prior = true;
                fx.delta_bookmarks = -1;
                if !profile.liked_scene_ids.contains(target.scene_id) {
                    profile.remove_positive_history(target.scene_id);
                    fx.remove_positive_coengagement = true;
                }
                -1.4
            } else {
                -0.25
            }
        }
        SceneBehaviorEventKind::Comment => {
            if target_is_hard_blocked {
                0.0
            } else {
                clear_or_decay_fatigue = true;
                fx.delta_comments = 1;
                fx.add_positive_coengagement = true;
                3.0
            }
        }
        SceneBehaviorEventKind::Share => {
            if target_is_hard_blocked {
                0.0
            } else {
                clear_or_decay_fatigue = true;
                fx.delta_shares = 1;
                fx.add_positive_coengagement = true;
                3.6
            }
        }
        SceneBehaviorEventKind::FollowAuthor => {
            if target_is_hard_blocked {
                0.0
            } else if let Some(author) = resolved_author {
                if profile.followed_creators.insert(author) {
                    clear_or_decay_fatigue = true;
                    let w = profile.creator_weights.entry(author).or_insert(0.0);
                    *w = (*w + 0.80).clamp(-3.0, 4.0);
                    if !target.scene_id.is_empty() {
                        fx.add_positive_coengagement = true;
                    }
                    3.4
                } else {
                    0.0
                }
            } else {
                0.0
            }
        }
        SceneBehaviorEventKind::UnfollowAuthor => {
            is_undo = true;
            if let Some(author) = resolve_author(&ev, target) {
                if profile.followed_creators.remove(&author) {
                    undo_reverted_prior = true;
                    let w = profile.creator_weights.entry(author).or_insert(0.0);
                    *w = (*w - 0.70).clamp(-3.0, 4.0);
                    -1.3
                } else {
                    -0.30
                }
            } else {
                -0.20
            }
        }
        SceneBehaviorEventKind::InspectCode => {
            if target_is_hard_blocked {
                0.0
            } else {
                clear_or_decay_fatigue = true;
                fx.add_positive_coengagement = true;
                2.0
            }
        }
        SceneBehaviorEventKind::Remix => {
            if target_is_hard_blocked {
                0.0
            } else {
                clear_or_decay_fatigue = true;
                fx.delta_remixes = 1;
                fx.add_positive_coengagement = true;
                4.8
            }
        }
        SceneBehaviorEventKind::FastSkip => {
            profile.record_seen(target.scene_id, event_ts);
            fast_skip_detected = true;
            bump_fatigue = true;
            fx.add_skip = true;
            fx.clear_just_published_fast_track = true;
            if dwell_ms == 0 || dwell_ms < FAST_SKIP_HARD_MS {
                -1.05
            } else if dwell_ms < FAST_SKIP_SOFT_MS {
                -0.85
            } else {
                -0.65
            }
        }
        SceneBehaviorEventKind::NotInterested => {
            hard_negative_applied = true;
            bump_fatigue = true;
            fx.add_skip = true;
            fx.add_not_interested = true;
            fx.remove_positive_coengagement = true;
            fx.clear_just_published_fast_track = true;
            if !target.scene_id.is_empty() {
                profile.disliked_scene_ids.insert(target.scene_id.to_string());
                profile.remove_positive_history(target.scene_id);
            }
            -2.2
        }
        SceneBehaviorEventKind::HideAuthor => {
            hard_negative_applied = true;
            bump_fatigue = true;
            fx.add_skip = true;
            fx.add_not_interested = true;
            fx.remove_positive_coengagement = true;
            fx.clear_just_published_fast_track = true;
            if let Some(author) = resolve_author(&ev, target) {
                profile.hidden_author_ids.insert(author);
                profile.followed_creators.remove(&author);
                profile.creator_weights.insert(author, -3.0);
            }
            if !target.scene_id.is_empty() {
                profile.disliked_scene_ids.insert(target.scene_id.to_string());
                profile.remove_positive_history(target.scene_id);
            }
            -2.5
        }
        SceneBehaviorEventKind::HideTag => {
            hard_negative_applied = true;
            bump_fatigue = true;
            fx.add_skip = true;
            fx.add_not_interested = true;
            fx.remove_positive_coengagement = true;
            fx.clear_just_published_fast_track = true;
            if let Some(tag) = resolve_hidden_tag(&ev, target) {
                profile.hidden_tags.insert(tag.clone());
                profile.tag_weights.insert(tag, -3.0);
            }
            if !target.scene_id.is_empty() {
                profile.remove_positive_history(target.scene_id);
            }
            -2.4
        }
    };

    let reward = if reward_raw.is_finite() {
        reward_raw.clamp(REWARD_MIN, REWARD_MAX)
    } else {
        0.0
    };

    let mut profile_updated = matches!(
        ev.kind,
        SceneBehaviorEventKind::Impression
            | SceneBehaviorEventKind::NotInterested
            | SceneBehaviorEventKind::HideAuthor
            | SceneBehaviorEventKind::HideTag
    );

    if reward > 0.0 {
        profile_updated = true;
        profile.total_positives = profile.total_positives.saturating_add(1);
        if fx.add_positive_coengagement {
            profile.push_positive_history(target.scene_id);
        }
        apply_positive_profile_update(profile, target, reward, cfg);
        if clear_or_decay_fatigue {
            profile.decay_fatigue(cfg.fatigue_decay);
            clear_matched_fatigue(profile, target);
        }
    } else if reward < 0.0 {
        profile_updated = true;
        if !is_undo {
            profile.total_negatives = profile.total_negatives.saturating_add(1);
        }
        apply_negative_profile_update(
            profile,
            target,
            reward,
            is_undo,
            hard_negative_applied,
            cfg,
        );
        if bump_fatigue && !is_undo {
            let bump = if hard_negative_applied {
                cfg.fatigue_fast_skip_bump * 1.6
            } else {
                cfg.fatigue_fast_skip_bump
            };
            bump_fatigue_amount(profile, target, bump, cfg.fatigue_cap);
        }
    }

    profile.total_events = profile.total_events.saturating_add(1);
    if profile_updated {
        profile.revision = profile.revision.saturating_add(1);
    }
    profile.updated_at_ms = profile.updated_at_ms.max(event_ts);

    let outcome = RewardOutcome {
        event_kind: ev.kind,
        scene_id: target.scene_id.to_string(),
        asset_kind: effective_kind,
        reward,
        profile_updated,
        hard_negative_applied,
        fast_skip_detected,
        undo_reverted_prior,
    };

    (outcome, fx)
}

fn resolve_author(ev: &SceneBehaviorEventDto, target: &ResolvedEventTarget<'_>) -> Option<i64> {
    if let Some(a) = target.author_user_id {
        return Some(a);
    }
    if let Some(a) = ev.author_user_id {
        return Some(a);
    }
    ev.reason
        .as_deref()
        .and_then(|s| s.trim().parse::<i64>().ok())
}

fn resolve_hidden_tag(
    ev: &SceneBehaviorEventDto,
    target: &ResolvedEventTarget<'_>,
) -> Option<String> {
    if let Some(raw) = ev.reason.as_deref() {
        let norm = normalize_tag(raw);
        if !norm.is_empty() {
            return Some(norm);
        }
    }
    let from_ev = normalize_tags(&ev.tags);
    if let Some(primary) = first_non_generic_tag(&from_ev) {
        return Some(primary);
    }
    if let Some(primary) = first_non_generic_tag(&target.normalized_tags) {
        return Some(primary);
    }
    effective_primary_tag(&target.normalized_tags).or_else(|| effective_primary_tag(&from_ev))
}

fn apply_positive_profile_update(
    profile: &mut UserInterestProfile,
    target: &ResolvedEventTarget<'_>,
    reward: f64,
    cfg: &RecommenderConfig,
) {
    if let Some(item_vec) = target.item_embedding {
        let strength = (reward / 2.2).clamp(0.25, 2.0);
        let alpha_s = (cfg.eta_short * strength).clamp(0.12, 0.78);
        let alpha_l = (cfg.eta_long * strength).clamp(0.02, 0.22);

        blend_unit_vector(&mut profile.u_short, item_vec, alpha_s);
        blend_unit_vector(&mut profile.u_long, item_vec, alpha_l);

        // Damp conflicting negative vector to allow rapid mid-session taste pivots
        let neg_cos = cosine_similarity(&profile.u_neg, item_vec);
        if neg_cos > 0.0 {
            let damp = (0.35 * neg_cos).min(0.60) as f32;
            for i in 0..EMBEDDING_DIM.min(profile.u_neg.len()).min(item_vec.len()) {
                profile.u_neg[i] -= damp * item_vec[i];
            }
            l2_normalize(&mut profile.u_neg);
        }
    }

    let ema = cfg.ema_alpha.clamp(0.05, 0.60);
    let target_tag = (reward * 1.10).clamp(0.15, 3.8);
    let target_kind = (reward * 1.05).clamp(0.15, 3.3);
    let target_creator = (reward * 1.05).clamp(0.15, 3.8);

    // Gentle competitive decay on unmatched kinds/tags when strong positive arrives
    if reward >= 1.2 {
        if let Some(kind) = target.asset_kind {
            for (k, w) in profile.kind_weights.iter_mut() {
                if *k != kind {
                    *w *= 0.94;
                }
            }
        }
        for (t, w) in profile.tag_weights.iter_mut() {
            if !target.normalized_tags.contains(t) {
                *w *= 0.96;
            }
        }
    }

    for tag in &target.normalized_tags {
        if profile.hidden_tags.contains(tag) {
            continue;
        }
        let w = profile.tag_weights.entry(tag.clone()).or_insert(0.0);
        *w = ((1.0 - ema) * *w + ema * target_tag).clamp(-3.0, 4.0);
    }

    if let Some(kind) = target.asset_kind {
        let kw = profile.kind_weights.entry(kind).or_insert(0.0);
        *kw = ((1.0 - ema) * *kw + ema * target_kind).clamp(-2.5, 3.5);
    }

    if let Some(author) = target.author_user_id {
        if !profile.hidden_author_ids.contains(&author) {
            let cw = profile.creator_weights.entry(author).or_insert(0.0);
            *cw = ((1.0 - ema) * *cw + ema * target_creator).clamp(-3.0, 4.0);
        }
    }
}

fn apply_negative_profile_update(
    profile: &mut UserInterestProfile,
    target: &ResolvedEventTarget<'_>,
    reward: f64,
    is_undo: bool,
    is_hard_neg: bool,
    cfg: &RecommenderConfig,
) {
    let neg_mag = (-reward).clamp(0.15, 2.5);
    if let Some(item_vec) = target.item_embedding {
        if !is_undo {
            let mult = if is_hard_neg { 1.45 } else { 1.0 };
            let alpha_n = (cfg.eta_neg * neg_mag * mult).clamp(0.12, 0.65);
            blend_unit_vector(&mut profile.u_neg, item_vec, alpha_n);
        }
        let short_cos = cosine_similarity(&profile.u_short, item_vec);
        if short_cos > 0.0 {
            let repel = if is_undo {
                (0.12 * neg_mag * short_cos).min(0.20) as f32
            } else {
                (0.24 * neg_mag * short_cos).min(0.42) as f32
            };
            for i in 0..EMBEDDING_DIM.min(profile.u_short.len()).min(item_vec.len()) {
                profile.u_short[i] -= repel * item_vec[i];
            }
            l2_normalize(&mut profile.u_short);
        }
    }

    let ema = if is_undo {
        (cfg.ema_alpha * 0.35).clamp(0.03, 0.25)
    } else {
        cfg.ema_alpha.clamp(0.05, 0.60)
    };
    let target_val = reward.clamp(-2.5, -0.15);

    for tag in &target.normalized_tags {
        if profile.hidden_tags.contains(tag) {
            profile.tag_weights.insert(tag.clone(), -3.0);
            continue;
        }
        let w = profile.tag_weights.entry(tag.clone()).or_insert(0.0);
        *w = ((1.0 - ema) * *w + ema * target_val).clamp(-3.0, 4.0);
    }

    if let Some(kind) = target.asset_kind {
        let kw = profile.kind_weights.entry(kind).or_insert(0.0);
        *kw = ((1.0 - ema) * *kw + ema * target_val).clamp(-2.5, 3.5);
    }

    if let Some(author) = target.author_user_id {
        if profile.hidden_author_ids.contains(&author) {
            profile.creator_weights.insert(author, -3.0);
        } else {
            let cw = profile.creator_weights.entry(author).or_insert(0.0);
            *cw = ((1.0 - ema) * *cw + ema * target_val).clamp(-3.0, 4.0);
        }
    }
}

fn blend_unit_vector(dst: &mut Vec<f32>, src: &[f32], alpha: f64) {
    if dst.len() != EMBEDDING_DIM {
        dst.resize(EMBEDDING_DIM, 0.0);
    }
    if src.len() != EMBEDDING_DIM {
        return;
    }
    let is_zero = dst.iter().all(|&x| x.abs() <= 1e-9);
    let a = alpha.clamp(0.0, 1.0) as f32;
    if is_zero {
        for (d, &s) in dst.iter_mut().zip(src.iter()) {
            *d = if s.is_finite() { s } else { 0.0 };
        }
    } else {
        let keep = 1.0 - a;
        for (d, &s) in dst.iter_mut().zip(src.iter()) {
            let sv = if s.is_finite() { s } else { 0.0 };
            *d = keep * *d + a * sv;
        }
    }
    l2_normalize(dst);
}

fn clear_matched_fatigue(profile: &mut UserInterestProfile, target: &ResolvedEventTarget<'_>) {
    for tag in &target.normalized_tags {
        profile.tag_fatigue.remove(tag);
    }
    if let Some(kind) = target.asset_kind {
        profile.kind_fatigue.remove(&kind);
    }
    if let Some(author) = target.author_user_id {
        profile.creator_fatigue.remove(&author);
    }
}

fn bump_fatigue_amount(
    profile: &mut UserInterestProfile,
    target: &ResolvedEventTarget<'_>,
    bump: f64,
    cap: f64,
) {
    let c = cap.max(0.1);
    for tag in &target.normalized_tags {
        let f = profile.tag_fatigue.entry(tag.clone()).or_insert(0.0);
        *f = (*f + bump).min(c);
    }
    if let Some(kind) = target.asset_kind {
        let kf = profile.kind_fatigue.entry(kind).or_insert(0.0);
        *kf = (*kf + bump).min(c);
    }
    if let Some(author) = target.author_user_id {
        let cf = profile.creator_fatigue.entry(author).or_insert(0.0);
        *cf = (*cf + bump * 0.8).min(c);
    }
}
