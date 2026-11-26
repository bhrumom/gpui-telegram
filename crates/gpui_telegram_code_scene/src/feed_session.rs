//! Pure-Rust Douyin-style `FeedSessionController` (`CS-F07`).
//!
//! Manages the active card index, `[i-1, i, i+1]` pre-warmed window, tab/tag/query
//! filters, swipe-away dwell/completion/fast-skip/game telemetry emission, live
//! unseen-tail `[i+1..]` re-ranking with strict viewed-prefix immutability, and
//! zero-duplication cursor prefetching.

use std::collections::HashSet;

use chrono::Utc;
use gpui_telegram_proto::{
    AssetKind, CodeSceneFeedRequest, CodeSceneFeedTab, DEFAULT_FEED_LIMIT, MAX_FEED_LIMIT,
    RankedSceneItemDto, SceneBehaviorEventDto, SceneBehaviorEventKind,
};

use crate::model::GameSessionTelemetry;
use crate::reco::embedding::{first_non_generic_tag, normalize_tag, normalize_tags};
use crate::reco::engine::CodeSceneRecommender;
use crate::reco::telemetry::{FAST_SKIP_SOFT_MS, RewardOutcome};
use crate::runtime::FrameOutput;

pub const DEFAULT_PREFETCH_THRESHOLD: usize = 3;
pub const FAST_SKIP_THRESHOLD_MS: u64 = 1_500;
pub const COMPLETE_RATIO_THRESHOLD: f64 = 0.85;
pub const DEFAULT_ANIM_REF_DURATION_MS: u64 = 10_000;
pub const DEFAULT_GAME_REF_DURATION_MS: u64 = 15_000;

#[derive(Debug, Clone, Default)]
struct ActiveCardTelemetry {
    scene_id: String,
    asset_kind: AssetKind,
    author_user_id: i64,
    tags: Vec<String>,
    duration_ms: u64,
    active_dwell_ms: u64,
    active_replays: u32,
    active_game_telemetry: GameSessionTelemetry,
    had_explicit_positive: bool,
    had_hard_negative: bool,
    flushed: bool,
}

impl ActiveCardTelemetry {
    fn from_item(item: &RankedSceneItemDto) -> Self {
        let duration_ms = match item.asset.manifest.duration_sec {
            Some(sec) if sec.is_finite() && sec > 0.0 => (sec * 1_000.0).round() as u64,
            _ => match item.asset.kind {
                AssetKind::Video => 15_000,
                AssetKind::Animation => DEFAULT_ANIM_REF_DURATION_MS,
                AssetKind::Game => DEFAULT_GAME_REF_DURATION_MS,
            },
        };
        Self {
            scene_id: item.asset.scene_id.clone(),
            asset_kind: item.asset.kind,
            author_user_id: item.asset.author_user_id,
            tags: normalize_tags(&item.asset.tags),
            duration_ms,
            active_dwell_ms: 0,
            active_replays: 0,
            active_game_telemetry: GameSessionTelemetry::default(),
            had_explicit_positive: false,
            had_hard_negative: false,
            flushed: false,
        }
    }

    fn completion_ratio(&self) -> f64 {
        let denom = self.duration_ms.max(5_000) as f64;
        ((self.active_dwell_ms as f64) / denom).clamp(0.0, 3.0)
    }
}

/// Pure-Rust stateful controller driving the Douyin-like vertical feed experience.
#[derive(Debug, Clone)]
pub struct FeedSessionController {
    user_id: i64,
    session_id: String,
    active_tab: CodeSceneFeedTab,
    tag_filter: Option<String>,
    query: Option<String>,
    page_size: usize,
    prefetch_threshold: usize,
    items: Vec<RankedSceneItemDto>,
    current_index: usize,
    next_cursor: Option<String>,
    total_catalog: usize,
    active_telemetry: ActiveCardTelemetry,
    emitted_telemetry: Vec<SceneBehaviorEventDto>,
    event_seq: u64,
    clock_ms: i64,
}

