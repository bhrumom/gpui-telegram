//! Stage 1: Hard Pre-Filter, 4-Tier Traffic Pool & 6-Channel Recall (`CS-F06`).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use gpui_telegram_proto::{AssetKind, CodeSceneAssetDto, CodeSceneFeedRequest, CodeSceneFeedTab};

use crate::reco::embedding::{
    bilingual_concepts_for_text, cosine_similarity, effective_query_vector, embed_query,
    jaccard_similarity, normalize_tag, tokenize_bilingual,
};
use crate::reco::profile::{RecommenderConfig, UserInterestProfile};
use crate::reco::telemetry::ItemEngagementStats;

pub const TIER0_COLD_START: &str = "tier0_cold_start";
pub const TIER1_SEED: &str = "tier1_seed";
pub const TIER2_RISING: &str = "tier2_rising";
pub const TIER3_VIRAL: &str = "tier3_viral";

pub const CHANNEL_EMBEDDING: &str = "embedding";
pub const CHANNEL_SWING_CF: &str = "swing_cf";
pub const CHANNEL_TAG_KIND: &str = "tag_kind";
pub const CHANNEL_CREATOR_GRAPH: &str = "creator_graph";
pub const CHANNEL_HOT: &str = "hot";
pub const CHANNEL_COLD_START: &str = "cold_start";
pub const CHANNEL_JUST_PUBLISHED: &str = "creator_just_published";

/// Indexed catalog entry stored inside `CodeSceneRecommender`.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub asset: CodeSceneAssetDto,
    pub normalized_tags: Vec<String>,
    pub effective_primary_tag: Option<String>,
    pub code_tokens: Vec<String>,
    pub embedding: Vec<f32>,
    pub stats: ItemEngagementStats,
    /// `(publisher_user_id, published_at_ms)` for immediate fast-track.
    pub just_published_by: Option<(i64, i64)>,
}

impl CatalogEntry {
    pub fn effective_plays(&self) -> u64 {
        self.asset.plays_count.max(self.stats.impressions)
    }

    pub fn traffic_tier(&self) -> &'static str {
        traffic_tier_for_plays(self.effective_plays())
    }

    pub fn is_cold_start(&self) -> bool {
        self.effective_plays() < 20
    }

    pub fn duration_ms(&self) -> u64 {
        match self.asset.manifest.duration_sec {
            Some(sec) if sec.is_finite() && sec > 0.0 => (sec * 1_000.0).round() as u64,
            _ => match self.asset.kind {
                AssetKind::Video => 15_000,
                AssetKind::Animation => 10_000,
                AssetKind::Game => 15_000,
            },
        }
    }
}

/// Exact 4-tier traffic pool classification.
pub fn traffic_tier_for_plays(effective_plays: u64) -> &'static str {
    if effective_plays < 20 {
        TIER0_COLD_START
    } else if effective_plays < 100 {
        TIER1_SEED
    } else if effective_plays < 500 {
        TIER2_RISING
    } else {
        TIER3_VIRAL
    }
}

