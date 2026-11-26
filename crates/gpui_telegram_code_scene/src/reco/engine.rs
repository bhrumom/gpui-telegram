//! `Send + Sync` 5-Stage Douyin-Grade `CodeSceneRecommender` Engine (`CS-F05`, `CS-F06`).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::Utc;
use gpui_telegram_proto::{
    CodeSceneAssetDto, CodeSceneFeedRequest, CodeSceneFeedResponse, ListCodeSceneAssetsQuery,
    ListCodeSceneAssetsResponse, RankedSceneItemDto, SceneBehaviorEventDto, SceneBehaviorEventKind,
    ToggleBookmarkResponse, ToggleFollowResponse, ToggleLikeResponse,
};

use crate::reco::embedding::{
    effective_primary_tag, embed_asset, extract_code_feature_tokens,
    extract_compiled_feature_tokens, normalize_tag, normalize_tags,
};
use crate::reco::profile::{RecommenderConfig, UserInterestProfile};
use crate::reco::recall::{CatalogEntry, recall_candidates, recall_single_asset};
use crate::reco::reranker::{
    CursorParseResult, FeedCursorPayload, MAX_CURSOR_DELIVERED, RerankOutput, ScatterSeed,
    build_ranked_item, encode_cursor, parse_cursor, rerank_and_scatter,
};
use crate::reco::scorer::{score_candidates, score_one_candidate};
use crate::reco::telemetry::{
    ItemEngagementStats, ResolvedEventTarget, RewardOutcome, apply_behavior_event,
};
use crate::runtime::VerifiedProgram;
use crate::seed_library::official_seed_assets;

/// Pure-Rust, 100% `Send + Sync` 5-stage recommendation engine for Code Scene.
#[derive(Debug, Clone)]
pub struct CodeSceneRecommender {
    config: RecommenderConfig,
    catalog: BTreeMap<String, CatalogEntry>,
    profiles: BTreeMap<i64, UserInterestProfile>,
    /// Swing CF reverse index: `scene_id -> set of user_ids` with positive engagement.
    scene_positive_users: BTreeMap<String, BTreeSet<i64>>,
    /// Swing CF forward index: `user_id -> set of scene_ids` with positive engagement.
    user_positive_scenes: BTreeMap<i64, BTreeSet<String>>,
    /// Deterministic clock override for reproducible evaluation & tests.
    fixed_now_ms: Option<i64>,
}

impl Default for CodeSceneRecommender {
    fn default() -> Self {
        Self::new_with_seeds()
    }
}

impl CodeSceneRecommender {
    /// Creates a recommender pre-loaded with all 18 official real-code seed assets.
    pub fn new_with_seeds() -> Self {
        Self::new_with_seeds_and_config(RecommenderConfig::default())
    }

    /// Creates an empty recommender with zero catalog assets.
    pub fn new_empty() -> Self {
        Self::with_config(RecommenderConfig::default())
    }

    /// Creates an empty recommender with custom hyperparameters.
    pub fn with_config(config: RecommenderConfig) -> Self {
        Self {
            config,
            catalog: BTreeMap::new(),
            profiles: BTreeMap::new(),
            scene_positive_users: BTreeMap::new(),
            user_positive_scenes: BTreeMap::new(),
            fixed_now_ms: None,
        }
    }

    /// Creates a recommender with custom config pre-loaded with the 18 official seeds.
    pub fn new_with_seeds_and_config(config: RecommenderConfig) -> Self {
        let mut engine = Self::with_config(config);
        for asset in official_seed_assets() {
            engine.register_or_update_asset(asset, None);
        }
        engine
    }

    /// Overrides the engine clock (`Some(ms)`) for deterministic evaluation or clears it (`None`).
    pub fn set_now_ms(&mut self, now_ms: Option<i64>) {
        self.fixed_now_ms = now_ms;
    }

    /// Returns the active engine timestamp in milliseconds.
    pub fn now_ms(&self) -> i64 {
        self.fixed_now_ms
            .unwrap_or_else(|| Utc::now().timestamp_millis())
    }

    /// Returns the active engine configuration.
    pub fn config(&self) -> &RecommenderConfig {
        &self.config
    }

    /// Runs the full 5-stage recommendation pipeline using `self.now_ms()`.
    pub fn recommend_feed(&self, req: &CodeSceneFeedRequest) -> CodeSceneFeedResponse {
        self.recommend_feed_at(req, self.now_ms())
    }