impl FeedSessionController {
    pub fn new(user_id: i64, session_id: impl Into<String>) -> Self {
        Self {
            user_id,
            session_id: session_id.into(),
            active_tab: CodeSceneFeedTab::ForYou,
            tag_filter: None,
            query: None,
            page_size: DEFAULT_FEED_LIMIT,
            prefetch_threshold: DEFAULT_PREFETCH_THRESHOLD,
            items: Vec::new(),
            current_index: 0,
            next_cursor: None,
            total_catalog: 0,
            active_telemetry: ActiveCardTelemetry::default(),
            emitted_telemetry: Vec::new(),
            event_seq: 0,
            clock_ms: Utc::now().timestamp_millis(),
        }
    }

    pub fn with_page_size(mut self, limit: usize) -> Self {
        self.page_size = if limit == 0 {
            DEFAULT_FEED_LIMIT
        } else {
            limit.clamp(1, MAX_FEED_LIMIT)
        };
        self
    }

    pub fn with_clock_ms(mut self, now_ms: i64) -> Self {
        self.clock_ms = now_ms;
        self
    }

    pub fn set_clock_ms(&mut self, now_ms: i64) {
        self.clock_ms = now_ms;
    }

    pub fn clock_ms(&self) -> i64 {
        self.clock_ms
    }

    pub fn current_item(&self) -> Option<&RankedSceneItemDto> {
        self.items.get(self.current_index)
    }

    pub fn current_index(&self) -> usize {
        self.current_index
    }

    /// Returns sorted valid in-bounds indices covering `[i-1, i, i+1]` (empty if feed is empty).
    pub fn preload_window_indices(&self) -> Vec<usize> {
        if self.items.is_empty() {
            return Vec::new();
        }
        let idx = self.current_index.min(self.items.len() - 1);
        let start = idx.saturating_sub(1);
        let end = (idx + 1).min(self.items.len() - 1);
        (start..=end).collect()
    }

    /// Pairs each index in `preload_window_indices()` with its ranked scene item.
    pub fn preload_window_items(&self) -> Vec<(usize, &RankedSceneItemDto)> {
        self.preload_window_indices()
            .into_iter()
            .map(|i| (i, &self.items[i]))
            .collect()
    }

    pub fn active_tab(&self) -> CodeSceneFeedTab {
        self.active_tab
    }

