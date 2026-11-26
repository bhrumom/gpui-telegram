use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::Utc;
use gpui_telegram_code_scene::{
    compile_and_verify, validate_to_report, CodeSceneRecommender,
    DeterministicProceduralSynthesizer, FeedSessionController, FrameOutput, GameSessionTelemetry,
    RewardOutcome, SynthRequest, UserInterestProfile, OFFICIAL_FABUSHI_BOT_ID,
};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_proto::code_scene_contracts::{
    AiGenerateSceneRequest, AiGenerateSceneResponse, AiIterateSceneRequest, AiPublishSceneRequest,
    AssetKind, AssetManifest, CodeSceneAssetDto, CodeSceneCommentDto, CodeSceneCommentSort,
    CodeSceneFeedTab, CreateCodeSceneCommentRequest, CreateCodeSceneCommentResponse,
    ListCodeSceneAssetsQuery, ListCodeSceneAssetsResponse, ListCodeSceneCommentsQuery,
    ListCodeSceneCommentsResponse, PublishCodeSceneRequest, PublishCodeSceneResponse,
    RankedSceneItemDto, RemixSceneRequest, RemixSceneResponse, SceneBehaviorEventDto,
    SceneBehaviorEventKind, ToggleBookmarkResponse, ToggleFollowResponse, ToggleLikeResponse,
    ValidationDiagnosticDto, ValidationReportDto, ValidationSeverity, CODE_SCENE_FORMAT_CS1,
};
use parking_lot::RwLock;
use uuid::Uuid;

use crate::cache::{
    CachedCodeSceneComment, CachedCodeSceneRecord, CodeSceneLocalSocialEntry,
    CodeSceneSocialRelationKind, LocalCodeSceneDraft, LocalStore,
};
use crate::net::{CodeSceneHttpClient, HttpClientError};

const MAX_LOCAL_COMMENT_CHARS: usize = 2000;
const TELEMETRY_FLUSH_BATCH_SIZE: usize = 50;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelemetryFlushReport {
    pub batches_sent: usize,
    pub events_acked: usize,
    pub remaining_pending: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeSceneSyncReport {
    pub pushed_assets: usize,
    pub pushed_comments: usize,
    pub telemetry: TelemetryFlushReport,
    pub pulled_assets: usize,
    pub reconciled_social: usize,
    pub errors: Vec<String>,
}

struct CodeSceneServiceInner {
    server_url: String,
    user_id: i64,
    username: String,
    recommender: CodeSceneRecommender,
    session: FeedSessionController,
    synthesizer: DeterministicProceduralSynthesizer,
    session_seq: u64,
}

fn normalize_server_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

fn default_username(user_id: i64, username: Option<String>) -> String {
    username
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if user_id > 0 {
                format!("user_{user_id}")
            } else {
                "guest".to_string()
            }
        })
}

fn is_offline_transport_error(err: &HttpClientError) -> bool {
    match err {
        HttpClientError::Network(_) | HttpClientError::Proxy(_) => true,
        HttpClientError::Api { status, .. } => *status >= 500,
        _ => false,
    }
}

fn hydrate_ranked_item(
    inner: &CodeSceneServiceInner,
    mut item: RankedSceneItemDto,
) -> RankedSceneItemDto {
    if let Some(live_asset) = inner
        .recommender
        .get_asset_for_user(&item.asset.scene_id, inner.user_id)
    {
        item.asset = live_asset;
    }
    item
}

/// Orders local-only scenes topologically parent-first so offline parent scenes are always
/// pushed to the server before their offline child/grandchild remixes (`created_at_ms ASC`,
/// `scene_id ASC` tie-break).
fn sort_local_scenes_parent_first(
    mut scenes: Vec<CachedCodeSceneRecord>,
) -> Vec<CachedCodeSceneRecord> {
    scenes.sort_by(|a, b| {
        a.asset
            .created_at_ms
            .cmp(&b.asset.created_at_ms)
            .then_with(|| a.asset.scene_id.cmp(&b.asset.scene_id))
    });
    let local_ids: HashSet<String> = scenes.iter().map(|r| r.asset.scene_id.clone()).collect();
    let mut emitted: HashSet<String> = HashSet::with_capacity(scenes.len());
    let mut ordered: Vec<CachedCodeSceneRecord> = Vec::with_capacity(scenes.len());
    let mut remaining = scenes;

    while !remaining.is_empty() {
        let mut next_remaining = Vec::new();
        let mut progressed = false;
        for rec in remaining {
            let parent_ready = match rec.asset.remix_parent_id.as_deref() {
                None => true,
                Some(pid) if !local_ids.contains(pid) => true,
                Some(pid) => emitted.contains(pid),
            };
            if parent_ready {
                emitted.insert(rec.asset.scene_id.clone());
                ordered.push(rec);
                progressed = true;
            } else {
                next_remaining.push(rec);
            }
        }
        if !progressed {
            // Break any hypothetical cycle deterministically using earliest created_at_ms/scene_id.
            let first = next_remaining.remove(0);
            emitted.insert(first.asset.scene_id.clone());
            ordered.push(first);
        }
        remaining = next_remaining;
    }

    ordered
}

/// Orders pending offline comments topologically parent-first so offline root comments are always
/// pushed to the server before their offline replies/nested replies (`created_at_ms ASC`,
/// `comment_id ASC` tie-break), permitting external/already-synced parents without stalling and
/// breaking hypothetical cycles deterministically.
fn sort_pending_comments_parent_first(
    mut comments: Vec<CachedCodeSceneComment>,
) -> Vec<CachedCodeSceneComment> {
    comments.sort_by(|a, b| {
        a.comment
            .created_at_ms
            .cmp(&b.comment.created_at_ms)
            .then_with(|| a.comment.comment_id.cmp(&b.comment.comment_id))
    });
    let pending_ids: HashSet<String> = comments
        .iter()
        .map(|c| c.comment.comment_id.clone())
        .collect();
    let mut emitted: HashSet<String> = HashSet::with_capacity(comments.len());
    let mut ordered: Vec<CachedCodeSceneComment> = Vec::with_capacity(comments.len());
    let mut remaining = comments;

    while !remaining.is_empty() {
        let mut next_remaining = Vec::new();
        let mut progressed = false;
        for rec in remaining {
            let parent_ready = match rec.comment.parent_comment_id.as_deref() {
                None => true,
                Some(pid) if !pending_ids.contains(pid) => true,
                Some(pid) => emitted.contains(pid),
            };
            if parent_ready {
                emitted.insert(rec.comment.comment_id.clone());
                ordered.push(rec);
                progressed = true;
            } else {
                next_remaining.push(rec);
            }
        }
        if !progressed {
            let first = next_remaining.remove(0);
            emitted.insert(first.comment.comment_id.clone());
            ordered.push(first);
        }
        remaining = next_remaining;
    }

    ordered
}

/// Resolves an event's author user ID using the exact `telemetry::resolve_author` precedence:
/// (1) catalog asset `author_user_id` for `ev.scene_id`, (2) positive `ev.author_user_id`,
/// (3) positive parsed `ev.reason`.
fn resolve_event_author_id(
    recommender: &CodeSceneRecommender,
    ev: &SceneBehaviorEventDto,
) -> Option<i64> {
    if !ev.scene_id.is_empty() {
        if let Some(asset) = recommender.get_asset(&ev.scene_id) {
            if asset.author_user_id > 0 {
                return Some(asset.author_user_id);
            }
        }
    }
    if let Some(cid) = ev.author_user_id.filter(|&id| id > 0) {
        return Some(cid);
    }
    ev.reason
        .as_deref()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&id| id > 0)
}

fn build_hydrated_inner(
    local_store: &LocalStore,
    server_url: String,
    user_id: i64,
    username: String,
    session_seq: u64,
) -> CodeSceneServiceInner {
    let mut recommender = CodeSceneRecommender::new_with_seeds();

    // 1. Hydrate cached scenes FIRST (catalog before profile!)
    for rec in local_store.list_cached_code_scenes() {
        let just_pub = if rec.is_local_only && rec.asset.author_user_id == user_id {
            Some(user_id)
        } else {
            None
        };
        recommender.register_or_update_asset(rec.asset, just_pub);
    }

    // 2. Hydrate user profile + reconcile local social/blocklist edges strictly for `user_id`
    let mut profile = local_store
        .get_code_scene_profile(user_id)
        .unwrap_or_else(|| UserInterestProfile::new(user_id));
    let social_entries = local_store.list_code_scene_social(user_id);
    for entry in social_entries {
        match entry.relation_kind {
            CodeSceneSocialRelationKind::Like => {
                if entry.active {
                    profile.liked_scene_ids.insert(entry.target_id);
                } else {
                    profile.liked_scene_ids.remove(&entry.target_id);
                }
            }
            CodeSceneSocialRelationKind::Bookmark => {
                if entry.active {
                    profile.bookmarked_scene_ids.insert(entry.target_id);
                } else {
                    profile.bookmarked_scene_ids.remove(&entry.target_id);
                }
            }
            CodeSceneSocialRelationKind::Follow => {
                if let Ok(cid) = entry.target_id.parse::<i64>() {
                    if entry.active {
                        profile.followed_creators.insert(cid);
                    } else {
                        profile.followed_creators.remove(&cid);
                    }
                }
            }
            CodeSceneSocialRelationKind::Dislike => {
                if entry.active {
                    profile.disliked_scene_ids.insert(entry.target_id);
                }
            }
            CodeSceneSocialRelationKind::HideAuthor => {
                if let Ok(cid) = entry.target_id.parse::<i64>() {
                    if entry.active {
                        profile.hidden_author_ids.insert(cid);
                    } else {
                        profile.hidden_author_ids.remove(&cid);
                    }
                }
            }
            CodeSceneSocialRelationKind::HideTag => {
                if entry.active {
                    profile
                        .hidden_tags
                        .insert(entry.target_id.trim().to_ascii_lowercase());
                }
            }
        }
    }
    profile.sanitize();
    recommender.import_user_profile(profile);

    let session_id = format!("local_cs_{user_id}_{session_seq}");
    let session = FeedSessionController::new(user_id, session_id);
    CodeSceneServiceInner {
        server_url,
        user_id,
        username,
        recommender,
        session,
        synthesizer: DeterministicProceduralSynthesizer::new(),
        session_seq,
    }
}

/// Offline-first + server-synced Code Scene facade (`CS-F11`) consumable by GPUI views and tests.
///
/// Strict Lock Discipline:
/// - NEVER hold `self.inner` across `.await` OR while calling `local_store.bind_account_scope` / `clear_all`.
/// - Every async operation checks `account_epoch` before mutating local state so mid-flight account
///   switches cannot contaminate the newly active account.
#[derive(Clone)]
pub struct LocalCodeSceneService {
    local_store: LocalStore,
    remote: Arc<RwLock<Option<CodeSceneHttpClient>>>,
    inner: Arc<RwLock<CodeSceneServiceInner>>,
    account_epoch: Arc<AtomicU64>,
}

impl LocalCodeSceneService {
    pub fn new_offline(user_id: u64, username: Option<String>) -> Self {
        Self::new_with_remote(LocalStore::new(), "", user_id, username, None)
    }

    pub fn open_with_dir(
        cache_dir: impl AsRef<Path>,
        server_url: impl Into<String>,
        user_id: u64,
        username: Option<String>,
        remote: Option<CodeSceneHttpClient>,
    ) -> Result<Self> {
        let store = LocalStore::open_with_dir(cache_dir)?;
        Ok(Self::new_with_remote(store, server_url, user_id, username, remote))
    }

