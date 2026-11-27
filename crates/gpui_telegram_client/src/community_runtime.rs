//! Dual Online/Offline Account-Scoped Community Runtime & Client Facade (Feature F14).
//!
//! `TelegramClient::community()` exposes `CommunityClientFacade`, which routes calls
//! online-first to `/api/v1/community/*` and transparently falls back to an account-scoped
//! SQLite `LocalCommunityRuntime` (`<app_dir>/community.db`) on transport failures or legacy
//! unmounted HTTP `404` responses while preserving all genuine API errors (`400`, `401`, `403`,
//! `409`, `410`, `422`, `429`, `5xx`, and live domain `404`s).

mod facade_ops;
mod local_bots;
mod local_ops;

use crate::net::api_community::CommunityApiError;
use crate::net::{AuthResponse, HttpClient};
use chrono::Utc;
use gpui_telegram_community::identity::{can_user_act_as, human_actor_id, validate_act_as};
use gpui_telegram_community::store::ops;
use gpui_telegram_community::{
    BotLoopGuard, CommunityStore, TelegramBotSyncInput, TelegramHumanSyncInput,
};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_proto::community_contracts::{
    ActorKind, BotAutonomyMode, CommunityActorDto, CommunityIdentitiesResponse,
    RegisterBotActorRequest, RegisterHumanActorRequest, VerifiedTier,
};
use parking_lot::RwLock;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;

pub const LOCAL_GUEST_USER_ID: i64 = 0;
pub const LOCAL_GUEST_ACTOR_ID: &str = "u:0";
pub const COMMUNITY_DB_FILENAME: &str = "community.db";

const META_KEY_OWNER_USER_ID: &str = "owner_user_id";
const META_KEY_ACTIVE_ACTOR_ID: &str = "active_actor_id";
const META_KEY_GUEST_DIRTY: &str = "guest_dirty";
const TIMEOUT_BACKOFF_SECS: u64 = 5;

const META_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS community_client_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
"#;

/// Strict child-before-parent reverse-FK wipe across all 23 V42 community tables + meta table.
const ACCOUNT_SWITCH_WIPE_SQL: &str = r#"
DELETE FROM community_bot_cooldowns;
DELETE FROM community_algo_prefs;
DELETE FROM community_actor_affinities;
DELETE FROM community_user_signals;
DELETE FROM community_mod_logs;
DELETE FROM community_mod_queue;
DELETE FROM community_notifications;
DELETE FROM community_lists;
DELETE FROM community_awards;
DELETE FROM community_poll_votes;
DELETE FROM community_bookmarks;
DELETE FROM community_reposts;
DELETE FROM community_likes;
DELETE FROM community_votes;
UPDATE community_comments SET parent_comment_id = NULL;
DELETE FROM community_comments;
DELETE FROM community_post_edits;
DELETE FROM community_posts;
DELETE FROM community_wiki_revisions;
DELETE FROM community_memberships;
DELETE FROM communities;
DELETE FROM community_mutes_blocks;
DELETE FROM community_follows;
DELETE FROM community_actors;
DELETE FROM community_client_meta;
"#;

/// Active execution mode of `CommunityClientFacade`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunityRuntimeMode {
    Online,
    LocalFallback,
}

/// Account-scoped local SQLite Community runtime backed by `CommunityStore` (`features = ["sqlite"]`).
pub struct LocalCommunityRuntime {
    pub(crate) store: CommunityStore,
    db_path: Option<PathBuf>,
    pub(crate) bot_loop_guard: RwLock<BotLoopGuard>,
    persisted_owner_user_id: RwLock<Option<u64>>,
    pub(crate) bound_user_id: RwLock<Option<u64>>,
    pub(crate) active_actor_id: RwLock<Option<String>>,
    pub(crate) next_local_bot_id: AtomicI64,
}

pub(crate) fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

fn read_meta_conn(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM community_client_meta WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| TelegramError::Database(format!("read community_client_meta '{key}': {e}")))
}

fn write_meta_conn(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO community_client_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(|e| TelegramError::Database(format!("write community_client_meta '{key}': {e}")))?;
    Ok(())
}

fn delete_meta_conn(conn: &Connection, key: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM community_client_meta WHERE key = ?1",
        params![key],
    )
    .map_err(|e| TelegramError::Database(format!("delete community_client_meta '{key}': {e}")))?;
    Ok(())
}