/// Exact freshness decay formula: `1.0 / (age_hours + 2.0)^1.25`.
pub fn freshness_score(created_at_ms: i64, now_ms: i64) -> f64 {
    let age_ms = now_ms.saturating_sub(created_at_ms).max(0);
    let age_hours = (age_ms as f64) / 3_600_000.0;
    let raw = 1.0 / (age_hours + 2.0).powf(1.25);
    if raw.is_finite() {
        raw.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Wilson score lower bound (`z = 1.645` for 95% one-sided / 90% two-sided confidence).
pub fn wilson_lower_bound(positives: f64, trials: f64, z: f64) -> f64 {
    if !positives.is_finite() || !trials.is_finite() || trials <= 0.0 {
        return 0.0;
    }
    let p = (positives / trials).clamp(0.0, 1.0);
    let z2 = z * z;
    let denom = 1.0 + z2 / trials;
    let center = p + z2 / (2.0 * trials);
    let rad = (p * (1.0 - p) / trials + z2 / (4.0 * trials * trials))
        .max(0.0)
        .sqrt();
    let lb = (center - z * rad) / denom;
    if lb.is_finite() {
        lb.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Multi-channel recall output passed to Stage 2 scorer.
#[derive(Debug, Clone)]
pub struct RecallCandidate {
    pub scene_id: String,
    pub recall_channels: Vec<String>,
    pub embedding_sim: f64,
    pub swing_cf: f64,
    pub tag_kind_affinity: f64,
    pub creator_affinity: f64,
    pub hot_score: f64,
    pub freshness: f64,
    pub query_match_score: f64,
    pub is_fast_track: bool,
    pub in_seen_cooldown: bool,
    pub is_cold_start: bool,
}

/// Precomputed query context for bilingual substring + semantic matching.
struct QueryContext {
    raw_lower: String,
    parsed_kind: Option<AssetKind>,
    tokens: HashSet<String>,
    concepts: HashSet<String>,
    embedding: Vec<f32>,
}

impl QueryContext {
    fn from_opt(query: Option<&str>) -> Option<Self> {
        let trimmed = query?.trim();
        if trimmed.is_empty() {
            return None;
        }
        let raw_lower = trimmed.to_lowercase();
        let parsed_kind = AssetKind::parse(trimmed);
        let tokens: HashSet<String> = tokenize_bilingual(trimmed).into_iter().collect();
        let concepts: HashSet<String> = bilingual_concepts_for_text(trimmed).into_iter().collect();
        let embedding = embed_query(trimmed);
        Some(Self {
            raw_lower,
            parsed_kind,
            tokens,
            concepts,
            embedding,
        })
    }

    fn match_score(&self, entry: &CatalogEntry) -> Option<f64> {
        let title_lower = entry.asset.title.to_lowercase();
        let desc_lower = entry.asset.description.to_lowercase();
        let author_lower = entry.asset.author_username.to_lowercase();
        let bot_lower = entry
            .asset
            .creator_bot_username
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        let ctrl_lower = entry.asset.controls_hint.to_lowercase();
        let id_lower = entry.asset.scene_id.to_lowercase();

        let exact_title_or_id = title_lower.contains(&self.raw_lower)
            || id_lower.contains(&self.raw_lower)
            || entry.normalized_tags.iter().any(|t| t == &self.raw_lower || t.contains(&self.raw_lower));
        let exact_meta = desc_lower.contains(&self.raw_lower)
            || author_lower.contains(&self.raw_lower)
            || (!bot_lower.is_empty() && bot_lower.contains(&self.raw_lower))
            || ctrl_lower.contains(&self.raw_lower);
        let kind_hit = self.parsed_kind == Some(entry.asset.kind);

        let mut token_hits = 0usize;
        if !self.tokens.is_empty() {
            let item_tokens: HashSet<String> = tokenize_bilingual(&entry.asset.title)
                .into_iter()
                .chain(tokenize_bilingual(&entry.asset.description))
                .chain(entry.normalized_tags.iter().cloned())
                .collect();
            token_hits = self.tokens.intersection(&item_tokens).count();
        }
        let token_ratio = if self.tokens.is_empty() {
            0.0
        } else {
            (token_hits as f64) / (self.tokens.len() as f64)
        };

        let mut concept_hits = 0usize;
        if !self.concepts.is_empty() {
            let mut item_concepts: HashSet<String> =
                bilingual_concepts_for_text(&entry.asset.title)
                    .into_iter()
                    .chain(bilingual_concepts_for_text(&entry.asset.description))
                    .collect();
            for tag in &entry.normalized_tags {
                for c in bilingual_concepts_for_text(tag) {
                    item_concepts.insert(c);
                }
            }
            item_concepts.insert(entry.asset.kind.as_str().to_string());
            concept_hits = self.concepts.intersection(&item_concepts).count();
        }
        let concept_ratio = if self.concepts.is_empty() {
            0.0
        } else {
            (concept_hits as f64) / (self.concepts.len() as f64)
        };

        let emb_sim = cosine_similarity(&self.embedding, &entry.embedding).max(0.0);

        if exact_title_or_id
            || exact_meta
            || kind_hit
            || token_hits > 0
            || concept_hits > 0
            || emb_sim >= 0.28
        {
            let mut score = 0.0;
            if exact_title_or_id {
                score += 0.55;
            } else if exact_meta {
                score += 0.35;
            }
            if kind_hit {
                score += 0.25;
            }
            score += 0.35 * token_ratio + 0.40 * concept_ratio + 0.35 * emb_sim;
            Some(score.clamp(0.15, 1.0))
        } else {
            None
        }
    }
}

/// Result of Stage 1 recall, keeping fresh and seen-cooldown pools separated for
/// two-phase ranking so cooldown backfill never displaces fresh items across pages.
pub struct RecallOutput {
    pub candidates: Vec<RecallCandidate>,
    pub cooldown_candidates: Vec<RecallCandidate>,
    pub total_eligible: usize,
}

/// Runs Stage 1 hard pre-filtering and 6-channel recall across `catalog`.
#[allow(clippy::too_many_arguments)]
pub fn recall_candidates(
    catalog: &BTreeMap<String, CatalogEntry>,
    profile: &UserInterestProfile,
    scene_positive_users: &BTreeMap<String, BTreeSet<i64>>,
    user_positive_scenes: &BTreeMap<i64, BTreeSet<String>>,
    req: &CodeSceneFeedRequest,
    exclude_scene_ids: &HashSet<String>,
    apply_seen_cooldown: bool,
    fetch_limit: usize,
    cfg: &RecommenderConfig,
    now_ms: i64,
) -> RecallOutput {
    if catalog.is_empty() {
        return RecallOutput {
            candidates: Vec::new(),
            cooldown_candidates: Vec::new(),
            total_eligible: 0,
        };
    }

    let effective_limit = fetch_limit.max(req.clamped_limit());
    let tag_filter = req
        .tag_filter
        .as_deref()
        .map(normalize_tag)
        .filter(|t| !t.is_empty());
    let query_ctx = QueryContext::from_opt(req.query.as_deref());

    // Step 1: Hard Pre-Filter (UNCONDITIONAL hard blocklist + tab + tag + query + exclude)
    let mut fresh_pool: Vec<(&CatalogEntry, f64)> = Vec::new();
    let mut cooldown_pool: Vec<(&CatalogEntry, f64, i64)> = Vec::new();

    for entry in catalog.values() {
        if profile.is_blocked(
            &entry.asset.scene_id,
            entry.asset.author_user_id,
            &entry.normalized_tags,
        ) {
            continue;
        }
        if !passes_tab_filter(req.tab, req.user_id, profile, entry) {
            continue;
        }
        if let Some(ref tf) = tag_filter {
            if !entry.normalized_tags.iter().any(|t| t == tf) {
                continue;
            }
        }
        let query_score = if let Some(ref qctx) = query_ctx {
            match qctx.match_score(entry) {
                Some(s) => s,
                None => continue,
            }
        } else {
            0.0
        };
        if exclude_scene_ids.contains(&entry.asset.scene_id) {
            continue;
        }

        let is_fast_track = is_entry_fast_track(entry, req.user_id, now_ms, cfg);
        if apply_seen_cooldown && !is_fast_track {
            if let Some(last_seen) = profile.last_seen_ms(&entry.asset.scene_id) {
                if now_ms.saturating_sub(last_seen) < cfg.seen_cooldown_ms {
                    cooldown_pool.push((entry, query_score, last_seen));
                    continue;
                }
            }
        }
        fresh_pool.push((entry, query_score));
    }

    let total_eligible = fresh_pool.len() + cooldown_pool.len();
    if total_eligible == 0 {
        return RecallOutput {
            candidates: Vec::new(),
            cooldown_candidates: Vec::new(),
            total_eligible: 0,
        };
    }

    let u_eff = effective_query_vector(&profile.u_short, &profile.u_long, &profile.u_neg);
    let has_user_vec = u_eff.iter().any(|&v| v.abs() > 1e-6);
    let recent_positives: Vec<String> = profile
        .positive_scene_history
        .iter()
        .rev()
        .take(8)
        .cloned()
        .collect();

    // Stable channel quota independent of small pagination shifts so NumericOffset
    // pages recall the exact same fresh/cooldown candidate sets.
    let quota = effective_limit.saturating_mul(4).max(256);

    // Phase 1 pool: strictly fresh (non-cooldown) eligible items
    let candidates = select_pool_candidates(
        &fresh_pool,
        false,
        quota,
        catalog,
        profile,
        &u_eff,
        has_user_vec,
        &recent_positives,
        scene_positive_users,
        user_positive_scenes,
        req.user_id,
        cfg,
        now_ms,
    );

    // Phase 2 pool: seen-cooldown items recalled ONLY when fresh candidates are fewer than needed
    let cooldown_candidates = if candidates.len() < effective_limit && !cooldown_pool.is_empty() {
        cooldown_pool.sort_by(|a, b| {
            a.2.cmp(&b.2)
                .then_with(|| a.0.asset.scene_id.cmp(&b.0.asset.scene_id))
        });
        let cooldown_slice: Vec<(&CatalogEntry, f64)> = cooldown_pool
            .into_iter()
            .map(|(e, qs, _)| (e, qs))
            .collect();
        select_pool_candidates(
            &cooldown_slice,
            true,
            quota,
            catalog,
            profile,
            &u_eff,
            has_user_vec,
            &recent_positives,
            scene_positive_users,
            user_positive_scenes,
            req.user_id,
            cfg,
            now_ms,
        )
    } else {
        Vec::new()
    };

    RecallOutput {
        candidates,
        cooldown_candidates,
        total_eligible,
    }
}

#[allow(clippy::too_many_arguments)]
fn select_pool_candidates(
    pool: &[(&CatalogEntry, f64)],
    in_seen_cooldown: bool,
    quota: usize,
    catalog: &BTreeMap<String, CatalogEntry>,
    profile: &UserInterestProfile,
    u_eff: &[f32],
    has_user_vec: bool,
    recent_positives: &[String],
    scene_positive_users: &BTreeMap<String, BTreeSet<i64>>,
    user_positive_scenes: &BTreeMap<i64, BTreeSet<String>>,
    user_id: i64,
    cfg: &RecommenderConfig,
    now_ms: i64,
) -> Vec<RecallCandidate> {
    if pool.is_empty() {
        return Vec::new();
    }
    let mut raw_candidates: Vec<RecallCandidate> = Vec::with_capacity(pool.len());
    for &(entry, query_match_score) in pool {
        let cand = evaluate_candidate_signals(
            entry,
            catalog,
            profile,
            u_eff,
            has_user_vec,
            recent_positives,
            scene_positive_users,
            user_positive_scenes,
            user_id,
            query_match_score,
            in_seen_cooldown,
            cfg,
            now_ms,
        );
        raw_candidates.push(cand);
    }

    // 6-Channel Union Selection
    let mut selected_indices: BTreeSet<usize> = BTreeSet::new();
    select_top_k_indices(
        &raw_candidates,
        quota,
        |c| c.is_fast_track,
        |c| if c.is_fast_track { 1.0 } else { 0.0 },
        &mut selected_indices,
    );
    select_top_k_indices(
        &raw_candidates,
        quota,
        |c| c.embedding_sim > 0.05,
        |c| c.embedding_sim,
        &mut selected_indices,
    );
    select_top_k_indices(
        &raw_candidates,
        quota,
        |c| c.swing_cf > 0.12,
        |c| c.swing_cf,
        &mut selected_indices,
    );
    select_top_k_indices(
        &raw_candidates,
        quota,
        |c| c.tag_kind_affinity > 0.05,
        |c| c.tag_kind_affinity,
        &mut selected_indices,
    );
    select_top_k_indices(
        &raw_candidates,
        quota,
        |c| c.creator_affinity > 0.05,
        |c| c.creator_affinity,
        &mut selected_indices,
    );
    // R5 Hot always acts as safety net for cold-start users & small catalogs
    select_top_k_indices(
        &raw_candidates,
        quota,
        |_| true,
        |c| c.hot_score + 0.5 * c.query_match_score,
        &mut selected_indices,
    );
    // R6 Cold-start & orthogonal exploration
    select_top_k_indices(
        &raw_candidates,
        quota,
        |c| c.is_cold_start || (c.embedding_sim.abs() < 0.25 && c.hot_score > 0.0),
        |c| (if c.is_cold_start { 1.0 } else { 0.0 }) + c.freshness,
        &mut selected_indices,
    );

    selected_indices
        .into_iter()
        .map(|idx| raw_candidates[idx].clone())
        .collect()
}

/// Evaluates recall signals for one specific asset (used by `explain_asset_for_user`).
#[allow(clippy::too_many_arguments)]
pub fn recall_single_asset(
    entry: &CatalogEntry,
    catalog: &BTreeMap<String, CatalogEntry>,
    profile: &UserInterestProfile,
    scene_positive_users: &BTreeMap<String, BTreeSet<i64>>,
    user_positive_scenes: &BTreeMap<i64, BTreeSet<String>>,
    user_id: i64,
    cfg: &RecommenderConfig,
    now_ms: i64,
) -> RecallCandidate {
    let u_eff = effective_query_vector(&profile.u_short, &profile.u_long, &profile.u_neg);
    let has_user_vec = u_eff.iter().any(|&v| v.abs() > 1e-6);
    let recent_positives: Vec<String> = profile
        .positive_scene_history
        .iter()
        .rev()
        .take(8)
        .cloned()
        .collect();
    evaluate_candidate_signals(
        entry,
        catalog,
        profile,
        &u_eff,
        has_user_vec,
        &recent_positives,
        scene_positive_users,
        user_positive_scenes,
        user_id,
        0.0,
        false,
        cfg,
        now_ms,
    )
}

fn passes_tab_filter(
    tab: CodeSceneFeedTab,
    user_id: i64,
    profile: &UserInterestProfile,
    entry: &CatalogEntry,
) -> bool {
    match tab {
        CodeSceneFeedTab::ForYou => true,
        CodeSceneFeedTab::Videos => entry.asset.kind == AssetKind::Video,
        CodeSceneFeedTab::Animations => entry.asset.kind == AssetKind::Animation,
        CodeSceneFeedTab::Games => entry.asset.kind == AssetKind::Game,
        CodeSceneFeedTab::Following => profile
            .followed_creators
            .contains(&entry.asset.author_user_id),
        CodeSceneFeedTab::MyWorks => entry.asset.author_user_id == user_id,
    }
}

fn is_entry_fast_track(
    entry: &CatalogEntry,
    user_id: i64,
    now_ms: i64,
    cfg: &RecommenderConfig,
) -> bool {
    let Some((pub_uid, pub_ts)) = entry.just_published_by else {
        return false;
    };
    if pub_uid != user_id {
        return false;
    }
    let age = now_ms.saturating_sub(pub_ts).max(0);
    age <= cfg.just_published_ttl_ms
}

#[allow(clippy::too_many_arguments)]
fn evaluate_candidate_signals(
    entry: &CatalogEntry,
    catalog: &BTreeMap<String, CatalogEntry>,
    profile: &UserInterestProfile,
    u_eff: &[f32],
    has_user_vec: bool,
    recent_positives: &[String],
    scene_positive_users: &BTreeMap<String, BTreeSet<i64>>,
    user_positive_scenes: &BTreeMap<i64, BTreeSet<String>>,
    user_id: i64,
    query_match_score: f64,
    in_seen_cooldown: bool,
    cfg: &RecommenderConfig,
    now_ms: i64,
) -> RecallCandidate {
    let is_fast_track = is_entry_fast_track(entry, user_id, now_ms, cfg);
    let is_cold_start = entry.is_cold_start();
    let freshness = freshness_score(entry.asset.created_at_ms, now_ms);

    // R1: Dual-Timescale Dense Embedding
    let embedding_sim = if has_user_vec {
        cosine_similarity(u_eff, &entry.embedding)
    } else {
        0.0
    };

    // R2: Real Multi-User Swing CF with tag+code-token fallback
    let swing_cf = compute_swing_cf(
        entry,
        catalog,
        recent_positives,
        scene_positive_users,
        user_positive_scenes,
        user_id,
    );

    // R3: Tag + Kind Affinity (fatigue-adjusted)
    let mut tag_sum = 0.0_f64;
    let mut tag_cnt = 0usize;
    for tag in &entry.normalized_tags {
        if let Some(&w) = profile.tag_weights.get(tag) {
            let fat = profile.tag_fatigue.get(tag).copied().unwrap_or(0.0);
            tag_sum += w - 0.50 * fat;
            tag_cnt += 1;
        }
    }
    let mean_tag = if tag_cnt > 0 {
        tag_sum / (tag_cnt as f64)
    } else {
        0.0
    };
    let kind_w = profile.kind_weights.get(&entry.asset.kind).copied().unwrap_or(0.0);
    let kind_fat = profile.kind_fatigue.get(&entry.asset.kind).copied().unwrap_or(0.0);
    let tag_kind_affinity = ((mean_tag * 0.65) + ((kind_w - 0.50 * kind_fat) * 0.55))
        .clamp(-2.5, 3.0);

    // R4: Creator / Bot Social Graph & Remix Lineage
    let mut creator_aff = 0.0_f64;
    if profile
        .followed_creators
        .contains(&entry.asset.author_user_id)
    {
        creator_aff += 1.0;
    }
    if let Some(&cw) = profile.creator_weights.get(&entry.asset.author_user_id) {
        creator_aff += (cw / 1.8).tanh();
    }
    if let Some(ref parent_id) = entry.asset.remix_parent_id {
        if profile.liked_scene_ids.contains(parent_id)
            || profile.bookmarked_scene_ids.contains(parent_id)
            || profile.positive_scene_history.iter().any(|s| s == parent_id)
        {
            creator_aff += 0.35;
        }
    }
    let creator_affinity = creator_aff.clamp(-1.5, 2.5);

    // R5: 4-Tier Traffic Pool Hot Score (Wilson lower bound z=1.645 x freshness)
    let eff_plays = entry.effective_plays() as f64;
    let weighted_pos = (entry.asset.likes_count as f64
        + 1.5 * entry.asset.bookmarks_count as f64
        + 1.2 * entry.asset.comments_count as f64
        + 1.5 * entry.asset.shares_count as f64
        + 2.0 * entry.asset.remixes_count as f64
        + 0.4 * entry.stats.finish_count as f64)
        .min(eff_plays);
    let wilson = wilson_lower_bound(weighted_pos + 1.0, eff_plays + 4.0, 1.645);
    let hot_score = (wilson * (0.25 + 1.5 * freshness)).clamp(0.0, 2.0);

    // Assemble recall channel attribution tags
    let mut recall_channels = Vec::with_capacity(7);
    if is_fast_track {
        recall_channels.push(CHANNEL_JUST_PUBLISHED.to_string());
    }
    if embedding_sim > 0.05 {
        recall_channels.push(CHANNEL_EMBEDDING.to_string());
    }
    if swing_cf > 0.12 {
        recall_channels.push(CHANNEL_SWING_CF.to_string());
    }
    if tag_kind_affinity > 0.05 {
        recall_channels.push(CHANNEL_TAG_KIND.to_string());
    }
    if creator_affinity > 0.05 {
        recall_channels.push(CHANNEL_CREATOR_GRAPH.to_string());
    }
    if is_cold_start {
        recall_channels.push(CHANNEL_COLD_START.to_string());
    } else if hot_score > 0.03 {
        recall_channels.push(CHANNEL_HOT.to_string());
    }
    if recall_channels.is_empty() {
        recall_channels.push(CHANNEL_HOT.to_string());
    }

    RecallCandidate {
        scene_id: entry.asset.scene_id.clone(),
        recall_channels,
        embedding_sim,
        swing_cf,
        tag_kind_affinity,
        creator_affinity,
        hot_score,
        freshness,
        query_match_score,
        is_fast_track,
        in_seen_cooldown,
        is_cold_start,
    }
}

fn compute_swing_cf(
    entry: &CatalogEntry,
    catalog: &BTreeMap<String, CatalogEntry>,
    recent_positives: &[String],
    scene_positive_users: &BTreeMap<String, BTreeSet<i64>>,
    user_positive_scenes: &BTreeMap<i64, BTreeSet<String>>,
    user_id: i64,
) -> f64 {
    if recent_positives.is_empty() {
        return 0.0;
    }
    let users_i = scene_positive_users.get(&entry.asset.scene_id);
    let mut weighted_sum = 0.0_f64;
    let mut weight_total = 0.0_f64;

    for (idx, hist_id) in recent_positives.iter().enumerate() {
        if hist_id == &entry.asset.scene_id {
            continue;
        }
        let recency = 0.85_f64.powi(idx as i32);
        weight_total += recency;

        // Multi-user Alibaba Swing CF with user-activity damping and non-saturating rational compression:
        // sum_{u < v in (U_i ∩ U_j) \ {viewer}} (w(u) * w(v)) / (1 + |I_u^+ ∩ I_v^+|)
        let mut raw_swing = 0.0_f64;
        if let (Some(ui), Some(uj)) = (users_i, scene_positive_users.get(hist_id)) {
            let mut co_users: Vec<i64> = ui
                .intersection(uj)
                .copied()
                .filter(|&uid| uid != user_id)
                .collect();
            co_users.sort_by(|a, b| {
                let deg_a = user_positive_scenes.get(a).map_or(0, BTreeSet::len);
                let deg_b = user_positive_scenes.get(b).map_or(0, BTreeSet::len);
                deg_a.cmp(&deg_b).then_with(|| a.cmp(b))
            });
            co_users.truncate(24);
            let user_weight = |deg: usize| -> f64 {
                (4.0 / (deg.max(1) as f64 + 1.0)).powf(0.75)
            };
            if co_users.len() >= 2 {
                let weights: Vec<f64> = co_users
                    .iter()
                    .map(|uid| user_weight(user_positive_scenes.get(uid).map_or(1, BTreeSet::len)))
                    .collect();
                for a in 0..co_users.len() {
                    let va = user_positive_scenes.get(&co_users[a]);
                    let wa = weights[a];
                    for b in (a + 1)..co_users.len() {
                        let overlap = match (va, user_positive_scenes.get(&co_users[b])) {
                            (Some(sa), Some(sb)) => sa.intersection(sb).count() as f64,
                            _ => 0.0,
                        };
                        raw_swing += (wa * weights[b]) / (1.0 + overlap);
                    }
                }
            } else if co_users.len() == 1 {
                let deg = user_positive_scenes
                    .get(&co_users[0])
                    .map_or(1, BTreeSet::len);
                let w = user_weight(deg);
                raw_swing += 0.45 * (w * w) / (1.0 + deg.max(1) as f64);
            }
        }
        let multi_score = raw_swing / (raw_swing + 1.0);

        // Structural tag + code-token Jaccard fallback
        let struct_sim = if let Some(hist_entry) = catalog.get(hist_id) {
            let tag_jac = jaccard_similarity(&entry.normalized_tags, &hist_entry.normalized_tags);
            let code_jac = jaccard_similarity(&entry.code_tokens, &hist_entry.code_tokens);
            let same_kind = if entry.asset.kind == hist_entry.asset.kind {
                1.0
            } else {
                0.0
            };
            0.45 * tag_jac + 0.35 * code_jac + 0.20 * same_kind
        } else {
            0.0
        };

        let pair_score = if raw_swing > 0.0 {
            0.70 * multi_score + 0.30 * struct_sim
        } else {
            0.55 * struct_sim
        };
        weighted_sum += recency * pair_score;
    }

    if weight_total <= 1e-9 {
        0.0
    } else {
        (weighted_sum / weight_total).clamp(0.0, 1.0)
    }
}

fn select_top_k_indices<P, S>(
    candidates: &[RecallCandidate],
    quota: usize,
    predicate: P,
    score_fn: S,
    selected: &mut BTreeSet<usize>,
) where
    P: Fn(&RecallCandidate) -> bool,
    S: Fn(&RecallCandidate) -> f64,
{
    let mut ranked: Vec<(usize, f64)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| predicate(c))
        .map(|(i, c)| {
            let s = score_fn(c);
            (i, if s.is_finite() { s } else { f64::NEG_INFINITY })
        })
        .collect();

    ranked.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| candidates[a.0].scene_id.cmp(&candidates[b.0].scene_id))
    });

    for (idx, _) in ranked.into_iter().take(quota) {
        selected.insert(idx);
    }
}
