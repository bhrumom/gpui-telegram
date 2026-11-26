//! Serializable dual-timescale `UserInterestProfile` and `RecommenderConfig`.
//!
//! Uses deterministic `BTreeMap`/`BTreeSet` collections so JSON round-trips for
//! SQLite `V42` and offline `SqliteClientCache` persistence are bit-reproducible.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use gpui_telegram_proto::AssetKind;
use serde::{Deserialize, Serialize};

use crate::reco::embedding::{EMBEDDING_DIM, l2_normalize, normalize_tag};

pub const USER_PROFILE_SCHEMA_VERSION: u32 = 1;
pub const SEEN_RING_CAPACITY: usize = 256;
pub const POSITIVE_HISTORY_CAPACITY: usize = 64;

/// Single entry in the user's bounded seen ring buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeenEntry {
    pub scene_id: String,
    pub seen_at_ms: i64,
}

impl Default for SeenEntry {
    fn default() -> Self {
        Self {
            scene_id: String::new(),
            seen_at_ms: 0,
        }
    }
}

/// Hyperparameters for the 5-stage Douyin-grade recommendation engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecommenderConfig {
    /// Short-term interest learning rate (`u_short`).
    pub eta_short: f64,
    /// Long-term interest learning rate (`u_long`).
    pub eta_long: f64,
    /// Negative interest push learning rate (`u_neg`).
    pub eta_neg: f64,
    /// EMA alpha for `tag_weights` / `kind_weights` / `creator_weights` rapid online adaptation.
    pub ema_alpha: f64,
    /// Fatigue bump applied on each fast-skip.
    pub fatigue_fast_skip_bump: f64,
    /// Hard cap on per-tag/creator/kind fatigue.
    pub fatigue_cap: f64,
    /// Multiplicative fatigue decay applied on unrelated positive events.
    pub fatigue_decay: f64,
    /// Bayesian empirical smoothing prior strength (`alpha`).
    pub bayes_alpha: f64,
    /// Greedy MMR relevance-vs-diversity trade-off (`lambda`).
    pub mmr_lambda: f64,
    /// Hard cap on UCB1 exploration bonus.
    pub ucb_cap: f64,
    /// Cooldown window (ms) before a seen item is promoted again in normal refresh.
    pub seen_cooldown_ms: i64,
    /// TTL (ms) for `CreatorJustPublished` immediate fast-track.
    pub just_published_ttl_ms: i64,
}

impl Default for RecommenderConfig {
    fn default() -> Self {
        Self {
            eta_short: 0.35,
            eta_long: 0.08,
            eta_neg: 0.30,
            ema_alpha: 0.28,
            fatigue_fast_skip_bump: 0.35,
            fatigue_cap: 1.80,
            fatigue_decay: 0.85,
            bayes_alpha: 8.0,
            mmr_lambda: 0.72,
            ucb_cap: 0.85,
            seen_cooldown_ms: 600_000,
            just_published_ttl_ms: 600_000,
        }
    }
}

/// Complete persistent user interest & blocklist profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserInterestProfile {
    pub schema_version: u32,
    pub user_id: i64,
    /// 64-dim L2-normalized short-term interest vector.
    pub u_short: Vec<f32>,
    /// 64-dim L2-normalized long-term interest vector.
    pub u_long: Vec<f32>,
    /// 64-dim L2-normalized negative/repulsion interest vector.
    pub u_neg: Vec<f32>,
    /// EMA affinity weights per normalized tag (`[-3.0, +4.0]`).
    pub tag_weights: BTreeMap<String, f64>,
    /// EMA affinity weights per `AssetKind` (`[-2.5, +3.5]`).
    pub kind_weights: BTreeMap<AssetKind, f64>,
    /// EMA affinity weights per `author_user_id` (`[-3.0, +4.0]`).
    pub creator_weights: BTreeMap<i64, f64>,
    /// Short-term fatigue accumulator per normalized tag (`[0.0, fatigue_cap]`).
    pub tag_fatigue: BTreeMap<String, f64>,
    /// Short-term fatigue accumulator per `author_user_id` (`[0.0, fatigue_cap]`).
    pub creator_fatigue: BTreeMap<i64, f64>,
    /// Short-term fatigue accumulator per `AssetKind` (`[0.0, fatigue_cap]`).
    pub kind_fatigue: BTreeMap<AssetKind, f64>,
    /// Persistent hard blocklist of disliked scene IDs (0% leak).
    pub disliked_scene_ids: BTreeSet<String>,
    /// Persistent hard blocklist of hidden author IDs (0% leak).
    pub hidden_author_ids: BTreeSet<i64>,
    /// Persistent hard blocklist of normalized hidden tags (0% leak).
    pub hidden_tags: BTreeSet<String>,
    /// Bounded seen ring buffer (`<= 256`).
    pub seen_ring: VecDeque<SeenEntry>,
    /// Followed author user IDs (never contains entries in `hidden_author_ids`).
    pub followed_creators: BTreeSet<i64>,
    /// Scenes currently liked by this user.
    pub liked_scene_ids: BTreeSet<String>,
    /// Scenes currently bookmarked by this user.
    pub bookmarked_scene_ids: BTreeSet<String>,
    /// Most recent positively engaged scenes (back = newest, cap `64`), driving Swing CF.
    pub positive_scene_history: VecDeque<String>,
    pub total_events: u64,
    pub total_impressions: u64,
    pub total_positives: u64,
    pub total_negatives: u64,
    pub revision: u64,
    pub updated_at_ms: i64,
}