    /// Runs the full 5-stage recommendation pipeline at an explicit deterministic timestamp.
    pub fn recommend_feed_at(
        &self,
        req: &CodeSceneFeedRequest,
        now_ms: i64,
    ) -> CodeSceneFeedResponse {
        let total_catalog = self.catalog.len();
        if total_catalog == 0 {
            return CodeSceneFeedResponse {
                items: Vec::new(),
                next_cursor: None,
                total_catalog: 0,
            };
        }

        let limit = req.clamped_limit();
        let default_profile;
        let profile = match self.profiles.get(&req.user_id) {
            Some(p) => p,
            None => {
                default_profile = UserInterestProfile::new(req.user_id);
                &default_profile
            }
        };

        let (exclude_set, prior_delivered, seed, apply_seen_cooldown, skip_numeric) =
            match parse_cursor(req.cursor.as_deref()) {
                CursorParseResult::Invalid => {
                    return CodeSceneFeedResponse {
                        items: Vec::new(),
                        next_cursor: None,
                        total_catalog,
                    };
                }
                CursorParseResult::Fresh => (
                    HashSet::new(),
                    Vec::new(),
                    ScatterSeed::default(),
                    true,
                    0usize,
                ),
                CursorParseResult::NumericOffset(off) => {
                    if off >= total_catalog {
                        return CodeSceneFeedResponse {
                            items: Vec::new(),
                            next_cursor: None,
                            total_catalog,
                        };
                    }
                    (
                        HashSet::new(),
                        Vec::new(),
                        ScatterSeed::default(),
                        true,
                        off,
                    )
                }
                CursorParseResult::Valid(payload) => {
                    let excl: HashSet<String> = payload.delivered.iter().cloned().collect();
                    let recent_scene_ids: Vec<String> = payload
                        .delivered
                        .iter()
                        .rev()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    let seed = ScatterSeed {
                        offset: payload.offset,
                        tail_authors: payload.tail_authors,
                        tail_kinds: payload.tail_kinds,
                        tail_tags: payload.tail_tags,
                        recent_scene_ids,
                    };
                    (excl, payload.delivered, seed, false, 0usize)
                }
            };

        let fetch_limit = limit.saturating_add(skip_numeric);
        let recall_out = recall_candidates(
            &self.catalog,
            profile,
            &self.scene_positive_users,
            &self.user_positive_scenes,
            req,
            &exclude_set,
            apply_seen_cooldown,
            fetch_limit,
            &self.config,
            now_ms,
        );
        if recall_out.candidates.is_empty() && recall_out.cooldown_candidates.is_empty() {
            return CodeSceneFeedResponse {
                items: Vec::new(),
                next_cursor: None,
                total_catalog,
            };
        }

        let total_catalog_plays = self.total_catalog_plays();
        let has_query = req
            .query
            .as_deref()
            .is_some_and(|q| !q.trim().is_empty());

        // Phase 1: Score + rerank strictly on fresh (non-cooldown) candidates so seen-cooldown
        // backfill never displaces fresh items across NumericOffset pagination boundaries.
        let mut rerank_out = if !recall_out.candidates.is_empty() {
            let scored = score_candidates(
                &recall_out.candidates,
                &self.catalog,
                profile,
                total_catalog_plays,
                has_query,
                &self.config,
            );
            rerank_and_scatter(
                scored,
                &self.catalog,
                profile,
                req.tab,
                fetch_limit,
                &seed,
                &self.config,
            )
        } else {
            RerankOutput {
                items: Vec::new(),
                tail_authors: seed.tail_authors.clone(),
                tail_kinds: seed.tail_kinds.clone(),
                tail_tags: seed.tail_tags.clone(),
                tail_scene_ids: seed.recent_scene_ids.clone(),
            }
        };

        // Phase 2: ONLY if fresh pool has fewer than `fetch_limit` items, score + rerank
        // seen-cooldown candidates for the remaining slots, seeded from Phase 1's exact tail.
        if rerank_out.items.len() < fetch_limit && !recall_out.cooldown_candidates.is_empty() {
            let remaining = fetch_limit - rerank_out.items.len();
            let cooldown_scored = score_candidates(
                &recall_out.cooldown_candidates,
                &self.catalog,
                profile,
                total_catalog_plays,
                has_query,
                &self.config,
            );
            let cooldown_seed = ScatterSeed {
                offset: seed.offset.saturating_add(rerank_out.items.len()),
                tail_authors: rerank_out.tail_authors,
                tail_kinds: rerank_out.tail_kinds,
                tail_tags: rerank_out.tail_tags,
                recent_scene_ids: rerank_out.tail_scene_ids,
            };
            let cooldown_out = rerank_and_scatter(
                cooldown_scored,
                &self.catalog,
                profile,
                req.tab,
                remaining,
                &cooldown_seed,
                &self.config,
            );
            rerank_out.items.extend(cooldown_out.items);
            rerank_out.tail_authors = cooldown_out.tail_authors;
            rerank_out.tail_kinds = cooldown_out.tail_kinds;
            rerank_out.tail_tags = cooldown_out.tail_tags;
            rerank_out.tail_scene_ids = cooldown_out.tail_scene_ids;
        }

        let all_selected = rerank_out.items;
        let consumed = all_selected.len().min(skip_numeric.saturating_add(limit));
        let mut delivered = prior_delivered;
        for item in all_selected.iter().take(consumed) {
            if delivered.len() < MAX_CURSOR_DELIVERED {
                delivered.push(item.asset.scene_id.clone());
            }
        }
        let items: Vec<RankedSceneItemDto> = if skip_numeric > 0 {
            all_selected.into_iter().skip(skip_numeric).take(limit).collect()
        } else {
            all_selected
        };
        let has_more = recall_out.total_eligible > consumed
            && !items.is_empty()
            && delivered.len() < MAX_CURSOR_DELIVERED;

        let next_cursor = if has_more {
            let next_payload = FeedCursorPayload {
                v: 1,
                offset: seed.offset.saturating_add(consumed),
                delivered,
                tail_authors: rerank_out.tail_authors,
                tail_kinds: rerank_out.tail_kinds,
                tail_tags: rerank_out.tail_tags,
            };
            encode_cursor(&next_payload)
        } else {
            None
        };

        CodeSceneFeedResponse {
            items,
            next_cursor,
            total_catalog,
        }
    }