fn ensure_guest_actor_conn(conn: &Connection, now_ms: i64) -> Result<()> {
    if ops::get_actor_conn(conn, LOCAL_GUEST_ACTOR_ID, None)?.is_some() {
        return Ok(());
    }
    let guest = CommunityActorDto {
        actor_id: LOCAL_GUEST_ACTOR_ID.to_string(),
        linked_user_id: Some(LOCAL_GUEST_USER_ID),
        owner_user_id: None,
        actor_kind: ActorKind::Human,
        handle: "local_citizen".to_string(),
        display_name: "Local Citizen".to_string(),
        bio: "Offline local community citizen".to_string(),
        location: None,
        website: None,
        avatar_media_id: None,
        banner_media_id: None,
        avatar_shape: "classic".to_string(),
        avatar_color: "blue".to_string(),
        verified_tier: VerifiedTier::None,
        bot_badge: None,
        bot_model: None,
        bot_capabilities: Vec::new(),
        bot_autonomy_mode: BotAutonomyMode::Manual,
        bot_persona: None,
        post_karma: 0,
        comment_karma: 0,
        awardee_karma: 0,
        followers_count: 0,
        following_count: 0,
        posts_count: 0,
        trust_score: 0.65,
        is_following: false,
        is_muted: false,
        is_blocked: false,
        created_at_ms: now_ms,
    };
    ops::upsert_actor_conn(conn, &guest, now_ms)
}

fn build_human_sync_input(
    user_id: u64,
    username: &str,
    first_name: Option<&str>,
) -> TelegramHumanSyncInput {
    let uname = username.trim();
    let display = first_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if !uname.is_empty() {
                uname.to_string()
            } else {
                format!("User {user_id}")
            }
        });
    TelegramHumanSyncInput {
        user_id: user_id as i64,
        username: (!uname.is_empty()).then(|| uname.to_string()),
        display_name: display,
        bio: None,
        avatar_media_id: None,
    }
}

impl LocalCommunityRuntime {
    /// Opens (or creates) `<dir>/community.db`, runs V42 migrations, ensures metadata & seed data.
    pub fn open_in_dir(dir: &Path) -> Result<Self> {
        let db_path = dir.join(COMMUNITY_DB_FILENAME);
        let store = CommunityStore::open(&db_path)?;
        Self::init_from_store(store, Some(db_path))
    }

    /// Opens an ephemeral RAII `TempDir` SQLite store (`CommunityStore::open_in_memory`) so
    /// in-memory `TelegramClient::new` / `new_with_url` NEVER write to `~/.config`.
    pub fn open_in_memory() -> Result<Self> {
        let store = CommunityStore::open_in_memory()?;
        Self::init_from_store(store, None)
    }

    fn init_from_store(store: CommunityStore, db_path: Option<PathBuf>) -> Result<Self> {
        store.with_conn(|conn| {
            conn.execute_batch(META_TABLE_SQL)
                .map_err(|e| TelegramError::Database(format!("create community_client_meta: {e}")))
        })?;
        store.purge_starter_seed_data()?;

        let (persisted_owner, persisted_active) = store.with_conn(|conn| {
            let owner = read_meta_conn(conn, META_KEY_OWNER_USER_ID)?
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|&uid| uid > 0);
            let active = read_meta_conn(conn, META_KEY_ACTIVE_ACTOR_ID)?
                .filter(|s| !s.trim().is_empty());
            Ok((owner, active))
        })?;

        // Only restore active_actor_id immediately on anonymous (owner-less) DBs.
        // For owned DBs, defer restoring active_actor_id until `bind_account_scope` verifies the user.
        let initial_active = if persisted_owner.is_none() {
            persisted_active
        } else {
            None
        };