impl Default for UserInterestProfile {
    fn default() -> Self {
        Self::new(0)
    }
}

impl UserInterestProfile {
    pub fn new(user_id: i64) -> Self {
        Self {
            schema_version: USER_PROFILE_SCHEMA_VERSION,
            user_id,
            u_short: vec![0.0; EMBEDDING_DIM],
            u_long: vec![0.0; EMBEDDING_DIM],
            u_neg: vec![0.0; EMBEDDING_DIM],
            tag_weights: BTreeMap::new(),
            kind_weights: BTreeMap::new(),
            creator_weights: BTreeMap::new(),
            tag_fatigue: BTreeMap::new(),
            creator_fatigue: BTreeMap::new(),
            kind_fatigue: BTreeMap::new(),
            disliked_scene_ids: BTreeSet::new(),
            hidden_author_ids: BTreeSet::new(),
            hidden_tags: BTreeSet::new(),
            seen_ring: VecDeque::new(),
            followed_creators: BTreeSet::new(),
            liked_scene_ids: BTreeSet::new(),
            bookmarked_scene_ids: BTreeSet::new(),
            positive_scene_history: VecDeque::new(),
            total_events: 0,
            total_impressions: 0,
            total_positives: 0,
            total_negatives: 0,
            revision: 0,
            updated_at_ms: 0,
        }
    }

    /// Sanitizes all vectors, weights, fatigues, blocklists, and bounded rings.
    /// Guarantees exact 64-dim finite L2-normalized (or zero) vectors and strict invariants.
    pub fn sanitize(&mut self) {
        self.schema_version = USER_PROFILE_SCHEMA_VERSION;
        sanitize_vec(&mut self.u_short);
        sanitize_vec(&mut self.u_long);
        sanitize_vec(&mut self.u_neg);

        let cleaned_tags: BTreeMap<String, f64> = self
            .tag_weights
            .iter()
            .filter_map(|(k, &v)| {
                let norm = normalize_tag(k);
                if norm.is_empty() || !v.is_finite() {
                    None
                } else {
                    Some((norm, v.clamp(-3.0, 4.0)))
                }
            })
            .collect();
        self.tag_weights = cleaned_tags;

        self.kind_weights.retain(|_, v| {
            if v.is_finite() {
                *v = v.clamp(-2.5, 3.5);
                true
            } else {
                false
            }
        });

        self.creator_weights.retain(|_, v| {
            if v.is_finite() {
                *v = v.clamp(-3.0, 4.0);
                true
            } else {
                false
            }
        });

        let cleaned_tag_fatigue: BTreeMap<String, f64> = self
            .tag_fatigue
            .iter()
            .filter_map(|(k, &v)| {
                let norm = normalize_tag(k);
                if norm.is_empty() || !v.is_finite() || v <= 0.005 {
                    None
                } else {
                    Some((norm, v.clamp(0.0, 3.0)))
                }
            })
            .collect();
        self.tag_fatigue = cleaned_tag_fatigue;

        self.creator_fatigue.retain(|_, v| {
            if v.is_finite() && *v > 0.005 {
                *v = v.clamp(0.0, 3.0);
                true
            } else {
                false
            }
        });

        self.kind_fatigue.retain(|_, v| {
            if v.is_finite() && *v > 0.005 {
                *v = v.clamp(0.0, 2.0);
                true
            } else {
                false
            }
        });

        self.hidden_tags = self
            .hidden_tags
            .iter()
            .map(|t| normalize_tag(t))
            .filter(|t| !t.is_empty())
            .collect();

        for hidden_author in &self.hidden_author_ids {
            self.followed_creators.remove(hidden_author);
        }

        self.disliked_scene_ids.retain(|s| !s.trim().is_empty());
        self.liked_scene_ids.retain(|s| !s.trim().is_empty());
        self.bookmarked_scene_ids.retain(|s| !s.trim().is_empty());

        self.positive_scene_history.retain(|s| {
            !s.trim().is_empty() && !self.disliked_scene_ids.contains(s)
        });
        while self.positive_scene_history.len() > POSITIVE_HISTORY_CAPACITY {
            self.positive_scene_history.pop_front();
        }

        self.seen_ring.retain(|e| !e.scene_id.trim().is_empty());
        while self.seen_ring.len() > SEEN_RING_CAPACITY {
            self.seen_ring.pop_front();
        }
    }