    /// Ranks an unseen tail starting immediately after `recent_prefix` (preserving sliding-window
    /// scatter continuity across the seam and excluding `exclude_scene_ids`).
    pub fn recommend_unseen_tail(
        &self,
        req: &CodeSceneFeedRequest,
        exclude_scene_ids: &HashSet<String>,
        recent_prefix: &[RankedSceneItemDto],
        now_ms: i64,
    ) -> CodeSceneFeedResponse {
        let total_catalog = self.catalog.len();
        if total_catalog == 0 {
            return CodeSceneFeedResponse {
                items: Vec::new(),
                next_cursor: None,
                total_catalog: 0,
            };
        }

        let limit = req.clamped_limit();
        let default_profile;
        let profile = match self.profiles.get(&req.user_id) {
            Some(p) => p,
            None => {
                default_profile = UserInterestProfile::new(req.user_id);
                &default_profile
            }
        };

        let recall_out = recall_candidates(
            &self.catalog,
            profile,
            &self.scene_positive_users,
            &self.user_positive_scenes,
            req,
            exclude_scene_ids,
            false,
            limit,
            &self.config,
            now_ms,
        );
        if recall_out.candidates.is_empty() {
            return CodeSceneFeedResponse {
                items: Vec::new(),
                next_cursor: None,
                total_catalog,
            };
        }

        let total_catalog_plays = self.total_catalog_plays();
        let has_query = req
            .query
            .as_deref()
            .is_some_and(|q| !q.trim().is_empty());
        let scored = score_candidates(
            &recall_out.candidates,
            &self.catalog,
            profile,
            total_catalog_plays,
            has_query,
            &self.config,
        );

        let seed = ScatterSeed::from_prefix_items(exclude_scene_ids.len(), recent_prefix);
        let rerank_out = rerank_and_scatter(
            scored,
            &self.catalog,
            profile,
            req.tab,
            limit,
            &seed,
            &self.config,
        );

        let mut delivered: Vec<String> = exclude_scene_ids.iter().cloned().collect();
        delivered.sort();
        for item in &rerank_out.items {
            if delivered.len() < MAX_CURSOR_DELIVERED {
                delivered.push(item.asset.scene_id.clone());
            }
        }
        let has_more = recall_out.total_eligible > rerank_out.items.len()
            && !rerank_out.items.is_empty()
            && delivered.len() < MAX_CURSOR_DELIVERED;

        let next_cursor = if has_more {
            let payload = FeedCursorPayload {
                v: 1,
                offset: seed.offset.saturating_add(rerank_out.items.len()),
                delivered,
                tail_authors: rerank_out.tail_authors,
                tail_kinds: rerank_out.tail_kinds,
                tail_tags: rerank_out.tail_tags,
            };
            encode_cursor(&payload)
        } else {
            None
        };

        CodeSceneFeedResponse {
            items: rerank_out.items,
            next_cursor,
            total_catalog,
        }
    }