    pub fn new_with_remote(
        local_store: LocalStore,
        server_url: impl Into<String>,
        user_id: u64,
        username: Option<String>,
        remote: Option<CodeSceneHttpClient>,
    ) -> Self {
        let norm_srv = normalize_server_url(&server_url.into());
        if user_id > 0 {
            let had_prev_scope = local_store.owner_scope().is_some();
            local_store.bind_account_scope(&norm_srv, user_id);
            if !had_prev_scope {
                // Wipe all 6 Code Scene tables cleanly so anonymous guest (`user_id == 0`) rows
                // (including mutated seed cache rows) cannot leak into first-bound account.
                local_store.clear_code_scene_state();
            }
        }
        let uid_i64 = user_id as i64;
        let uname = default_username(uid_i64, username);
        let inner = Arc::new(RwLock::new(build_hydrated_inner(
            &local_store,
            norm_srv,
            uid_i64,
            uname,
            1,
        )));
        let account_epoch = Arc::new(AtomicU64::new(1));

        // Register Weak-backed Code Scene clear hook so both `LocalStore::clear_all()` and
        // Guest (`None`) -> Authenticated (`Some(uid)`) transitions in `bind_account_scope`
        // unconditionally bump `account_epoch` and reset in-memory state with zero leaks/cycles.
        let inner_weak = Arc::downgrade(&inner);
        let epoch_ref = Arc::clone(&account_epoch);
        local_store.register_code_scene_clear_hook(Arc::new(move || {
            epoch_ref.fetch_add(1, Ordering::SeqCst);
            if let Some(strong) = inner_weak.upgrade() {
                let mut guard = strong.write();
                guard.session_seq = guard.session_seq.saturating_add(1);
                let next_seq = guard.session_seq;
                let uid = guard.user_id;
                guard.recommender = CodeSceneRecommender::new_with_seeds();
                guard.session =
                    FeedSessionController::new(uid, format!("local_cs_{uid}_{next_seq}"));
            }
        }));

        Self {
            local_store,
            remote: Arc::new(RwLock::new(remote)),
            inner,
            account_epoch,
        }
    }

    pub fn local_store(&self) -> &LocalStore {
        &self.local_store
    }

    pub fn set_remote(&self, remote: Option<CodeSceneHttpClient>) {
        *self.remote.write() = remote;
    }

    pub fn active_account_scope(&self) -> (String, i64) {
        let guard = self.inner.read();
        (guard.server_url.clone(), guard.user_id)
    }

    /// Switches active account scope, clearing prior account state in SQLite + RAM when switching
    /// accounts, servers, or from guest (`0`) to an authenticated account.
    pub fn switch_account_scope(
        &self,
        server_url: &str,
        user_id: u64,
        username: Option<String>,
    ) {
        let norm_srv = normalize_server_url(server_url);
        let target_uid = user_id as i64;

        let (prev_uid, next_seq, already_same) = {
            let mut guard = self.inner.write();
            if guard.user_id == target_uid && guard.server_url == norm_srv {
                if let Some(un) = username.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    guard.username = un.to_string();
                }
                (guard.user_id, guard.session_seq, true)
            } else {
                guard.session_seq = guard.session_seq.saturating_add(1);
                (guard.user_id, guard.session_seq, false)
            }
        };
        if already_same {
            return;
        }

        // Bump account epoch BEFORE mutating LocalStore (with `inner` lock released!).
        self.account_epoch.fetch_add(1, Ordering::SeqCst);

        if user_id > 0 {
            self.local_store.bind_account_scope(&norm_srv, user_id);
            if prev_uid == 0 {
                self.local_store.clear_code_scene_state();
            }
        } else if prev_uid > 0 {
            self.local_store.clear_all();
        }