    /// Hard persistent blocklist predicate. MUST NEVER be relaxed on any tab, page, or exhaustion.
    pub fn is_blocked(
        &self,
        scene_id: &str,
        author_user_id: i64,
        normalized_tags: &[String],
    ) -> bool {
        if self.disliked_scene_ids.contains(scene_id) {
            return true;
        }
        if self.hidden_author_ids.contains(&author_user_id) {
            return true;
        }
        if !self.hidden_tags.is_empty()
            && normalized_tags.iter().any(|t| self.hidden_tags.contains(t))
        {
            return true;
        }
        false
    }

    /// Records a scene visit into `seen_ring`, deduplicating older occurrences.
    pub fn record_seen(&mut self, scene_id: &str, now_ms: i64) {
        if scene_id.trim().is_empty() {
            return;
        }
        if let Some(pos) = self.seen_ring.iter().position(|e| e.scene_id == scene_id) {
            self.seen_ring.remove(pos);
        }
        self.seen_ring.push_back(SeenEntry {
            scene_id: scene_id.to_string(),
            seen_at_ms: now_ms,
        });
        while self.seen_ring.len() > SEEN_RING_CAPACITY {
            self.seen_ring.pop_front();
        }
    }

    /// Returns the most recent timestamp at which `scene_id` was seen, if present in `seen_ring`.
    pub fn last_seen_ms(&self, scene_id: &str) -> Option<i64> {
        self.seen_ring
            .iter()
            .rev()
            .find(|e| e.scene_id == scene_id)
            .map(|e| e.seen_at_ms)
    }

    /// Decays all fatigue accumulators multiplicatively by `factor`, pruning negligible entries.
    pub fn decay_fatigue(&mut self, factor: f64) {
        let f = if factor.is_finite() {
            factor.clamp(0.0, 1.0)
        } else {
            0.85
        };
        self.tag_fatigue.retain(|_, v| {
            *v *= f;
            v.is_finite() && *v > 0.01
        });
        self.creator_fatigue.retain(|_, v| {
            *v *= f;
            v.is_finite() && *v > 0.01
        });
        self.kind_fatigue.retain(|_, v| {
            *v *= f;
            v.is_finite() && *v > 0.01
        });
    }

    /// Pushes `scene_id` as the newest entry in `positive_scene_history` (deduplicated).
    pub fn push_positive_history(&mut self, scene_id: &str) {
        if scene_id.trim().is_empty() || self.disliked_scene_ids.contains(scene_id) {
            return;
        }
        if let Some(pos) = self.positive_scene_history.iter().position(|s| s == scene_id) {
            self.positive_scene_history.remove(pos);
        }
        self.positive_scene_history.push_back(scene_id.to_string());
        while self.positive_scene_history.len() > POSITIVE_HISTORY_CAPACITY {
            self.positive_scene_history.pop_front();
        }
    }

    /// Removes `scene_id` from `positive_scene_history`.
    pub fn remove_positive_history(&mut self, scene_id: &str) {
        self.positive_scene_history.retain(|s| s != scene_id);
    }
}

fn sanitize_vec(v: &mut Vec<f32>) {
    if v.len() != EMBEDDING_DIM {
        v.resize(EMBEDDING_DIM, 0.0);
    }
    l2_normalize(v);
}