    /// Computes a full hydrated `RankedSceneItemDto` and explanation for a specific `scene_id`
    /// (used for `code_scene:open:<id>` deep-links). Returns `None` if not in catalog.
    pub fn explain_asset_for_user(
        &self,
        user_id: i64,
        scene_id: &str,
        now_ms: i64,
    ) -> Option<RankedSceneItemDto> {
        let entry = self.catalog.get(scene_id)?;
        let default_profile;
        let profile = match self.profiles.get(&user_id) {
            Some(p) => p,
            None => {
                default_profile = UserInterestProfile::new(user_id);
                &default_profile
            }
        };
        let recall = recall_single_asset(
            entry,
            &self.catalog,
            profile,
            &self.scene_positive_users,
            &self.user_positive_scenes,
            user_id,
            &self.config,
            now_ms,
        );
        let scored = score_one_candidate(
            &recall,
            entry,
            profile,
            self.total_catalog_plays(),
            false,
            &self.config,
        );
        Some(build_ranked_item(
            entry,
            &scored,
            profile,
            0.0,
            scored.exp_score,
            false,
        ))
    }

    /// Stage 0: ingests one behavioral telemetry event, updating user profile,
    /// Swing CF co-engagement index, and catalog engagement counters in real time.
    pub fn record_behavior(&mut self, event: &SceneBehaviorEventDto) -> RewardOutcome {
        let now_ms = self.now_ms();
        let (asset_kind, author_user_id, norm_tags, catalog_duration_ms, item_emb) =
            if let Some(entry) = self.catalog.get(&event.scene_id) {
                (
                    Some(entry.asset.kind),
                    Some(entry.asset.author_user_id),
                    entry.normalized_tags.clone(),
                    entry.duration_ms(),
                    Some(entry.embedding.clone()),
                )
            } else {
                (
                    event.asset_kind,
                    event.author_user_id,
                    normalize_tags(&event.tags),
                    if event.duration_ms > 0 {
                        event.duration_ms
                    } else {
                        10_000
                    },
                    None,
                )
            };

        let target = ResolvedEventTarget {
            scene_id: &event.scene_id,
            asset_kind,
            author_user_id,
            normalized_tags: norm_tags,
            catalog_duration_ms,
            item_embedding: item_emb.as_deref(),
        };

        let profile = self
            .profiles
            .entry(event.user_id)
            .or_insert_with(|| UserInterestProfile::new(event.user_id));

        let (outcome, fx) = apply_behavior_event(event, &target, profile, &self.config, now_ms);

        // Sync Swing CF co-engagement index
        if !event.scene_id.is_empty() {
            if fx.remove_positive_coengagement {
                if let Some(users) = self.scene_positive_users.get_mut(&event.scene_id) {
                    users.remove(&event.user_id);
                    if users.is_empty() {
                        self.scene_positive_users.remove(&event.scene_id);
                    }
                }
                if let Some(scenes) = self.user_positive_scenes.get_mut(&event.user_id) {
                    scenes.remove(&event.scene_id);
                    if scenes.is_empty() {
                        self.user_positive_scenes.remove(&event.user_id);
                    }
                }
            } else if fx.add_positive_coengagement {
                self.scene_positive_users
                    .entry(event.scene_id.clone())
                    .or_default()
                    .insert(event.user_id);
                self.user_positive_scenes
                    .entry(event.user_id)
                    .or_default()
                    .insert(event.scene_id.clone());
            }
        }
        if outcome.hard_negative_applied {
            self.purge_blocked_positives_for_user(event.user_id);
        }

        // Sync catalog counters & engagement stats
        if let Some(entry) = self.catalog.get_mut(&event.scene_id) {
            if fx.delta_plays > 0 {
                entry.asset.plays_count = entry.asset.plays_count.saturating_add(fx.delta_plays);
            }
            if fx.delta_likes > 0 {
                entry.asset.likes_count =
                    entry.asset.likes_count.saturating_add(fx.delta_likes as u64);
            } else if fx.delta_likes < 0 {
                entry.asset.likes_count =
                    entry.asset.likes_count.saturating_sub((-fx.delta_likes) as u64);
            }
            if fx.delta_bookmarks > 0 {
                entry.asset.bookmarks_count = entry
                    .asset
                    .bookmarks_count
                    .saturating_add(fx.delta_bookmarks as u64);
            } else if fx.delta_bookmarks < 0 {
                entry.asset.bookmarks_count = entry
                    .asset
                    .bookmarks_count
                    .saturating_sub((-fx.delta_bookmarks) as u64);
            }
            if fx.delta_comments > 0 {
                entry.asset.comments_count =
                    entry.asset.comments_count.saturating_add(fx.delta_comments);
            }
            if fx.delta_shares > 0 {
                entry.asset.shares_count = entry.asset.shares_count.saturating_add(fx.delta_shares);
            }
            if fx.delta_remixes > 0 {
                entry.asset.remixes_count =
                    entry.asset.remixes_count.saturating_add(fx.delta_remixes);
            }
            if fx.add_impression {
                entry.stats.impressions = entry.stats.impressions.saturating_add(1);
            }
            if let Some(dwell_norm) = fx.add_dwell_norm {
                if dwell_norm.is_finite() {
                    entry.stats.dwell_score_sum += dwell_norm.clamp(0.0, 1.0);
                    entry.stats.dwell_events = entry.stats.dwell_events.saturating_add(1);
                }
            }
            if fx.add_finish {
                entry.stats.finish_count = entry.stats.finish_count.saturating_add(1);
            }
            if fx.add_skip {
                entry.stats.skip_count = entry.stats.skip_count.saturating_add(1);
            }
            if fx.add_not_interested {
                entry.stats.not_interested_count =
                    entry.stats.not_interested_count.saturating_add(1);
            }
            if fx.clear_just_published_fast_track {
                if let Some((pub_uid, _)) = entry.just_published_by {
                    if pub_uid == event.user_id {
                        entry.just_published_by = None;
                    }
                }
            }
        }

        outcome
    }