        Ok(Self {
            store,
            db_path,
            bot_loop_guard: RwLock::new(BotLoopGuard::default()),
            persisted_owner_user_id: RwLock::new(persisted_owner),
            bound_user_id: RwLock::new(None),
            active_actor_id: RwLock::new(initial_active),
            next_local_bot_id: AtomicI64::new(950_000),
        })
    }

    pub fn db_path(&self) -> Option<PathBuf> {
        self.db_path.clone()
    }

    pub fn store(&self) -> &CommunityStore {
        &self.store
    }

    pub fn active_actor_id(&self) -> Option<String> {
        self.active_actor_id.read().clone()
    }

    pub fn bound_user_id(&self) -> Option<u64> {
        *self.bound_user_id.read()
    }

    pub fn persisted_owner_user_id(&self) -> Option<u64> {
        *self.persisted_owner_user_id.read()
    }

    pub fn set_active_actor_id(&self, actor_id: Option<String>) {
        let normalized = actor_id
            .map(|s| s.trim_matches(' ').to_string())
            .filter(|s| !s.is_empty());
        *self.active_actor_id.write() = normalized.clone();
        // Never overwrite persisted active_actor_id on disk while unbound on an owned DB,
        // nor persist invalid actor IDs (control chars, non-ASCII, embedded whitespace) —
        // let outbound validation reject them explicitly.
        if self.bound_user_id.read().is_none() && self.persisted_owner_user_id.read().is_some() {
            return;
        }
        if normalized.as_deref().is_some_and(|id| {
            crate::net::api_community::validate_and_normalize_actor_id(id).is_err()
        }) {
            return;
        }
        let _ = self.store.with_conn(|conn| match normalized.as_deref() {
            Some(id) => write_meta_conn(conn, META_KEY_ACTIVE_ACTOR_ID, id),
            None => delete_meta_conn(conn, META_KEY_ACTIVE_ACTOR_ID),
        });
    }

    fn wipe_and_reseed_tx(
        tx: &Transaction<'_>,
        user_id: u64,
        human_input: &TelegramHumanSyncInput,
        now_ms: i64,
    ) -> Result<String> {
        tx.execute_batch(ACCOUNT_SWITCH_WIPE_SQL)
            .map_err(|e| TelegramError::Database(format!("wipe community account scope: {e}")))?;
        ops::sync_telegram_identities_tx(tx, human_input, &[], now_ms)?;
        let human_id = human_actor_id(user_id as i64);
        write_meta_conn(tx, META_KEY_OWNER_USER_ID, &user_id.to_string())?;
        write_meta_conn(tx, META_KEY_ACTIVE_ACTOR_ID, &human_id)?;
        Ok(human_id)
    }

    /// Binds `<app_dir>/community.db` to `user_id`.
    /// - Different `user_id` (or dirty anonymous DB) -> atomic 24-table reverse-FK wipe + reseed + sync.
    /// - Same `user_id` (even across a different `server_url`!) -> preserves all offline state!
    pub fn bind_account_scope(
        &self,
        user_id: u64,
        username: &str,
        first_name: Option<&str>,
    ) -> Result<()> {
        if user_id == 0 {
            return Ok(());
        }
        let now_ms = now_millis();
        let human_input = build_human_sync_input(user_id, username, first_name);
        let human_id = human_actor_id(user_id as i64);

        let prev_owner = *self.persisted_owner_user_id.read();
        let guest_dirty = self
            .store
            .with_conn(|conn| read_meta_conn(conn, META_KEY_GUEST_DIRTY))?
            .as_deref()
            == Some("1");

        if prev_owner.is_some_and(|prev| prev != user_id) || guest_dirty {
            let active = self.store.with_transaction(|tx| {
                Self::wipe_and_reseed_tx(tx, user_id, &human_input, now_ms)
            })?;
            *self.persisted_owner_user_id.write() = Some(user_id);
            *self.bound_user_id.write() = Some(user_id);
            *self.active_actor_id.write() = Some(active);
            *self.bot_loop_guard.write() = BotLoopGuard::default();
            return Ok(());
        }

        if prev_owner == Some(user_id) {
            // Same account reopen (including across different server URLs): preserve all offline state!
            let _ = self.store.sync_telegram_identities(&human_input, &[], now_ms)?;
            let persisted_active = self
                .store
                .with_conn(|conn| read_meta_conn(conn, META_KEY_ACTIVE_ACTOR_ID))?
                .filter(|s| crate::net::api_community::validate_and_normalize_actor_id(s).is_ok());
            let resolved_active = match persisted_active {
                Some(ref cand) => match self.store.get_actor(cand, None)? {
                    Some(actor) if can_user_act_as(user_id as i64, &actor) => actor.actor_id,
                    _ => human_id.clone(),
                },
                None => human_id.clone(),
            };
            let _ = self
                .store
                .with_conn(|conn| write_meta_conn(conn, META_KEY_ACTIVE_ACTOR_ID, &resolved_active));
            *self.bound_user_id.write() = Some(user_id);
            *self.active_actor_id.write() = Some(resolved_active);
            return Ok(());
        }

        // Fresh unowned DB with zero anonymous writes: adopt cleanly without redundant reseed.
        let _ = self.store.sync_telegram_identities(&human_input, &[], now_ms)?;
        self.store.with_transaction(|tx| {
            write_meta_conn(tx, META_KEY_OWNER_USER_ID, &user_id.to_string())?;
            write_meta_conn(tx, META_KEY_ACTIVE_ACTOR_ID, &human_id)?;
            Ok(())
        })?;
        *self.persisted_owner_user_id.write() = Some(user_id);
        *self.bound_user_id.write() = Some(user_id);
        *self.active_actor_id.write() = Some(human_id);
        Ok(())
    }

    /// Clears live in-memory session state on logout while preserving `persisted_owner_user_id`
    /// on disk so a subsequent login as a different account triggers an atomic wipe.
    pub fn clear_session_state(&self) {
        *self.bound_user_id.write() = None;
        *self.active_actor_id.write() = None;
        *self.bot_loop_guard.write() = BotLoopGuard::default();
    }

    pub(crate) fn ensure_local_write_allowed(&self) -> Result<()> {
        if self.bound_user_id.read().is_none() && self.persisted_owner_user_id.read().is_some() {
            return Err(TelegramError::PermissionDenied(
                "Sign in required before mutating an account-owned community database".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn mark_guest_dirty_if_unbound(&self) {
        if self.bound_user_id.read().is_none() {
            let _ = self
                .store
                .with_conn(|conn| write_meta_conn(conn, META_KEY_GUEST_DIRTY, "1"));
        }
    }

    pub(crate) fn resolve_caller_actor_id(&self, explicit_act_as: Option<&str>) -> Result<String> {
        let explicit_candidate = match explicit_act_as {
            Some(raw) if !raw.is_empty() => {
                if raw.bytes().any(|b| b < 0x20 || b >= 0x7f) {
                    return Err(TelegramError::Validation(
                        "Invalid control or non-ASCII characters in active actor id".to_string(),
                    ));
                }
                let trimmed = raw.trim_matches(' ');
                if trimmed.is_empty() {
                    None
                } else {
                    let clean = crate::net::api_community::validate_and_normalize_actor_id(trimmed)
                        .map_err(|msg| TelegramError::Validation(msg.to_string()))?;
                    Some(clean.to_string())
                }
            }
            _ => None,
        };
        let candidate = explicit_candidate.or_else(|| self.active_actor_id.read().clone());
        let bound_uid = *self.bound_user_id.read();

        if let Some(cand) = candidate {
            let clean = crate::net::api_community::validate_and_normalize_actor_id(&cand)
                .map_err(|msg| TelegramError::Validation(msg.to_string()))?;
            let actor = self
                .store
                .get_actor(clean, None)?
                .ok_or_else(|| TelegramError::NotFound(format!("actor '{clean}' not found")))?;
            let effective_uid = bound_uid.map(|u| u as i64).unwrap_or(LOCAL_GUEST_USER_ID);
            validate_act_as(effective_uid, &actor)?;
            return Ok(actor.actor_id);
        }

        if let Some(uid) = bound_uid {
            let hid = human_actor_id(uid as i64);
            if self.store.get_actor(&hid, None)?.is_none() {
                let fallback_input = build_human_sync_input(uid, "", None);
                let _ = self
                    .store
                    .sync_telegram_identities(&fallback_input, &[], now_millis())?;
            }
            return Ok(hid);
        }

        self.store
            .with_transaction(|tx| ensure_guest_actor_conn(tx, now_millis()))?;
        Ok(LOCAL_GUEST_ACTOR_ID.to_string())
    }

    pub(crate) fn run_lazy_maintenance(&self, now_ms: i64) {
        if self.bound_user_id.read().is_none() && self.persisted_owner_user_id.read().is_some() {
            return;
        }
        let _ = self.store.publish_due_scheduled_posts(now_ms);
        let _ = self.store.close_expired_polls(now_ms);
    }
}

/// Unified Online-First + Transparent Offline/Legacy-404 Fallback facade exposed by
/// `TelegramClient::community()`. Cheaply `Clone` (`Arc` interior) for GPUI UI tasks.
#[derive(Clone)]
pub struct CommunityClientFacade {
    pub(crate) inner: Arc<CommunityClientInner>,
}

pub(crate) struct CommunityClientInner {
    pub(crate) http_client: HttpClient,
    pub(crate) runtime_handle: Handle,
    pub(crate) auth_token: Arc<RwLock<Option<String>>>,
    pub(crate) auth_session: Arc<RwLock<Option<AuthResponse>>>,
    pub(crate) session_epoch: Arc<AtomicU64>,
    pub(crate) local: Arc<LocalCommunityRuntime>,
    pub(crate) mode: RwLock<CommunityRuntimeMode>,
    pub(crate) remote_community_verified: AtomicBool,
    pub(crate) force_local: AtomicBool,
    pub(crate) timeout_backoff_until: RwLock<Option<Instant>>,
}

impl CommunityClientFacade {
    pub(crate) fn new(
        http_client: HttpClient,
        runtime_handle: Handle,
        auth_token: Arc<RwLock<Option<String>>>,
        auth_session: Arc<RwLock<Option<AuthResponse>>>,
        session_epoch: Arc<AtomicU64>,
        local: Arc<LocalCommunityRuntime>,
    ) -> Self {
        Self {
            inner: Arc::new(CommunityClientInner {
                http_client,
                runtime_handle,
                auth_token,
                auth_session,
                session_epoch,
                local,
                mode: RwLock::new(CommunityRuntimeMode::Online),
                remote_community_verified: AtomicBool::new(false),
                force_local: AtomicBool::new(false),
                timeout_backoff_until: RwLock::new(None),
            }),
        }
    }

    pub fn runtime_handle(&self) -> &Handle {
        &self.inner.runtime_handle
    }

    /// Executes an async community call synchronously from non-async or Tokio-hosted threads
    /// safely via `std::thread::scope` when already inside a Tokio runtime.
    pub fn block_on<F, T>(&self, fut: F) -> Result<T>
    where
        F: Future<Output = Result<T>> + Send,
        T: Send,
    {
        if Handle::try_current().is_ok() {
            let handle = self.inner.runtime_handle.clone();
            std::thread::scope(|scope| scope.spawn(move || handle.block_on(fut)).join()).map_err(
                |_| TelegramError::Internal("Community runtime scope thread panicked".to_string()),
            )?
        } else {
            self.inner.runtime_handle.block_on(fut)
        }
    }

    pub fn runtime_mode(&self) -> CommunityRuntimeMode {
        *self.inner.mode.read()
    }

    pub fn set_force_local(&self, force: bool) {
        self.inner.force_local.store(force, Ordering::SeqCst);
        if force {
            *self.inner.mode.write() = CommunityRuntimeMode::LocalFallback;
        }
    }

    pub fn is_force_local(&self) -> bool {
        self.inner.force_local.load(Ordering::SeqCst)
    }

    pub fn active_actor_id(&self) -> Option<String> {
        self.inner.local.active_actor_id()
    }

    pub fn set_active_actor_id(&self, actor_id: Option<String>) {
        self.inner.local.set_active_actor_id(actor_id);
    }

    pub fn local_db_path(&self) -> Option<PathBuf> {
        self.inner.local.db_path()
    }

    pub fn local_runtime(&self) -> &Arc<LocalCommunityRuntime> {
        &self.inner.local
    }

    pub fn bind_account_scope(&self, user_id: u64, username: &str, first_name: Option<&str>) {
        self.inner
            .remote_community_verified
            .store(false, Ordering::SeqCst);
        *self.inner.timeout_backoff_until.write() = None;
        *self.inner.mode.write() = if self.inner.force_local.load(Ordering::SeqCst) {
            CommunityRuntimeMode::LocalFallback
        } else {
            CommunityRuntimeMode::Online
        };
        if let Err(err) = self
            .inner
            .local
            .bind_account_scope(user_id, username, first_name)
        {
            tracing::warn!(
                user_id,
                error = %err,
                "Failed to bind local community account scope"
            );
        }
    }

    pub fn clear_session_state(&self) {
        self.inner.local.clear_session_state();
        self.inner
            .remote_community_verified
            .store(false, Ordering::SeqCst);
        *self.inner.timeout_backoff_until.write() = None;
        *self.inner.mode.write() = if self.inner.force_local.load(Ordering::SeqCst) {
            CommunityRuntimeMode::LocalFallback
        } else {
            CommunityRuntimeMode::Online
        };
    }

    pub(crate) fn current_token(&self) -> Option<String> {
        self.inner
            .auth_token
            .read()
            .clone()
            .or_else(|| self.inner.auth_session.read().as_ref().map(|s| s.token.clone()))
            .filter(|t| !t.trim_matches(' ').is_empty())
    }

    pub(crate) fn effective_act_as(&self, explicit: Option<&str>) -> Option<String> {
        explicit
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .or_else(|| self.inner.local.active_actor_id())
    }

    pub(crate) async fn execute_with_fallback<T, RemoteFut, LocalFn>(
        &self,
        allow_downgrade_on_404: bool,
        explicit_act_as: Option<&str>,
        remote_call: impl FnOnce(HttpClient, Option<String>, Option<String>) -> RemoteFut,
        local_call: LocalFn,
    ) -> Result<T>
    where
        T: Send + 'static,
        RemoteFut: Future<Output = std::result::Result<T, CommunityApiError>> + Send,
        LocalFn: FnOnce(Arc<LocalCommunityRuntime>, Option<String>) -> Result<T> + Send + 'static,
    {
        self.execute_with_fallback_tagged(
            allow_downgrade_on_404,
            explicit_act_as,
            remote_call,
            local_call,
        )
        .await
        .map(|(val, _was_online, _epoch)| val)
    }

    pub(crate) async fn execute_with_fallback_tagged<T, RemoteFut, LocalFn>(
        &self,
        allow_downgrade_on_404: bool,
        explicit_act_as: Option<&str>,
        remote_call: impl FnOnce(HttpClient, Option<String>, Option<String>) -> RemoteFut,
        local_call: LocalFn,
    ) -> Result<(T, bool, u64)>
    where
        T: Send + 'static,
        RemoteFut: Future<Output = std::result::Result<T, CommunityApiError>> + Send,
        LocalFn: FnOnce(Arc<LocalCommunityRuntime>, Option<String>) -> Result<T> + Send + 'static,
    {
        let start_epoch = self.inner.session_epoch.load(Ordering::SeqCst);
        let token = self.current_token();
        let act_as = self.effective_act_as(explicit_act_as);

        let force_local = self.inner.force_local.load(Ordering::SeqCst);
        let in_timeout_backoff = self
            .inner
            .timeout_backoff_until
            .read()
            .is_some_and(|until| until > Instant::now());

        if !force_local && !in_timeout_backoff {
            match remote_call(self.inner.http_client.clone(), token, act_as.clone()).await {
                Ok(val) => {
                    if self.inner.session_epoch.load(Ordering::SeqCst) == start_epoch {
                        *self.inner.mode.write() = CommunityRuntimeMode::Online;
                        self.inner
                            .remote_community_verified
                            .store(true, Ordering::SeqCst);
                        *self.inner.timeout_backoff_until.write() = None;
                    }
                    return Ok((val, true, start_epoch));
                }
                Err(api_err) => {
                    let should_fallback = match &api_err {
                        CommunityApiError::Transport { is_timeout, .. } => {
                            if self.inner.session_epoch.load(Ordering::SeqCst) == start_epoch {
                                *self.inner.mode.write() = CommunityRuntimeMode::LocalFallback;
                                self.inner
                                    .remote_community_verified
                                    .store(false, Ordering::SeqCst);
                                if *is_timeout {
                                    *self.inner.timeout_backoff_until.write() = Some(
                                        Instant::now() + Duration::from_secs(TIMEOUT_BACKOFF_SECS),
                                    );
                                }
                            }
                            true
                        }
                        CommunityApiError::LegacyRouteNotFound { .. } => {
                            let verified = self
                                .inner
                                .remote_community_verified
                                .load(Ordering::SeqCst);
                            if verified && !allow_downgrade_on_404 {
                                false
                            } else {
                                if self.inner.session_epoch.load(Ordering::SeqCst) == start_epoch {
                                    self.inner
                                        .remote_community_verified
                                        .store(false, Ordering::SeqCst);
                                    *self.inner.mode.write() = CommunityRuntimeMode::LocalFallback;
                                }
                                true
                            }
                        }
                        CommunityApiError::DomainNotFound { .. } => {
                            if self.inner.session_epoch.load(Ordering::SeqCst) == start_epoch {
                                self.inner
                                    .remote_community_verified
                                    .store(true, Ordering::SeqCst);
                            }
                            false
                        }
                        CommunityApiError::Http { .. }
                        | CommunityApiError::Proxy(_)
                        | CommunityApiError::InvalidInput(_)
                        | CommunityApiError::Serialization(_) => false,
                    };

                    if !should_fallback {
                        return Err(TelegramError::from(api_err));
                    }
                }
            }
        } else if force_local {
            *self.inner.mode.write() = CommunityRuntimeMode::LocalFallback;
        }

        // Session-epoch fence: never write fallback state after an account switch mid-flight.
        if self.inner.session_epoch.load(Ordering::SeqCst) != start_epoch {
            return Err(TelegramError::Auth(
                "Session changed before community local fallback".to_string(),
            ));
        }

        let local = Arc::clone(&self.inner.local);
        let session_epoch = Arc::clone(&self.inner.session_epoch);
        let handle = self.inner.runtime_handle.clone();
        let val = handle
            .spawn_blocking(move || {
                if session_epoch.load(Ordering::SeqCst) != start_epoch {
                    return Err(TelegramError::Auth(
                        "Session changed before community local fallback".to_string(),
                    ));
                }
                local_call(local, act_as)
            })
            .await
            .map_err(|e| TelegramError::Internal(format!("Community local task panicked: {e}")))??;
        Ok((val, false, start_epoch))
    }

    pub(crate) async fn mirror_owned_actors_local(
        &self,
        start_epoch: u64,
        actors: Vec<CommunityActorDto>,
    ) {
        if actors.is_empty() || self.inner.session_epoch.load(Ordering::SeqCst) != start_epoch {
            return;
        }
        let session_epoch = Arc::clone(&self.inner.session_epoch);
        let local = Arc::clone(&self.inner.local);
        let _ = self
            .inner
            .runtime_handle
            .spawn_blocking(move || {
                if session_epoch.load(Ordering::SeqCst) != start_epoch {
                    return;
                }
                let Some(bound_uid) = local.bound_user_id() else {
                    return;
                };
                let now_ms = now_millis();
                for actor in actors {
                    if session_epoch.load(Ordering::SeqCst) != start_epoch {
                        return;
                    }
                    if !can_user_act_as(bound_uid as i64, &actor) {
                        continue;
                    }
                    if let Err(err) = local
                        .store
                        .with_conn(|conn| ops::upsert_actor_conn(conn, &actor, now_ms))
                    {
                        tracing::debug!(
                            actor_id = %actor.actor_id,
                            error = %err,
                            "Skipping non-fatal local actor mirror"
                        );
                    }
                }
            })
            .await;
    }

    pub(crate) fn collect_identities_actors(
        resp: &CommunityIdentitiesResponse,
    ) -> Vec<CommunityActorDto> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for actor in resp
            .human_actor
            .iter()
            .chain(resp.bot_actors.iter())
            .chain(resp.all_actors.iter())
        {
            if seen.insert(actor.actor_id.clone()) {
                out.push(actor.clone());
            }
        }
        out
    }

    /// Synchronizes the logged-in Human identity and owned Bots both locally and remotely.
    pub async fn sync_identities(
        &self,
        human: &TelegramHumanSyncInput,
        owned_bots: &[TelegramBotSyncInput],
    ) -> Result<CommunityIdentitiesResponse> {
        let start_epoch = self.inner.session_epoch.load(Ordering::SeqCst);
        let human_owned = human.clone();
        let bots_owned = owned_bots.to_vec();
        let local = Arc::clone(&self.inner.local);
        let session_epoch = Arc::clone(&self.inner.session_epoch);
        let local_resp = self
            .inner
            .runtime_handle
            .spawn_blocking(move || {
                if session_epoch.load(Ordering::SeqCst) != start_epoch {
                    return Err(TelegramError::Auth(
                        "Session changed before community local identity sync".to_string(),
                    ));
                }
                local.sync_identities_local(&human_owned, &bots_owned)
            })
            .await
            .map_err(|e| {
                TelegramError::Internal(format!("Community local identity sync panicked: {e}"))
            })??;

        if self.inner.force_local.load(Ordering::SeqCst) {
            *self.inner.mode.write() = CommunityRuntimeMode::LocalFallback;
            return Ok(local_resp);
        }

        let token = self.current_token();
        let act_as = self.effective_act_as(None);
        match self
            .inner
            .http_client
            .community_get_identities(token.as_deref(), act_as.as_deref())
            .await
        {
            Ok(mut remote_resp) => {
                if self.inner.session_epoch.load(Ordering::SeqCst) == start_epoch {
                    *self.inner.mode.write() = CommunityRuntimeMode::Online;
                    self.inner
                        .remote_community_verified
                        .store(true, Ordering::SeqCst);
                    *self.inner.timeout_backoff_until.write() = None;
                }
                // Best-effort remote registration for human / owned bots missing remotely.
                if token.is_some() {
                    let mut registered_any = false;
                    if remote_resp.human_actor.is_none() {
                        let req = RegisterHumanActorRequest {
                            handle: human
                                .username
                                .clone()
                                .unwrap_or_else(|| format!("user_{}", human.user_id)),
                            display_name: human.display_name.clone(),
                            bio: human.bio.clone(),
                            location: None,
                            website: None,
                            avatar_media_id: human.avatar_media_id.clone(),
                            banner_media_id: None,
                            avatar_shape: None,
                            avatar_color: None,
                        };
                        if self
                            .inner
                            .http_client
                            .community_register_human_actor(
                                token.as_deref(),
                                act_as.as_deref(),
                                &req,
                            )
                            .await
                            .is_ok()
                        {
                            registered_any = true;
                        }
                    }
                    for bot in owned_bots {
                        let target_id = gpui_telegram_community::bot_actor_id(bot.bot_user_id);
                        if !remote_resp
                            .bot_actors
                            .iter()
                            .any(|b| b.actor_id == target_id)
                        {
                            let req = RegisterBotActorRequest {
                                bot_user_id: Some(bot.bot_user_id),
                                actor_kind: Some(ActorKind::Bot),
                                handle: bot.username.clone(),
                                display_name: bot.display_name.clone(),
                                bio: bot.bio.clone(),
                                avatar_media_id: bot.avatar_media_id.clone(),
                                banner_media_id: None,
                                avatar_shape: bot.avatar_shape.clone(),
                                avatar_color: bot.avatar_color.clone(),
                                bot_badge: None,
                                bot_model: bot.bot_model.clone(),
                                bot_capabilities: bot.bot_capabilities.clone(),
                                bot_autonomy_mode: BotAutonomyMode::MentionOnly,
                                bot_persona: bot.bot_persona.clone(),
                                cooldown_secs: None,
                                max_posts_per_hour: None,
                            };
                            if self
                                .inner
                                .http_client
                                .community_register_bot_actor(
                                    token.as_deref(),
                                    act_as.as_deref(),
                                    &req,
                                )
                                .await
                                .is_ok()
                            {
                                registered_any = true;
                            }
                        }
                    }
                    if registered_any {
                        if let Ok(refreshed) = self
                            .inner
                            .http_client
                            .community_get_identities(token.as_deref(), act_as.as_deref())
                            .await
                        {
                            remote_resp = refreshed;
                        }
                    }
                }
                let actors_to_mirror = Self::collect_identities_actors(&remote_resp);
                self.mirror_owned_actors_local(start_epoch, actors_to_mirror)
                    .await;
                Ok(remote_resp)
            }
            Err(api_err) if api_err.should_fallback_to_local() => {
                if self.inner.session_epoch.load(Ordering::SeqCst) == start_epoch {
                    *self.inner.mode.write() = CommunityRuntimeMode::LocalFallback;
                    self.inner
                        .remote_community_verified
                        .store(false, Ordering::SeqCst);
                }
                Ok(local_resp)
            }
            Err(api_err) => Err(TelegramError::from(api_err)),
        }
    }
}
