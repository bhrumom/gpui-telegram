//! Stage 3: Min-Max Normalized Greedy MMR (`lambda=0.72`), Hard Sliding-Window
//! Scatter Rules, Cold-Start Slot Injection (`3, 10, 17, ...`), Stateless Versioned
//! Cursor Pagination, and Bilingual `RecommendationExplanation` (`CS-F06`).

use std::collections::{BTreeMap, VecDeque};

use gpui_telegram_proto::{
    AssetKind, CodeSceneFeedTab, RankedSceneItemDto, RecommendationExplanation,
};
use serde::{Deserialize, Serialize};

use crate::reco::embedding::{effective_primary_tag, item_similarity, normalize_tags};
use crate::reco::profile::{RecommenderConfig, UserInterestProfile};
use crate::reco::recall::{
    CHANNEL_COLD_START, CHANNEL_CREATOR_GRAPH, CHANNEL_EMBEDDING, CHANNEL_HOT,
    CHANNEL_JUST_PUBLISHED, CHANNEL_SWING_CF, CHANNEL_TAG_KIND, CatalogEntry,
};
use crate::reco::scorer::ScoredCandidate;

pub const CURSOR_PREFIX: &str = "csc1:";
pub const MAX_CURSOR_BYTES: usize = 32_768;
pub const MAX_CURSOR_DELIVERED: usize = 512;
const MMR_TOP_POOL: usize = 60;
const MMR_WINDOW_LEN: usize = 5;

/// Stateless versioned cursor payload encoded as `csc1:<lowercase-hex-json>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedCursorPayload {
    pub v: u8,
    pub offset: usize,
    #[serde(default)]
    pub delivered: Vec<String>,
    #[serde(default)]
    pub tail_authors: Vec<i64>,
    #[serde(default)]
    pub tail_kinds: Vec<AssetKind>,
    #[serde(default)]
    pub tail_tags: Vec<Option<String>>,
}

/// Result of parsing an incoming pagination cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorParseResult {
    Fresh,
    NumericOffset(usize),
    Valid(FeedCursorPayload),
    Invalid,
}

/// Parses a cursor string fail-closed (enforcing `<= 32_768` bytes, valid hex, version `1`).
pub fn parse_cursor(raw: Option<&str>) -> CursorParseResult {
    let Some(s) = raw else {
        return CursorParseResult::Fresh;
    };
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return CursorParseResult::Fresh;
    }
    if trimmed.len() > MAX_CURSOR_BYTES {
        return CursorParseResult::Invalid;
    }
    if trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return match trimmed.parse::<usize>() {
            Ok(n) => CursorParseResult::NumericOffset(n),
            Err(_) => CursorParseResult::Invalid,
        };
    }
    let Some(hex_body) = trimmed.strip_prefix(CURSOR_PREFIX) else {
        return CursorParseResult::Invalid;
    };
    let Some(bytes) = decode_hex(hex_body) else {
        return CursorParseResult::Invalid;
    };
    let Ok(mut payload) = serde_json::from_slice::<FeedCursorPayload>(&bytes) else {
        return CursorParseResult::Invalid;
    };
    if payload.v != 1 {
        return CursorParseResult::Invalid;
    }
    if payload.delivered.len() > MAX_CURSOR_DELIVERED {
        payload.delivered.truncate(MAX_CURSOR_DELIVERED);
    }
    if payload.tail_authors.len() > 3 {
        let start = payload.tail_authors.len() - 3;
        payload.tail_authors.drain(0..start);
    }
    if payload.tail_kinds.len() > 2 {
        let start = payload.tail_kinds.len() - 2;
        payload.tail_kinds.drain(0..start);
    }
    if payload.tail_tags.len() > 2 {
        let start = payload.tail_tags.len() - 2;
        payload.tail_tags.drain(0..start);
    }
    CursorParseResult::Valid(payload)
}