    /// Batch helper recording multiple behavioral events in order.
    pub fn record_behaviors(&mut self, events: &[SceneBehaviorEventDto]) -> Vec<RewardOutcome> {
        events.iter().map(|e| self.record_behavior(e)).collect()
    }

    /// Registers or updates an asset in the catalog, extracting code feature tokens
    /// and computing its 64-dim dense embedding.
    pub fn register_or_update_asset(
        &mut self,
        asset: CodeSceneAssetDto,
        just_published_by_user: Option<i64>,
    ) {
        let code_tokens = extract_code_feature_tokens(&asset.source_code);
        self.upsert_entry(asset, code_tokens, just_published_by_user);
    }

    /// Registers or updates an asset using already-verified `VerifiedProgram.compiled`.
    pub fn register_verified_asset(
        &mut self,
        asset: CodeSceneAssetDto,
        verified: &VerifiedProgram,
        just_published_by_user: Option<i64>,
    ) {
        let code_tokens = extract_compiled_feature_tokens(&verified.compiled);
        self.upsert_entry(asset, code_tokens, just_published_by_user);
    }

    fn upsert_entry(
        &mut self,
        asset: CodeSceneAssetDto,
        code_tokens: Vec<String>,
        just_published_by_user: Option<i64>,
    ) {
        let now_ms = self.now_ms();
        let norm_tags = normalize_tags(&asset.tags);
        let eff_primary = effective_primary_tag(&norm_tags);
        let embedding = embed_asset(&asset, &code_tokens);
        let existing_stats = self
            .catalog
            .get(&asset.scene_id)
            .map(|e| e.stats.clone())
            .unwrap_or_else(ItemEngagementStats::default);
        let fast_track = just_published_by_user.map(|uid| (uid, now_ms));

        let entry = CatalogEntry {
            asset,
            normalized_tags: norm_tags,
            effective_primary_tag: eff_primary,
            code_tokens,
            embedding,
            stats: existing_stats,
            just_published_by: fast_track,
        };
        self.catalog.insert(entry.asset.scene_id.clone(), entry);
    }

    /// Removes an asset from the catalog and cleans up its Swing CF index entries.
    pub fn remove_asset(&mut self, scene_id: &str) -> bool {
        let removed = self.catalog.remove(scene_id).is_some();
        if let Some(users) = self.scene_positive_users.remove(scene_id) {
            for uid in users {
                if let Some(scenes) = self.user_positive_scenes.get_mut(&uid) {
                    scenes.remove(scene_id);
                    if scenes.is_empty() {
                        self.user_positive_scenes.remove(&uid);
                    }
                }
            }
        }
        removed
    }

    /// Exports a clone of a user's persistent interest profile for DB/cache storage.
    pub fn export_user_profile(&self, user_id: i64) -> Option<UserInterestProfile> {
        self.profiles.get(&user_id).cloned()
    }