        let uname = default_username(target_uid, username);
        let hydrated =
            build_hydrated_inner(&self.local_store, norm_srv, target_uid, uname, next_seq);
        *self.inner.write() = hydrated;
    }

    fn persist_session_side_effects(&self, inner: &mut CodeSceneServiceInner) {
        let now_ms = Utc::now().timestamp_millis();
        let drained = inner.session.drain_emitted_telemetry();
        let mut touched_scenes: HashSet<String> = HashSet::new();

        for mut ev in drained {
            if ev.user_id != inner.user_id {
                ev.user_id = inner.user_id;
            }
            if ev.event_id.trim().is_empty() {
                ev.event_id = format!("cs_ev_{}", Uuid::new_v4().simple());
            }
            if ev.timestamp_ms <= 0 {
                ev.timestamp_ms = now_ms;
            }
            if !ev.scene_id.is_empty() {
                touched_scenes.insert(ev.scene_id.clone());
            }

            match ev.kind {
                SceneBehaviorEventKind::Like | SceneBehaviorEventKind::DoubleTapLike => {
                    if !ev.scene_id.is_empty() {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::Like,
                                target_id: ev.scene_id.clone(),
                                active: true,
                                pending_sync: true,
                                updated_at_ms: now_ms,
                            });
                    }
                    // Handled via social reconciliation in step 5 to prevent double-toggle.
                    continue;
                }
                SceneBehaviorEventKind::Unlike => {
                    if !ev.scene_id.is_empty() {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::Like,
                                target_id: ev.scene_id.clone(),
                                active: false,
                                pending_sync: true,
                                updated_at_ms: now_ms,
                            });
                    }
                    continue;
                }
                SceneBehaviorEventKind::Bookmark => {
                    if !ev.scene_id.is_empty() {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::Bookmark,
                                target_id: ev.scene_id.clone(),
                                active: true,
                                pending_sync: true,
                                updated_at_ms: now_ms,
                            });
                    }
                    continue;
                }
                SceneBehaviorEventKind::Unbookmark => {
                    if !ev.scene_id.is_empty() {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::Bookmark,
                                target_id: ev.scene_id.clone(),
                                active: false,
                                pending_sync: true,
                                updated_at_ms: now_ms,
                            });
                    }
                    continue;
                }
                SceneBehaviorEventKind::FollowAuthor => {
                    if let Some(cid) = resolve_event_author_id(&inner.recommender, &ev) {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::HideAuthor,
                                target_id: cid.to_string(),
                                active: false,
                                pending_sync: false,
                                updated_at_ms: now_ms,
                            });
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::Follow,
                                target_id: cid.to_string(),
                                active: true,
                                pending_sync: true,
                                updated_at_ms: now_ms,
                            });
                    }
                    continue;
                }
                SceneBehaviorEventKind::UnfollowAuthor => {
                    if let Some(cid) = resolve_event_author_id(&inner.recommender, &ev) {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::Follow,
                                target_id: cid.to_string(),
                                active: false,
                                pending_sync: true,
                                updated_at_ms: now_ms,
                            });
                    }
                    continue;
                }
                SceneBehaviorEventKind::NotInterested => {
                    if !ev.scene_id.is_empty() {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::Dislike,
                                target_id: ev.scene_id.clone(),
                                active: true,
                                pending_sync: false,
                                updated_at_ms: now_ms,
                            });
                    }
                }
                SceneBehaviorEventKind::HideAuthor => {
                    if let Some(cid) = resolve_event_author_id(&inner.recommender, &ev) {
                        ev.author_user_id = Some(cid);
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::HideAuthor,
                                target_id: cid.to_string(),
                                active: true,
                                pending_sync: false,
                                updated_at_ms: now_ms,
                            });
                        // Immediately supersede any prior pending offline Follow for `cid` so
                        // `sync_with_server` Step 5 never replays Follow after Step 3 flushes HideAuthor.
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::Follow,
                                target_id: cid.to_string(),
                                active: false,
                                pending_sync: false,
                                updated_at_ms: now_ms,
                            });
                    }
                }
                SceneBehaviorEventKind::HideTag => {
                    if let Some(tag) = ev.hidden_tag() {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: inner.user_id,
                                relation_kind: CodeSceneSocialRelationKind::HideTag,
                                target_id: tag.to_string(),
                                active: true,
                                pending_sync: false,
                                updated_at_ms: now_ms,
                            });
                    }
                }
                _ => {}
            }

            let _ = self.local_store.enqueue_code_scene_telemetry(ev);
        }

        if let Some(profile) = inner.recommender.export_user_profile(inner.user_id) {
            self.local_store.upsert_code_scene_profile(profile);
        }
        for sid in touched_scenes {
            if let Some(asset) = inner.recommender.get_asset(&sid) {
                let is_local = self
                    .local_store
                    .get_cached_code_scene(&sid)
                    .is_some_and(|r| r.is_local_only);
                self.local_store.upsert_cached_code_scene(asset, is_local);
            }
        }
    }

    pub fn load_feed(&self) -> usize {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let count = inner.session.load_initial_feed(&mut inner.recommender);
        self.persist_session_side_effects(inner);
        count
    }

    pub fn ensure_feed_loaded(&self) -> usize {
        let already = self.inner.read().session.items().len();
        if already > 0 {
            return already;
        }
        self.load_feed()
    }

    pub fn current_item(&self) -> Option<RankedSceneItemDto> {
        let inner = self.inner.read();
        inner
            .session
            .current_item()
            .cloned()
            .map(|item| hydrate_ranked_item(&inner, item))
    }

    pub fn current_index(&self) -> usize {
        self.inner.read().session.current_index()
    }

    pub fn items(&self) -> Vec<RankedSceneItemDto> {
        let inner = self.inner.read();
        inner
            .session
            .items()
            .iter()
            .cloned()
            .map(|item| hydrate_ranked_item(&inner, item))
            .collect()
    }

    pub fn preload_window_indices(&self) -> Vec<usize> {
        self.inner.read().session.preload_window_indices()
    }

    pub fn preload_window_items(&self) -> Vec<(usize, RankedSceneItemDto)> {
        let inner = self.inner.read();
        inner
            .session
            .preload_window_items()
            .into_iter()
            .map(|(i, item)| (i, hydrate_ranked_item(&inner, item.clone())))
            .collect()
    }

    pub fn active_tab(&self) -> CodeSceneFeedTab {
        self.inner.read().session.active_tab()
    }

    pub fn tag_filter(&self) -> Option<String> {
        self.inner.read().session.tag_filter().map(str::to_string)
    }

    pub fn query(&self) -> Option<String> {
        self.inner.read().session.query().map(str::to_string)
    }

    pub fn total_catalog(&self) -> usize {
        self.inner.read().session.total_catalog()
    }

    pub fn catalog_len(&self) -> usize {
        self.inner.read().recommender.catalog_len()
    }

    pub fn switch_tab(&self, tab: CodeSceneFeedTab) -> usize {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let n = inner.session.switch_tab(&mut inner.recommender, tab);
        self.persist_session_side_effects(inner);
        n
    }

    pub fn set_tag_filter(&self, tag: Option<String>) -> usize {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let n = inner.session.set_tag_filter(&mut inner.recommender, tag);
        self.persist_session_side_effects(inner);
        n
    }

    pub fn set_query(&self, query: Option<String>) -> usize {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let n = inner.session.set_query(&mut inner.recommender, query);
        self.persist_session_side_effects(inner);
        n
    }

    pub fn swipe_next(&self) -> Option<RankedSceneItemDto> {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let item = inner.session.swipe_next(&mut inner.recommender).cloned();
        self.persist_session_side_effects(inner);
        item.map(|it| hydrate_ranked_item(inner, it))
    }

    pub fn swipe_prev(&self) -> Option<RankedSceneItemDto> {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let item = inner.session.swipe_prev(&mut inner.recommender).cloned();
        self.persist_session_side_effects(inner);
        item.map(|it| hydrate_ranked_item(inner, it))
    }

    pub fn open_scene_by_id(&self, scene_id: &str) -> Option<RankedSceneItemDto> {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let item = inner
            .session
            .open_scene_by_id(&mut inner.recommender, scene_id)
            .cloned();
        self.persist_session_side_effects(inner);
        item.map(|it| hydrate_ranked_item(inner, it))
    }

    pub fn on_card_dwell_tick(&self, delta_ms: u64, loop_count: u32) {
        self.inner
            .write()
            .session
            .on_card_dwell_tick(delta_ms, loop_count);
    }

    pub fn record_game_telemetry(&self, telemetry: GameSessionTelemetry) {
        self.inner.write().session.record_game_telemetry(telemetry);
    }

    pub fn on_frame_output(
        &self,
        delta_ms: u64,
        frame: &FrameOutput,
        game_telemetry: Option<GameSessionTelemetry>,
    ) {
        self.inner
            .write()
            .session
            .on_frame_output(delta_ms, frame, game_telemetry);
    }

    pub fn flush_active_card_telemetry(&self) -> Option<SceneBehaviorEventDto> {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let ev = inner.session.flush_active_telemetry(&mut inner.recommender);
        self.persist_session_side_effects(inner);
        ev
    }

    fn unhide_author_in_recommender(inner: &mut CodeSceneServiceInner, author_user_id: i64) {
        let mut profile = inner
            .recommender
            .export_user_profile(inner.user_id)
            .unwrap_or_else(|| UserInterestProfile::new(inner.user_id));
        let unhid = profile.hidden_author_ids.remove(&author_user_id);
        let cleared_neg_weight = if profile
            .creator_weights
            .get(&author_user_id)
            .copied()
            .unwrap_or(0.0)
            < 0.0
        {
            profile.creator_weights.remove(&author_user_id).is_some()
        } else {
            false
        };
        if unhid || cleared_neg_weight {
            profile.sanitize();
            inner.recommender.import_user_profile(profile);
        }
    }

    fn prune_queued_hide_author_outbox(&self, inner: &CodeSceneServiceInner, author_user_id: i64) {
        // Drop any stale queued HideAuthor telemetry for this creator so a later sync
        // does not replay an older hide after an explicit follow.
        let stale_hide_ids: Vec<i64> = self
            .local_store
            .list_code_scene_telemetry_outbox(inner.user_id, usize::MAX)
            .into_iter()
            .filter(|rec| {
                rec.event.kind == SceneBehaviorEventKind::HideAuthor
                    && resolve_event_author_id(&inner.recommender, &rec.event)
                        == Some(author_user_id)
            })
            .map(|rec| rec.outbox_id)
            .collect();
        if !stale_hide_ids.is_empty() {
            self.local_store
                .ack_code_scene_telemetry_outbox(&stale_hide_ids);
        }
    }

    /// Authoritatively reconciles `profile.liked_scene_ids` (preserving `disliked_scene_ids`
    /// and `hidden_tags`, matching server `routes/code_scene.rs`) when `recommender` skips a
    /// social transition on a hard-blocked scene, optionally adjusting local `likes_count`.
    fn reconcile_like_in_recommender(
        inner: &mut CodeSceneServiceInner,
        scene_id: &str,
        was_liked: bool,
        prev_likes_count: u64,
        target_liked: bool,
        adjust_counter: bool,
    ) {
        let mut profile = inner
            .recommender
            .export_user_profile(inner.user_id)
            .unwrap_or_else(|| UserInterestProfile::new(inner.user_id));
        let changed = if target_liked {
            profile.liked_scene_ids.insert(scene_id.to_string())
        } else {
            profile.liked_scene_ids.remove(scene_id)
        };
        if changed {
            profile.sanitize();
            inner.recommender.import_user_profile(profile);
        }
        if adjust_counter {
            let next_count = if target_liked && !was_liked {
                prev_likes_count.saturating_add(1)
            } else if !target_liked && was_liked {
                prev_likes_count.saturating_sub(1)
            } else {
                prev_likes_count
            };
            inner.recommender.update_social_counters(
                scene_id,
                Some(next_count),
                None,
                None,
                None,
                None,
                None,
            );
        }
    }

    /// Authoritatively reconciles `profile.bookmarked_scene_ids` (preserving `disliked_scene_ids`
    /// and `hidden_tags`, matching server `routes/code_scene.rs`) when `recommender` skips a
    /// social transition on a hard-blocked scene, optionally adjusting local `bookmarks_count`.
    fn reconcile_bookmark_in_recommender(
        inner: &mut CodeSceneServiceInner,
        scene_id: &str,
        was_bookmarked: bool,
        prev_bookmarks_count: u64,
        target_bookmarked: bool,
        adjust_counter: bool,
    ) {
        let mut profile = inner
            .recommender
            .export_user_profile(inner.user_id)
            .unwrap_or_else(|| UserInterestProfile::new(inner.user_id));
        let changed = if target_bookmarked {
            profile.bookmarked_scene_ids.insert(scene_id.to_string())
        } else {
            profile.bookmarked_scene_ids.remove(scene_id)
        };
        if changed {
            profile.sanitize();
            inner.recommender.import_user_profile(profile);
        }
        if adjust_counter {
            let next_count = if target_bookmarked && !was_bookmarked {
                prev_bookmarks_count.saturating_add(1)
            } else if !target_bookmarked && was_bookmarked {
                prev_bookmarks_count.saturating_sub(1)
            } else {
                prev_bookmarks_count
            };
            inner.recommender.update_social_counters(
                scene_id,
                None,
                Some(next_count),
                None,
                None,
                None,
                None,
            );
        }
    }

    pub fn record_interaction(
        &self,
        kind: SceneBehaviorEventKind,
        reason: Option<String>,
    ) -> Option<RewardOutcome> {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let active_snapshot = inner.session.current_item().map(|item| {
            let sid = item.asset.scene_id.clone();
            let live = inner
                .recommender
                .get_asset_for_user(&sid, inner.user_id)
                .unwrap_or_else(|| item.asset.clone());
            let author_id = if live.author_user_id > 0 {
                Some(live.author_user_id)
            } else {
                reason
                    .as_deref()
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .filter(|&id| id > 0)
            };
            (
                sid,
                live.viewer_liked,
                live.likes_count,
                live.viewer_bookmarked,
                live.bookmarks_count,
                author_id,
            )
        });
        if kind == SceneBehaviorEventKind::FollowAuthor {
            if let Some((_, _, _, _, _, Some(author_user_id))) = &active_snapshot {
                Self::unhide_author_in_recommender(inner, *author_user_id);
            }
        }
        let out = inner
            .session
            .record_interaction(&mut inner.recommender, kind, reason);
        if out.is_some() {
            if let Some((
                scene_id,
                was_liked,
                prev_likes,
                was_bookmarked,
                prev_bookmarks,
                author_id,
            )) = active_snapshot
            {
                match kind {
                    SceneBehaviorEventKind::Like
                    | SceneBehaviorEventKind::DoubleTapLike
                    | SceneBehaviorEventKind::Unlike => {
                        if !scene_id.is_empty() {
                            let target_liked = kind != SceneBehaviorEventKind::Unlike;
                            let post_liked = inner
                                .recommender
                                .get_asset_for_user(&scene_id, inner.user_id)
                                .map(|a| a.viewer_liked)
                                .unwrap_or(was_liked);
                            if post_liked != target_liked {
                                Self::reconcile_like_in_recommender(
                                    inner,
                                    &scene_id,
                                    was_liked,
                                    prev_likes,
                                    target_liked,
                                    true,
                                );
                            }
                        }
                    }
                    SceneBehaviorEventKind::Bookmark | SceneBehaviorEventKind::Unbookmark => {
                        if !scene_id.is_empty() {
                            let target_bm = kind == SceneBehaviorEventKind::Bookmark;
                            let post_bm = inner
                                .recommender
                                .get_asset_for_user(&scene_id, inner.user_id)
                                .map(|a| a.viewer_bookmarked)
                                .unwrap_or(was_bookmarked);
                            if post_bm != target_bm {
                                Self::reconcile_bookmark_in_recommender(
                                    inner,
                                    &scene_id,
                                    was_bookmarked,
                                    prev_bookmarks,
                                    target_bm,
                                    true,
                                );
                            }
                        }
                    }
                    SceneBehaviorEventKind::FollowAuthor => {
                        if let Some(author_user_id) = author_id {
                            // If the active scene itself was individually disliked (e.g., via prior HideAuthor)
                            // or carries a hidden tag, card-scoped FollowAuthor is ignored by `telemetry.rs`;
                            // apply creator-level toggle and authoritatively sync profile follow/hidden sets.
                            let now_following = inner
                                .recommender
                                .export_user_profile(inner.user_id)
                                .is_some_and(|p| p.followed_creators.contains(&author_user_id));
                            if !now_following {
                                let _ = inner
                                    .recommender
                                    .toggle_follow(inner.user_id, author_user_id);
                            }
                            let mut profile = inner
                                .recommender
                                .export_user_profile(inner.user_id)
                                .unwrap_or_else(|| UserInterestProfile::new(inner.user_id));
                            let unhid = profile.hidden_author_ids.remove(&author_user_id);
                            let followed = profile.followed_creators.insert(author_user_id);
                            if unhid || followed {
                                profile.sanitize();
                                inner.recommender.import_user_profile(profile);
                            }
                            self.prune_queued_hide_author_outbox(inner, author_user_id);
                        }
                    }
                    SceneBehaviorEventKind::UnfollowAuthor => {
                        if let Some(author_user_id) = author_id {
                            let still_following = inner
                                .recommender
                                .export_user_profile(inner.user_id)
                                .is_some_and(|p| p.followed_creators.contains(&author_user_id));
                            if still_following {
                                let mut profile = inner
                                    .recommender
                                    .export_user_profile(inner.user_id)
                                    .unwrap_or_else(|| UserInterestProfile::new(inner.user_id));
                                if profile.followed_creators.remove(&author_user_id) {
                                    profile.sanitize();
                                    inner.recommender.import_user_profile(profile);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        self.persist_session_side_effects(inner);
        out
    }

    pub fn get_asset(&self, scene_id: &str) -> Option<CodeSceneAssetDto> {
        let inner = self.inner.read();
        inner.recommender.get_asset_for_user(scene_id, inner.user_id)
    }

    pub fn list_assets(&self, query: &ListCodeSceneAssetsQuery) -> ListCodeSceneAssetsResponse {
        let inner = self.inner.read();
        let mut list = inner.recommender.list_assets(query);
        if let Some(profile) = inner.recommender.export_user_profile(inner.user_id) {
            for asset in &mut list.assets {
                asset.viewer_liked = profile.liked_scene_ids.contains(&asset.scene_id);
                asset.viewer_bookmarked = profile.bookmarked_scene_ids.contains(&asset.scene_id);
                asset.viewer_following_author =
                    profile.followed_creators.contains(&asset.author_user_id);
            }
        }
        list
    }

    pub fn validate_source(&self, source: &str, manifest: &AssetManifest) -> ValidationReportDto {
        validate_to_report(source, manifest)
    }

    pub fn export_active_profile(&self) -> UserInterestProfile {
        let inner = self.inner.read();
        inner
            .recommender
            .export_user_profile(inner.user_id)
            .unwrap_or_else(|| UserInterestProfile::new(inner.user_id))
    }

    pub fn pending_telemetry_count(&self) -> usize {
        let uid = self.inner.read().user_id;
        self.local_store.pending_code_scene_telemetry_count(uid)
    }

    fn apply_like_state(
        &self,
        scene_id: &str,
        desired_liked: Option<bool>,
        authoritative_count: Option<u64>,
        pending_sync: bool,
    ) -> Option<ToggleLikeResponse> {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let current = inner.recommender.get_asset_for_user(scene_id, inner.user_id)?;
        let was_liked = current.viewer_liked;
        let prev_likes_count = current.likes_count;
        let target_liked = desired_liked.unwrap_or(!was_liked);
        let is_active_card = inner
            .session
            .current_item()
            .is_some_and(|item| item.asset.scene_id == scene_id);

        let ev_kind = if target_liked {
            SceneBehaviorEventKind::Like
        } else {
            SceneBehaviorEventKind::Unlike
        };

        if target_liked != was_liked {
            if is_active_card {
                let _ = inner
                    .session
                    .record_interaction(&mut inner.recommender, ev_kind, None);
                // Drain emitted social event so we control pending_sync accurately below.
                let _ = inner.session.drain_emitted_telemetry();
            } else {
                let _ = inner.recommender.toggle_like(inner.user_id, scene_id);
            }
        }

        let post_liked = inner
            .recommender
            .get_asset_for_user(scene_id, inner.user_id)
            .map(|a| a.viewer_liked)
            .unwrap_or(was_liked);
        if post_liked != target_liked {
            Self::reconcile_like_in_recommender(
                inner,
                scene_id,
                was_liked,
                prev_likes_count,
                target_liked,
                authoritative_count.is_none(),
            );
        }

        if let Some(cnt) = authoritative_count {
            inner
                .recommender
                .update_social_counters(scene_id, Some(cnt), None, None, None, None, None);
        }

        let updated = inner.recommender.get_asset_for_user(scene_id, inner.user_id)?;
        let now_ms = Utc::now().timestamp_millis();
        self.local_store
            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                user_id: inner.user_id,
                relation_kind: CodeSceneSocialRelationKind::Like,
                target_id: scene_id.to_string(),
                active: updated.viewer_liked,
                pending_sync,
                updated_at_ms: now_ms,
            });
        let is_local = self
            .local_store
            .get_cached_code_scene(scene_id)
            .is_some_and(|r| r.is_local_only);
        self.local_store
            .upsert_cached_code_scene(updated.clone(), is_local);
        if let Some(profile) = inner.recommender.export_user_profile(inner.user_id) {
            self.local_store.upsert_code_scene_profile(profile);
        }

        Some(ToggleLikeResponse {
            liked: updated.viewer_liked,
            likes_count: updated.likes_count,
        })
    }

    pub fn toggle_like_local(&self, scene_id: &str) -> Option<ToggleLikeResponse> {
        self.apply_like_state(scene_id, None, None, true)
    }

    pub async fn toggle_like(
        &self,
        scene_id: &str,
        token: Option<&str>,
    ) -> Result<ToggleLikeResponse> {
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let remote = self.remote.read().clone();
        if let (Some(client), Some(tok)) = (remote, token.filter(|t| !t.trim().is_empty())) {
            match client.toggle_like(tok, scene_id).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        let _ = self.apply_like_state(
                            scene_id,
                            Some(resp.liked),
                            Some(resp.likes_count),
                            false,
                        );
                    }
                    return Ok(resp);
                }
                Err(err) if !is_offline_transport_error(&err) => {
                    return Err(TelegramError::from(err));
                }
                Err(_) => {}
            }
        }
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Account scope changed during request".into(),
            ));
        }
        self.toggle_like_local(scene_id)
            .ok_or_else(|| TelegramError::NotFound(format!("Code Scene '{scene_id}' not found")))
    }

    fn apply_bookmark_state(
        &self,
        scene_id: &str,
        desired_bookmarked: Option<bool>,
        authoritative_count: Option<u64>,
        pending_sync: bool,
    ) -> Option<ToggleBookmarkResponse> {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let current = inner.recommender.get_asset_for_user(scene_id, inner.user_id)?;
        let was_bookmarked = current.viewer_bookmarked;
        let prev_bookmarks_count = current.bookmarks_count;
        let target_bm = desired_bookmarked.unwrap_or(!was_bookmarked);
        let is_active_card = inner
            .session
            .current_item()
            .is_some_and(|item| item.asset.scene_id == scene_id);

        let ev_kind = if target_bm {
            SceneBehaviorEventKind::Bookmark
        } else {
            SceneBehaviorEventKind::Unbookmark
        };

        if target_bm != was_bookmarked {
            if is_active_card {
                let _ = inner
                    .session
                    .record_interaction(&mut inner.recommender, ev_kind, None);
                let _ = inner.session.drain_emitted_telemetry();
            } else {
                let _ = inner.recommender.toggle_bookmark(inner.user_id, scene_id);
            }
        }

        let post_bookmarked = inner
            .recommender
            .get_asset_for_user(scene_id, inner.user_id)
            .map(|a| a.viewer_bookmarked)
            .unwrap_or(was_bookmarked);
        if post_bookmarked != target_bm {
            Self::reconcile_bookmark_in_recommender(
                inner,
                scene_id,
                was_bookmarked,
                prev_bookmarks_count,
                target_bm,
                authoritative_count.is_none(),
            );
        }

        if let Some(cnt) = authoritative_count {
            inner
                .recommender
                .update_social_counters(scene_id, None, Some(cnt), None, None, None, None);
        }

        let updated = inner.recommender.get_asset_for_user(scene_id, inner.user_id)?;
        let now_ms = Utc::now().timestamp_millis();
        self.local_store
            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                user_id: inner.user_id,
                relation_kind: CodeSceneSocialRelationKind::Bookmark,
                target_id: scene_id.to_string(),
                active: updated.viewer_bookmarked,
                pending_sync,
                updated_at_ms: now_ms,
            });
        let is_local = self
            .local_store
            .get_cached_code_scene(scene_id)
            .is_some_and(|r| r.is_local_only);
        self.local_store
            .upsert_cached_code_scene(updated.clone(), is_local);
        if let Some(profile) = inner.recommender.export_user_profile(inner.user_id) {
            self.local_store.upsert_code_scene_profile(profile);
        }

        Some(ToggleBookmarkResponse {
            bookmarked: updated.viewer_bookmarked,
            bookmarks_count: updated.bookmarks_count,
        })
    }

    pub fn toggle_bookmark_local(&self, scene_id: &str) -> Option<ToggleBookmarkResponse> {
        self.apply_bookmark_state(scene_id, None, None, true)
    }

    pub async fn toggle_bookmark(
        &self,
        scene_id: &str,
        token: Option<&str>,
    ) -> Result<ToggleBookmarkResponse> {
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let remote = self.remote.read().clone();
        if let (Some(client), Some(tok)) = (remote, token.filter(|t| !t.trim().is_empty())) {
            match client.toggle_bookmark(tok, scene_id).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        let _ = self.apply_bookmark_state(
                            scene_id,
                            Some(resp.bookmarked),
                            Some(resp.bookmarks_count),
                            false,
                        );
                    }
                    return Ok(resp);
                }
                Err(err) if !is_offline_transport_error(&err) => {
                    return Err(TelegramError::from(err));
                }
                Err(_) => {}
            }
        }
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Account scope changed during request".into(),
            ));
        }
        self.toggle_bookmark_local(scene_id)
            .ok_or_else(|| TelegramError::NotFound(format!("Code Scene '{scene_id}' not found")))
    }

    fn apply_follow_state(
        &self,
        author_user_id: i64,
        desired_following: Option<bool>,
        authoritative_followers: Option<u64>,
        pending_sync: bool,
    ) -> ToggleFollowResponse {
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let current_following = inner
            .recommender
            .export_user_profile(inner.user_id)
            .is_some_and(|p| p.followed_creators.contains(&author_user_id));
        let target_following = desired_following.unwrap_or(!current_following);
        let is_active_author = inner
            .session
            .current_item()
            .is_some_and(|item| item.asset.author_user_id == author_user_id);

        if target_following {
            // Un-hide previously hidden creator BEFORE applying FollowAuthor / toggle_follow so
            // `telemetry.rs` does not drop the follow due to `target_is_hard_blocked`.
            Self::unhide_author_in_recommender(inner, author_user_id);
        }

        if target_following != current_following {
            let ev_kind = if target_following {
                SceneBehaviorEventKind::FollowAuthor
            } else {
                SceneBehaviorEventKind::UnfollowAuthor
            };
            if is_active_author {
                let _ = inner
                    .session
                    .record_interaction(&mut inner.recommender, ev_kind, None);
                let _ = inner.session.drain_emitted_telemetry();
                // If the active scene itself was individually disliked (e.g. via prior HideAuthor),
                // card-scoped FollowAuthor is ignored by telemetry; apply creator-level toggle directly.
                let now_following = inner
                    .recommender
                    .export_user_profile(inner.user_id)
                    .is_some_and(|p| p.followed_creators.contains(&author_user_id));
                if now_following != target_following {
                    let _ = inner
                        .recommender
                        .toggle_follow(inner.user_id, author_user_id);
                }
            } else {
                let _ = inner
                    .recommender
                    .toggle_follow(inner.user_id, author_user_id);
            }
        }

        // Ensure profile follows/hidden sets match `target_following` authoritatively.
        let mut profile = inner
            .recommender
            .export_user_profile(inner.user_id)
            .unwrap_or_else(|| UserInterestProfile::new(inner.user_id));
        let profile_fixed = if target_following {
            let unhid = profile.hidden_author_ids.remove(&author_user_id);
            profile.followed_creators.insert(author_user_id) || unhid
        } else {
            profile.followed_creators.remove(&author_user_id)
        };
        if profile_fixed {
            profile.sanitize();
            inner.recommender.import_user_profile(profile.clone());
        }

        let now_ms = Utc::now().timestamp_millis();
        if target_following {
            self.local_store
                .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                    user_id: inner.user_id,
                    relation_kind: CodeSceneSocialRelationKind::HideAuthor,
                    target_id: author_user_id.to_string(),
                    active: false,
                    pending_sync: false,
                    updated_at_ms: now_ms,
                });
            self.prune_queued_hide_author_outbox(inner, author_user_id);
        }
        self.local_store
            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                user_id: inner.user_id,
                relation_kind: CodeSceneSocialRelationKind::Follow,
                target_id: author_user_id.to_string(),
                active: target_following,
                pending_sync,
                updated_at_ms: now_ms,
            });
        self.local_store.upsert_code_scene_profile(profile);

        ToggleFollowResponse {
            following: target_following,
            followers_count: authoritative_followers.unwrap_or(if target_following { 1 } else { 0 }),
        }
    }

    pub fn toggle_follow_local(&self, author_user_id: i64) -> ToggleFollowResponse {
        self.apply_follow_state(author_user_id, None, None, true)
    }

    pub async fn toggle_follow(
        &self,
        author_user_id: i64,
        token: Option<&str>,
    ) -> Result<ToggleFollowResponse> {
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let remote = self.remote.read().clone();
        if let (Some(client), Some(tok)) = (remote, token.filter(|t| !t.trim().is_empty())) {
            match client.toggle_follow(tok, author_user_id).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        let _ = self.apply_follow_state(
                            author_user_id,
                            Some(resp.following),
                            Some(resp.followers_count),
                            false,
                        );
                    }
                    return Ok(resp);
                }
                Err(err) if !is_offline_transport_error(&err) => {
                    return Err(TelegramError::from(err));
                }
                Err(_) => {}
            }
        }
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Account scope changed during request".into(),
            ));
        }
        Ok(self.toggle_follow_local(author_user_id))
    }

    pub fn list_comments_local(
        &self,
        scene_id: &str,
        sort: CodeSceneCommentSort,
    ) -> ListCodeSceneCommentsResponse {
        let records = self.local_store.list_code_scene_comments(scene_id, sort);
        let comments: Vec<CodeSceneCommentDto> = records.into_iter().map(|r| r.comment).collect();
        let total = comments.len();
        ListCodeSceneCommentsResponse {
            scene_id: scene_id.to_string(),
            comments,
            total,
        }
    }

    pub async fn fetch_comments(
        &self,
        scene_id: &str,
        query: &ListCodeSceneCommentsQuery,
        token: Option<&str>,
    ) -> Result<ListCodeSceneCommentsResponse> {
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let sort = query.sort.unwrap_or(CodeSceneCommentSort::Hot);
        let remote = self.remote.read().clone();
        if let (Some(client), Some(tok)) = (remote, token.filter(|t| !t.trim().is_empty())) {
            match client.list_comments(tok, scene_id, query).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        let records: Vec<CachedCodeSceneComment> = resp
                            .comments
                            .iter()
                            .cloned()
                            .map(|c| CachedCodeSceneComment {
                                comment: c,
                                pending_sync: false,
                            })
                            .collect();
                        self.local_store
                            .replace_code_scene_comments_for_scene(scene_id, records);
                    }
                    return Ok(resp);
                }
                Err(err) if !is_offline_transport_error(&err) => {
                    return Err(TelegramError::from(err));
                }
                Err(_) => {}
            }
        }
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Account scope changed during request".into(),
            ));
        }
        Ok(self.list_comments_local(scene_id, sort))
    }

    pub fn add_comment_local(
        &self,
        scene_id: &str,
        content: &str,
        parent_comment_id: Option<String>,
    ) -> Result<CreateCodeSceneCommentResponse> {
        let trimmed = content.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_LOCAL_COMMENT_CHARS {
            return Err(TelegramError::Validation(format!(
                "Comment content must be 1..={MAX_LOCAL_COMMENT_CHARS} characters"
            )));
        }
        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let existing = inner
            .recommender
            .get_asset(scene_id)
            .ok_or_else(|| TelegramError::NotFound(format!("Code Scene '{scene_id}' not found")))?;

        let now_ms = Utc::now().timestamp_millis();
        let comment = CodeSceneCommentDto {
            comment_id: format!("local_csc_{}", Uuid::new_v4().simple()),
            scene_id: scene_id.to_string(),
            author_user_id: inner.user_id,
            author_username: inner.username.clone(),
            content: trimmed.to_string(),
            parent_comment_id,
            likes_count: 0,
            viewer_liked: false,
            created_at_ms: now_ms,
        };
        let new_count = existing.comments_count.saturating_add(1);
        inner
            .recommender
            .update_social_counters(scene_id, None, None, Some(new_count), None, None, None);

        self.local_store
            .upsert_code_scene_comment(CachedCodeSceneComment {
                comment: comment.clone(),
                pending_sync: true,
            });
        if let Some(updated_asset) = inner.recommender.get_asset(scene_id) {
            let is_local = self
                .local_store
                .get_cached_code_scene(scene_id)
                .is_some_and(|r| r.is_local_only);
            self.local_store
                .upsert_cached_code_scene(updated_asset, is_local);
        }

        Ok(CreateCodeSceneCommentResponse {
            comment,
            comments_count: new_count,
        })
    }

    pub async fn add_comment(
        &self,
        scene_id: &str,
        req: &CreateCodeSceneCommentRequest,
        token: Option<&str>,
    ) -> Result<CreateCodeSceneCommentResponse> {
        let trimmed = req.content.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_LOCAL_COMMENT_CHARS {
            return Err(TelegramError::Validation(format!(
                "Comment content must be 1..={MAX_LOCAL_COMMENT_CHARS} characters"
            )));
        }
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let remote = self.remote.read().clone();
        if let (Some(client), Some(tok)) = (remote, token.filter(|t| !t.trim().is_empty())) {
            match client.add_comment(tok, scene_id, req).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        self.local_store
                            .upsert_code_scene_comment(CachedCodeSceneComment {
                                comment: resp.comment.clone(),
                                pending_sync: false,
                            });
                        let mut inner = self.inner.write();
                        inner.recommender.update_social_counters(
                            scene_id,
                            None,
                            None,
                            Some(resp.comments_count),
                            None,
                            None,
                            None,
                        );
                    }
                    return Ok(resp);
                }
                Err(err) if !is_offline_transport_error(&err) => {
                    return Err(TelegramError::from(err));
                }
                Err(_) => {}
            }
        }
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Account scope changed during request".into(),
            ));
        }
        self.add_comment_local(scene_id, trimmed, req.parent_comment_id.clone())
    }

    pub fn publish_scene_local(
        &self,
        req: &PublishCodeSceneRequest,
    ) -> Result<PublishCodeSceneResponse> {
        let title = req.title.trim();
        if title.is_empty() {
            return Err(TelegramError::Validation("title cannot be empty".into()));
        }
        let mut manifest = req.manifest.clone();
        manifest.kind = req.kind;
        manifest.clamp_in_place();

        let verified = compile_and_verify(&req.source_code, &manifest)
            .map_err(|d| TelegramError::Validation(format!("[{}] {}", d.code, d.message)))?;
        let report = ValidationReportDto {
            ok: true,
            diagnostics: verified
                .warnings
                .iter()
                .map(|w| w.to_dto(ValidationSeverity::Warning))
                .collect::<Vec<ValidationDiagnosticDto>>(),
            frames_executed: verified.draw_cmds_per_frame.len() as u32,
            draw_cmds_per_frame: verified.draw_cmds_per_frame.clone(),
            has_pixel_output: verified.has_pixel_output,
            peak_ops_used: verified.peak_ops_used,
            cover_hash: Some(verified.cover_hash.clone()),
            feature_tokens: verified.feature_tokens.clone(),
        };

        let now_ms = Utc::now().timestamp_millis();
        let scene_id = req
            .scene_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("local_scene_{}", Uuid::new_v4().simple()));

        let mut inner = self.inner.write();
        let inner = &mut *inner;
        let mut asset = CodeSceneAssetDto {
            scene_id,
            title: title.to_string(),
            description: req.description.trim().to_string(),
            kind: req.kind,
            tags: req.tags.clone(),
            author_user_id: inner.user_id,
            author_username: inner.username.clone(),
            creator_bot_id: req.creator_bot_id,
            creator_bot_username: req.creator_bot_username.clone(),
            remix_parent_id: req.remix_parent_id.clone(),
            source_code: req.source_code.clone(),
            code_format: if req.code_format.trim().is_empty() {
                CODE_SCENE_FORMAT_CS1.to_string()
            } else {
                req.code_format.clone()
            },
            manifest,
            controls_hint: req.controls_hint.clone(),
            plays_count: 0,
            likes_count: 0,
            bookmarks_count: 0,
            comments_count: 0,
            shares_count: 0,
            remixes_count: 0,
            viewer_liked: false,
            viewer_bookmarked: false,
            viewer_following_author: false,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        asset.tags = asset.normalized_tags();

        inner
            .recommender
            .register_verified_asset(asset.clone(), &verified, Some(inner.user_id));
        if let Some(ref parent_id) = asset.remix_parent_id {
            if let Some(parent) = inner.recommender.get_asset(parent_id) {
                let next_rmx = parent.remixes_count.saturating_add(1);
                inner.recommender.update_social_counters(
                    parent_id,
                    None,
                    None,
                    None,
                    None,
                    Some(next_rmx),
                    None,
                );
                if let Some(updated_parent) = inner.recommender.get_asset(parent_id) {
                    let p_local = self
                        .local_store
                        .get_cached_code_scene(parent_id)
                        .is_some_and(|r| r.is_local_only);
                    self.local_store
                        .upsert_cached_code_scene(updated_parent, p_local);
                }
            }
        }
        self.local_store
            .upsert_cached_code_scene(asset.clone(), true);

        Ok(PublishCodeSceneResponse {
            asset,
            validation: report,
            posted_chat_message_id: None,
        })
    }

    pub async fn publish_scene(
        &self,
        req: &PublishCodeSceneRequest,
        token: Option<&str>,
    ) -> Result<PublishCodeSceneResponse> {
        let mut manifest = req.manifest.clone();
        manifest.kind = req.kind;
        manifest.clamp_in_place();
        let local_report = validate_to_report(&req.source_code, &manifest);
        if !local_report.ok {
            let msg = local_report
                .diagnostics
                .first()
                .map(|d| format!("[{}] {}", d.code, d.message))
                .unwrap_or_else(|| "Code Scene validation failed".into());
            return Err(TelegramError::Validation(msg));
        }

        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let remote = self.remote.read().clone();
        if let (Some(client), Some(tok)) = (remote, token.filter(|t| !t.trim().is_empty())) {
            match client.publish_asset(tok, req).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        let mut inner = self.inner.write();
                        let uid = inner.user_id;
                        inner
                            .recommender
                            .register_or_update_asset(resp.asset.clone(), Some(uid));
                        self.local_store
                            .upsert_cached_code_scene(resp.asset.clone(), false);
                    }
                    return Ok(resp);
                }
                Err(err) if !is_offline_transport_error(&err) => {
                    return Err(TelegramError::from(err));
                }
                Err(_) => {}
            }
        }
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Account scope changed during request".into(),
            ));
        }
        self.publish_scene_local(req)
    }

    pub fn ai_generate_local(&self, req: &AiGenerateSceneRequest) -> LocalCodeSceneDraft {
        let now_ms = Utc::now().timestamp_millis();
        let draft_id = format!("local_draft_{}", Uuid::new_v4().simple());
        let (uid, synthesized) = {
            let inner = self.inner.read();
            let synth_req = SynthRequest::from(req);
            (inner.user_id, inner.synthesizer.synthesize(&synth_req))
        };
        let bot_id = req.bot_id.or(Some(OFFICIAL_FABUSHI_BOT_ID));
        let bot_username = req
            .bot_username
            .clone()
            .or_else(|| Some("fabushi_ai_bot".to_string()));
        let draft_resp = synthesized.into_ai_generate_response(
            draft_id.clone(),
            bot_id,
            bot_username,
            0,
        );
        let rec = LocalCodeSceneDraft {
            draft_id,
            user_id: uid,
            prompt: req.prompt.clone(),
            remix_parent_id: None,
            draft: draft_resp,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        self.local_store.upsert_code_scene_draft(rec.clone());
        rec
    }

    pub async fn ai_generate(
        &self,
        req: &AiGenerateSceneRequest,
        token: Option<&str>,
    ) -> Result<AiGenerateSceneResponse> {
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let remote = self.remote.read().clone();
        if let (Some(client), Some(tok)) = (remote, token.filter(|t| !t.trim().is_empty())) {
            match client.ai_generate(tok, req).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        let uid = self.inner.read().user_id;
                        let now_ms = Utc::now().timestamp_millis();
                        self.local_store.upsert_code_scene_draft(LocalCodeSceneDraft {
                            draft_id: resp.draft_id.clone(),
                            user_id: uid,
                            prompt: req.prompt.clone(),
                            remix_parent_id: None,
                            draft: resp.clone(),
                            created_at_ms: now_ms,
                            updated_at_ms: now_ms,
                        });
                    }
                    return Ok(resp);
                }
                Err(err) if !is_offline_transport_error(&err) => {
                    return Err(TelegramError::from(err));
                }
                Err(_) => {}
            }
        }
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Account scope changed during request".into(),
            ));
        }
        Ok(self.ai_generate_local(req).draft)
    }

    pub fn ai_iterate_local(&self, req: &AiIterateSceneRequest) -> Result<LocalCodeSceneDraft> {
        let instruction = req.instruction.trim();
        if instruction.is_empty() {
            return Err(TelegramError::Validation("instruction cannot be empty".into()));
        }
        let uid = self.inner.read().user_id;
        let existing = req
            .draft_id
            .as_deref()
            .and_then(|id| self.local_store.get_code_scene_draft(id));
        let (base_src, mut base_manifest, base_title, base_tags, draft_id, remix_parent_id, created_at_ms) =
            if let Some(d) = existing {
                (
                    req.source_code
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(d.draft.source_code),
                    req.manifest.clone().unwrap_or(d.draft.manifest),
                    Some(d.draft.title),
                    d.draft.tags,
                    d.draft_id,
                    d.remix_parent_id,
                    d.created_at_ms,
                )
            } else if let Some(src) = req.source_code.as_deref().filter(|s| !s.trim().is_empty()) {
                (
                    src.to_string(),
                    req.manifest
                        .clone()
                        .unwrap_or_else(|| AssetManifest::for_kind(AssetKind::Animation)),
                    None,
                    Vec::new(),
                    format!("local_draft_{}", Uuid::new_v4().simple()),
                    None,
                    Utc::now().timestamp_millis(),
                )
            } else {
                return Err(TelegramError::Validation(
                    "ai_iterate_local requires 'draft_id' or 'source_code'".into(),
                ));
            };
        base_manifest.clamp_in_place();
        let synthesized = self.inner.read().synthesizer.remix_source(
            &base_src,
            &base_manifest,
            instruction,
            base_title.as_deref(),
            &base_tags,
        );
        let now_ms = Utc::now().timestamp_millis();
        let bot_id = req.bot_id.or(Some(OFFICIAL_FABUSHI_BOT_ID));
        let bot_username = req
            .bot_username
            .clone()
            .or_else(|| Some("fabushi_ai_bot".to_string()));
        let draft_resp = synthesized.into_ai_generate_response(
            draft_id.clone(),
            bot_id,
            bot_username,
            0,
        );
        let rec = LocalCodeSceneDraft {
            draft_id,
            user_id: uid,
            prompt: instruction.to_string(),
            remix_parent_id,
            draft: draft_resp,
            created_at_ms,
            updated_at_ms: now_ms,
        };
        self.local_store.upsert_code_scene_draft(rec.clone());
        Ok(rec)
    }

    pub async fn ai_iterate(
        &self,
        req: &AiIterateSceneRequest,
        token: Option<&str>,
    ) -> Result<AiGenerateSceneResponse> {
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let remote = self.remote.read().clone();
        if let (Some(client), Some(tok)) = (remote, token.filter(|t| !t.trim().is_empty())) {
            match client.ai_iterate(tok, req).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        let uid = self.inner.read().user_id;
                        let now_ms = Utc::now().timestamp_millis();
                        let prev = self.local_store.get_code_scene_draft(&resp.draft_id);
                        self.local_store.upsert_code_scene_draft(LocalCodeSceneDraft {
                            draft_id: resp.draft_id.clone(),
                            user_id: uid,
                            prompt: req.instruction.clone(),
                            remix_parent_id: prev.as_ref().and_then(|d| d.remix_parent_id.clone()),
                            draft: resp.clone(),
                            created_at_ms: prev.map(|d| d.created_at_ms).unwrap_or(now_ms),
                            updated_at_ms: now_ms,
                        });
                    }
                    return Ok(resp);
                }
                Err(err) if !is_offline_transport_error(&err) => {
                    return Err(TelegramError::from(err));
                }
                Err(_) => {}
            }
        }
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Account scope changed during request".into(),
            ));
        }
        Ok(self.ai_iterate_local(req)?.draft)
    }

    pub fn ai_remix_local(&self, req: &RemixSceneRequest) -> Result<RemixSceneResponse> {
        let parent = self
            .get_asset(&req.parent_scene_id)
            .ok_or_else(|| {
                TelegramError::NotFound(format!("Parent scene '{}' not found", req.parent_scene_id))
            })?;
        let prompt = req.prompt.trim();
        if prompt.is_empty() {
            return Err(TelegramError::Validation("prompt cannot be empty".into()));
        }
        let (uid, synthesized) = {
            let inner = self.inner.read();
            (
                inner.user_id,
                inner.synthesizer.remix_source(
                    &parent.source_code,
                    &parent.manifest,
                    prompt,
                    Some(&parent.title),
                    &parent.tags,
                ),
            )
        };
        let now_ms = Utc::now().timestamp_millis();
        let draft_id = format!("local_draft_{}", Uuid::new_v4().simple());
        let bot_id = req.bot_id.or(Some(OFFICIAL_FABUSHI_BOT_ID));
        let bot_username = req
            .bot_username
            .clone()
            .or_else(|| Some("fabushi_ai_bot".to_string()));
        let draft_resp = synthesized.into_ai_generate_response(
            draft_id.clone(),
            bot_id,
            bot_username.clone(),
            0,
        );
        self.local_store.upsert_code_scene_draft(LocalCodeSceneDraft {
            draft_id: draft_id.clone(),
            user_id: uid,
            prompt: prompt.to_string(),
            remix_parent_id: Some(parent.scene_id.clone()),
            draft: draft_resp.clone(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        });

        let published = if req.auto_publish {
            let pub_req = PublishCodeSceneRequest {
                scene_id: None,
                title: draft_resp.title.clone(),
                description: draft_resp.description.clone(),
                kind: draft_resp.kind,
                tags: draft_resp.tags.clone(),
                source_code: draft_resp.source_code.clone(),
                code_format: draft_resp.code_format.clone(),
                manifest: draft_resp.manifest.clone(),
                controls_hint: draft_resp.controls_hint.clone(),
                creator_bot_id: bot_id,
                creator_bot_username: bot_username,
                remix_parent_id: Some(parent.scene_id),
                share_to_chat_id: req.chat_id,
            };
            let res = self.publish_scene_local(&pub_req)?;
            self.local_store.delete_code_scene_draft(&draft_id);
            Some(res)
        } else {
            None
        };

        Ok(RemixSceneResponse {
            draft: draft_resp,
            published,
        })
    }

    pub async fn ai_remix(
        &self,
        req: &RemixSceneRequest,
        token: Option<&str>,
    ) -> Result<RemixSceneResponse> {
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let remote = self.remote.read().clone();
        if let (Some(client), Some(tok)) = (remote, token.filter(|t| !t.trim().is_empty())) {
            match client.ai_remix(tok, req).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        if let Some(ref published) = resp.published {
                            let mut inner = self.inner.write();
                            let uid = inner.user_id;
                            inner
                                .recommender
                                .register_or_update_asset(published.asset.clone(), Some(uid));
                            self.local_store
                                .upsert_cached_code_scene(published.asset.clone(), false);
                        } else {
                            let uid = self.inner.read().user_id;
                            let now_ms = Utc::now().timestamp_millis();
                            self.local_store.upsert_code_scene_draft(LocalCodeSceneDraft {
                                draft_id: resp.draft.draft_id.clone(),
                                user_id: uid,
                                prompt: req.prompt.clone(),
                                remix_parent_id: Some(req.parent_scene_id.clone()),
                                draft: resp.draft.clone(),
                                created_at_ms: now_ms,
                                updated_at_ms: now_ms,
                            });
                        }
                    }
                    return Ok(resp);
                }
                Err(err) if !is_offline_transport_error(&err) => {
                    return Err(TelegramError::from(err));
                }
                Err(_) => {}
            }
        }
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Account scope changed during request".into(),
            ));
        }
        self.ai_remix_local(req)
    }

    pub async fn ai_publish(
        &self,
        req: &AiPublishSceneRequest,
        token: Option<&str>,
    ) -> Result<PublishCodeSceneResponse> {
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let remote = self.remote.read().clone();
        if let (Some(client), Some(tok)) = (remote, token.filter(|t| !t.trim().is_empty())) {
            match client.ai_publish(tok, req).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        if let Some(ref did) = req.draft_id {
                            self.local_store.delete_code_scene_draft(did);
                        }
                        let mut inner = self.inner.write();
                        let uid = inner.user_id;
                        inner
                            .recommender
                            .register_or_update_asset(resp.asset.clone(), Some(uid));
                        self.local_store
                            .upsert_cached_code_scene(resp.asset.clone(), false);
                    }
                    return Ok(resp);
                }
                Err(err) if !is_offline_transport_error(&err) => {
                    return Err(TelegramError::from(err));
                }
                Err(_) => {}
            }
        }
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Account scope changed during request".into(),
            ));
        }

        let loaded = req
            .draft_id
            .as_deref()
            .and_then(|id| self.local_store.get_code_scene_draft(id));
        let (source_code, manifest, title, description, tags, controls_hint, bot_id, bot_username, remix_parent_id) =
            if let Some(d) = loaded {
                (
                    req.source_code
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(d.draft.source_code),
                    req.manifest.clone().unwrap_or(d.draft.manifest),
                    req.title
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(d.draft.title),
                    req.description.clone().unwrap_or(d.draft.description),
                    req.tags.clone().unwrap_or(d.draft.tags),
                    req.controls_hint.clone().unwrap_or(d.draft.controls_hint),
                    req.bot_id.or(d.draft.bot_id),
                    req.bot_username.clone().or(d.draft.bot_username),
                    req.remix_parent_id.clone().or(d.remix_parent_id),
                )
            } else if let Some(src) = req.source_code.as_deref().filter(|s| !s.trim().is_empty()) {
                let man = req
                    .manifest
                    .clone()
                    .unwrap_or_else(|| AssetManifest::for_kind(AssetKind::Animation));
                (
                    src.to_string(),
                    man,
                    req.title
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| "AI Code Scene".to_string()),
                    req.description.clone().unwrap_or_default(),
                    req.tags.clone().unwrap_or_default(),
                    req.controls_hint.clone().unwrap_or_default(),
                    req.bot_id,
                    req.bot_username.clone(),
                    req.remix_parent_id.clone(),
                )
            } else {
                return Err(TelegramError::Validation(
                    "ai_publish requires 'draft_id' or 'source_code'".into(),
                ));
            };

        let pub_req = PublishCodeSceneRequest {
            scene_id: None,
            title,
            description,
            kind: manifest.kind,
            tags,
            source_code,
            code_format: CODE_SCENE_FORMAT_CS1.to_string(),
            manifest,
            controls_hint,
            creator_bot_id: bot_id,
            creator_bot_username: bot_username,
            remix_parent_id,
            share_to_chat_id: req.chat_id,
        };
        let res = self.publish_scene_local(&pub_req)?;
        if let Some(ref did) = req.draft_id {
            self.local_store.delete_code_scene_draft(did);
        }
        Ok(res)
    }

    pub fn list_drafts(&self) -> Vec<LocalCodeSceneDraft> {
        let uid = self.inner.read().user_id;
        self.local_store.list_code_scene_drafts(uid)
    }

    pub fn get_draft(&self, draft_id: &str) -> Option<LocalCodeSceneDraft> {
        self.local_store.get_code_scene_draft(draft_id)
    }

    pub fn delete_draft(&self, draft_id: &str) {
        self.local_store.delete_code_scene_draft(draft_id);
    }

    /// Drains queued telemetry events in bounded batches of 50 ordered by `outbox_id ASC`.
    /// Deletes strictly acked `outbox_id`s only, preserves rows with incremented `retry_count` on failure,
    /// and aborts cleanly without mutating state if an account switch occurs mid-flight.
    pub async fn flush_telemetry_outbox(
        &self,
        token: &str,
    ) -> std::result::Result<TelemetryFlushReport, HttpClientError> {
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let client = self.remote.read().clone().ok_or_else(|| {
            HttpClientError::Api {
                status: 503,
                message: "Remote Code Scene client not configured".into(),
            }
        })?;
        let uid = self.inner.read().user_id;
        let mut report = TelemetryFlushReport::default();

        loop {
            if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
                break;
            }
            let batch = self
                .local_store
                .list_code_scene_telemetry_outbox(uid, TELEMETRY_FLUSH_BATCH_SIZE);
            if batch.is_empty() {
                break;
            }
            let batch_ids: Vec<i64> = batch.iter().map(|r| r.outbox_id).collect();
            let batch_events: Vec<SceneBehaviorEventDto> =
                batch.into_iter().map(|r| r.event).collect();
            let batch_len = batch_events.len();

            match client.submit_telemetry_batch(token, batch_events).await {
                Ok(_) => {
                    if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
                        break;
                    }
                    self.local_store
                        .ack_code_scene_telemetry_outbox(&batch_ids);
                    report.batches_sent += 1;
                    report.events_acked += batch_len;
                }
                Err(err) => {
                    if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
                        let err_str = err.to_string();
                        self.local_store
                            .mark_code_scene_telemetry_outbox_failed(&batch_ids, &err_str);
                        report.errors.push(err_str);
                        report.remaining_pending =
                            self.local_store.pending_code_scene_telemetry_count(uid);
                    }
                    return Err(err);
                }
            }
        }

        if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
            report.remaining_pending = self.local_store.pending_code_scene_telemetry_count(uid);
        }
        Ok(report)
    }

    /// 6-step deterministic convergence sync between local offline cache/outbox and live server.
    /// Fenced by `account_epoch` after every `.await` and immune to double-toggle bugs.
    pub async fn sync_with_server(
        &self,
        token: &str,
    ) -> std::result::Result<CodeSceneSyncReport, HttpClientError> {
        let start_epoch = self.account_epoch.load(Ordering::SeqCst);
        let client = self.remote.read().clone().ok_or_else(|| {
            HttpClientError::Api {
                status: 503,
                message: "Remote Code Scene client not configured".into(),
            }
        })?;
        let uid = self.inner.read().user_id;
        let mut report = CodeSceneSyncReport::default();
        let mut remapped_scene_ids: BTreeMap<String, String> = BTreeMap::new();

        // Step 1: Push local-only published scenes (`is_local_only = true`) in topological
        // parent-first order so offline parents are always created before their child remixes.
        let local_scenes =
            sort_local_scenes_parent_first(self.local_store.list_local_only_code_scenes());
        for rec in local_scenes {
            if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
                return Ok(report);
            }
            let old_id = rec.asset.scene_id.clone();
            let remapped_parent_id = rec.asset.remix_parent_id.as_ref().map(|pid| {
                remapped_scene_ids
                    .get(pid)
                    .cloned()
                    .unwrap_or_else(|| pid.clone())
            });
            let req = PublishCodeSceneRequest {
                scene_id: Some(old_id.clone()),
                title: rec.asset.title.clone(),
                description: rec.asset.description.clone(),
                kind: rec.asset.kind,
                tags: rec.asset.tags.clone(),
                source_code: rec.asset.source_code.clone(),
                code_format: rec.asset.code_format.clone(),
                manifest: rec.asset.manifest.clone(),
                controls_hint: rec.asset.controls_hint.clone(),
                creator_bot_id: rec.asset.creator_bot_id,
                creator_bot_username: rec.asset.creator_bot_username.clone(),
                remix_parent_id: remapped_parent_id,
                share_to_chat_id: None,
            };
            match client.publish_asset(token, &req).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
                        return Ok(report);
                    }
                    let new_id = resp.asset.scene_id.clone();
                    if new_id != old_id {
                        let remap_ts = Utc::now().timestamp_millis();
                        remapped_scene_ids.insert(old_id.clone(), new_id.clone());
                        self.local_store.delete_cached_code_scene(&old_id);

                        // Migrate pending offline comments referencing `old_id` -> `new_id`.
                        for mut pending in self.local_store.list_pending_code_scene_comments(uid) {
                            if pending.comment.scene_id == old_id {
                                let cid = pending.comment.comment_id.clone();
                                self.local_store.delete_code_scene_comment(&old_id, &cid);
                                pending.comment.scene_id = new_id.clone();
                                self.local_store.upsert_code_scene_comment(pending);
                            }
                        }

                        // Migrate `Like`/`Bookmark`/`Dislike` social rows targeting `old_id` ->
                        // `new_id` and clear the stale `old_id` row so future syncs/reopens never
                        // re-fire `old_id`.
                        for entry in self.local_store.list_code_scene_social(uid) {
                            if matches!(
                                entry.relation_kind,
                                CodeSceneSocialRelationKind::Like
                                    | CodeSceneSocialRelationKind::Bookmark
                                    | CodeSceneSocialRelationKind::Dislike
                            ) && entry.target_id == old_id
                            {
                                self.local_store.upsert_code_scene_social(
                                    CodeSceneLocalSocialEntry {
                                        target_id: new_id.clone(),
                                        updated_at_ms: remap_ts,
                                        ..entry.clone()
                                    },
                                );
                                self.local_store.upsert_code_scene_social(
                                    CodeSceneLocalSocialEntry {
                                        active: false,
                                        pending_sync: false,
                                        updated_at_ms: remap_ts,
                                        ..entry
                                    },
                                );
                            }
                        }

                        // Persist remapped `remix_parent_id` on remaining local-only child scenes
                        // and drafts so a transient error + restart still pushes with `new_id`.
                        for mut child_rec in self.local_store.list_local_only_code_scenes() {
                            if child_rec.asset.remix_parent_id.as_deref() == Some(old_id.as_str()) {
                                child_rec.asset.remix_parent_id = Some(new_id.clone());
                                self.local_store
                                    .upsert_cached_code_scene(child_rec.asset.clone(), true);
                                self.inner
                                    .write()
                                    .recommender
                                    .register_or_update_asset(child_rec.asset, Some(uid));
                            }
                        }
                        for mut draft in self.local_store.list_code_scene_drafts(uid) {
                            if draft.remix_parent_id.as_deref() == Some(old_id.as_str()) {
                                draft.remix_parent_id = Some(new_id.clone());
                                draft.updated_at_ms = remap_ts;
                                self.local_store.upsert_code_scene_draft(draft);
                            }
                        }
                    }

                    self.local_store
                        .upsert_cached_code_scene(resp.asset.clone(), false);
                    {
                        let mut inner = self.inner.write();
                        if new_id != old_id {
                            inner.recommender.remove_asset(&old_id);
                        }
                        inner
                            .recommender
                            .register_or_update_asset(resp.asset, Some(uid));
                        if new_id != old_id {
                            if let Some(mut profile) = inner.recommender.export_user_profile(uid) {
                                let mut changed = false;
                                if profile.liked_scene_ids.remove(&old_id) {
                                    profile.liked_scene_ids.insert(new_id.clone());
                                    changed = true;
                                }
                                if profile.bookmarked_scene_ids.remove(&old_id) {
                                    profile.bookmarked_scene_ids.insert(new_id.clone());
                                    changed = true;
                                }
                                if profile.disliked_scene_ids.remove(&old_id) {
                                    profile.disliked_scene_ids.insert(new_id.clone());
                                    changed = true;
                                }
                                for sid in &mut profile.positive_scene_history {
                                    if *sid == old_id {
                                        *sid = new_id.clone();
                                        changed = true;
                                    }
                                }
                                for seen in &mut profile.seen_ring {
                                    if seen.scene_id == old_id {
                                        seen.scene_id = new_id.clone();
                                        changed = true;
                                    }
                                }
                                if changed {
                                    profile.sanitize();
                                    inner.recommender.import_user_profile(profile.clone());
                                    self.local_store.upsert_code_scene_profile(profile);
                                }
                            }
                        }
                    }
                    report.pushed_assets += 1;
                }
                Err(err) => {
                    if matches!(err, HttpClientError::Unauthorized(_)) {
                        return Err(err);
                    }
                    report.errors.push(format!("push asset {old_id}: {err}"));
                }
            }
        }

        // Step 2: Push pending offline comments in topological parent-first order, remapping both
        // `scene_id` and `parent_comment_id` as local comment IDs receive real server IDs.
        let mut remapped_comment_ids: BTreeMap<String, String> = BTreeMap::new();
        let pending_comments = sort_pending_comments_parent_first(
            self.local_store.list_pending_code_scene_comments(uid),
        );
        for rec in pending_comments {
            if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
                return Ok(report);
            }
            let old_comment_id = rec.comment.comment_id.clone();
            let target_scene_id = remapped_scene_ids
                .get(&rec.comment.scene_id)
                .cloned()
                .unwrap_or_else(|| rec.comment.scene_id.clone());
            let parent_comment_id = rec.comment.parent_comment_id.as_ref().map(|pid| {
                remapped_comment_ids
                    .get(pid)
                    .cloned()
                    .unwrap_or_else(|| pid.clone())
            });
            let req = CreateCodeSceneCommentRequest {
                content: rec.comment.content.clone(),
                parent_comment_id,
            };
            match client.add_comment(token, &target_scene_id, &req).await {
                Ok(resp) => {
                    if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
                        return Ok(report);
                    }
                    let new_comment_id = resp.comment.comment_id.clone();
                    self.local_store
                        .delete_code_scene_comment(&rec.comment.scene_id, &old_comment_id);
                    if target_scene_id != rec.comment.scene_id {
                        self.local_store
                            .delete_code_scene_comment(&target_scene_id, &old_comment_id);
                    }
                    self.local_store
                        .upsert_code_scene_comment(CachedCodeSceneComment {
                            comment: resp.comment,
                            pending_sync: false,
                        });
                    if new_comment_id != old_comment_id {
                        remapped_comment_ids
                            .insert(old_comment_id.clone(), new_comment_id.clone());
                        // Persist remapped parent_comment_id on any remaining pending replies so
                        // a transient failure on a child still succeeds on the next sync retry.
                        for mut pending in self.local_store.list_pending_code_scene_comments(uid) {
                            if pending.comment.parent_comment_id.as_deref()
                                == Some(old_comment_id.as_str())
                            {
                                pending.comment.parent_comment_id = Some(new_comment_id.clone());
                                self.local_store.upsert_code_scene_comment(pending);
                            }
                        }
                    }
                    self.inner.write().recommender.update_social_counters(
                        &target_scene_id,
                        None,
                        None,
                        Some(resp.comments_count),
                        None,
                        None,
                        None,
                    );
                    report.pushed_comments += 1;
                }
                Err(err) => {
                    if matches!(err, HttpClientError::Unauthorized(_)) {
                        return Err(err);
                    }
                    report
                        .errors
                        .push(format!("push comment {}: {err}", old_comment_id));
                }
            }
        }

        // Step 3: Flush telemetry outbox
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Ok(report);
        }
        match self.flush_telemetry_outbox(token).await {
            Ok(tel_rep) => report.telemetry = tel_rep,
            Err(err) => {
                if matches!(err, HttpClientError::Unauthorized(_)) {
                    return Err(err);
                }
                report.telemetry.errors.push(err.to_string());
                report.errors.push(format!("flush telemetry: {err}"));
            }
        }

        // Step 4: Pull server catalog (`GET /api/v1/code-scene/assets?limit=200&offset=0`)
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Ok(report);
        }
        let pulled = client
            .list_assets(
                token,
                &ListCodeSceneAssetsQuery {
                    limit: Some(200),
                    offset: Some(0),
                    ..Default::default()
                },
            )
            .await?;
        if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
            return Ok(report);
        }

        // Collect pending local social overrides & existing stored social rows BEFORE Step 4
        // so server-authoritative viewer flags hydrate cleanly without clobbering unsynced offline ops.
        let existing_social = self.local_store.list_code_scene_social(uid);
        let mut pending_social_keys: HashSet<(CodeSceneSocialRelationKind, String)> = HashSet::new();
        let mut stored_social_active: HashMap<(CodeSceneSocialRelationKind, String), bool> =
            HashMap::new();
        for entry in existing_social {
            if matches!(
                entry.relation_kind,
                CodeSceneSocialRelationKind::Like | CodeSceneSocialRelationKind::Bookmark
            ) && remapped_scene_ids.contains_key(&entry.target_id)
                && !entry.pending_sync
                && !entry.active
            {
                continue;
            }
            let mapped_target = match entry.relation_kind {
                CodeSceneSocialRelationKind::Like | CodeSceneSocialRelationKind::Bookmark => {
                    remapped_scene_ids
                        .get(&entry.target_id)
                        .cloned()
                        .unwrap_or(entry.target_id)
                }
                _ => entry.target_id,
            };
            if entry.pending_sync {
                pending_social_keys.insert((entry.relation_kind, mapped_target.clone()));
            }
            stored_social_active.insert((entry.relation_kind, mapped_target), entry.active);
        }

        let now_ms = Utc::now().timestamp_millis();
        let mut server_assets_by_id: BTreeMap<String, CodeSceneAssetDto> = BTreeMap::new();
        let mut server_follow_by_author: BTreeMap<i64, bool> = BTreeMap::new();
        {
            let mut inner = self.inner.write();
            let mut profile = inner
                .recommender
                .export_user_profile(uid)
                .unwrap_or_else(|| UserInterestProfile::new(uid));
            let mut profile_changed = false;

            for asset in pulled.assets {
                server_follow_by_author.insert(asset.author_user_id, asset.viewer_following_author);
                server_assets_by_id.insert(asset.scene_id.clone(), asset.clone());

                let like_key = (CodeSceneSocialRelationKind::Like, asset.scene_id.clone());
                if !pending_social_keys.contains(&like_key) {
                    if asset.viewer_liked {
                        if profile.liked_scene_ids.insert(asset.scene_id.clone()) {
                            profile_changed = true;
                        }
                    } else if profile.liked_scene_ids.remove(&asset.scene_id) {
                        profile_changed = true;
                    }
                    if asset.viewer_liked
                        || stored_social_active.get(&like_key).copied() == Some(true)
                    {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: uid,
                                relation_kind: CodeSceneSocialRelationKind::Like,
                                target_id: asset.scene_id.clone(),
                                active: asset.viewer_liked,
                                pending_sync: false,
                                updated_at_ms: now_ms,
                            });
                    }
                }

                let bm_key = (
                    CodeSceneSocialRelationKind::Bookmark,
                    asset.scene_id.clone(),
                );
                if !pending_social_keys.contains(&bm_key) {
                    if asset.viewer_bookmarked {
                        if profile.bookmarked_scene_ids.insert(asset.scene_id.clone()) {
                            profile_changed = true;
                        }
                    } else if profile.bookmarked_scene_ids.remove(&asset.scene_id) {
                        profile_changed = true;
                    }
                    if asset.viewer_bookmarked
                        || stored_social_active.get(&bm_key).copied() == Some(true)
                    {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: uid,
                                relation_kind: CodeSceneSocialRelationKind::Bookmark,
                                target_id: asset.scene_id.clone(),
                                active: asset.viewer_bookmarked,
                                pending_sync: false,
                                updated_at_ms: now_ms,
                            });
                    }
                }

                self.local_store
                    .upsert_cached_code_scene(asset.clone(), false);
                inner.recommender.register_or_update_asset(asset, None);
                report.pulled_assets += 1;
            }

            // Collect authors with un-acked `HideAuthor` events still queued in telemetry outbox
            // (e.g., when Step 3 transiently failed with 503) so stale server `following == true`
            // never wipes local `hidden_author_ids` before Step 3 delivers the hide.
            let mut pending_hidden_authors: HashSet<i64> = HashSet::new();
            for rec in self
                .local_store
                .list_code_scene_telemetry_outbox(uid, usize::MAX)
            {
                if rec.event.kind == SceneBehaviorEventKind::HideAuthor {
                    if let Some(cid) = resolve_event_author_id(&inner.recommender, &rec.event) {
                        pending_hidden_authors.insert(cid);
                    }
                }
            }

            for (&creator_uid, &server_following) in &server_follow_by_author {
                let follow_key = (
                    CodeSceneSocialRelationKind::Follow,
                    creator_uid.to_string(),
                );
                if !pending_social_keys.contains(&follow_key)
                    && !(server_following && pending_hidden_authors.contains(&creator_uid))
                {
                    if server_following {
                        if profile.hidden_author_ids.remove(&creator_uid) {
                            profile_changed = true;
                        }
                        if profile
                            .creator_weights
                            .get(&creator_uid)
                            .copied()
                            .unwrap_or(0.0)
                            < 0.0
                        {
                            profile.creator_weights.remove(&creator_uid);
                            profile_changed = true;
                        }
                        if profile.followed_creators.insert(creator_uid) {
                            profile_changed = true;
                        }
                        // Persist HideAuthor(active=false) so drop+reopen never resurrects a prior hide.
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: uid,
                                relation_kind: CodeSceneSocialRelationKind::HideAuthor,
                                target_id: creator_uid.to_string(),
                                active: false,
                                pending_sync: false,
                                updated_at_ms: now_ms,
                            });
                    } else if profile.followed_creators.remove(&creator_uid) {
                        profile_changed = true;
                    }
                    if server_following
                        || stored_social_active.get(&follow_key).copied() == Some(true)
                    {
                        self.local_store
                            .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                user_id: uid,
                                relation_kind: CodeSceneSocialRelationKind::Follow,
                                target_id: creator_uid.to_string(),
                                active: server_following,
                                pending_sync: false,
                                updated_at_ms: now_ms,
                            });
                    }
                }
            }

            if profile_changed {
                profile.sanitize();
                inner.recommender.import_user_profile(profile.clone());
                self.local_store.upsert_code_scene_profile(profile);
            }
        }

        // Step 5: Reconcile `pending_sync = true` social rows WITHOUT double-toggling!
        let pending_social = self.local_store.list_pending_code_scene_social(uid);
        for entry in pending_social {
            if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
                return Ok(report);
            }
            match entry.relation_kind {
                CodeSceneSocialRelationKind::Like => {
                    let target_id = remapped_scene_ids
                        .get(&entry.target_id)
                        .cloned()
                        .unwrap_or_else(|| entry.target_id.clone());
                    let server_liked = server_assets_by_id
                        .get(&target_id)
                        .map(|a| a.viewer_liked)
                        .unwrap_or(false);
                    if server_liked != entry.active {
                        match client.toggle_like(token, &target_id).await {
                            Ok(resp) => {
                                if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
                                    return Ok(report);
                                }
                                let _ = self.apply_like_state(
                                    &target_id,
                                    Some(resp.liked),
                                    Some(resp.likes_count),
                                    false,
                                );
                                if target_id != entry.target_id {
                                    self.local_store.upsert_code_scene_social(
                                        CodeSceneLocalSocialEntry {
                                            active: false,
                                            pending_sync: false,
                                            updated_at_ms: now_ms,
                                            ..entry
                                        },
                                    );
                                }
                                report.reconciled_social += 1;
                            }
                            Err(err) => {
                                if matches!(err, HttpClientError::Unauthorized(_)) {
                                    return Err(err);
                                }
                                report.errors.push(format!("reconcile like {target_id}: {err}"));
                            }
                        }
                    } else {
                        if target_id != entry.target_id {
                            self.local_store
                                .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                    target_id: target_id.clone(),
                                    pending_sync: false,
                                    updated_at_ms: now_ms,
                                    ..entry.clone()
                                });
                            self.local_store
                                .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                    active: false,
                                    pending_sync: false,
                                    updated_at_ms: now_ms,
                                    ..entry
                                });
                        } else {
                            self.local_store
                                .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                    pending_sync: false,
                                    updated_at_ms: now_ms,
                                    ..entry
                                });
                        }
                        report.reconciled_social += 1;
                    }
                }
                CodeSceneSocialRelationKind::Bookmark => {
                    let target_id = remapped_scene_ids
                        .get(&entry.target_id)
                        .cloned()
                        .unwrap_or_else(|| entry.target_id.clone());
                    let server_bm = server_assets_by_id
                        .get(&target_id)
                        .map(|a| a.viewer_bookmarked)
                        .unwrap_or(false);
                    if server_bm != entry.active {
                        match client.toggle_bookmark(token, &target_id).await {
                            Ok(resp) => {
                                if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
                                    return Ok(report);
                                }
                                let _ = self.apply_bookmark_state(
                                    &target_id,
                                    Some(resp.bookmarked),
                                    Some(resp.bookmarks_count),
                                    false,
                                );
                                if target_id != entry.target_id {
                                    self.local_store.upsert_code_scene_social(
                                        CodeSceneLocalSocialEntry {
                                            active: false,
                                            pending_sync: false,
                                            updated_at_ms: now_ms,
                                            ..entry
                                        },
                                    );
                                }
                                report.reconciled_social += 1;
                            }
                            Err(err) => {
                                if matches!(err, HttpClientError::Unauthorized(_)) {
                                    return Err(err);
                                }
                                report
                                    .errors
                                    .push(format!("reconcile bookmark {target_id}: {err}"));
                            }
                        }
                    } else {
                        if target_id != entry.target_id {
                            self.local_store
                                .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                    target_id: target_id.clone(),
                                    pending_sync: false,
                                    updated_at_ms: now_ms,
                                    ..entry.clone()
                                });
                            self.local_store
                                .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                    active: false,
                                    pending_sync: false,
                                    updated_at_ms: now_ms,
                                    ..entry
                                });
                        } else {
                            self.local_store
                                .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                    pending_sync: false,
                                    updated_at_ms: now_ms,
                                    ..entry
                                });
                        }
                        report.reconciled_social += 1;
                    }
                }
                CodeSceneSocialRelationKind::Follow => {
                    if let Ok(creator_uid) = entry.target_id.parse::<i64>() {
                        let server_following = server_follow_by_author
                            .get(&creator_uid)
                            .copied()
                            .unwrap_or(false);
                        if server_following != entry.active {
                            match client.toggle_follow(token, creator_uid).await {
                                Ok(resp) => {
                                    if self.account_epoch.load(Ordering::SeqCst) != start_epoch {
                                        return Ok(report);
                                    }
                                    let _ = self.apply_follow_state(
                                        creator_uid,
                                        Some(resp.following),
                                        Some(resp.followers_count),
                                        false,
                                    );
                                    report.reconciled_social += 1;
                                }
                                Err(err) => {
                                    if matches!(err, HttpClientError::Unauthorized(_)) {
                                        return Err(err);
                                    }
                                    report
                                        .errors
                                        .push(format!("reconcile follow {creator_uid}: {err}"));
                                }
                            }
                        } else {
                            self.local_store
                                .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                                    pending_sync: false,
                                    updated_at_ms: now_ms,
                                    ..entry
                                });
                            report.reconciled_social += 1;
                        }
                    }
                }
                _ => {
                    self.local_store
                        .upsert_code_scene_social(CodeSceneLocalSocialEntry {
                            pending_sync: false,
                            updated_at_ms: now_ms,
                            ..entry
                        });
                }
            }
        }

        // Step 6: Persist reconciled UserInterestProfile
        if self.account_epoch.load(Ordering::SeqCst) == start_epoch {
            if let Some(profile) = self.inner.read().recommender.export_user_profile(uid) {
                self.local_store.upsert_code_scene_profile(profile);
            }
        }

        Ok(report)
    }
}