    pub fn tag_filter(&self) -> Option<&str> {
        self.tag_filter.as_deref()
    }

    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    pub fn next_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }

    pub fn total_catalog(&self) -> usize {
        self.total_catalog
    }

    pub fn items(&self) -> &[RankedSceneItemDto] {
        &self.items
    }

    /// Drains all telemetry events emitted since the last call (for M3 client outbox / sync).
    pub fn drain_emitted_telemetry(&mut self) -> Vec<SceneBehaviorEventDto> {
        std::mem::take(&mut self.emitted_telemetry)
    }

    /// Loads page 1 for the current tab/tag/query filter, resetting `current_index = 0`
    /// and emitting an initial `Impression` if non-empty.
    pub fn load_initial_feed(&mut self, reco: &mut CodeSceneRecommender) -> usize {
        let req = self.build_request(None, self.page_size);
        let resp = reco.recommend_feed_at(&req, self.clock_ms);
        self.items = resp.items;
        self.current_index = 0;
        self.next_cursor = resp.next_cursor;
        self.total_catalog = resp.total_catalog;
        self.begin_active_card(reco);
        self.items.len()
    }

    /// Switches active feed tab, flushing active-card swipe-away telemetry first.
    pub fn switch_tab(&mut self, reco: &mut CodeSceneRecommender, tab: CodeSceneFeedTab) -> usize {
        self.flush_active_telemetry(reco);
        self.active_tab = tab;
        self.load_initial_feed(reco)
    }

    /// Updates tag filter (trimmed, stripped `#`, lowercased; empty -> `None`) and reloads feed.
    pub fn set_tag_filter(
        &mut self,
        reco: &mut CodeSceneRecommender,
        tag: Option<String>,
    ) -> usize {
        self.flush_active_telemetry(reco);
        self.tag_filter = tag
            .as_deref()
            .map(normalize_tag)
            .filter(|s| !s.is_empty());
        self.load_initial_feed(reco)
    }

    /// Updates search query (trimmed; empty -> `None`) and reloads feed.
    pub fn set_query(
        &mut self,
        reco: &mut CodeSceneRecommender,
        query: Option<String>,
    ) -> usize {
        self.flush_active_telemetry(reco);
        self.query = query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        self.load_initial_feed(reco)
    }

    /// Advances to the next vertical card (`current_index + 1`).
    ///
    /// Exact order: flush active card telemetry -> advance `current_index` (locking
    /// `items[0..=current_index]`) -> re-rank ONLY unseen tail `items[current_index+1..]`
    /// if the flushed signal was `FastSkip`/`Dwell`/`Complete`/`GameInteraction` -> prefetch if
    /// remaining tail `<= prefetch_threshold` -> emit `Impression` for new active card.
    pub fn swipe_next(&mut self, reco: &mut CodeSceneRecommender) -> Option<&RankedSceneItemDto> {
        if self.items.is_empty() {
            return None;
        }
        if self.current_index + 1 >= self.items.len() {
            self.prefetch_if_needed(reco);
            if self.current_index + 1 >= self.items.len() {
                return self.current_item();
            }
        }

        let flushed = self.flush_active_telemetry(reco);
        self.current_index += 1;

        if let Some(ref ev) = flushed {
            if matches!(
                ev.kind,
                SceneBehaviorEventKind::FastSkip
                    | SceneBehaviorEventKind::Dwell
                    | SceneBehaviorEventKind::Complete
                    | SceneBehaviorEventKind::GameInteraction
            ) {
                self.rerank_unseen_tail(reco);
            }
        }

        self.prefetch_if_needed(reco);
        self.begin_active_card(reco);
        self.current_item()
    }

    /// Swipes back to the previous card (`current_index - 1`) without ever reordering `items`.
    pub fn swipe_prev(&mut self, reco: &mut CodeSceneRecommender) -> Option<&RankedSceneItemDto> {
        if self.items.is_empty() || self.current_index == 0 {
            return self.current_item();
        }
        self.flush_active_telemetry(reco);
        self.current_index -= 1;
        self.begin_active_card(reco);
        self.current_item()
    }

    /// Deep-links directly to `scene_id` (e.g., from `code_scene:open:<id>` chat card action).
    pub fn open_scene_by_id(
        &mut self,
        reco: &mut CodeSceneRecommender,
        scene_id: &str,
    ) -> Option<&RankedSceneItemDto> {
        if let Some(cur) = self.current_item() {
            if cur.asset.scene_id == scene_id {
                return self.current_item();
            }
        }
        if let Some(pos) = self.items.iter().position(|i| i.asset.scene_id == scene_id) {
            self.flush_active_telemetry(reco);
            if pos > self.current_index {
                let item = self.items.remove(pos);
                let insert_at = (self.current_index + 1).min(self.items.len());
                self.items.insert(insert_at, item);
                self.current_index = insert_at;
                if self.items.len() <= self.current_index + 1 {
                    self.populate_unseen_tail(reco, self.page_size.saturating_sub(1).max(1));
                } else {
                    self.rerank_unseen_tail(reco);
                }
            } else {
                self.current_index = pos;
            }
            self.begin_active_card(reco);
            return self.current_item();
        }

        let item = reco.explain_asset_for_user(self.user_id, scene_id, self.clock_ms)?;
        if !self.items.is_empty() {
            self.flush_active_telemetry(reco);
            let insert_at = (self.current_index + 1).min(self.items.len());
            self.items.insert(insert_at, item);
            self.current_index = insert_at;
        } else {
            self.items.push(item);
            self.current_index = 0;
        }
        self.total_catalog = reco.catalog_len();
        if self.items.len() <= self.current_index + 1 {
            self.populate_unseen_tail(reco, self.page_size.saturating_sub(1).max(1));
        } else {
            self.rerank_unseen_tail(reco);
        }
        self.begin_active_card(reco);
        self.current_item()
    }

    /// Accumulates active-card dwell time and loop count (called on viewport frame ticks).
    pub fn on_card_dwell_tick(&mut self, delta_ms: u64, loop_count: u32) {
        if self.items.is_empty() || self.active_telemetry.flushed {
            return;
        }
        self.active_telemetry.active_dwell_ms = self
            .active_telemetry
            .active_dwell_ms
            .saturating_add(delta_ms);
        self.clock_ms = self
            .clock_ms
            .saturating_add(i64::try_from(delta_ms).unwrap_or(i64::MAX));
        self.active_telemetry.active_replays =
            self.active_telemetry.active_replays.max(loop_count);
    }

    /// Monotonically merges `GameSessionTelemetry` from the active `VmInstance`.
    pub fn record_game_telemetry(&mut self, telemetry: GameSessionTelemetry) {
        if self.items.is_empty() || self.active_telemetry.flushed {
            return;
        }
        let g = &mut self.active_telemetry.active_game_telemetry;
        g.input_count = g.input_count.max(telemetry.input_count);
        g.play_ms = g.play_ms.max(telemetry.play_ms);
        g.restarts = g.restarts.max(telemetry.restarts);
        g.best_score = g.best_score.max(telemetry.best_score);
        g.current_score = telemetry.current_score;
        g.game_over_count = g.game_over_count.max(telemetry.game_over_count);
        self.active_telemetry.active_dwell_ms =
            self.active_telemetry.active_dwell_ms.max(telemetry.play_ms);
    }

    /// Ergonomic M4 60fps hook combining dwell tick + optional game session telemetry.
    pub fn on_frame_output(
        &mut self,
        delta_ms: u64,
        frame: &FrameOutput,
        game_telemetry: Option<GameSessionTelemetry>,
    ) {
        self.on_card_dwell_tick(delta_ms, frame.loop_count);
        if let Some(gt) = game_telemetry {
            self.record_game_telemetry(gt);
        }
    }

    /// Records an explicit user interaction on the currently active card (`items[current_index]`),
    /// updates the recommender in real time, and live re-ranks ONLY the unseen tail.
    pub fn record_interaction(
        &mut self,
        reco: &mut CodeSceneRecommender,
        kind: SceneBehaviorEventKind,
        reason: Option<String>,
    ) -> Option<RewardOutcome> {
        let cur = self.items.get(self.current_index)?.clone();
        let norm_tags = normalize_tags(&cur.asset.tags);
        let resolved_reason = if kind == SceneBehaviorEventKind::HideTag {
            reason
                .as_deref()
                .map(normalize_tag)
                .filter(|s| !s.is_empty())
                .or_else(|| first_non_generic_tag(&norm_tags))
        } else {
            reason
        };

        let event = SceneBehaviorEventDto {
            event_id: self.next_event_id(),
            user_id: self.user_id,
            session_id: self.session_id.clone(),
            scene_id: cur.asset.scene_id.clone(),
            kind,
            asset_kind: Some(cur.asset.kind),
            author_user_id: Some(cur.asset.author_user_id),
            tags: cur.asset.tags.clone(),
            dwell_ms: self.active_telemetry.active_dwell_ms,
            duration_ms: self.active_telemetry.duration_ms,
            completion_ratio: self.active_telemetry.completion_ratio(),
            loop_count: self.active_telemetry.active_replays,
            game_inputs: self.active_telemetry.active_game_telemetry.input_count,
            game_play_ms: self.active_telemetry.active_game_telemetry.play_ms,
            game_restarts: self.active_telemetry.active_game_telemetry.restarts,
            game_score: if self.active_telemetry.active_game_telemetry.best_score > 0 {
                Some(self.active_telemetry.active_game_telemetry.best_score as f64)
            } else {
                None
            },
            reason: resolved_reason,
            timestamp_ms: self.clock_ms,
        };

        let outcome = reco.record_behavior(&event);
        self.emitted_telemetry.push(event);

        if kind.is_hard_negative() {
            // Preserve non-blocked viewed prefix items[0..current_index], purge any newly
            // blocked prior cards (HideAuthor/HideTag/NotInterested) so neither
            // preload_window_items() nor swipe_prev() can ever land on blocked content,
            // dismiss active card, and rebuild the unseen tail.
            self.active_telemetry.had_hard_negative = true;
            self.active_telemetry.flushed = true;

            let prefix_len = self.current_index;
            let tail_target = self.items.len().saturating_sub(prefix_len).max(self.page_size);
            self.items.truncate(prefix_len);
            self.items
                .retain(|it| !reco.is_asset_blocked_for_user(self.user_id, &it.asset));
            self.current_index = self.items.len();

            let exclude: HashSet<String> = self
                .items
                .iter()
                .map(|i| i.asset.scene_id.clone())
                .collect();
            let prefix_slice_start = self.items.len().saturating_sub(3);
            let req = self.build_request(None, tail_target);
            let resp = reco.recommend_unseen_tail(
                &req,
                &exclude,
                &self.items[prefix_slice_start..],
                self.clock_ms,
            );
            self.items.extend(resp.items);
            self.next_cursor = resp.next_cursor;
            self.total_catalog = resp.total_catalog;

            if self.items.is_empty() {
                self.current_index = 0;
                self.active_telemetry = ActiveCardTelemetry::default();
            } else {
                if self.current_index >= self.items.len() {
                    self.current_index = self.items.len() - 1;
                }
                self.begin_active_card(reco);
            }
        } else {
            // Strict prefix immutability: items[0..=current_index] order & current_index never change
            if kind.is_explicit_positive() {
                self.active_telemetry.had_explicit_positive = true;
            }
            if let Some(updated) = reco.get_asset_for_user(&cur.asset.scene_id, self.user_id) {
                if let Some(slot) = self.items.get_mut(self.current_index) {
                    slot.asset.viewer_liked = updated.viewer_liked;
                    slot.asset.viewer_bookmarked = updated.viewer_bookmarked;
                    slot.asset.viewer_following_author = updated.viewer_following_author;
                    slot.asset.likes_count = updated.likes_count;
                    slot.asset.bookmarks_count = updated.bookmarks_count;
                    slot.asset.comments_count = updated.comments_count;
                    slot.asset.shares_count = updated.shares_count;
                    slot.asset.remixes_count = updated.remixes_count;
                }
            }
            self.rerank_unseen_tail(reco);
        }

        Some(outcome)
    }

    /// Prefetches additional unseen non-blocked items when remaining tail `<= prefetch_threshold`,
    /// deduplicating strictly against already-loaded scene IDs and preserving seam scatter rules.
    pub fn prefetch_if_needed(&mut self, reco: &CodeSceneRecommender) -> usize {
        if self.next_cursor.is_none() {
            return 0;
        }
        let remaining_tail = self
            .items
            .len()
            .saturating_sub(self.current_index.saturating_add(1));
        if remaining_tail > self.prefetch_threshold {
            return 0;
        }

        let exclude: HashSet<String> = self
            .items
            .iter()
            .map(|i| i.asset.scene_id.clone())
            .collect();
        let prefix_start = self.items.len().saturating_sub(3);
        let req = self.build_request(None, self.page_size);
        let resp = reco.recommend_unseen_tail(
            &req,
            &exclude,
            &self.items[prefix_start..],
            self.clock_ms,
        );

        let mut added = 0usize;
        let mut seen_ids = exclude;
        for item in resp.items {
            if seen_ids.insert(item.asset.scene_id.clone()) {
                self.items.push(item);
                added += 1;
            }
        }
        self.next_cursor = if added > 0 { resp.next_cursor } else { None };
        self.total_catalog = resp.total_catalog;
        added
    }

    /// Flushes swipe-away telemetry (`GameInteraction` / `FastSkip` / `Complete` / `Dwell`)
    /// for the active card cleanly and idempotently.
    pub fn flush_active_telemetry(
        &mut self,
        reco: &mut CodeSceneRecommender,
    ) -> Option<SceneBehaviorEventDto> {
        if self.items.is_empty()
            || self.active_telemetry.flushed
            || self.active_telemetry.scene_id.is_empty()
        {
            return None;
        }
        self.active_telemetry.flushed = true;
        if self.active_telemetry.had_hard_negative {
            return None;
        }

        let gt = self.active_telemetry.active_game_telemetry.clone();
        let comp = self.active_telemetry.completion_ratio();
        let kind = if self.active_telemetry.asset_kind == AssetKind::Game
            && (gt.input_count > 0 || gt.play_ms >= FAST_SKIP_THRESHOLD_MS || gt.restarts > 0)
        {
            SceneBehaviorEventKind::GameInteraction
        } else if self.active_telemetry.active_dwell_ms < FAST_SKIP_THRESHOLD_MS
            && !self.active_telemetry.had_explicit_positive
        {
            SceneBehaviorEventKind::FastSkip
        } else if comp >= COMPLETE_RATIO_THRESHOLD || self.active_telemetry.active_replays > 0 {
            SceneBehaviorEventKind::Complete
        } else if self.active_telemetry.had_explicit_positive
            && self.active_telemetry.active_dwell_ms < FAST_SKIP_SOFT_MS
        {
            // Explicit positive already recorded for this card; suppress contradictory
            // sub-3s Dwell event on immediate swipe-away.
            return None;
        } else {
            SceneBehaviorEventKind::Dwell
        };

        let event_id = self.next_event_id();
        let event = SceneBehaviorEventDto {
            event_id,
            user_id: self.user_id,
            session_id: self.session_id.clone(),
            scene_id: self.active_telemetry.scene_id.clone(),
            kind,
            asset_kind: Some(self.active_telemetry.asset_kind),
            author_user_id: Some(self.active_telemetry.author_user_id),
            tags: self.active_telemetry.tags.clone(),
            dwell_ms: self.active_telemetry.active_dwell_ms,
            duration_ms: self.active_telemetry.duration_ms,
            completion_ratio: comp,
            loop_count: self.active_telemetry.active_replays,
            game_inputs: gt.input_count,
            game_play_ms: gt.play_ms,
            game_restarts: gt.restarts,
            game_score: if gt.best_score > 0 {
                Some(gt.best_score as f64)
            } else {
                None
            },
            reason: None,
            timestamp_ms: self.clock_ms,
        };

        reco.record_behavior(&event);
        self.emitted_telemetry.push(event.clone());
        Some(event)
    }

    fn rerank_unseen_tail(&mut self, reco: &CodeSceneRecommender) {
        if self.items.is_empty() {
            return;
        }
        let locked_len = (self.current_index + 1).min(self.items.len());
        let existing_tail_len = self.items.len().saturating_sub(locked_len);
        if existing_tail_len == 0 {
            return;
        }
        self.populate_unseen_tail(reco, existing_tail_len);
    }

    fn populate_unseen_tail(&mut self, reco: &CodeSceneRecommender, tail_limit: usize) {
        if self.items.is_empty() || tail_limit == 0 {
            return;
        }
        let locked_len = (self.current_index + 1).min(self.items.len());
        let exclude: HashSet<String> = self.items[..locked_len]
            .iter()
            .map(|i| i.asset.scene_id.clone())
            .collect();
        let prefix_start = locked_len.saturating_sub(3);
        let req = self.build_request(None, tail_limit);
        let resp = reco.recommend_unseen_tail(
            &req,
            &exclude,
            &self.items[prefix_start..locked_len],
            self.clock_ms,
        );

        self.items.truncate(locked_len);
        let mut seen = exclude;
        for item in resp.items {
            if seen.insert(item.asset.scene_id.clone()) {
                self.items.push(item);
            }
        }
        self.next_cursor = resp.next_cursor;
        self.total_catalog = resp.total_catalog;
    }

    fn begin_active_card(&mut self, reco: &mut CodeSceneRecommender) {
        let Some(cur) = self.items.get(self.current_index).cloned() else {
            self.active_telemetry = ActiveCardTelemetry::default();
            return;
        };
        self.active_telemetry = ActiveCardTelemetry::from_item(&cur);
        let ev = SceneBehaviorEventDto {
            event_id: self.next_event_id(),
            user_id: self.user_id,
            session_id: self.session_id.clone(),
            scene_id: cur.asset.scene_id.clone(),
            kind: SceneBehaviorEventKind::Impression,
            asset_kind: Some(cur.asset.kind),
            author_user_id: Some(cur.asset.author_user_id),
            tags: cur.asset.tags.clone(),
            duration_ms: self.active_telemetry.duration_ms,
            timestamp_ms: self.clock_ms,
            ..Default::default()
        };
        reco.record_behavior(&ev);
        self.emitted_telemetry.push(ev);
    }

    fn build_request(&self, cursor: Option<String>, limit: usize) -> CodeSceneFeedRequest {
        CodeSceneFeedRequest {
            user_id: self.user_id,
            session_id: self.session_id.clone(),
            tab: self.active_tab,
            tag_filter: self.tag_filter.clone(),
            query: self.query.clone(),
            cursor,
            limit,
        }
    }

    fn next_event_id(&mut self) -> String {
        self.event_seq = self.event_seq.saturating_add(1);
        format!("{}:ev:{}", self.session_id, self.event_seq)
    }
}