    /// Imports and sanitizes a user interest profile, rebuilding its Swing CF reverse index.
    pub fn import_user_profile(&mut self, mut profile: UserInterestProfile) {
        profile.sanitize();
        let uid = profile.user_id;

        // Clear old reverse index entries for this user
        if let Some(old_scenes) = self.user_positive_scenes.remove(&uid) {
            for sid in old_scenes {
                if let Some(users) = self.scene_positive_users.get_mut(&sid) {
                    users.remove(&uid);
                    if users.is_empty() {
                        self.scene_positive_users.remove(&sid);
                    }
                }
            }
        }

        // Purge any newly blocked scenes from imported positive_scene_history
        let blocked_hist: Vec<String> = profile
            .positive_scene_history
            .iter()
            .filter(|sid| is_scene_blocked_for_profile(&self.catalog, &profile, sid))
            .cloned()
            .collect();
        if !blocked_hist.is_empty() {
            let blocked_set: HashSet<&str> = blocked_hist.iter().map(String::as_str).collect();
            profile
                .positive_scene_history
                .retain(|sid| !blocked_set.contains(sid.as_str()));
        }

        // Rebuild reverse index from non-blocked imported positive history, likes, and bookmarks
        let mut positive_set = BTreeSet::new();
        for sid in &profile.positive_scene_history {
            if !is_scene_blocked_for_profile(&self.catalog, &profile, sid) {
                positive_set.insert(sid.clone());
            }
        }
        for sid in &profile.liked_scene_ids {
            if !is_scene_blocked_for_profile(&self.catalog, &profile, sid) {
                positive_set.insert(sid.clone());
            }
        }
        for sid in &profile.bookmarked_scene_ids {
            if !is_scene_blocked_for_profile(&self.catalog, &profile, sid) {
                positive_set.insert(sid.clone());
            }
        }
        for sid in &positive_set {
            self.scene_positive_users
                .entry(sid.clone())
                .or_default()
                .insert(uid);
        }
        if !positive_set.is_empty() {
            self.user_positive_scenes.insert(uid, positive_set);
        }

        self.profiles.insert(uid, profile);
    }

    /// Returns a cloned asset DTO from the catalog.
    pub fn get_asset(&self, scene_id: &str) -> Option<CodeSceneAssetDto> {
        self.catalog.get(scene_id).map(|e| e.asset.clone())
    }

    /// Returns a cloned asset DTO hydrated with viewer like/bookmark/follow flags.
    pub fn get_asset_for_user(&self, scene_id: &str, user_id: i64) -> Option<CodeSceneAssetDto> {
        let mut asset = self.catalog.get(scene_id)?.asset.clone();
        if let Some(profile) = self.profiles.get(&user_id) {
            asset.viewer_liked = profile.liked_scene_ids.contains(&asset.scene_id);
            asset.viewer_bookmarked = profile.bookmarked_scene_ids.contains(&asset.scene_id);
            asset.viewer_following_author =
                profile.followed_creators.contains(&asset.author_user_id);
        }
        Some(asset)
    }

    /// Lists and filters catalog assets (for `GET /api/v1/code-scene/assets`).
    pub fn list_assets(&self, query: &ListCodeSceneAssetsQuery) -> ListCodeSceneAssetsResponse {
        let q_lower = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase);
        let tag_norm = query
            .tag
            .as_deref()
            .map(normalize_tag)
            .filter(|s| !s.is_empty());

        let mut matched: Vec<CodeSceneAssetDto> = self
            .catalog
            .values()
            .filter(|entry| {
                if let Some(k) = query.kind {
                    if entry.asset.kind != k {
                        return false;
                    }
                }
                if let Some(uid) = query.author_user_id {
                    if entry.asset.author_user_id != uid {
                        return false;
                    }
                }
                if let Some(bid) = query.creator_bot_id {
                    if entry.asset.creator_bot_id != Some(bid) {
                        return false;
                    }
                }
                if let Some(ref tf) = tag_norm {
                    if !entry.normalized_tags.iter().any(|t| t == tf) {
                        return false;
                    }
                }
                if let Some(ref q) = q_lower {
                    let hit = entry.asset.title.to_lowercase().contains(q)
                        || entry.asset.description.to_lowercase().contains(q)
                        || entry.asset.scene_id.to_lowercase().contains(q)
                        || entry.asset.author_username.to_lowercase().contains(q)
                        || entry.normalized_tags.iter().any(|t| t.contains(q));
                    if !hit {
                        return false;
                    }
                }
                true
            })
            .map(|e| e.asset.clone())
            .collect();