/// Encodes a `FeedCursorPayload` as `csc1:<lowercase-hex-json>`.
pub fn encode_cursor(payload: &FeedCursorPayload) -> Option<String> {
    let json = serde_json::to_vec(payload).ok()?;
    let mut out = String::with_capacity(CURSOR_PREFIX.len() + json.len() * 2);
    out.push_str(CURSOR_PREFIX);
    for byte in json {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    if out.len() > MAX_CURSOR_BYTES {
        None
    } else {
        Some(out)
    }
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.is_empty() || hex.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Trailing sliding-window scatter state carried across page or tail seams.
#[derive(Debug, Clone, Default)]
pub struct ScatterSeed {
    pub offset: usize,
    pub tail_authors: Vec<i64>,
    pub tail_kinds: Vec<AssetKind>,
    pub tail_tags: Vec<Option<String>>,
    pub recent_scene_ids: Vec<String>,
}

impl ScatterSeed {
    pub fn from_prefix_items(offset: usize, prefix: &[RankedSceneItemDto]) -> Self {
        let tail3 = if prefix.len() > 3 {
            &prefix[prefix.len() - 3..]
        } else {
            prefix
        };
        let tail2 = if prefix.len() > 2 {
            &prefix[prefix.len() - 2..]
        } else {
            prefix
        };
        let tail5 = if prefix.len() > MMR_WINDOW_LEN {
            &prefix[prefix.len() - MMR_WINDOW_LEN..]
        } else {
            prefix
        };
        Self {
            offset,
            tail_authors: tail3.iter().map(|i| i.asset.author_user_id).collect(),
            tail_kinds: tail2.iter().map(|i| i.asset.kind).collect(),
            tail_tags: tail2
                .iter()
                .map(|i| effective_primary_tag(&normalize_tags(&i.asset.tags)))
                .collect(),
            recent_scene_ids: tail5.iter().map(|i| i.asset.scene_id.clone()).collect(),
        }
    }
}

/// Output of Stage 3 reranking, including updated trailing scatter state for cursor emission.
pub struct RerankOutput {
    pub items: Vec<RankedSceneItemDto>,
    pub tail_authors: Vec<i64>,
    pub tail_kinds: Vec<AssetKind>,
    pub tail_tags: Vec<Option<String>>,
    pub tail_scene_ids: Vec<String>,
}

/// Runs Stage 3: top-60 min-max normalization, greedy MMR (`lambda=0.72`),
/// hard sliding-window scatter rules, cold-start slot injection, viewer flag hydration,
/// and bilingual explanation generation.
///
/// Guarantees exact prefix invariance (`rerank(scored, k1) == rerank(scored, k2)[..k1]`
/// for all `k1 <= k2`) by normalizing and ranking the top-60 primary pool first and
/// processing any overflow items in subsequent `MMR_TOP_POOL` chunks only when needed.
pub fn rerank_and_scatter(
    mut scored: Vec<ScoredCandidate>,
    catalog: &BTreeMap<String, CatalogEntry>,
    profile: &UserInterestProfile,
    tab: CodeSceneFeedTab,
    limit: usize,
    seed: &ScatterSeed,
    cfg: &RecommenderConfig,
) -> RerankOutput {
    if scored.is_empty() || limit == 0 {
        return RerankOutput {
            items: Vec::new(),
            tail_authors: seed.tail_authors.clone(),
            tail_kinds: seed.tail_kinds.clone(),
            tail_tags: seed.tail_tags.clone(),
            tail_scene_ids: seed.recent_scene_ids.clone(),
        };
    }

    // Sort descending by exp_score (deterministic tie-break: scene_id ASC)
    scored.sort_by(|a, b| {
        b.exp_score
            .total_cmp(&a.exp_score)
            .then_with(|| a.recall.scene_id.cmp(&b.recall.scene_id))
    });

    // Partition into fixed top-60 primary pool (plus all fast-track and tier0_cold_start items)
    // and overflow pool so normalization & MMR on slots 0..primary_pool.len() never depend on `limit`.
    let mut primary_pool: Vec<ScoredCandidate> =
        Vec::with_capacity(scored.len().min(MMR_TOP_POOL + 16));
    let mut overflow_pool: Vec<ScoredCandidate> = Vec::new();
    for (idx, item) in scored.into_iter().enumerate() {
        if idx < MMR_TOP_POOL || item.recall.is_fast_track || item.recall.is_cold_start {
            primary_pool.push(item);
        } else {
            overflow_pool.push(item);
        }
    }

    let primary_limit = limit.min(primary_pool.len());
    let mut combined = rerank_pool_chunk(
        &primary_pool,
        catalog,
        profile,
        tab,
        primary_limit,
        seed,
        cfg,
    );

    if combined.items.len() < limit && !overflow_pool.is_empty() {
        let mut cursor_idx = 0usize;
        while combined.items.len() < limit && cursor_idx < overflow_pool.len() {
            let chunk_end = (cursor_idx + MMR_TOP_POOL).min(overflow_pool.len());
            let chunk = &overflow_pool[cursor_idx..chunk_end];
            let chunk_seed = ScatterSeed {
                offset: seed.offset.saturating_add(combined.items.len()),
                tail_authors: combined.tail_authors,
                tail_kinds: combined.tail_kinds,
                tail_tags: combined.tail_tags,
                recent_scene_ids: combined.tail_scene_ids,
            };
            let remaining = limit - combined.items.len();
            let chunk_out = rerank_pool_chunk(
                chunk,
                catalog,
                profile,
                tab,
                remaining,
                &chunk_seed,
                cfg,
            );
            combined.items.extend(chunk_out.items);
            combined.tail_authors = chunk_out.tail_authors;
            combined.tail_kinds = chunk_out.tail_kinds;
            combined.tail_tags = chunk_out.tail_tags;
            combined.tail_scene_ids = chunk_out.tail_scene_ids;
            cursor_idx = chunk_end;
        }
    }

    combined
}

fn rerank_pool_chunk(
    pool: &[ScoredCandidate],
    catalog: &BTreeMap<String, CatalogEntry>,
    profile: &UserInterestProfile,
    tab: CodeSceneFeedTab,
    limit: usize,
    seed: &ScatterSeed,
    cfg: &RecommenderConfig,
) -> RerankOutput {
    if pool.is_empty() || limit == 0 {
        return RerankOutput {
            items: Vec::new(),
            tail_authors: seed.tail_authors.clone(),
            tail_kinds: seed.tail_kinds.clone(),
            tail_tags: seed.tail_tags.clone(),
            tail_scene_ids: seed.recent_scene_ids.clone(),
        };
    }

    // Min-max normalize exp_score into [0.0, 1.0] across non-fast-track pool candidates
    let mut min_s = f64::INFINITY;
    let mut max_s = f64::NEG_INFINITY;
    for c in pool {
        if c.recall.is_fast_track {
            continue;
        }
        if c.exp_score.is_finite() {
            min_s = min_s.min(c.exp_score);
            max_s = max_s.max(c.exp_score);
        }
    }
    let span = if min_s.is_finite() && max_s.is_finite() && (max_s - min_s) > 1e-9 {
        max_s - min_s
    } else {
        1.0
    };
    let s_norms: Vec<f64> = pool
        .iter()
        .map(|c| {
            if c.recall.is_fast_track {
                1.0
            } else if min_s.is_finite() && max_s.is_finite() {
                if (max_s - min_s) <= 1e-9 {
                    0.5
                } else {
                    ((c.exp_score - min_s) / span).clamp(0.0, 1.0)
                }
            } else {
                0.5
            }
        })
        .collect();

    let lambda = cfg.mmr_lambda.clamp(0.0, 1.0);
    let enforce_kind_scatter = tab == CodeSceneFeedTab::ForYou;
    let target_len = limit.min(pool.len());

    let mut used = vec![false; pool.len()];
    let mut selected_items: Vec<RankedSceneItemDto> = Vec::with_capacity(target_len);

    let mut recent_authors: VecDeque<i64> = seed.tail_authors.iter().copied().collect();
    while recent_authors.len() > 3 {
        recent_authors.pop_front();
    }
    let mut recent_kinds: VecDeque<AssetKind> = seed.tail_kinds.iter().copied().collect();
    while recent_kinds.len() > 2 {
        recent_kinds.pop_front();
    }
    let mut recent_tags: VecDeque<Option<String>> = seed.tail_tags.iter().cloned().collect();
    while recent_tags.len() > 2 {
        recent_tags.pop_front();
    }
    let mut window_w5: VecDeque<String> = seed.recent_scene_ids.iter().cloned().collect();
    while window_w5.len() > MMR_WINDOW_LEN {
        window_w5.pop_front();
    }

    // Progressive scatter relaxation passes: (author_lookback, check_kind, check_tag).
    // Preserve Author Window 3/2 when relaxing check_tag or check_kind so single-tag /
    // tag_filter feeds with >= 4 distinct authors retain zero Author-Window-3 violations.
    let passes: [(usize, bool, bool); 12] = [
        (3, enforce_kind_scatter, true),
        (2, enforce_kind_scatter, true),
        (1, enforce_kind_scatter, true),
        (0, enforce_kind_scatter, true),
        (3, enforce_kind_scatter, false),
        (2, enforce_kind_scatter, false),
        (1, enforce_kind_scatter, false),
        (0, enforce_kind_scatter, false),
        (3, false, false),
        (2, false, false),
        (1, false, false),
        (0, false, false),
    ];

    let mut pending_cold_slot = false;

    for slot_idx in 0..target_len {
        let global_idx = seed.offset.saturating_add(slot_idx);
        if global_idx % 7 == 3 {
            pending_cold_slot = true;
        }
        let mut chosen: Option<(usize, f64, f64, bool)> = None; // (idx, mmr_penalty, final_score, forced_cold_slot)

        // Priority 1: CreatorJustPublished fast-track.
        // Unconditional ONLY at global_idx == 0; for global_idx > 0 (e.g. Page 2+ seams),
        // enforce strict scatter so Author Window 3 / Kind / Tag rules are never violated.
        if global_idx == 0 || slot_idx == 0 {
            let (strict_auth, strict_kind, strict_tag) = passes[0];
            if let Some(best_ft) = (0..pool.len())
                .filter(|&i| {
                    if used[i] || !pool[i].recall.is_fast_track {
                        return false;
                    }
                    if global_idx == 0 {
                        true
                    } else {
                        catalog.get(&pool[i].recall.scene_id).is_some_and(|entry| {
                            satisfies_scatter(
                                entry,
                                &recent_authors,
                                &recent_kinds,
                                &recent_tags,
                                strict_auth,
                                strict_kind,
                                strict_tag,
                            )
                        })
                    }
                })
                .max_by(|&a, &b| {
                    pool[a]
                        .exp_score
                        .total_cmp(&pool[b].exp_score)
                        .then_with(|| pool[b].recall.scene_id.cmp(&pool[a].recall.scene_id))
                })
            {
                let ft_is_cold = pool[best_ft].recall.is_cold_start;
                chosen = Some((best_ft, 0.0, pool[best_ft].exp_score, false));
                if pending_cold_slot && ft_is_cold {
                    pending_cold_slot = false;
                }
            }
        }

        // Priority 2: Cold-Start Traffic Pool Slot (global idx 3, 10, 17, ..., deferred if strict scatter blocks all remaining Tier-0 authors)
        if chosen.is_none() && pending_cold_slot {
            let (strict_auth, strict_kind, strict_tag) = passes[0];
            let mut best: Option<(usize, f64, f64)> = None;
            for i in 0..pool.len() {
                if used[i] || !pool[i].recall.is_cold_start {
                    continue;
                }
                let Some(entry) = catalog.get(&pool[i].recall.scene_id) else {
                    continue;
                };
                if !satisfies_scatter(
                    entry,
                    &recent_authors,
                    &recent_kinds,
                    &recent_tags,
                    strict_auth,
                    strict_kind,
                    strict_tag,
                ) {
                    continue;
                }
                let zero_guard = if pool[i].zero_impression_brand_new {
                    0.85
                } else {
                    0.0
                };
                let cold_score = s_norms[i]
                    + pool[i].exploration_bonus
                    + 1.5 * pool[i].recall.freshness
                    + zero_guard;
                let max_sim = max_window_similarity(entry, &window_w5, catalog);
                let mmr_pen = ((1.0 - lambda) * max_sim).max(0.0);
                let is_better = match best {
                    None => true,
                    Some((bi, bs, _)) => {
                        cold_score > bs
                            || ((cold_score - bs).abs() <= 1e-12
                                && pool[i].recall.scene_id < pool[bi].recall.scene_id)
                    }
                };
                if is_better {
                    best = Some((i, cold_score, mmr_pen));
                }
            }
            if let Some((bi, cold_score, mmr_pen)) = best {
                chosen = Some((bi, mmr_pen, cold_score, true));
                pending_cold_slot = false;
            }
        }

        // Priority 3: Standard Greedy MMR (lambda = 0.72) under Hard Sliding-Window Scatter
        if chosen.is_none() {
            for &(auth_win, chk_kind, chk_tag) in &passes {
                let mut best: Option<(usize, f64, f64)> = None;
                for i in 0..pool.len() {
                    if used[i] {
                        continue;
                    }
                    let Some(entry) = catalog.get(&pool[i].recall.scene_id) else {
                        continue;
                    };
                    if !satisfies_scatter(
                        entry,
                        &recent_authors,
                        &recent_kinds,
                        &recent_tags,
                        auth_win,
                        chk_kind,
                        chk_tag,
                    ) {
                        continue;
                    }
                    let max_sim = max_window_similarity(entry, &window_w5, catalog);
                    let mmr_pen = ((1.0 - lambda) * max_sim).max(0.0);
                    let mmr_val = lambda * s_norms[i] - mmr_pen;
                    let is_better = match best {
                        None => true,
                        Some((bi, bs, _)) => {
                            mmr_val > bs + 1e-12
                                || ((mmr_val - bs).abs() <= 1e-12
                                    && (pool[i].exp_score > pool[bi].exp_score + 1e-12
                                        || ((pool[i].exp_score - pool[bi].exp_score).abs() <= 1e-12
                                            && pool[i].recall.scene_id < pool[bi].recall.scene_id)))
                        }
                    };
                    if is_better {
                        best = Some((i, mmr_val, mmr_pen));
                    }
                }
                if let Some((bi, mmr_val, mmr_pen)) = best {
                    chosen = Some((bi, mmr_pen, mmr_val, false));
                    break;
                }
            }
        }

        let Some((idx, mmr_pen, final_score, forced_cold_slot)) = chosen else {
            break;
        };
        used[idx] = true;
        let cand = &pool[idx];
        let Some(entry) = catalog.get(&cand.recall.scene_id) else {
            continue;
        };

        // Update sliding windows
        if recent_authors.len() == 3 {
            recent_authors.pop_front();
        }
        recent_authors.push_back(entry.asset.author_user_id);

        if recent_kinds.len() == 2 {
            recent_kinds.pop_front();
        }
        recent_kinds.push_back(entry.asset.kind);

        if recent_tags.len() == 2 {
            recent_tags.pop_front();
        }
        recent_tags.push_back(entry.effective_primary_tag.clone());

        if window_w5.len() == MMR_WINDOW_LEN {
            window_w5.pop_front();
        }
        window_w5.push_back(entry.asset.scene_id.clone());

        selected_items.push(build_ranked_item(
            entry,
            cand,
            profile,
            mmr_pen,
            final_score,
            forced_cold_slot,
        ));
    }

    RerankOutput {
        items: selected_items,
        tail_authors: recent_authors.into_iter().collect(),
        tail_kinds: recent_kinds.into_iter().collect(),
        tail_tags: recent_tags.into_iter().collect(),
        tail_scene_ids: window_w5.into_iter().collect(),
    }
}

fn satisfies_scatter(
    entry: &CatalogEntry,
    recent_authors: &VecDeque<i64>,
    recent_kinds: &VecDeque<AssetKind>,
    recent_tags: &VecDeque<Option<String>>,
    author_lookback: usize,
    check_kind: bool,
    check_tag: bool,
) -> bool {
    if author_lookback > 0 {
        let len = recent_authors.len();
        let skip = len.saturating_sub(author_lookback);
        if recent_authors
            .iter()
            .skip(skip)
            .any(|&a| a == entry.asset.author_user_id)
        {
            return false;
        }
    }
    if check_kind && recent_kinds.len() >= 2 {
        let last_two_same = recent_kinds[recent_kinds.len() - 1] == entry.asset.kind
            && recent_kinds[recent_kinds.len() - 2] == entry.asset.kind;
        if last_two_same {
            return false;
        }
    }
    if check_tag && recent_tags.len() >= 2 {
        if let Some(ref tag) = entry.effective_primary_tag {
            let t1 = recent_tags[recent_tags.len() - 1].as_deref();
            let t2 = recent_tags[recent_tags.len() - 2].as_deref();
            if t1 == Some(tag.as_str()) && t2 == Some(tag.as_str()) {
                return false;
            }
        }
    }
    true
}

fn max_window_similarity(
    entry: &CatalogEntry,
    window_w5: &VecDeque<String>,
    catalog: &BTreeMap<String, CatalogEntry>,
) -> f64 {
    let mut max_sim = 0.0_f64;
    for prev_id in window_w5 {
        if let Some(prev) = catalog.get(prev_id) {
            let sim = item_similarity(
                &entry.embedding,
                &entry.normalized_tags,
                entry.asset.kind,
                &prev.embedding,
                &prev.normalized_tags,
                prev.asset.kind,
            );
            if sim > max_sim {
                max_sim = sim;
            }
        }
    }
    max_sim.clamp(0.0, 1.0)
}

/// Hydrates viewer flags on `CodeSceneAssetDto` and builds a localized `RankedSceneItemDto`.
pub fn build_ranked_item(
    entry: &CatalogEntry,
    cand: &ScoredCandidate,
    profile: &UserInterestProfile,
    mmr_penalty: f64,
    final_score: f64,
    forced_cold_slot: bool,
) -> RankedSceneItemDto {
    let mut asset = entry.asset.clone();
    asset.viewer_liked = profile.liked_scene_ids.contains(&asset.scene_id);
    asset.viewer_bookmarked = profile.bookmarked_scene_ids.contains(&asset.scene_id);
    asset.viewer_following_author = profile.followed_creators.contains(&asset.author_user_id);

    let mut channels = cand.recall.recall_channels.clone();
    if forced_cold_slot && !channels.iter().any(|c| c == CHANNEL_COLD_START) {
        channels.push(CHANNEL_COLD_START.to_string());
    }
    dedup_preserve_order(&mut channels);
    if channels.is_empty() {
        channels.push(CHANNEL_HOT.to_string());
    }

    let mut breakdown = cand.breakdown;
    breakdown.mmr_penalty = mmr_penalty;
    breakdown.final_score = final_score;
    let breakdown = breakdown.sanitized();

    let (primary_reason_zh, primary_reason_en) =
        localize_primary_reason(entry, cand, forced_cold_slot);

    RankedSceneItemDto {
        asset,
        explanation: RecommendationExplanation {
            recall_channels: channels,
            primary_reason_zh,
            primary_reason_en,
            traffic_tier: cand.traffic_tier.to_string(),
            breakdown,
        },
    }
}

fn localize_primary_reason(
    entry: &CatalogEntry,
    cand: &ScoredCandidate,
    forced_cold_slot: bool,
) -> (String, String) {
    if cand.recall.is_fast_track {
        return (
            "刚刚由你的 AI Bot 创作发布".to_string(),
            "Just published by your AI Bot".to_string(),
        );
    }
    if forced_cold_slot || (cand.recall.is_cold_start && cand.exploration_bonus >= 0.45) {
        return (
            "新锐冷启动代码作品推荐".to_string(),
            "Fresh cold-start code creation spotlight".to_string(),
        );
    }
    if cand.recall.query_match_score >= 0.30 {
        return (
            "精准匹配你的搜索主题".to_string(),
            "Matches your search query".to_string(),
        );
    }
    if cand.recall.creator_affinity >= 0.85 {
        return (
            format!("来自你关注的创作者 @{}", entry.asset.author_username),
            format!("From creator @{} you follow", entry.asset.author_username),
        );
    }
    if cand.recall.swing_cf >= 0.35
        && cand.recall.swing_cf >= cand.recall.embedding_sim * 0.85
    {
        return (
            "喜欢相似代码场景的用户也在刷".to_string(),
            "Popular with viewers who liked similar code scenes".to_string(),
        );
    }
    if cand.recall.embedding_sim >= 0.25 || cand.recall.tag_kind_affinity >= 0.45 {
        if let Some(ref tag) = entry.effective_primary_tag {
            return (
                format!("因为你喜欢 #{} · {}", tag, entry.asset.kind.label_zh()),
                format!(
                    "Because you enjoy #{} · {}",
                    tag,
                    entry.asset.kind.label_en()
                ),
            );
        }
        return (
            format!("根据你的{}偏好实时推荐", entry.asset.kind.label_zh()),
            format!(
                "Personalized for your {} taste",
                entry.asset.kind.label_en()
            ),
        );
    }
    if cand.recall.recall_channels.iter().any(|c| c == CHANNEL_CREATOR_GRAPH) {
        return (
            format!("创作者 @{} 的精选代码作品", entry.asset.author_username),
            format!("Featured creation by @{}", entry.asset.author_username),
        );
    }
    if cand.recall.recall_channels.iter().any(|c| c == CHANNEL_EMBEDDING || c == CHANNEL_TAG_KIND) {
        return (
            "根据你的实时兴趣为你精选".to_string(),
            "Tailored to your live interests".to_string(),
        );
    }
    if cand.recall.recall_channels.iter().any(|c| c == CHANNEL_SWING_CF) {
        return (
            "协同发现 · 高完成率代码作品".to_string(),
            "Co-engagement pick · high completion code scene".to_string(),
        );
    }
    if cand.recall.recall_channels.iter().any(|c| c == CHANNEL_JUST_PUBLISHED) {
        return (
            "最新发布的 AI Bot 作品".to_string(),
            "Newly published AI Bot creation".to_string(),
        );
    }
    (
        format!("高口碑热门{}", entry.asset.kind.label_zh()),
        format!("Trending high-engagement {}", entry.asset.kind.label_en()),
    )
}

fn dedup_preserve_order(items: &mut Vec<String>) {
    let mut out = Vec::with_capacity(items.len());
    for s in items.drain(..) {
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }
    *items = out;
}