        matched.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| a.scene_id.cmp(&b.scene_id))
        });

        let total = matched.len();
        let offset = query.offset.unwrap_or(0);
        let limit = query.limit.unwrap_or(50).clamp(1, 200);
        let assets: Vec<CodeSceneAssetDto> =
            matched.into_iter().skip(offset).take(limit).collect();

        ListCodeSceneAssetsResponse {
            assets,
            total,
            limit,
            offset,
        }
    }

    pub fn catalog_len(&self) -> usize {
        self.catalog.len()
    }

    /// Synchronizes authoritative social counters from external persistence (e.g., SQLite `V42`).
    #[allow(clippy::too_many_arguments)]
    pub fn update_social_counters(
        &mut self,
        scene_id: &str,
        likes: Option<u64>,
        bookmarks: Option<u64>,
        comments: Option<u64>,
        shares: Option<u64>,
        remixes: Option<u64>,
        plays: Option<u64>,
    ) -> bool {
        let Some(entry) = self.catalog.get_mut(scene_id) else {
            return false;
        };
        if let Some(v) = likes {
            entry.asset.likes_count = v;
        }
        if let Some(v) = bookmarks {
            entry.asset.bookmarks_count = v;
        }
        if let Some(v) = comments {
            entry.asset.comments_count = v;
        }
        if let Some(v) = shares {
            entry.asset.shares_count = v;
        }
        if let Some(v) = remixes {
            entry.asset.remixes_count = v;
        }
        if let Some(v) = plays {
            entry.asset.plays_count = v;
        }
        true
    }

    /// Toggles like for `(user_id, scene_id)` and records the corresponding telemetry event.
    pub fn toggle_like(&mut self, user_id: i64, scene_id: &str) -> Option<ToggleLikeResponse> {
        if !self.catalog.contains_key(scene_id) {
            return None;
        }
        let currently_liked = self
            .profiles
            .get(&user_id)
            .is_some_and(|p| p.liked_scene_ids.contains(scene_id));
        let kind = if currently_liked {
            SceneBehaviorEventKind::Unlike
        } else {
            SceneBehaviorEventKind::Like
        };
        let ev = SceneBehaviorEventDto {
            user_id,
            scene_id: scene_id.to_string(),
            kind,
            timestamp_ms: self.now_ms(),
            ..Default::default()
        };
        self.record_behavior(&ev);
        let liked = self
            .profiles
            .get(&user_id)
            .is_some_and(|p| p.liked_scene_ids.contains(scene_id));
        let likes_count = self.catalog.get(scene_id).map(|e| e.asset.likes_count)?;
        Some(ToggleLikeResponse { liked, likes_count })
    }

    /// Toggles bookmark for `(user_id, scene_id)` and records the corresponding telemetry event.
    pub fn toggle_bookmark(
        &mut self,
        user_id: i64,
        scene_id: &str,
    ) -> Option<ToggleBookmarkResponse> {
        if !self.catalog.contains_key(scene_id) {
            return None;
        }
        let currently_bookmarked = self
            .profiles
            .get(&user_id)
            .is_some_and(|p| p.bookmarked_scene_ids.contains(scene_id));
        let kind = if currently_bookmarked {
            SceneBehaviorEventKind::Unbookmark
        } else {
            SceneBehaviorEventKind::Bookmark
        };
        let ev = SceneBehaviorEventDto {
            user_id,
            scene_id: scene_id.to_string(),
            kind,
            timestamp_ms: self.now_ms(),
            ..Default::default()
        };
        self.record_behavior(&ev);
        let bookmarked = self
            .profiles
            .get(&user_id)
            .is_some_and(|p| p.bookmarked_scene_ids.contains(scene_id));
        let bookmarks_count = self
            .catalog
            .get(scene_id)
            .map(|e| e.asset.bookmarks_count)?;
        Some(ToggleBookmarkResponse {
            bookmarked,
            bookmarks_count,
        })
    }

    /// Toggles follow for `(user_id, author_user_id)` and records the corresponding telemetry event.
    pub fn toggle_follow(&mut self, user_id: i64, author_user_id: i64) -> ToggleFollowResponse {
        let currently_following = self
            .profiles
            .get(&user_id)
            .is_some_and(|p| p.followed_creators.contains(&author_user_id));
        let kind = if currently_following {
            SceneBehaviorEventKind::UnfollowAuthor
        } else {
            SceneBehaviorEventKind::FollowAuthor
        };
        let ev = SceneBehaviorEventDto {
            user_id,
            author_user_id: Some(author_user_id),
            kind,
            timestamp_ms: self.now_ms(),
            ..Default::default()
        };
        self.record_behavior(&ev);
        let following = self
            .profiles
            .get(&user_id)
            .is_some_and(|p| p.followed_creators.contains(&author_user_id));
        let followers_count = self
            .profiles
            .values()
            .filter(|p| p.followed_creators.contains(&author_user_id))
            .count() as u64;
        ToggleFollowResponse {
            following,
            followers_count,
        }
    }

    /// Returns the indexed 64-dim dense embedding for `scene_id`.
    pub fn item_embedding(&self, scene_id: &str) -> Option<&[f32]> {
        self.catalog.get(scene_id).map(|e| e.embedding.as_slice())
    }

    /// Returns the extracted static `cs1` feature tokens for `scene_id`.
    pub fn item_feature_tokens(&self, scene_id: &str) -> Option<&[String]> {
        self.catalog
            .get(scene_id)
            .map(|e| e.code_tokens.as_slice())
    }

    /// Returns the current 4-tier traffic pool label for `scene_id`.
    pub fn item_traffic_tier(&self, scene_id: &str) -> Option<&'static str> {
        self.catalog.get(scene_id).map(|e| e.traffic_tier())
    }

    /// Returns `true` if `scene_id` is currently blocked for `user_id` by any hard blocklist
    /// (`disliked_scene_ids`, `hidden_author_ids`, or `hidden_tags`).
    pub fn is_scene_blocked_for_user(&self, user_id: i64, scene_id: &str) -> bool {
        let Some(profile) = self.profiles.get(&user_id) else {
            return false;
        };
        if let Some(entry) = self.catalog.get(scene_id) {
            profile.is_blocked(
                &entry.asset.scene_id,
                entry.asset.author_user_id,
                &entry.normalized_tags,
            )
        } else {
            profile.disliked_scene_ids.contains(scene_id)
        }
    }

    /// Returns `true` if `asset` is currently blocked for `user_id` by any hard blocklist,
    /// falling back to `asset` metadata if it is no longer present in `catalog`.
    pub fn is_asset_blocked_for_user(&self, user_id: i64, asset: &CodeSceneAssetDto) -> bool {
        let Some(profile) = self.profiles.get(&user_id) else {
            return false;
        };
        if let Some(entry) = self.catalog.get(&asset.scene_id) {
            profile.is_blocked(
                &entry.asset.scene_id,
                entry.asset.author_user_id,
                &entry.normalized_tags,
            )
        } else {
            let norm_tags = normalize_tags(&asset.tags);
            profile.is_blocked(&asset.scene_id, asset.author_user_id, &norm_tags)
        }
    }

    fn purge_blocked_positives_for_user(&mut self, user_id: i64) {
        let Some(profile) = self.profiles.get_mut(&user_id) else {
            return;
        };
        let mut to_remove: Vec<String> = profile
            .positive_scene_history
            .iter()
            .filter(|sid| is_scene_blocked_for_profile(&self.catalog, profile, sid))
            .cloned()
            .collect();
        if !to_remove.is_empty() {
            let blocked_set: HashSet<&str> = to_remove.iter().map(String::as_str).collect();
            profile
                .positive_scene_history
                .retain(|sid| !blocked_set.contains(sid.as_str()));
        }
        if let Some(scenes) = self.user_positive_scenes.get_mut(&user_id) {
            for sid in scenes.iter() {
                if is_scene_blocked_for_profile(&self.catalog, profile, sid) {
                    to_remove.push(sid.clone());
                }
            }
            for sid in &to_remove {
                scenes.remove(sid);
            }
            if scenes.is_empty() {
                self.user_positive_scenes.remove(&user_id);
            }
        }
        for sid in &to_remove {
            if let Some(users) = self.scene_positive_users.get_mut(sid) {
                users.remove(&user_id);
                if users.is_empty() {
                    self.scene_positive_users.remove(sid);
                }
            }
        }
    }

    fn total_catalog_plays(&self) -> u64 {
        self.catalog.values().map(|e| e.effective_plays()).sum()
    }
}

fn is_scene_blocked_for_profile(
    catalog: &BTreeMap<String, CatalogEntry>,
    profile: &UserInterestProfile,
    scene_id: &str,
) -> bool {
    if let Some(entry) = catalog.get(scene_id) {
        profile.is_blocked(
            &entry.asset.scene_id,
            entry.asset.author_user_id,
            &entry.normalized_tags,
        )
    } else {
        profile.disliked_scene_ids.contains(scene_id)
    }
}
