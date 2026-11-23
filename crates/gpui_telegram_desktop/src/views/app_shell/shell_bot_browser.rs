//! AppShell bridge for Pure-Obscura `BotBrowserOrchestrator`, strictly-local Bot Assist cards,
//! in-process local-first `FabushiBotHost`, and client-side Zero-Knowledge E2EE User Vault sync.
//!
//! Security Red Lines:
//! 1. Every Bot Assist card (`VaultLogin`, `ReuseConfirm`, `QrLogin`, `ClickCaptcha`) and every
//!    credential submission/approval/click action executes strictly in-process and locally.
//!    Nothing here ever touches `send_message_durable`, `send_rich_text_options`, `upload_media`,
//!    or `gpui_telegram_proto::message::MessageContent`.
//! 2. `/vault-setup`, `/vault-unlock`, `/vault-recover`, `/vault-sync` commands are intercepted
//!    at the top of `append_outgoing_text` before chat stage insertion or `send_message_durable`,
//!    and passphrases/recovery keys NEVER leave the local device.
//! 3. Passwords/TOTP, browser profile cookies, and AI OAuth subscriptions are encrypted and
//!    decrypted ONLY client-side via `bot_browser::e2ee_sync`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bot_browser::e2ee_sync::{
    derive_split_key, AiOauthVaultBundle, Argon2KdfParams, BrowserProfileSnapshot,
    DeviceKeyTransfer, DeviceSyncKeypair, EncryptedVaultBlob, UnlockedUserVaultKey,
    UserVaultCategory, UserVaultCrypto, UserVaultKeyEnvelope, USER_VAULT_KDF_ALG_ARGON2ID,
    USER_VAULT_KDF_ALG_SPLIT_KEY_V2,
};
use bot_browser::orchestrator::{BotBrowserOrchestrator, OrchestratorConfig};
use bot_browser::vault::{
    CredentialVault, FileMasterKeyStore, SystemKeyringMasterKeyStore, VaultSyncCredentialPayload,
};
use bot_browser::{
    AssistCardPayload, AssistCardStatus, BotBrowserError, BotBrowserEvent, CredentialMeta,
    ReusePolicy,
};
use gpui::{App, AsyncApp, Context, WeakEntity};
use gpui_telegram_agent::fabushi_bot::{
    FabushiBotHost, FabushiBotProvisionSpec, OFFICIAL_DEFAULT_BOT_USER_ID,
};
use gpui_telegram_agent::hermes::security::ApprovalMode;
use gpui_telegram_agent::hermes::toolsets::ToolsetPreset;
use gpui_telegram_agent::hermes::{HermesEngineConfig, HermesTurnRequest};
use gpui_telegram_core::types::ChatId;
use gpui_telegram_proto::miniapp_contracts::BOTFATHER_USER_ID;
use gpui_telegram_proto::user_vault::{
    ApproveDeviceSyncRequest, ArchiveLocalBotReplyRequest, Argon2KdfParamsDto,
    DeviceSyncRequestBody, UpgradeSplitKeyRequest, UpsertUserVaultItemRequest,
    UserVaultKeyEnvelopeDto, USER_VAULT_CATEGORY_PASSWORD,
};
use magpie_gateway::ClientSubscriptionSyncItem;
use zeroize::{Zeroize as _, Zeroizing};

use crate::i18n::tr_str;
use crate::views::app_shell::local_subscription_bridge::{
    build_ai_oauth_vault_bundle, discover_local_subscriptions_for_server,
    discovered_subscriptions_fingerprint, has_usable_local_fabushi_provider_for_user,
    hydrate_ai_oauth_vault_bundle_into_fabushi_for_user,
    hydrate_local_subscriptions_into_fabushi_for_user,
};
use crate::views::app_shell::shell_actions::{peer_avatar_color, resolve_active_target_chat_id};
use crate::views::app_shell::shell_settings_sync::format_ms_human;
use crate::views::app_shell::shell_state::reconcile_stage_message_list;
use crate::views::app_shell::shell_truth::{derive_initials, format_authoritative_timestamp};
use crate::views::auth::PendingAuthVaultBootstrap;
use crate::views::chat_sidebar::DialogDeliveryStatus;
use crate::views::settings::sections::{PendingDeviceSyncRow, VaultCustodyStatus};
use crate::views::shell::AppShell;
use crate::views::stage::cards::{
    BotAssistCardAction, DeliveryStatus, InlineButtonType, InlineKeyboardButton, MessageContent,
    MessageDirection, MessageItemModel, ServiceType, BOT_ASSIST_LOCAL_MSG_ID_BASE,
};

pub const MAX_PROFILE_BUNDLE_BYTES: usize = 16 * 1024 * 1024;
const AI_OAUTH_VAULT_ITEM_KEY: &str = "subscriptions_v1";

/// Ephemeral in-memory X25519 Device Trust Ring session awaiting approval from a trusted device.
/// Secret keypair is strictly zeroized on drop and never persisted or serialized.
pub struct PendingDeviceSyncSession {
    pub request_id: String,
    pub verification_code: String,
    pub expires_at: i64,
    pub keypair: Option<DeviceSyncKeypair>,
}

impl std::fmt::Debug for PendingDeviceSyncSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingDeviceSyncSession")
            .field("request_id", &self.request_id)
            .field("verification_code", &self.verification_code)
            .field("expires_at", &self.expires_at)
            .field("has_keypair", &self.keypair.is_some())
            .finish()
    }
}

/// AppShell-owned state tracking the local `BotBrowserOrchestrator`, in-process `FabushiBotHost`,
/// client-side E2EE `UnlockedUserVaultKey`, Device Trust Ring state, and `card_id -> (chat_id, local_msg_id)`.
pub struct ShellBotBrowserState {
    pub orchestrator: Option<Arc<BotBrowserOrchestrator>>,
    pub local_fabushi: Option<Arc<FabushiBotHost>>,
    pub unlocked_vault_key: Option<Arc<UnlockedUserVaultKey>>,
    pub vault_sync_revision: i64,
    pub vault_locked_needs_passphrase: bool,
    pub last_recovery_key_notice: Option<Zeroizing<String>>,
    pub vault_custody_status: VaultCustodyStatus,
    pub vault_keychain_saved: bool,
    pub vault_last_sync_at_ms: Option<i64>,
    pub vault_last_sync_count: usize,
    pub vault_custody_status_note: Option<String>,
    pub first_creation_banner_open: bool,
    pub pending_device_sync: Option<PendingDeviceSyncSession>,
    pub device_sync_poll_generation: u64,
    pub pending_device_approvals: Vec<PendingDeviceSyncRow>,
    pub cached_credential_metas: Vec<CredentialMeta>,
    pub active_browser_sessions: HashMap<String, String>,
    pub card_locations: HashMap<String, (u64, u64)>,
    pub next_local_msg_id: u64,
    pub listener_generation: u64,
    pub hydrated_subscriptions_fingerprint: Option<String>,
    pub vault_bootstrap_generation: u64,
    pub active_vault_user_id: Option<u64>,
}

impl Default for ShellBotBrowserState {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellBotBrowserState {
    pub fn new() -> Self {
        Self {
            orchestrator: None,
            local_fabushi: None,
            unlocked_vault_key: None,
            vault_sync_revision: 0,
            vault_locked_needs_passphrase: false,
            last_recovery_key_notice: None,
            vault_custody_status: VaultCustodyStatus::LockedOrUninitialized,
            vault_keychain_saved: false,
            vault_last_sync_at_ms: None,
            vault_last_sync_count: 0,
            vault_custody_status_note: None,
            first_creation_banner_open: false,
            pending_device_sync: None,
            device_sync_poll_generation: 0,
            pending_device_approvals: Vec::new(),
            cached_credential_metas: Vec::new(),
            active_browser_sessions: HashMap::new(),
            card_locations: HashMap::new(),
            next_local_msg_id: BOT_ASSIST_LOCAL_MSG_ID_BASE + 1,
            listener_generation: 0,
            hydrated_subscriptions_fingerprint: None,
            vault_bootstrap_generation: 0,
            active_vault_user_id: None,
        }
    }

    pub fn reset_on_logout(&mut self) {
        self.orchestrator = None;
        self.local_fabushi = None;
        self.unlocked_vault_key = None;
        self.vault_sync_revision = 0;
        self.vault_locked_needs_passphrase = false;
        if let Some(ref mut ek) = self.last_recovery_key_notice {
            ek.zeroize();
        }
        self.last_recovery_key_notice = None;
        self.vault_custody_status = VaultCustodyStatus::LockedOrUninitialized;
        self.vault_keychain_saved = false;
        self.vault_last_sync_at_ms = None;
        self.vault_last_sync_count = 0;
        self.vault_custody_status_note = None;
        self.first_creation_banner_open = false;
        self.pending_device_sync = None;
        self.device_sync_poll_generation = self.device_sync_poll_generation.wrapping_add(1);
        self.pending_device_approvals.clear();
        self.cached_credential_metas.clear();
        self.active_browser_sessions.clear();
        self.card_locations.clear();
        self.next_local_msg_id = BOT_ASSIST_LOCAL_MSG_ID_BASE + 1;
        self.listener_generation = self.listener_generation.wrapping_add(1);
        self.hydrated_subscriptions_fingerprint = None;
        self.vault_bootstrap_generation = self.vault_bootstrap_generation.wrapping_add(1);
        self.active_vault_user_id = None;
    }

    /// Returns the deterministic local message ID for `card_id` in `chat_id`, allocating one if new.
    pub fn allocate_local_msg_id(&mut self, card_id: &str, chat_id: u64) -> u64 {
        if let Some(&(_, existing_id)) = self.card_locations.get(card_id) {
            return existing_id;
        }
        let assigned = self.next_local_msg_id;
        self.next_local_msg_id = self.next_local_msg_id.saturating_add(1);
        self.card_locations
            .insert(card_id.to_string(), (chat_id, assigned));
        assigned
    }

    /// Allocates a strictly local message ID >= `BOT_ASSIST_LOCAL_MSG_ID_BASE` without binding to a card.
    pub fn next_ephemeral_local_msg_id(&mut self) -> u64 {
        let assigned = self.next_local_msg_id;
        self.next_local_msg_id = self.next_local_msg_id.saturating_add(1);
        assigned
    }
}

/// Strictly-local vault command parsed from outgoing composer input before network/stage dispatch.
#[derive(Clone, PartialEq, Eq)]
pub enum LocalVaultCommand {
    Setup { passphrase: Zeroizing<String> },
    Unlock { passphrase: Zeroizing<String> },
    Recover { recovery_key: Zeroizing<String> },
    Sync,
    UsageError { message: String },
}

impl std::fmt::Debug for LocalVaultCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setup { .. } => f
                .debug_struct("Setup")
                .field("passphrase", &"[REDACTED]")
                .finish(),
            Self::Unlock { .. } => f
                .debug_struct("Unlock")
                .field("passphrase", &"[REDACTED]")
                .finish(),
            Self::Recover { .. } => f
                .debug_struct("Recover")
                .field("recovery_key", &"[REDACTED]")
                .finish(),
            Self::Sync => f.write_str("Sync"),
            Self::UsageError { message } => f
                .debug_struct("UsageError")
                .field("message", message)
                .finish(),
        }
    }
}

/// Parses strictly-local `/vault-setup`, `/vault-unlock`, `/vault-recover`, `/vault-sync` commands.
/// Returns `None` for any non-vault message so normal chat messages proceed normally.
pub fn parse_local_vault_command(raw: &str) -> Option<LocalVaultCommand> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let (cmd_part, rest) = match trimmed.find(char::is_whitespace) {
        Some(idx) => (&trimmed[..idx], trimmed[idx..].trim()),
        None => (trimmed, ""),
    };
    let cmd = cmd_part
        .split('@')
        .next()
        .unwrap_or(cmd_part)
        .to_ascii_lowercase();
    match cmd.as_str() {
        "/vault-setup" => {
            if rest.is_empty() {
                Some(LocalVaultCommand::UsageError {
                    message: "Usage: /vault-setup <passphrase>".to_string(),
                })
            } else {
                Some(LocalVaultCommand::Setup {
                    passphrase: Zeroizing::new(rest.to_string()),
                })
            }
        }
        "/vault-unlock" => {
            if rest.is_empty() {
                Some(LocalVaultCommand::UsageError {
                    message: "Usage: /vault-unlock <passphrase>".to_string(),
                })
            } else {
                Some(LocalVaultCommand::Unlock {
                    passphrase: Zeroizing::new(rest.to_string()),
                })
            }
        }
        "/vault-recover" => {
            if rest.is_empty() {
                Some(LocalVaultCommand::UsageError {
                    message: "Usage: /vault-recover <recovery-key>".to_string(),
                })
            } else {
                Some(LocalVaultCommand::Recover {
                    recovery_key: Zeroizing::new(rest.to_string()),
                })
            }
        }
        "/vault-sync" => Some(LocalVaultCommand::Sync),
        _ => None,
    }
}

/// Target metadata for executing a Fabushi Bot turn locally in-process.
#[derive(Debug, Clone)]
pub struct LocalFabushiBotTarget {
    pub bot_user_id: i64,
    pub display_name: String,
    pub provision_spec: Option<FabushiBotProvisionSpec>,
}

/// Summary of pulled & locally decrypted E2EE vault items.
#[derive(Debug, Default)]
pub struct VaultPullReport {
    pub latest_revision: i64,
    pub passwords_applied: usize,
    pub cookies_applied: usize,
    pub ai_oauth_bundle: Option<AiOauthVaultBundle>,
}

/// Summary of sealed & uploaded E2EE vault items.
#[derive(Debug, Default)]
pub struct VaultPushReport {
    pub latest_revision: i64,
    pub passwords_pushed: usize,
    pub cookies_pushed: usize,
    pub ai_oauth_pushed: bool,
}

pub fn user_bot_browser_dir(preferences_dir: &Path, user_id: u64) -> PathBuf {
    preferences_dir.join("bot_browser").join(user_id.to_string())
}

pub fn user_local_fabushi_dir(preferences_dir: &Path, user_id: u64) -> PathBuf {
    preferences_dir.join("local_fabushi").join(user_id.to_string())
}

pub fn is_safe_profile_item_key(key: &str) -> bool {
    let trimmed = key.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 128
        && !trimmed.contains("..")
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

pub fn user_vault_envelope_to_proto(env: &UserVaultKeyEnvelope) -> UserVaultKeyEnvelopeDto {
    UserVaultKeyEnvelopeDto {
        key_version: env.key_version,
        kdf_alg: env.kdf_alg.clone(),
        kdf_salt_b64: env.kdf_salt_b64.clone(),
        kdf_m_cost_kib: env.kdf_params.m_cost_kib,
        kdf_t_cost: env.kdf_params.t_cost,
        kdf_p_cost: env.kdf_params.p_cost,
        wrapped_root_key_b64: env.wrapped_root_key_b64.clone(),
        recovery_wrapped_root_key_b64: env.recovery_wrapped_root_key_b64.clone(),
        updated_at_ms: env.updated_at_ms,
    }
}

pub fn user_vault_envelope_from_proto(dto: &UserVaultKeyEnvelopeDto) -> UserVaultKeyEnvelope {
    UserVaultKeyEnvelope {
        key_version: dto.key_version,
        kdf_alg: dto.kdf_alg.clone(),
        kdf_salt_b64: dto.kdf_salt_b64.clone(),
        kdf_params: Argon2KdfParams {
            m_cost_kib: dto.kdf_m_cost_kib,
            t_cost: dto.kdf_t_cost,
            p_cost: dto.kdf_p_cost,
        },
        wrapped_root_key_b64: dto.wrapped_root_key_b64.clone(),
        recovery_wrapped_root_key_b64: dto.recovery_wrapped_root_key_b64.clone(),
        updated_at_ms: dto.updated_at_ms,
    }
}

fn use_headless_file_keystore_directly() -> bool {
    std::env::var("GPUI_OFFLINE_TEST").as_deref() == Ok("1")
}

/// On macOS, default to strict `0o600` local key files instead of legacy `login.keychain-db`
/// so dev builds, notarized upgrades, and cold launches never trigger OS Keychain password prompts.
fn use_local_file_keystore() -> bool {
    if use_headless_file_keystore_directly() {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var("FABUSHI_USE_MACOS_KEYCHAIN").as_deref() != Ok("1")
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn open_default_local_vault(
    vault_db_path: &Path,
    fallback_key_path: &Path,
) -> Result<CredentialVault, BotBrowserError> {
    if !use_local_file_keystore() {
        let keyring_store = SystemKeyringMasterKeyStore::default();
        match CredentialVault::open(vault_db_path, &keyring_store) {
            Ok(vault) => return Ok(vault),
            Err(BotBrowserError::Keyring(_)) => {}
            Err(err) => return Err(err),
        }
    }
    let file_store = FileMasterKeyStore::new(fallback_key_path);
    CredentialVault::open(vault_db_path, &file_store)
}

fn user_vault_root_key_file_path(user_browser_dir: &Path) -> PathBuf {
    user_browser_dir.join("user_vault_root.key")
}

fn save_device_vault_root_key(
    user_id: u64,
    user_browser_dir: &Path,
    key: &UnlockedUserVaultKey,
) -> bool {
    if use_headless_file_keystore_directly() {
        return false;
    }
    let file_path = user_vault_root_key_file_path(user_browser_dir);
    if use_local_file_keystore() {
        return key.save_to_file(&file_path).is_ok();
    }
    key.save_to_keychain(user_id).is_ok() || key.save_to_file(&file_path).is_ok()
}

fn load_device_vault_root_key(user_id: u64, user_browser_dir: &Path) -> Option<UnlockedUserVaultKey> {
    if use_headless_file_keystore_directly() {
        return None;
    }
    let file_path = user_vault_root_key_file_path(user_browser_dir);
    if use_local_file_keystore() {
        return UnlockedUserVaultKey::load_from_file(&file_path)
            .ok()
            .flatten();
    }
    UnlockedUserVaultKey::load_from_keychain(user_id)
        .ok()
        .flatten()
        .or_else(|| {
            UnlockedUserVaultKey::load_from_file(&file_path)
                .ok()
                .flatten()
        })
}

fn delete_device_vault_root_key(user_id: u64, user_browser_dir: &Path) {
    let _ = UnlockedUserVaultKey::delete_file(&user_vault_root_key_file_path(user_browser_dir));
    if !use_local_file_keystore() {
        let _ = UnlockedUserVaultKey::delete_from_keychain(user_id);
    }
}

fn remove_sqlite_file_family(db_path: &Path) {
    let _ = fs::remove_file(db_path);
    if let Some(name) = db_path.file_name().and_then(|s| s.to_str()) {
        if let Some(parent) = db_path.parent() {
            let _ = fs::remove_file(parent.join(format!("{name}-wal")));
            let _ = fs::remove_file(parent.join(format!("{name}-shm")));
        }
    }
}

fn open_vault_with_e2ee_key_preserving(
    vault_db_path: &Path,
    key: &UnlockedUserVaultKey,
    pre_exported: Vec<VaultSyncCredentialPayload>,
    wipe_existing_before_import: bool,
) -> Result<CredentialVault, BotBrowserError> {
    if wipe_existing_before_import && vault_db_path.exists() {
        remove_sqlite_file_family(vault_db_path);
    }
    let vault = CredentialVault::open(vault_db_path, key)?;
    if !pre_exported.is_empty() {
        for rec in &pre_exported {
            let _ = vault.import_from_sync(rec)?;
        }
        return Ok(vault);
    }
    if let Err(BotBrowserError::Crypto(_)) = vault.export_all_for_sync() {
        drop(vault);
        remove_sqlite_file_family(vault_db_path);
        return CredentialVault::open(vault_db_path, key);
    }
    Ok(vault)
}

pub fn build_local_fabushi_host(
    fabushi_dir: &Path,
    orchestrator: Arc<BotBrowserOrchestrator>,
) -> Result<Arc<FabushiBotHost>, String> {
    let workspace_root = fabushi_dir.join("workspace");
    fs::create_dir_all(&workspace_root)
        .map_err(|e| format!("Failed creating local Fabushi workspace dir: {e}"))?;
    let cfg = HermesEngineConfig {
        workspace_root,
        data_dir: None,
        default_model: "hermes-local-v1".to_string(),
        default_preset: ToolsetPreset::Full,
        default_approval_mode: ApprovalMode::SmartApprove,
        ..Default::default()
    };
    let host = Arc::new(
        FabushiBotHost::new_unified(fabushi_dir, cfg, None)
            .map_err(|e| format!("Failed creating local FabushiBotHost: {e}"))?,
    );
    host.set_bot_browser(orchestrator);
    Ok(host)
}

fn dir_has_any_entry(dir: &Path) -> bool {
    fs::read_dir(dir)
        .ok()
        .is_some_and(|mut iter| iter.any(|res| res.is_ok()))
}

fn sanitize_session_slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_underscore = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '.' {
            out.push(ch.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore && !out.is_empty() {
            out.push('_');
            prev_underscore = true;
        }
    }
    let trimmed = out.trim_matches(['_', '.']);
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn collect_nonempty_profile_dirs(
    profiles_root: &Path,
    session_id_filter: Option<&str>,
) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(profiles_root) else {
        return Vec::new();
    };
    let session_prefix = session_id_filter.map(|sid| format!("{}__", sanitize_session_slug(sid)));
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !is_safe_profile_item_key(&name) {
            continue;
        }
        if let Some(ref prefix) = session_prefix {
            if !name.starts_with(prefix) && name != prefix.trim_end_matches("__") {
                continue;
            }
        }
        if dir_has_any_entry(&path) {
            out.push((name, path));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Seals `plaintext` using `key` and uploads it to `/api/v1/user-vault/items/{category}/{item_key}`.
/// Keeps the AEAD AAD `revision` strictly aligned with the server-assigned `revision`.
pub async fn seal_and_put_vault_item(
    client: &gpui_telegram_client::TelegramClient,
    key: &UnlockedUserVaultKey,
    user_id: u64,
    category: UserVaultCategory,
    item_key: &str,
    plaintext: &[u8],
    known_latest_rev: &mut i64,
) -> Result<i64, String> {
    for attempt in 0..4 {
        let predicted_rev = (*known_latest_rev).max(0) + 1;
        let blob = key
            .seal_item(user_id, category, item_key, predicted_rev, plaintext)
            .map_err(|e| format!("Failed sealing vault item '{item_key}': {e}"))?;
        let req = UpsertUserVaultItemRequest {
            expected_revision: if attempt > 0 {
                Some(*known_latest_rev)
            } else {
                None
            },
            nonce_b64: blob.nonce_b64,
            ciphertext_b64: blob.ciphertext_b64,
            deleted: false,
        };
        let resp = client
            .put_user_vault_item(category.as_str(), item_key, &req)
            .await
            .map_err(|e| format!("Vault upload failed ({}/{item_key}): {e}", category.as_str()))?;
        *known_latest_rev = resp.revision;
        if resp.revision == predicted_rev {
            return Ok(resp.revision);
        }
    }
    Err(format!(
        "Concurrent revision mismatch while sealing vault item '{item_key}'"
    ))
}

/// Pulls encrypted items since `since_revision`, decrypts locally, and applies passwords,
/// browser cookie storage directories, and AI OAuth bundles without server-side secrets.
pub async fn pull_and_apply_user_vault_items(
    client: &gpui_telegram_client::TelegramClient,
    key: &UnlockedUserVaultKey,
    user_id: u64,
    vault: &CredentialVault,
    profiles_root: &Path,
    since_revision: i64,
) -> Result<VaultPullReport, String> {
    let pulled = client
        .pull_user_vault_items(Some(since_revision), None)
        .await
        .map_err(|e| format!("Failed pulling user vault items: {e}"))?;

    let mut report = VaultPullReport {
        latest_revision: pulled.latest_revision.max(since_revision),
        ..Default::default()
    };

    for item in pulled.items {
        let category = UserVaultCategory::from_str(&item.category)
            .map_err(|e| format!("Invalid pulled category '{}': {e}", item.category))?;

        if item.deleted {
            match category {
                UserVaultCategory::Password => {
                    let _ = vault.delete_credential_if_exists(&item.item_key);
                }
                UserVaultCategory::BrowserCookie => {
                    if is_safe_profile_item_key(&item.item_key) {
                        let target_dir = profiles_root.join(&item.item_key);
                        if target_dir.exists() {
                            let _ = fs::remove_dir_all(&target_dir);
                        }
                    }
                }
                UserVaultCategory::AiOauth => {}
            }
            continue;
        }

        let blob = EncryptedVaultBlob {
            category,
            item_key: item.item_key.clone(),
            revision: item.revision,
            nonce_b64: item.nonce_b64,
            ciphertext_b64: item.ciphertext_b64,
            deleted: item.deleted,
            updated_at_ms: item.updated_at_ms,
        };
        let plaintext = key
            .open_item(user_id, &blob)
            .map_err(|e| format!("Failed decrypting {} '{}': {e}", item.category, item.item_key))?;

        match category {
            UserVaultCategory::Password => {
                let record: VaultSyncCredentialPayload = serde_json::from_slice(&plaintext)
                    .map_err(|e| format!("Corrupt password payload '{}': {e}", item.item_key))?;
                if vault
                    .import_from_sync(&record)
                    .map_err(|e| format!("Vault import failed '{}': {e}", item.item_key))?
                {
                    report.passwords_applied += 1;
                }
            }
            UserVaultCategory::BrowserCookie => {
                if !is_safe_profile_item_key(&item.item_key) {
                    return Err(format!(
                        "Rejected unsafe browser_cookie item_key '{}'",
                        item.item_key
                    ));
                }
                let target_dir = profiles_root.join(&item.item_key);
                BrowserProfileSnapshot::unpack_storage_dir(&plaintext, &target_dir).map_err(
                    |e| format!("Failed unpacking cookie profile '{}': {e}", item.item_key),
                )?;
                report.cookies_applied += 1;
            }
            UserVaultCategory::AiOauth => {
                let bundle = AiOauthVaultBundle::from_json_bytes(&plaintext)
                    .map_err(|e| format!("Corrupt ai_oauth bundle '{}': {e}", item.item_key))?;
                report.ai_oauth_bundle = Some(bundle);
            }
        }
    }

    Ok(report)
}

/// Exports all local passwords, Obscura browser profile dirs, and optional AI OAuth bundle,
/// sealing and pushing each item client-side to the encrypted user vault API.
pub async fn push_all_local_vault_items(
    client: &gpui_telegram_client::TelegramClient,
    key: &UnlockedUserVaultKey,
    user_id: u64,
    vault: &CredentialVault,
    profiles_root: &Path,
    ai_bundle: Option<&AiOauthVaultBundle>,
    known_latest_rev: &mut i64,
) -> Result<VaultPushReport, String> {
    let mut report = VaultPushReport {
        latest_revision: *known_latest_rev,
        ..Default::default()
    };

    let creds = vault
        .export_all_for_sync()
        .map_err(|e| format!("Failed exporting local credentials: {e}"))?;
    for cred in creds {
        let bytes = Zeroizing::new(
            serde_json::to_vec(&cred)
                .map_err(|e| format!("Failed serializing credential payload: {e}"))?,
        );
        seal_and_put_vault_item(
            client,
            key,
            user_id,
            UserVaultCategory::Password,
            &cred.credential_id,
            &bytes,
            known_latest_rev,
        )
        .await?;
        report.passwords_pushed += 1;
    }

    for (profile_key, dir_path) in collect_nonempty_profile_dirs(profiles_root, None) {
        if let Ok(packed) =
            BrowserProfileSnapshot::pack_storage_dir(&dir_path, None, MAX_PROFILE_BUNDLE_BYTES)
        {
            seal_and_put_vault_item(
                client,
                key,
                user_id,
                UserVaultCategory::BrowserCookie,
                &profile_key,
                &packed,
                known_latest_rev,
            )
            .await?;
            report.cookies_pushed += 1;
        }
    }

    if let Some(bundle) = ai_bundle {
        let bytes = bundle
            .to_json_bytes()
            .map_err(|e| format!("Failed serializing AI OAuth bundle: {e}"))?;
        seal_and_put_vault_item(
            client,
            key,
            user_id,
            UserVaultCategory::AiOauth,
            AI_OAUTH_VAULT_ITEM_KEY,
            &bytes,
            known_latest_rev,
        )
        .await?;
        report.ai_oauth_pushed = true;
    }

    report.latest_revision = *known_latest_rev;
    Ok(report)
}

fn split_text_chunks(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        if count >= max_chars {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
        current.push(ch);
        count += 1;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Projects decrypted `VaultSyncCredentialPayload` entries into strictly secret-free
/// `CredentialMeta` summaries suitable for Settings UI display. Input records zeroize on drop.
pub fn project_credential_metas(records: Vec<VaultSyncCredentialPayload>) -> Vec<CredentialMeta> {
    let mut metas: Vec<CredentialMeta> = records
        .into_iter()
        .filter(|rec| !rec.deleted)
        .map(|rec| CredentialMeta {
            credential_id: rec.credential_id.clone(),
            origin: rec.origin.clone(),
            account_hint: rec.account_hint.clone(),
            has_totp: rec
                .totp_secret
                .as_deref()
                .is_some_and(|t| !t.trim().is_empty()),
            reuse_policy: rec.reuse_policy,
            created_at_ms: rec.created_at_ms,
            last_used_at_ms: rec.last_used_at_ms,
        })
        .collect();
    metas.sort_by(|a, b| {
        b.last_used_at_ms
            .unwrap_or(b.created_at_ms)
            .cmp(&a.last_used_at_ms.unwrap_or(a.created_at_ms))
            .then_with(|| a.origin.cmp(&b.origin))
    });
    metas
}

fn collect_credential_metas_from_vault(vault: &CredentialVault) -> Vec<CredentialMeta> {
    vault
        .export_all_for_sync()
        .map(project_credential_metas)
        .unwrap_or_default()
}

fn resolve_device_display_name() -> String {
    let host = gpui_telegram_client::detect_local_hostname();
    let trimmed = host.trim();
    if trimmed.is_empty() {
        "Desktop".to_string()
    } else {
        trimmed.to_string()
    }
}

fn current_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn export_unmigrated_local_credentials_if_any(user_browser_dir: &Path) -> Vec<VaultSyncCredentialPayload> {
    let vault_db_path = user_browser_dir.join("vault.db");
    let fallback_key_path = user_browser_dir.join("vault.key");
    if !vault_db_path.exists() {
        return Vec::new();
    }
    open_default_local_vault(&vault_db_path, &fallback_key_path)
        .and_then(|v| v.export_all_for_sync())
        .unwrap_or_default()
}

async fn fetch_pending_device_sync_rows(
    client: &gpui_telegram_client::TelegramClient,
) -> Vec<PendingDeviceSyncRow> {
    match client.device_sync_pending().await {
        Ok(resp) => resp
            .items
            .into_iter()
            .filter(|item| item.status == "pending")
            .map(|item| PendingDeviceSyncRow {
                request_id: item.request_id,
                device_name: item.device_name,
                device_public_key_b64: item.device_public_key_b64,
                verification_code: item.verification_code,
                created_at: item.created_at,
                expires_at: item.expires_at,
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

struct UnlockedVaultRuntimeBundle {
    unlocked_key: Arc<UnlockedUserVaultKey>,
    orchestrator: Arc<BotBrowserOrchestrator>,
    host: Option<Arc<FabushiBotHost>>,
    pull_report: VaultPullReport,
    local_subs: Vec<ClientSubscriptionSyncItem>,
    credential_metas: Vec<CredentialMeta>,
    pending_approvals: Vec<PendingDeviceSyncRow>,
    recovery_key: Option<Zeroizing<String>>,
    is_first_creation: bool,
    keychain_saved: bool,
}

#[allow(clippy::too_many_arguments)]
async fn finalize_unlocked_vault_runtime(
    client: &gpui_telegram_client::TelegramClient,
    user_id: u64,
    user_browser_dir: &Path,
    fabushi_dir: &Path,
    base_url: &str,
    cached_subs: &[ClientSubscriptionSyncItem],
    unlocked_key: Arc<UnlockedUserVaultKey>,
    save_to_keychain: bool,
    already_in_keychain: bool,
    wipe_existing_before_import: bool,
    pre_exported: Vec<VaultSyncCredentialPayload>,
    recovery_key: Option<Zeroizing<String>>,
    is_first_creation: bool,
    pull_from_cloud: bool,
) -> Result<UnlockedVaultRuntimeBundle, String> {
    let vault_db_path = user_browser_dir.join("vault.db");
    let profiles_root = user_browser_dir.join("profiles");
    let user_browser_dir_owned = user_browser_dir.to_path_buf();
    let fabushi_dir_owned = fabushi_dir.to_path_buf();
    let base_url_owned = base_url.to_string();
    let cached_subs_owned = cached_subs.to_vec();
    let key_for_open = Arc::clone(&unlocked_key);

    let (vault, orch, host, local_subs, keychain_saved) = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let saved = if already_in_keychain {
            true
        } else if save_to_keychain {
            save_device_vault_root_key(user_id, &user_browser_dir_owned, &key_for_open)
        } else {
            false
        };
        let vault = Arc::new(
            open_vault_with_e2ee_key_preserving(
                &vault_db_path,
                &key_for_open,
                pre_exported,
                wipe_existing_before_import,
            )
            .map_err(|e| format!("failed opening user vault: {e}"))?,
        );
        let orch = Arc::new(BotBrowserOrchestrator::new(
            OrchestratorConfig::new(&user_browser_dir_owned),
            Arc::clone(&vault),
        ));
        let host = build_local_fabushi_host(&fabushi_dir_owned, Arc::clone(&orch)).ok();
        let subs = if !cached_subs_owned.is_empty() {
            cached_subs_owned
        } else {
            discover_local_subscriptions_for_server(&base_url_owned)
        };
        Ok((vault, orch, host, subs, saved))
    })
    .await
    .map_err(|e| format!("vault open task join error: {e}"))??;

    let mut pull_report = if pull_from_cloud {
        pull_and_apply_user_vault_items(
            client,
            &unlocked_key,
            user_id,
            &vault,
            &profiles_root,
            0,
        )
        .await
        .unwrap_or_default()
    } else {
        VaultPullReport::default()
    };

    if is_first_creation {
        let uid_str = user_id.to_string();
        if let Some(ref h) = host {
            if !local_subs.is_empty() {
                let _ = hydrate_local_subscriptions_into_fabushi_for_user(
                    h,
                    &local_subs,
                    Some(&uid_str),
                );
            }
        }
        let user_router = host
            .as_ref()
            .map(|h| h.core_engine().providers_for_user(&uid_str));
        let ai_bundle = if !local_subs.is_empty() || user_router.is_some() {
            build_ai_oauth_vault_bundle(&local_subs, user_router.as_ref()).ok()
        } else {
            None
        };
        let mut known_rev = pull_report.latest_revision;
        if let Ok(push_r) = push_all_local_vault_items(
            client,
            &unlocked_key,
            user_id,
            &vault,
            &profiles_root,
            ai_bundle.as_ref(),
            &mut known_rev,
        )
        .await
        {
            pull_report.latest_revision = pull_report.latest_revision.max(push_r.latest_revision);
        }
    }

    let vault_for_metas = Arc::clone(&vault);
    let credential_metas = tokio::task::spawn_blocking(move || collect_credential_metas_from_vault(&vault_for_metas))
        .await
        .unwrap_or_default();

    let pending_approvals = if pull_from_cloud {
        fetch_pending_device_sync_rows(client).await
    } else {
        Vec::new()
    };

    Ok(UnlockedVaultRuntimeBundle {
        unlocked_key,
        orchestrator: orch,
        host,
        pull_report,
        local_subs,
        credential_metas,
        pending_approvals,
        recovery_key,
        is_first_creation,
        keychain_saved,
    })
}

async fn execute_silent_split_key_upgrade(
    client: &gpui_telegram_client::TelegramClient,
    user_id: u64,
    unlocked_key: &UnlockedUserVaultKey,
    existing_envelope: Option<&UserVaultKeyEnvelope>,
    auth_bootstrap: Option<&PendingAuthVaultBootstrap>,
) -> Result<bool, String> {
    let Some(PendingAuthVaultBootstrap::SplitKeyV2 {
        split_kek,
        auth_salt_b64,
        kdf_params,
        legacy_upgrade_password: Some(old_pw),
        ..
    }) = auth_bootstrap
    else {
        return Ok(false);
    };

    if let Some(env) = existing_envelope {
        let needs_rewrap = env.kdf_alg != USER_VAULT_KDF_ALG_SPLIT_KEY_V2
            || env.kdf_salt_b64 != *auth_salt_b64;
        if needs_rewrap {
            let new_env = UserVaultCrypto::rewrap_root_key_with_kek(
                user_id,
                unlocked_key,
                env,
                split_kek,
                auth_salt_b64,
                *kdf_params,
            )
            .map_err(|e| format!("Failed rewrapping vault root key to split_key_v2: {e}"))?;
            let dto = user_vault_envelope_to_proto(&new_env);
            client
                .put_user_vault_envelope(&dto)
                .await
                .map_err(|e| format!("Failed uploading upgraded split_key_v2 envelope: {e}"))?;
        }
    }

    let pw_clone = old_pw.clone();
    let salt_clone = auth_salt_b64.clone();
    let params_copy = *kdf_params;
    let bundle = tokio::task::spawn_blocking(move || {
        derive_split_key(pw_clone.as_str(), &salt_clone, &params_copy)
    })
    .await
    .map_err(|e| format!("Split-key derivation task error: {e}"))?
    .map_err(|e| format!("Split-key derivation error: {e}"))?;

    let mut req = UpgradeSplitKeyRequest {
        old_password: old_pw.as_str().to_string(),
        auth_salt_b64: bundle.auth_salt_b64.clone(),
        auth_verifier_b64: bundle.auth_verifier_b64.as_str().to_string(),
        kdf_params: Argon2KdfParamsDto {
            m_cost_kib: params_copy.m_cost_kib,
            t_cost: params_copy.t_cost,
            p_cost: params_copy.p_cost,
        },
    };
    let upgrade_res = client.upgrade_split_key(&req).await;
    req.old_password.zeroize();
    req.auth_verifier_b64.zeroize();
    upgrade_res
        .map(|r| r.ok)
        .map_err(|e| format!("Silent split-key upgrade failed: {e}"))
}

impl AppShell {
    /// Attaches a `BotBrowserOrchestrator`, wires it into `local_fabushi` if present,
    /// rehydrates any active cards, and subscribes to `BotBrowserEvent`s.
    pub fn attach_bot_browser_orchestrator(
        &mut self,
        orchestrator: Arc<BotBrowserOrchestrator>,
        cx: &mut Context<Self>,
    ) {
        self.bot_browser.orchestrator = Some(Arc::clone(&orchestrator));
        if let Some(ref host) = self.bot_browser.local_fabushi {
            host.set_bot_browser(Arc::clone(&orchestrator));
        }
        self.bot_browser.listener_generation =
            self.bot_browser.listener_generation.wrapping_add(1);
        let listener_gen = self.bot_browser.listener_generation;

        for card in orchestrator.list_cards_for_chat(None) {
            self.apply_bot_browser_event(BotBrowserEvent::UpsertCard(card), cx);
        }
        self.spawn_bot_browser_event_listener(orchestrator, listener_gen, cx);
    }

    /// Attaches an in-process `FabushiBotHost`, immediately wiring the active `BotBrowserOrchestrator` if attached.
    pub fn attach_local_fabushi_host(&mut self, host: Arc<FabushiBotHost>) {
        if let Some(ref orch) = self.bot_browser.orchestrator {
            host.set_bot_browser(Arc::clone(orch));
        }
        if let Some(uid) = self.session_sync.user_id {
            let uid_str = uid.to_string();
            if !self.magpie_local.discovered_items.is_empty()
                && !has_usable_local_fabushi_provider_for_user(&host, Some(&uid_str))
            {
                hydrate_local_subscriptions_into_fabushi_for_user(
                    &host,
                    &self.magpie_local.discovered_items,
                    Some(&uid_str),
                );
                self.bot_browser.hydrated_subscriptions_fingerprint = Some(
                    discovered_subscriptions_fingerprint(&self.magpie_local.discovered_items),
                );
            }
        }
        self.bot_browser.local_fabushi = Some(host);
    }

    /// Lazily initializes a local `CredentialVault` + `BotBrowserOrchestrator` rooted at `preferences_dir/bot_browser`.
    pub fn ensure_bot_browser_orchestrator(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<Arc<BotBrowserOrchestrator>, BotBrowserError> {
        if let Some(ref existing) = self.bot_browser.orchestrator {
            return Ok(Arc::clone(existing));
        }
        let data_dir = match self.session_sync.user_id {
            Some(uid) => user_bot_browser_dir(&self.preferences_dir, uid),
            None => self.preferences_dir.join("bot_browser"),
        };
        let vault = if let Some(ref key) = self.bot_browser.unlocked_vault_key {
            CredentialVault::open(data_dir.join("vault.db"), key.as_ref())?
        } else {
            open_default_local_vault(&data_dir.join("vault.db"), &data_dir.join("vault.key"))?
        };
        let orch = Arc::new(BotBrowserOrchestrator::new(
            OrchestratorConfig::new(data_dir),
            Arc::new(vault),
        ));
        self.attach_bot_browser_orchestrator(Arc::clone(&orch), cx);
        Ok(orch)
    }

    /// Returns `true` when `local_fabushi` is attached and has a usable non-placeholder AI provider configured locally.
    pub fn can_run_local_fabushi_turn(&self, _cx: &App) -> bool {
        let uid_str = self.session_sync.user_id.map(|u| u.to_string());
        self.bot_browser.local_fabushi.as_ref().is_some_and(|host| {
            has_usable_local_fabushi_provider_for_user(host, uid_str.as_deref())
                || has_usable_local_fabushi_provider_for_user(host, None)
        })
    }

    /// Resolves whether `chat_id` is a conversational Fabushi Bot DM (excluding BotFather, groups, channels,
    /// non-owned bots, and known standard non-Fabushi bots).
    pub fn resolve_fabushi_bot_target(
        &self,
        chat_id: u64,
        cx: &App,
    ) -> Option<LocalFabushiBotTarget> {
        let rich_chat = self.session_sync.rich_chats_by_id.get(&chat_id);
        if rich_chat.is_some_and(|rc| rc.is_group || rc.is_channel || matches!(rc.chat_type, 1 | 2 | 3 | 5)) {
            return None;
        }
        let sb = self.sidebar.read(cx);
        if sb
            .dialogs
            .iter()
            .any(|d| d.id == chat_id && (d.folder == "Groups" || d.folder == "Channels"))
        {
            return None;
        }

        let viewer_uid = self.session_sync.user_id;
        let peer_uid = self
            .session_sync
            .chat_peer_user_ids
            .get(&chat_id)
            .copied()
            .or_else(|| rich_chat.and_then(|rc| rc.peer_user_id))
            .or_else(|| {
                ((700_000_000..750_000_000).contains(&chat_id))
                    .then_some(OFFICIAL_DEFAULT_BOT_USER_ID as u64)
            });

        if peer_uid == Some(BOTFATHER_USER_ID as u64) {
            return None;
        }

        if let Some(fb) = self.miniapps.fabushi_bots.iter().find(|fb| {
            peer_uid == Some(fb.bot_user_id.max(0) as u64)
                || fb.companion_chat_id == Some(chat_id as i64)
        }) {
            if fb.coding_agent.is_some() {
                return None;
            }
            if fb.bot_user_id != OFFICIAL_DEFAULT_BOT_USER_ID
                && viewer_uid.is_none_or(|u| fb.owner_user_id != u as i64)
            {
                return None;
            }
            return Some(LocalFabushiBotTarget {
                bot_user_id: fb.bot_user_id,
                display_name: if fb.display_name.trim().is_empty() {
                    fb.username.clone()
                } else {
                    fb.display_name.clone()
                },
                provision_spec: Some(FabushiBotProvisionSpec {
                    bot_user_id: fb.bot_user_id,
                    owner_user_id: fb.owner_user_id,
                    username: fb.username.clone(),
                    display_name: fb.display_name.clone(),
                    description: fb.description.clone(),
                    title: fb.title.clone(),
                    agent_id: (!fb.agent_id.trim().is_empty()).then(|| fb.agent_id.clone()),
                    avatar_shape: (!fb.avatar_shape.trim().is_empty())
                        .then(|| fb.avatar_shape.clone()),
                    avatar_color: (!fb.avatar_color.trim().is_empty())
                        .then(|| fb.avatar_color.clone()),
                    preferred_model: (!fb.preferred_model.trim().is_empty())
                        .then(|| fb.preferred_model.clone()),
                    thinking_level: (!fb.thinking_level.trim().is_empty())
                        .then(|| fb.thinking_level.clone()),
                    approval_mode: (!fb.approval_mode.trim().is_empty())
                        .then(|| fb.approval_mode.clone()),
                    toolset_preset: (!fb.toolset_preset.trim().is_empty())
                        .then(|| fb.toolset_preset.clone()),
                    commands: Vec::new(),
                    inline_mode_enabled: false,
                }),
            });
        }

        let peer_uname = rich_chat
            .and_then(|rc| rc.peer_username.as_deref().or(rc.username.as_deref()))
            .map(|u| u.trim().trim_start_matches('@').to_ascii_lowercase());
        if let Some(prof) = self.miniapps.bot_profiles_cache.values().find(|p| {
            peer_uid == Some(p.bot.bot_user_id.max(0) as u64)
                || peer_uname
                    .as_deref()
                    .is_some_and(|u| !u.is_empty() && p.bot.username.eq_ignore_ascii_case(u))
        }) {
            if prof.bot.coding_agent.is_some() {
                return None;
            }
            if !prof.bot.is_fabushi_bot && prof.bot.bot_user_id != OFFICIAL_DEFAULT_BOT_USER_ID {
                return None;
            }
            if prof.bot.bot_user_id != OFFICIAL_DEFAULT_BOT_USER_ID
                && viewer_uid.is_none_or(|u| prof.bot.owner_user_id != u as i64)
            {
                return None;
            }
            return Some(LocalFabushiBotTarget {
                bot_user_id: prof.bot.bot_user_id,
                display_name: if prof.bot.display_name.trim().is_empty() {
                    prof.bot.username.clone()
                } else {
                    prof.bot.display_name.clone()
                },
                provision_spec: Some(FabushiBotProvisionSpec {
                    bot_user_id: prof.bot.bot_user_id,
                    owner_user_id: prof.bot.owner_user_id,
                    username: prof.bot.username.clone(),
                    display_name: prof.bot.display_name.clone(),
                    description: prof.bot.description.clone(),
                    title: String::new(),
                    agent_id: (!prof.bot.agent_id.trim().is_empty())
                        .then(|| prof.bot.agent_id.clone()),
                    avatar_shape: (!prof.bot.avatar_shape.trim().is_empty())
                        .then(|| prof.bot.avatar_shape.clone()),
                    avatar_color: (!prof.bot.avatar_color.trim().is_empty())
                        .then(|| prof.bot.avatar_color.clone()),
                    preferred_model: (!prof.bot.preferred_model.trim().is_empty())
                        .then(|| prof.bot.preferred_model.clone()),
                    thinking_level: (!prof.bot.thinking_level.trim().is_empty())
                        .then(|| prof.bot.thinking_level.clone()),
                    approval_mode: (!prof.bot.approval_mode.trim().is_empty())
                        .then(|| prof.bot.approval_mode.clone()),
                    toolset_preset: (!prof.bot.toolset_preset.trim().is_empty())
                        .then(|| prof.bot.toolset_preset.clone()),
                    commands: prof.bot.commands.clone(),
                    inline_mode_enabled: prof.bot.is_inline,
                }),
            });
        }

        if self
            .miniapps
            .my_bots
            .iter()
            .chain(self.miniapps.attach_menu_bots.iter())
            .any(|b| {
                let matches_peer = peer_uid == Some(b.bot_user_id.max(0) as u64)
                    || peer_uname
                        .as_deref()
                        .is_some_and(|u| !u.is_empty() && b.username.eq_ignore_ascii_case(u));
                matches_peer
                    && (b.coding_agent.is_some()
                        || (b.bot_user_id != OFFICIAL_DEFAULT_BOT_USER_ID
                            && (!b.is_fabushi_bot
                                || viewer_uid.is_none_or(|u| b.owner_user_id != u as i64))))
            })
        {
            return None;
        }

        if peer_uid == Some(OFFICIAL_DEFAULT_BOT_USER_ID as u64) {
            let safe_title = sb
                .dialogs
                .iter()
                .find(|d| d.id == chat_id)
                .map(|d| d.peer_name.clone())
                .or_else(|| {
                    let st = self.stage.read(cx);
                    (st.active_chat_id == Some(chat_id)).then(|| st.chat_title.clone())
                })
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "Fabushi AI".to_string());
            return Some(LocalFabushiBotTarget {
                bot_user_id: OFFICIAL_DEFAULT_BOT_USER_ID,
                display_name: safe_title,
                provision_spec: None,
            });
        }

        if self.is_chat_bot(chat_id, cx)
            && peer_uid == Some(OFFICIAL_DEFAULT_BOT_USER_ID as u64)
        {
            let st_title = sb
                .dialogs
                .iter()
                .find(|d| d.id == chat_id)
                .map(|d| d.peer_name.clone())
                .or_else(|| {
                    let st = self.stage.read(cx);
                    (st.active_chat_id == Some(chat_id)).then(|| st.chat_title.clone())
                })
                .unwrap_or_default();
            let looks_like_fabushi = (700_000_000..750_000_000).contains(&chat_id)
                || st_title.contains("Autonomous Agent")
                || st_title.contains("Fabushi")
                || st_title.contains("Hermes")
                || peer_uname
                    .as_deref()
                    .is_some_and(|u| u.contains("fabushi") || u.contains("hermes"));
            if looks_like_fabushi {
                return Some(LocalFabushiBotTarget {
                    bot_user_id: OFFICIAL_DEFAULT_BOT_USER_ID,
                    display_name: if st_title.trim().is_empty() {
                        "Fabushi AI".to_string()
                    } else {
                        st_title
                    },
                    provision_spec: None,
                });
            }
        }
        None
    }

    /// Bootstraps per-user `CredentialVault`, `BotBrowserOrchestrator`, local `FabushiBotHost`,
    /// local AI OAuth hydration, and Keychain-backed E2EE cloud sync on session hydration.
    pub fn bootstrap_local_bot_browser_and_vault(&mut self, cx: &mut Context<Self>) {
        self.bootstrap_local_bot_browser_and_vault_with_auth(None, cx);
    }

    /// Zero-friction vault & custody bootstrap wired with optional in-memory `PendingAuthVaultBootstrap`.
    /// Silently initializes/unlocks Split-Key V2 or Keychain-Only vaults, upgrades legacy V1 auth,
    /// or starts automatic X25519 Device Trust Ring approval without `/vault-setup` or `/vault-unlock` prompts.
    pub fn bootstrap_local_bot_browser_and_vault_with_auth(
        &mut self,
        auth_bootstrap: Option<PendingAuthVaultBootstrap>,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated {
            return;
        }
        let Some(user_id) = self.session_sync.user_id else {
            return;
        };
        let auth_gen = self.session_sync.auth_generation;
        self.bot_browser.vault_bootstrap_generation =
            self.bot_browser.vault_bootstrap_generation.wrapping_add(1);
        self.bot_browser.device_sync_poll_generation =
            self.bot_browser.device_sync_poll_generation.wrapping_add(1);
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        self.bot_browser.active_vault_user_id = Some(user_id);
        self.bot_browser.pending_device_sync = None;
        self.bot_browser.vault_custody_status = VaultCustodyStatus::LockedOrUninitialized;
        self.bot_browser.vault_custody_status_note = None;

        let prefs_dir = self.preferences_dir.clone();
        let user_browser_dir = user_bot_browser_dir(&prefs_dir, user_id);
        let fabushi_dir = user_local_fabushi_dir(&prefs_dir, user_id);
        let cached_subs = self.magpie_local.discovered_items.clone();
        let client = self.client.clone();
        let base_url = self.client.http_client.base_url.clone();
        let runtime = self.client.runtime().clone();

        enum BootstrapOutcome {
            Unlocked(UnlockedVaultRuntimeBundle),
            AwaitingDeviceApproval {
                request_id: String,
                verification_code: String,
                expires_at: i64,
                keypair: DeviceSyncKeypair,
            },
            NeedsRecoveryKey(Option<String>),
            LocalOnlyFallback {
                orchestrator: Arc<BotBrowserOrchestrator>,
                host: Option<Arc<FabushiBotHost>>,
                local_subs: Vec<ClientSubscriptionSyncItem>,
                credential_metas: Vec<CredentialMeta>,
            },
        }

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let outcome_res = runtime
                .spawn(async move {
                    let is_new_reg = matches!(
                        auth_bootstrap.as_ref(),
                        Some(PendingAuthVaultBootstrap::SplitKeyV2 {
                            is_new_registration: true,
                            ..
                        })
                    );
                    let mut keyring_key =
                        if use_headless_file_keystore_directly() || is_new_reg {
                            None
                        } else {
                            let dir_for_load = user_browser_dir.clone();
                            tokio::task::spawn_blocking(move || {
                                load_device_vault_root_key(user_id, &dir_for_load)
                            })
                            .await
                            .unwrap_or(None)
                        };

                    if let Some(unlocked_key) = keyring_key.take() {
                        let cloud_env_res = client.get_user_vault_envelope().await;
                        let server_explicitly_empty = cloud_env_res
                            .as_ref()
                            .is_ok_and(|resp| resp.envelope.is_none());
                        if server_explicitly_empty && auth_bootstrap.is_some() {
                            let dir_for_del = user_browser_dir.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                delete_device_vault_root_key(user_id, &dir_for_del);
                            })
                            .await;
                        } else {
                            let existing_env = cloud_env_res.as_ref().ok().and_then(|resp| {
                                resp.envelope.as_ref().map(user_vault_envelope_from_proto)
                            });
                            if existing_env.is_some() {
                                let _ = execute_silent_split_key_upgrade(
                                    &client,
                                    user_id,
                                    &unlocked_key,
                                    existing_env.as_ref(),
                                    auth_bootstrap.as_ref(),
                                )
                                .await;
                            }
                            let pull_from_cloud = existing_env.is_some();
                            let bundle = finalize_unlocked_vault_runtime(
                                &client,
                                user_id,
                                &user_browser_dir,
                                &fabushi_dir,
                                &base_url,
                                &cached_subs,
                                Arc::new(unlocked_key),
                                false,
                                true,
                                false,
                                Vec::new(),
                                None,
                                false,
                                pull_from_cloud,
                            )
                            .await?;
                            return Ok::<_, String>(BootstrapOutcome::Unlocked(bundle));
                        }
                    }

                    // Keychain did not yield an unlocked root key (or was stale on fresh server DB).
                    let cloud_env_res = client.get_user_vault_envelope().await;
                    match cloud_env_res {
                        Ok(resp) if resp.envelope.is_none() => match &auth_bootstrap {
                            Some(PendingAuthVaultBootstrap::SplitKeyV2 {
                                split_kek,
                                auth_salt_b64,
                                kdf_params,
                                ..
                            }) => {
                                let kek_clone = split_kek.clone();
                                let salt_clone = auth_salt_b64.clone();
                                let params_copy = *kdf_params;
                                let browser_dir_clone = user_browser_dir.clone();
                                let (pre_exported, envelope, unlocked_key, recovery_key) =
                                    tokio::task::spawn_blocking(move || -> Result<_, String> {
                                        let pre_exported =
                                            export_unmigrated_local_credentials_if_any(
                                                &browser_dir_clone,
                                            );
                                        let (env, key, ek) =
                                            UserVaultCrypto::initialize_with_split_key(
                                                user_id,
                                                &kek_clone,
                                                &salt_clone,
                                                params_copy,
                                            )
                                            .map_err(|e| {
                                                format!("Split-Key vault initialization failed: {e}")
                                            })?;
                                        Ok((pre_exported, env, Arc::new(key), ek))
                                    })
                                    .await
                                    .map_err(|e| format!("vault init join error: {e}"))??;

                                let env_dto = user_vault_envelope_to_proto(&envelope);
                                client
                                    .put_user_vault_envelope(&env_dto)
                                    .await
                                    .map_err(|e| format!("Failed uploading split_key_v2 envelope: {e}"))?;

                                let _ = execute_silent_split_key_upgrade(
                                    &client,
                                    user_id,
                                    &unlocked_key,
                                    None,
                                    auth_bootstrap.as_ref(),
                                )
                                .await;

                                let bundle = finalize_unlocked_vault_runtime(
                                    &client,
                                    user_id,
                                    &user_browser_dir,
                                    &fabushi_dir,
                                    &base_url,
                                    &cached_subs,
                                    unlocked_key,
                                    true,
                                    false,
                                    true,
                                    pre_exported,
                                    Some(recovery_key),
                                    true,
                                    false,
                                )
                                .await?;
                                Ok(BootstrapOutcome::Unlocked(bundle))
                            }
                            Some(PendingAuthVaultBootstrap::PasswordlessDeviceOnly) => {
                                let browser_dir_clone = user_browser_dir.clone();
                                let (pre_exported, envelope, unlocked_key, recovery_key) =
                                    tokio::task::spawn_blocking(move || -> Result<_, String> {
                                        let pre_exported =
                                            export_unmigrated_local_credentials_if_any(
                                                &browser_dir_clone,
                                            );
                                        let (env, key, ek) =
                                            UserVaultCrypto::initialize_keychain_only(user_id)
                                                .map_err(|e| {
                                                    format!(
                                                        "Device keychain vault initialization failed: {e}"
                                                    )
                                                })?;
                                        Ok((pre_exported, env, Arc::new(key), ek))
                                    })
                                    .await
                                    .map_err(|e| format!("vault init join error: {e}"))??;

                                let env_dto = user_vault_envelope_to_proto(&envelope);
                                client
                                    .put_user_vault_envelope(&env_dto)
                                    .await
                                    .map_err(|e| {
                                        format!("Failed uploading device_keychain_v2 envelope: {e}")
                                    })?;

                                let bundle = finalize_unlocked_vault_runtime(
                                    &client,
                                    user_id,
                                    &user_browser_dir,
                                    &fabushi_dir,
                                    &base_url,
                                    &cached_subs,
                                    unlocked_key,
                                    true,
                                    false,
                                    true,
                                    pre_exported,
                                    Some(recovery_key),
                                    true,
                                    false,
                                )
                                .await?;
                                Ok(BootstrapOutcome::Unlocked(bundle))
                            }
                            None => {
                                // Restored session without password bootstrap and no cloud envelope yet:
                                // keep local orchestrator + Fabushi operational cleanly.
                                let (orch, host, local_subs, metas) =
                                    tokio::task::spawn_blocking(move || -> Result<_, String> {
                                        let vault = Arc::new(
                                            open_default_local_vault(
                                                &user_browser_dir.join("vault.db"),
                                                &user_browser_dir.join("vault.key"),
                                            )
                                            .map_err(|e| format!("local vault open error: {e}"))?,
                                        );
                                        let metas = collect_credential_metas_from_vault(&vault);
                                        let orch = Arc::new(BotBrowserOrchestrator::new(
                                            OrchestratorConfig::new(&user_browser_dir),
                                            vault,
                                        ));
                                        let subs = if !cached_subs.is_empty() {
                                            cached_subs
                                        } else {
                                            discover_local_subscriptions_for_server(&base_url)
                                        };
                                        let host =
                                            build_local_fabushi_host(&fabushi_dir, Arc::clone(&orch))
                                                .ok();
                                        Ok((orch, host, subs, metas))
                                    })
                                    .await
                                    .map_err(|e| format!("local vault task error: {e}"))??;
                                Ok(BootstrapOutcome::LocalOnlyFallback {
                                    orchestrator: orch,
                                    host,
                                    local_subs,
                                    credential_metas: metas,
                                })
                            }
                        },
                        Ok(resp) => {
                            let env_dto = resp.envelope.expect("checked Some above");
                            let envelope = user_vault_envelope_from_proto(&env_dto);

                            if let Some(PendingAuthVaultBootstrap::SplitKeyV2 {
                                split_kek, ..
                            }) = &auth_bootstrap
                            {
                                if matches!(
                                    envelope.kdf_alg.as_str(),
                                    USER_VAULT_KDF_ALG_SPLIT_KEY_V2 | USER_VAULT_KDF_ALG_ARGON2ID
                                ) {
                                    let kek_clone = split_kek.clone();
                                    let env_clone = envelope.clone();
                                    let split_unlock = tokio::task::spawn_blocking(move || {
                                        UserVaultCrypto::unlock_with_split_key(
                                            user_id, &env_clone, &kek_clone,
                                        )
                                    })
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok());

                                    if let Some(unlocked_key) = split_unlock {
                                        let unlocked_arc = Arc::new(unlocked_key);
                                        let _ = execute_silent_split_key_upgrade(
                                            &client,
                                            user_id,
                                            &unlocked_arc,
                                            Some(&envelope),
                                            auth_bootstrap.as_ref(),
                                        )
                                        .await;

                                        let bundle = finalize_unlocked_vault_runtime(
                                            &client,
                                            user_id,
                                            &user_browser_dir,
                                            &fabushi_dir,
                                            &base_url,
                                            &cached_subs,
                                            unlocked_arc,
                                            true,
                                            false,
                                            false,
                                            Vec::new(),
                                            None,
                                            false,
                                            true,
                                        )
                                        .await?;
                                        return Ok(BootstrapOutcome::Unlocked(bundle));
                                    }
                                }
                            }

                            // Fresh device on passwordless/device_keychain_v2 (or mismatched split-key):
                            // automatically request X25519 Device Trust Ring approval cleanly.
                            let keypair = DeviceSyncKeypair::generate();
                            let body = DeviceSyncRequestBody {
                                device_name: resolve_device_display_name(),
                                device_public_key_b64: keypair.public_key_b64(),
                            };
                            match client.device_sync_request(&body).await {
                                Ok(sync_resp) => Ok(BootstrapOutcome::AwaitingDeviceApproval {
                                    request_id: sync_resp.request_id,
                                    verification_code: sync_resp.verification_code,
                                    expires_at: sync_resp.expires_at,
                                    keypair,
                                }),
                                Err(e) => Ok(BootstrapOutcome::NeedsRecoveryKey(Some(format!("{e}")))),
                            }
                        }
                        Err(_) => {
                            let (orch, host, local_subs, metas) =
                                tokio::task::spawn_blocking(move || -> Result<_, String> {
                                    let vault = Arc::new(
                                        open_default_local_vault(
                                            &user_browser_dir.join("vault.db"),
                                            &user_browser_dir.join("vault.key"),
                                        )
                                        .map_err(|e| format!("local vault open error: {e}"))?,
                                    );
                                    let metas = collect_credential_metas_from_vault(&vault);
                                    let orch = Arc::new(BotBrowserOrchestrator::new(
                                        OrchestratorConfig::new(&user_browser_dir),
                                        vault,
                                    ));
                                    let subs = if !cached_subs.is_empty() {
                                        cached_subs
                                    } else {
                                        discover_local_subscriptions_for_server(&base_url)
                                    };
                                    let host =
                                        build_local_fabushi_host(&fabushi_dir, Arc::clone(&orch))
                                            .ok();
                                    Ok((orch, host, subs, metas))
                                })
                                .await
                                .map_err(|e| format!("local vault task error: {e}"))??;
                            Ok(BootstrapOutcome::LocalOnlyFallback {
                                orchestrator: orch,
                                host,
                                local_subs,
                                credential_metas: metas,
                            })
                        }
                    }
                })
                .await
                .map_err(|e| format!("vault bootstrap join error: {e}"))
                .and_then(|r| r);

            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.bot_browser.vault_bootstrap_generation != boot_gen
                {
                    return;
                }
                let lang = shell.active_lang_id();
                match outcome_res {
                    Ok(BootstrapOutcome::Unlocked(bundle)) => {
                        shell.apply_unlocked_vault_bundle_to_shell(bundle, cx);
                    }
                    Ok(BootstrapOutcome::AwaitingDeviceApproval {
                        request_id,
                        verification_code,
                        expires_at,
                        keypair,
                    }) => {
                        shell.bot_browser.vault_locked_needs_passphrase = true;
                        shell.bot_browser.vault_custody_status =
                            VaultCustodyStatus::AwaitingDeviceApproval {
                                request_id: request_id.clone(),
                                verification_code: verification_code.clone(),
                                expires_at,
                            };
                        shell.bot_browser.vault_custody_status_note = Some(if lang.starts_with("zh") {
                            format!(
                                "请在已信任设备上核对验证码 {verification_code} 批准同步，或在下方输入恢复密钥解锁"
                            )
                        } else {
                            format!(
                                "Verify code {verification_code} on a trusted device to approve sync, or unlock with your Emergency Kit below"
                            )
                        });
                        shell.bot_browser.pending_device_sync = Some(PendingDeviceSyncSession {
                            request_id: request_id.clone(),
                            verification_code: verification_code.clone(),
                            expires_at,
                            keypair: Some(keypair),
                        });
                        shell.window_runtime.enqueue_toast(
                            if lang.starts_with("zh") {
                                format!("多端同步验证码 {verification_code}：请在已登录设备（设置 → 隐私与安全）批准或输入恢复密钥")
                            } else {
                                format!("Device sync code {verification_code}: approve on a trusted device or unlock in Privacy & Security")
                            },
                            4500,
                        );
                        shell.sync_vault_custody_to_settings(cx);
                        shell.spawn_device_sync_status_poller(request_id, cx);
                    }
                    Ok(BootstrapOutcome::NeedsRecoveryKey(reason)) => {
                        shell.bot_browser.vault_locked_needs_passphrase = true;
                        shell.bot_browser.vault_custody_status =
                            VaultCustodyStatus::NeedsRecoveryKey;
                        let base_note = tr_str(
                            lang,
                            "请在「设置 → 隐私与安全」中输入恢复密钥 EK-... 解锁保管库，或发起多端同步请求",
                            "Enter your Emergency Kit EK-... or request device sync in Settings → Privacy & Security",
                        );
                        let note = match reason {
                            Some(r) => format!("{base_note} ({r})"),
                            None => base_note.to_string(),
                        };
                        shell.bot_browser.vault_custody_status_note = Some(note);
                        shell.sync_vault_custody_to_settings(cx);
                    }
                    Ok(BootstrapOutcome::LocalOnlyFallback {
                        orchestrator,
                        host,
                        local_subs,
                        credential_metas,
                    }) => {
                        shell.bot_browser.vault_locked_needs_passphrase = false;
                        shell.bot_browser.vault_custody_status =
                            VaultCustodyStatus::LockedOrUninitialized;
                        shell.bot_browser.cached_credential_metas = credential_metas;
                        shell.attach_bot_browser_orchestrator(orchestrator, cx);
                        if let Some(h) = host {
                            let merged_subs = if !shell.magpie_local.discovered_items.is_empty() {
                                shell.magpie_local.discovered_items.clone()
                            } else {
                                local_subs
                            };
                            if !merged_subs.is_empty() {
                                let uid_str = user_id.to_string();
                                let _ = hydrate_local_subscriptions_into_fabushi_for_user(
                                    &h,
                                    &merged_subs,
                                    Some(&uid_str),
                                );
                                shell.bot_browser.hydrated_subscriptions_fingerprint =
                                    Some(discovered_subscriptions_fingerprint(&merged_subs));
                            }
                            shell.attach_local_fabushi_host(h);
                        }
                        shell.sync_vault_custody_to_settings(cx);
                    }
                    Err(err) => {
                        shell.window_runtime.enqueue_toast(err, 3200);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_unlocked_vault_bundle_to_shell(
        &mut self,
        bundle: UnlockedVaultRuntimeBundle,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id();
        self.bot_browser.unlocked_vault_key = Some(Arc::clone(&bundle.unlocked_key));
        self.bot_browser.vault_sync_revision = self
            .bot_browser
            .vault_sync_revision
            .max(bundle.pull_report.latest_revision);
        self.bot_browser.vault_locked_needs_passphrase = false;
        self.bot_browser.vault_custody_status = VaultCustodyStatus::UnlockedAndSynced;
        self.bot_browser.vault_keychain_saved = bundle.keychain_saved;
        self.bot_browser.vault_last_sync_at_ms = Some(current_epoch_ms());
        self.bot_browser.vault_last_sync_count = bundle
            .credential_metas
            .len()
            .max(bundle.pull_report.passwords_applied + bundle.pull_report.cookies_applied);
        self.bot_browser.pending_device_sync = None;
        self.bot_browser.device_sync_poll_generation =
            self.bot_browser.device_sync_poll_generation.wrapping_add(1);
        self.bot_browser.pending_device_approvals = bundle.pending_approvals;
        self.bot_browser.cached_credential_metas = bundle.credential_metas;

        if let Some(ek) = bundle.recovery_key {
            self.bot_browser.last_recovery_key_notice = Some(ek);
            self.bot_browser.first_creation_banner_open = bundle.is_first_creation;
            self.bot_browser.vault_custody_status_note = Some(
                tr_str(
                    lang,
                    "已生成本次恢复密钥（仅本地可见），建议在下方复制并妥善保存",
                    "Emergency Kit generated locally — copy and store it safely below",
                )
                .to_string(),
            );
        } else {
            self.bot_browser.vault_custody_status_note = None;
        }

        self.attach_bot_browser_orchestrator(bundle.orchestrator, cx);
        if let Some(host) = bundle.host {
            let uid_str = self.session_sync.user_id.map(|u| u.to_string());
            if let Some(ref oauth_bundle) = bundle.pull_report.ai_oauth_bundle {
                let _ = hydrate_ai_oauth_vault_bundle_into_fabushi_for_user(
                    &host,
                    oauth_bundle,
                    uid_str.as_deref(),
                );
            }
            let merged_subs = if !self.magpie_local.discovered_items.is_empty() {
                self.magpie_local.discovered_items.clone()
            } else {
                bundle.local_subs
            };
            if !merged_subs.is_empty() {
                let _ = hydrate_local_subscriptions_into_fabushi_for_user(
                    &host,
                    &merged_subs,
                    uid_str.as_deref(),
                );
                self.bot_browser.hydrated_subscriptions_fingerprint =
                    Some(discovered_subscriptions_fingerprint(&merged_subs));
            }
            self.attach_local_fabushi_host(host);
        }
        self.sync_vault_custody_to_settings(cx);
    }

    /// Synchronizes current custody status, cached credential metadata (no secrets),
    /// pending device sync approvals, and session Emergency Kit into `SettingsPrivacySecuritySections`.
    pub fn sync_vault_custody_to_settings(&mut self, cx: &mut Context<Self>) {
        let status = self.bot_browser.vault_custody_status.clone();
        let keychain_saved = self.bot_browser.vault_keychain_saved;
        let last_sync_label = self
            .bot_browser
            .vault_last_sync_at_ms
            .map(format_ms_human)
            .filter(|s| !s.is_empty());
        let last_sync_count = self.bot_browser.vault_last_sync_count;
        let sync_revision = self.bot_browser.vault_sync_revision;
        let status_note = self.bot_browser.vault_custody_status_note.clone();
        let session_ek = self.bot_browser.last_recovery_key_notice.clone();
        let pending_reqs = self.bot_browser.pending_device_approvals.clone();
        let saved_creds = self.bot_browser.cached_credential_metas.clone();

        self.settings_router.update(cx, |router, rcx| {
            if let Some(sp) = router.settings_privacy.clone() {
                sp.update(rcx, |sec, scx| {
                    sec.sync_vault_custody_state(
                        status,
                        keychain_saved,
                        last_sync_label,
                        last_sync_count,
                        sync_revision,
                        status_note,
                        session_ek,
                        pending_reqs,
                        saved_creds,
                    );
                    scx.notify();
                });
            }
        });
    }

    /// Polls `/api/v1/user-vault/device-sync/{request_id}/status` until approved/denied/expired
    /// or session changes. Atomically consumes the ephemeral X25519 keypair on approval.
    pub fn spawn_device_sync_status_poller(&mut self, request_id: String, cx: &mut Context<Self>) {
        let Some(user_id) = self.session_sync.user_id else {
            return;
        };
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        self.bot_browser.device_sync_poll_generation =
            self.bot_browser.device_sync_poll_generation.wrapping_add(1);
        let poll_gen = self.bot_browser.device_sync_poll_generation;
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();
        let prefs_dir = self.preferences_dir.clone();
        let user_browser_dir = user_bot_browser_dir(&prefs_dir, user_id);
        let fabushi_dir = user_local_fabushi_dir(&prefs_dir, user_id);
        let base_url = self.client.http_client.base_url.clone();
        let cached_subs = self.magpie_local.discovered_items.clone();
        let poll_interval = if use_headless_file_keystore_directly() {
            Duration::from_millis(120)
        } else {
            Duration::from_secs(2)
        };

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            for _ in 0..300 {
                app.background_executor().timer(poll_interval).await;

                let still_active = weak
                    .update(app, |shell, _| {
                        shell.is_authenticated
                            && shell.session_sync.auth_generation == auth_gen
                            && shell.bot_browser.vault_bootstrap_generation == boot_gen
                            && shell.bot_browser.device_sync_poll_generation == poll_gen
                            && shell.bot_browser.unlocked_vault_key.is_none()
                            && shell.bot_browser.pending_device_sync.as_ref().is_some_and(
                                |p| p.request_id == request_id && p.keypair.is_some(),
                            )
                    })
                    .unwrap_or(false);
                if !still_active {
                    break;
                }

                let status_res = runtime
                    .spawn({
                        let client = client.clone();
                        let rid = request_id.clone();
                        async move { client.device_sync_status(&rid).await }
                    })
                    .await;

                let resp = match status_res {
                    Ok(Ok(r)) => r,
                    Ok(Err(_)) | Err(_) => continue,
                };

                match resp.status.as_str() {
                    "pending" => continue,
                    "approved" => {
                        let (Some(eph), Some(ct), Some(nonce)) = (
                            resp.approver_ephemeral_pub_b64,
                            resp.encrypted_root_key_b64,
                            resp.nonce_b64,
                        ) else {
                            continue;
                        };
                        let keypair_opt = weak
                            .update(app, |shell, _| {
                                if shell.session_sync.auth_generation == auth_gen
                                    && shell.bot_browser.vault_bootstrap_generation == boot_gen
                                    && shell.bot_browser.device_sync_poll_generation == poll_gen
                                {
                                    shell
                                        .bot_browser
                                        .pending_device_sync
                                        .as_mut()
                                        .and_then(|p| p.keypair.take())
                                } else {
                                    None
                                }
                            })
                            .ok()
                            .flatten();

                        let Some(keypair) = keypair_opt else {
                            break;
                        };
                        let transfer = DeviceKeyTransfer {
                            approver_ephemeral_pub_b64: eph,
                            encrypted_root_key_b64: ct,
                            nonce_b64: nonce,
                        };

                        let bundle_res = runtime
                            .spawn({
                                let client = client.clone();
                                let rid = request_id.clone();
                                let user_browser_dir = user_browser_dir.clone();
                                let fabushi_dir = fabushi_dir.clone();
                                let base_url = base_url.clone();
                                let cached_subs = cached_subs.clone();
                                async move {
                                    let unlocked_key = tokio::task::spawn_blocking(move || {
                                        keypair
                                            .open_root_key_transfer(&rid, user_id, &transfer)
                                            .map_err(|e| {
                                                format!("Device sync key unwrap failed: {e}")
                                            })
                                    })
                                    .await
                                    .map_err(|e| format!("device sync unwrap join error: {e}"))??;

                                    finalize_unlocked_vault_runtime(
                                        &client,
                                        user_id,
                                        &user_browser_dir,
                                        &fabushi_dir,
                                        &base_url,
                                        &cached_subs,
                                        Arc::new(unlocked_key),
                                        true,
                                        false,
                                        false,
                                        Vec::new(),
                                        None,
                                        false,
                                        true,
                                    )
                                    .await
                                }
                            })
                            .await
                            .map_err(|e| format!("device sync finalize join error: {e}"))
                            .and_then(|r| r);

                        let _ = weak.update(app, |shell, cx| {
                            if !shell.is_authenticated
                                || shell.session_sync.auth_generation != auth_gen
                                || shell.bot_browser.vault_bootstrap_generation != boot_gen
                            {
                                return;
                            }
                            let lang = shell.active_lang_id();
                            match bundle_res {
                                Ok(bundle) => {
                                    shell.apply_unlocked_vault_bundle_to_shell(bundle, cx);
                                    shell.window_runtime.enqueue_toast(
                                        tr_str(
                                            lang,
                                            "新设备同步已批准，安全保管库已自动解锁并同步",
                                            "Device sync approved — vault unlocked & synced automatically",
                                        ),
                                        3500,
                                    );
                                }
                                Err(err) => {
                                    shell.bot_browser.pending_device_sync = None;
                                    shell.bot_browser.vault_custody_status =
                                        VaultCustodyStatus::NeedsRecoveryKey;
                                    shell.bot_browser.vault_custody_status_note = Some(err.clone());
                                    shell.window_runtime.enqueue_toast(err, 3500);
                                    shell.sync_vault_custody_to_settings(cx);
                                }
                            }
                            cx.notify();
                        });
                        break;
                    }
                    "denied" | "expired" | "consumed" => {
                        let status_str = resp.status.clone();
                        let _ = weak.update(app, |shell, cx| {
                            if !shell.is_authenticated
                                || shell.session_sync.auth_generation != auth_gen
                                || shell.bot_browser.vault_bootstrap_generation != boot_gen
                                || shell.bot_browser.device_sync_poll_generation != poll_gen
                            {
                                return;
                            }
                            let lang = shell.active_lang_id();
                            shell.bot_browser.pending_device_sync = None;
                            shell.bot_browser.vault_custody_status =
                                VaultCustodyStatus::NeedsRecoveryKey;
                            shell.bot_browser.vault_custody_status_note = Some(if lang.starts_with("zh") {
                                format!("设备同步请求已结束（{status_str}），可重新发起请求或输入恢复密钥解锁")
                            } else {
                                format!("Device sync request ended ({status_str}). Retry request or enter Emergency Kit.")
                            });
                            shell.sync_vault_custody_to_settings(cx);
                            cx.notify();
                        });
                        break;
                    }
                    _ => continue,
                }
            }
        })
        .detach();
    }

    /// Fetches pending X25519 device sync approval requests and refreshes cached credential metadata.
    pub fn refresh_pending_device_sync_requests(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated {
            return;
        }
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let orch = self.bot_browser.orchestrator.clone();
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let (rows, metas_opt) = runtime
                .spawn(async move {
                    let rows = fetch_pending_device_sync_rows(&client).await;
                    let metas_opt = if let Some(orch_ref) = orch {
                        tokio::task::spawn_blocking(move || {
                            collect_credential_metas_from_vault(orch_ref.vault())
                        })
                        .await
                        .ok()
                    } else {
                        None
                    };
                    (rows, metas_opt)
                })
                .await
                .unwrap_or_default();

            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.bot_browser.vault_bootstrap_generation != boot_gen
                {
                    return;
                }
                shell.bot_browser.pending_device_approvals = rows;
                if let Some(metas) = metas_opt {
                    shell.bot_browser.cached_credential_metas = metas;
                }
                shell.sync_vault_custody_to_settings(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Initiates (or retries) an X25519 Device Trust Ring sync request from Settings → Privacy & Security.
    pub fn initiate_device_sync_request_from_settings(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.session_sync.user_id.is_none() {
            return;
        }
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let keypair = DeviceSyncKeypair::generate();
            let req_body = DeviceSyncRequestBody {
                device_name: resolve_device_display_name(),
                device_public_key_b64: keypair.public_key_b64(),
            };
            let res = runtime
                .spawn(async move { client.device_sync_request(&req_body).await })
                .await
                .map_err(|e| format!("device sync task join error: {e}"))
                .and_then(|r| r.map_err(|e| format!("Failed creating device sync request: {e}")));

            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.bot_browser.vault_bootstrap_generation != boot_gen
                {
                    return;
                }
                let lang = shell.active_lang_id();
                match res {
                    Ok(sync_resp) => {
                        shell.bot_browser.vault_locked_needs_passphrase = true;
                        shell.bot_browser.vault_custody_status =
                            VaultCustodyStatus::AwaitingDeviceApproval {
                                request_id: sync_resp.request_id.clone(),
                                verification_code: sync_resp.verification_code.clone(),
                                expires_at: sync_resp.expires_at,
                            };
                        shell.bot_browser.vault_custody_status_note = Some(if lang.starts_with("zh") {
                            format!(
                                "已发送设备同步请求，请在可信设备核对验证码 {}",
                                sync_resp.verification_code
                            )
                        } else {
                            format!(
                                "Device sync requested — verify code {} on a trusted device",
                                sync_resp.verification_code
                            )
                        });
                        shell.bot_browser.pending_device_sync = Some(PendingDeviceSyncSession {
                            request_id: sync_resp.request_id.clone(),
                            verification_code: sync_resp.verification_code,
                            expires_at: sync_resp.expires_at,
                            keypair: Some(keypair),
                        });
                        shell.sync_vault_custody_to_settings(cx);
                        shell.spawn_device_sync_status_poller(sync_resp.request_id, cx);
                    }
                    Err(err) => {
                        shell.bot_browser.vault_custody_status_note = Some(err.clone());
                        shell.window_runtime.enqueue_toast(err, 3200);
                        shell.sync_vault_custody_to_settings(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Unlocks the cloud E2EE vault using an Emergency Kit submitted in Settings → Privacy & Security.
    pub fn unlock_vault_with_recovery_key_from_settings(
        &mut self,
        recovery_key: Zeroizing<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(user_id) = self.session_sync.user_id else {
            return;
        };
        if !self.is_authenticated {
            return;
        }
        self.execute_local_vault_unlock(user_id, None, Some(recovery_key), None, cx);
    }

    /// Approves a pending X25519 Device Trust Ring sync request by sealing our `UnlockedUserVaultKey`
    /// with an ephemeral X25519 ECDH key for the target device's public key.
    pub fn approve_device_sync_request_from_settings(
        &mut self,
        request_id: String,
        device_public_key_b64: String,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id();
        let (Some(unlocked_key), Some(user_id)) = (
            self.bot_browser.unlocked_vault_key.clone(),
            self.session_sync.user_id,
        ) else {
            self.window_runtime.enqueue_toast(
                tr_str(
                    lang,
                    "请先解锁本机保管库后再批准其他设备同步",
                    "Unlock this device's vault before approving another device",
                ),
                3200,
            );
            return;
        };
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = runtime
                .spawn(async move {
                    let rid_seal = request_id.clone();
                    let transfer = tokio::task::spawn_blocking(move || {
                        DeviceSyncKeypair::seal_root_key_for_device(
                            &rid_seal,
                            user_id,
                            &unlocked_key,
                            &device_public_key_b64,
                        )
                        .map_err(|e| format!("Failed sealing vault root key for device: {e}"))
                    })
                    .await
                    .map_err(|e| format!("seal task join error: {e}"))??;

                    let req = ApproveDeviceSyncRequest {
                        approver_ephemeral_pub_b64: transfer.approver_ephemeral_pub_b64,
                        encrypted_root_key_b64: transfer.encrypted_root_key_b64,
                        nonce_b64: transfer.nonce_b64,
                    };
                    client
                        .device_sync_approve(&request_id, &req)
                        .await
                        .map_err(|e| format!("Failed approving device sync: {e}"))?;
                    let pending = fetch_pending_device_sync_rows(&client).await;
                    Ok::<_, String>((request_id, pending))
                })
                .await
                .map_err(|e| format!("approve task join error: {e}"))
                .and_then(|r| r);

            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.bot_browser.vault_bootstrap_generation != boot_gen
                {
                    return;
                }
                let lang = shell.active_lang_id();
                match res {
                    Ok((_rid, pending)) => {
                        shell.bot_browser.pending_device_approvals = pending;
                        shell.window_runtime.enqueue_toast(
                            tr_str(lang, "已批准设备同步请求", "Approved device sync request"),
                            2800,
                        );
                    }
                    Err(err) => {
                        shell.bot_browser.vault_custody_status_note = Some(err.clone());
                        shell.window_runtime.enqueue_toast(err, 3200);
                    }
                }
                shell.sync_vault_custody_to_settings(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Denies a pending X25519 Device Trust Ring sync request.
    pub fn deny_device_sync_request_from_settings(
        &mut self,
        request_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated {
            return;
        }
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = runtime
                .spawn(async move {
                    client
                        .device_sync_deny(&request_id)
                        .await
                        .map_err(|e| format!("Failed denying device sync: {e}"))?;
                    let pending = fetch_pending_device_sync_rows(&client).await;
                    Ok::<_, String>(pending)
                })
                .await
                .map_err(|e| format!("deny task join error: {e}"))
                .and_then(|r| r);

            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.bot_browser.vault_bootstrap_generation != boot_gen
                {
                    return;
                }
                let lang = shell.active_lang_id();
                match res {
                    Ok(pending) => {
                        shell.bot_browser.pending_device_approvals = pending;
                        shell.window_runtime.enqueue_toast(
                            tr_str(lang, "已拒绝设备同步请求", "Denied device sync request"),
                            2400,
                        );
                    }
                    Err(err) => {
                        shell.window_runtime.enqueue_toast(err, 3200);
                    }
                }
                shell.sync_vault_custody_to_settings(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Updates `ReusePolicy` locally for `credential_id`, re-seals & uploads to E2EE vault if unlocked,
    /// and refreshes Settings metadata without ever exposing credential secrets.
    pub fn toggle_credential_auto_reuse_from_settings(
        &mut self,
        credential_id: String,
        allow_auto_reuse: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(orch) = self.bot_browser.orchestrator.clone() else {
            return;
        };
        let policy = if allow_auto_reuse {
            ReusePolicy::AutoReuse
        } else {
            ReusePolicy::ConfirmEveryTime
        };
        let unlocked_key = self.bot_browser.unlocked_vault_key.clone();
        let user_id_opt = self.session_sync.user_id;
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let mut known_rev = self.bot_browser.vault_sync_revision;
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = runtime
                .spawn(async move {
                    let orch_update = Arc::clone(&orch);
                    let cid_update = credential_id.clone();
                    let metas = tokio::task::spawn_blocking(move || -> Result<_, String> {
                        orch_update
                            .vault()
                            .update_reuse_policy(&cid_update, policy)
                            .map_err(|e| format!("Failed updating reuse policy: {e}"))?;
                        Ok(collect_credential_metas_from_vault(orch_update.vault()))
                    })
                    .await
                    .map_err(|e| format!("reuse update join error: {e}"))??;

                    let mut pushed_rev = None;
                    if let (Some(key), Some(uid)) = (unlocked_key, user_id_opt) {
                        let orch_exp = Arc::clone(&orch);
                        let cid_exp = credential_id.clone();
                        if let Ok(Ok(record)) = tokio::task::spawn_blocking(move || {
                            orch_exp.vault().export_for_sync(&cid_exp)
                        })
                        .await
                        {
                            if let Ok(bytes) = serde_json::to_vec(&record).map(Zeroizing::new) {
                                if let Ok(rev) = seal_and_put_vault_item(
                                    &client,
                                    &key,
                                    uid,
                                    UserVaultCategory::Password,
                                    &credential_id,
                                    &bytes,
                                    &mut known_rev,
                                )
                                .await
                                {
                                    pushed_rev = Some(rev);
                                }
                            }
                        }
                    }
                    Ok::<_, String>((metas, pushed_rev))
                })
                .await
                .map_err(|e| format!("toggle task join error: {e}"))
                .and_then(|r| r);

            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.bot_browser.vault_bootstrap_generation != boot_gen
                {
                    return;
                }
                match res {
                    Ok((metas, pushed_rev)) => {
                        shell.bot_browser.cached_credential_metas = metas;
                        if let Some(rev) = pushed_rev {
                            shell.bot_browser.vault_sync_revision =
                                shell.bot_browser.vault_sync_revision.max(rev);
                        }
                        shell.sync_vault_custody_to_settings(cx);
                    }
                    Err(err) => {
                        shell.window_runtime.enqueue_toast(err, 3200);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Deletes a saved site credential locally and emits an E2EE tombstone upstream if cloud vault is unlocked.
    pub fn delete_credential_from_settings(
        &mut self,
        credential_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(orch) = self.bot_browser.orchestrator.clone() else {
            return;
        };
        let unlocked_key = self.bot_browser.unlocked_vault_key.clone();
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = runtime
                .spawn(async move {
                    let orch_del = Arc::clone(&orch);
                    let cid_del = credential_id.clone();
                    let metas = tokio::task::spawn_blocking(move || -> Result<_, String> {
                        orch_del
                            .vault()
                            .delete_credential_if_exists(&cid_del)
                            .map_err(|e| format!("Failed deleting local credential: {e}"))?;
                        Ok(collect_credential_metas_from_vault(orch_del.vault()))
                    })
                    .await
                    .map_err(|e| format!("delete task join error: {e}"))??;

                    let mut deleted_rev = None;
                    if unlocked_key.is_some() {
                        if let Ok(resp) = client
                            .delete_user_vault_item(
                                USER_VAULT_CATEGORY_PASSWORD,
                                &credential_id,
                                None,
                            )
                            .await
                        {
                            deleted_rev = Some(resp.revision);
                        }
                    }
                    Ok::<_, String>((metas, deleted_rev))
                })
                .await
                .map_err(|e| format!("delete task join error: {e}"))
                .and_then(|r| r);

            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.bot_browser.vault_bootstrap_generation != boot_gen
                {
                    return;
                }
                let lang = shell.active_lang_id();
                match res {
                    Ok((metas, deleted_rev)) => {
                        shell.bot_browser.vault_last_sync_count = metas.len();
                        shell.bot_browser.cached_credential_metas = metas;
                        if let Some(rev) = deleted_rev {
                            shell.bot_browser.vault_sync_revision =
                                shell.bot_browser.vault_sync_revision.max(rev);
                        }
                        shell.sync_vault_custody_to_settings(cx);
                        shell.window_runtime.enqueue_toast(
                            tr_str(lang, "已从保管库移除该站点凭据", "Removed credential from vault"),
                            2400,
                        );
                    }
                    Err(err) => {
                        shell.window_runtime.enqueue_toast(err, 3200);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Intercepts `/vault-setup`, `/vault-unlock`, `/vault-recover`, `/vault-sync` strictly locally
    /// before chat stage insertion or `send_message_durable` — NEVER sends passphrases or recovery keys to cloud.
    pub fn try_handle_local_vault_command(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(cmd) = parse_local_vault_command(text) else {
            return false;
        };
        let lang = self.active_lang_id();
        if !self.is_authenticated {
            self.window_runtime.enqueue_toast(
                tr_str(lang, "请先登录后再操作安全保险库", "Sign in before running vault commands"),
                2600,
            );
            return true;
        }
        let Some(user_id) = self.session_sync.user_id else {
            return true;
        };
        let active_chat_opt = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        );

        match cmd {
            LocalVaultCommand::UsageError { message } => {
                self.window_runtime.enqueue_toast(message.clone(), 2800);
                self.append_local_vault_notice(active_chat_opt, "vault_usage", message, cx);
                cx.notify();
            }
            LocalVaultCommand::Setup { passphrase } => {
                self.execute_local_vault_setup(user_id, passphrase, active_chat_opt, cx);
            }
            LocalVaultCommand::Unlock { passphrase } => {
                self.execute_local_vault_unlock(
                    user_id,
                    Some(passphrase),
                    None,
                    active_chat_opt,
                    cx,
                );
            }
            LocalVaultCommand::Recover { recovery_key } => {
                self.execute_local_vault_unlock(
                    user_id,
                    None,
                    Some(recovery_key),
                    active_chat_opt,
                    cx,
                );
            }
            LocalVaultCommand::Sync => {
                self.execute_local_vault_sync(user_id, active_chat_opt, cx);
            }
        }
        true
    }

    fn execute_local_vault_setup(
        &mut self,
        user_id: u64,
        passphrase: Zeroizing<String>,
        active_chat_opt: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id();
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let prefs_dir = self.preferences_dir.clone();
        let user_browser_dir = user_bot_browser_dir(&prefs_dir, user_id);
        let fabushi_dir = user_local_fabushi_dir(&prefs_dir, user_id);
        let existing_orch = self.bot_browser.orchestrator.clone();
        let existing_fabushi = self.bot_browser.local_fabushi.clone();
        let cached_subs = self.magpie_local.discovered_items.clone();
        let base_url = self.client.http_client.base_url.clone();
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        self.window_runtime.enqueue_toast(
            tr_str(lang, "正在初始化端到端加密保险库…", "Initializing E2EE user vault…"),
            2200,
        );

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let setup_res = runtime
                .spawn_blocking(move || -> Result<_, String> {
                let vault_db_path = user_browser_dir.join("vault.db");
                let fallback_key_path = user_browser_dir.join("vault.key");
                let pre_exported = if let Some(ref orch) = existing_orch {
                    orch.vault().export_all_for_sync().unwrap_or_default()
                } else if vault_db_path.exists() {
                    open_default_local_vault(&vault_db_path, &fallback_key_path)
                        .and_then(|v| v.export_all_for_sync())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };

                let kdf_params = if use_headless_file_keystore_directly() {
                    Argon2KdfParams::fast_for_tests()
                } else {
                    Argon2KdfParams::default()
                };
                let (unlocked_key, envelope, recovery_key) =
                    UserVaultCrypto::initialize_new(user_id, passphrase.as_str(), kdf_params)
                        .map_err(|e| format!("Vault crypto setup failed: {e}"))?;
                drop(passphrase);

                let keychain_saved =
                    save_device_vault_root_key(user_id, &user_browser_dir, &unlocked_key);

                let vault = Arc::new(
                    open_vault_with_e2ee_key_preserving(
                        &vault_db_path,
                        &unlocked_key,
                        pre_exported,
                        true,
                    )
                    .map_err(|e| format!("Failed opening E2EE vault DB: {e}"))?,
                );
                let metas = collect_credential_metas_from_vault(&vault);
                let orch = Arc::new(BotBrowserOrchestrator::new(
                    OrchestratorConfig::new(&user_browser_dir),
                    Arc::clone(&vault),
                ));
                let host = build_local_fabushi_host(&fabushi_dir, Arc::clone(&orch))?;
                let subs = if !cached_subs.is_empty() {
                    cached_subs
                } else {
                    discover_local_subscriptions_for_server(&base_url)
                };
                let uid_str = user_id.to_string();
                if !subs.is_empty() {
                    hydrate_local_subscriptions_into_fabushi_for_user(&host, &subs, Some(&uid_str));
                } else if let Some(ref prev_host) = existing_fabushi {
                    let prev_user_router = prev_host.core_engine().providers_for_user(&uid_str);
                    host.core_engine()
                        .providers_for_user(&uid_str)
                        .sync_from_router(&prev_user_router);
                    host.core_engine().persist_user_providers_state(&uid_str);
                    host.sync_all_providers();
                }
                let user_router = host.core_engine().providers_for_user(&uid_str);
                let ai_bundle = build_ai_oauth_vault_bundle(&subs, Some(&user_router))?;
                Ok((
                    Arc::new(unlocked_key),
                    envelope,
                    recovery_key,
                    vault,
                    orch,
                    host,
                    subs,
                    ai_bundle,
                    user_browser_dir.join("profiles"),
                    keychain_saved,
                    metas,
                ))
            })
            .await
            .map_err(|e| format!("vault setup task join error: {e}"))
            .and_then(|r| r);

            let (
                unlocked_key,
                envelope,
                recovery_key,
                vault,
                orch,
                host,
                subs,
                ai_bundle,
                profiles_root,
                keychain_saved,
                credential_metas,
            ) = match setup_res {
                Ok(v) => v,
                Err(err) => {
                    let _ = weak.update(app, |shell, cx| {
                        shell.window_runtime.enqueue_toast(err.clone(), 3500);
                        shell.append_local_vault_notice(active_chat_opt, "vault_err", err, cx);
                        cx.notify();
                    });
                    return;
                }
            };

            let env_dto = user_vault_envelope_to_proto(&envelope);
            let sync_res = runtime
                .spawn({
                    let client = client.clone();
                    let unlocked_key = Arc::clone(&unlocked_key);
                    async move {
                        client
                            .put_user_vault_envelope(&env_dto)
                            .await
                            .map_err(|e| format!("Failed uploading vault envelope: {e}"))?;
                        let mut known_rev = client
                            .pull_user_vault_items(None, None)
                            .await
                            .map(|r| r.latest_revision)
                            .unwrap_or(0);
                        push_all_local_vault_items(
                            &client,
                            &unlocked_key,
                            user_id,
                            &vault,
                            &profiles_root,
                            Some(&ai_bundle),
                            &mut known_rev,
                        )
                        .await
                    }
                })
                .await
                .map_err(|e| format!("vault sync task join error: {e}"))
                .and_then(|r| r);

            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.bot_browser.vault_bootstrap_generation != boot_gen
                {
                    return;
                }
                let lang = shell.active_lang_id();
                match sync_res {
                    Ok(report) => {
                        shell.bot_browser.unlocked_vault_key = Some(unlocked_key);
                        shell.bot_browser.vault_sync_revision = report.latest_revision;
                        shell.bot_browser.vault_locked_needs_passphrase = false;
                        shell.bot_browser.vault_custody_status =
                            VaultCustodyStatus::UnlockedAndSynced;
                        shell.bot_browser.vault_keychain_saved = keychain_saved;
                        shell.bot_browser.vault_last_sync_at_ms = Some(current_epoch_ms());
                        shell.bot_browser.vault_last_sync_count =
                            credential_metas.len().max(report.passwords_pushed + report.cookies_pushed);
                        shell.bot_browser.cached_credential_metas = credential_metas;
                        shell.bot_browser.last_recovery_key_notice = Some(recovery_key.clone());
                        shell.bot_browser.first_creation_banner_open = true;
                        shell.bot_browser.pending_device_sync = None;
                        shell.bot_browser.device_sync_poll_generation =
                            shell.bot_browser.device_sync_poll_generation.wrapping_add(1);
                        if !subs.is_empty() {
                            shell.bot_browser.hydrated_subscriptions_fingerprint =
                                Some(discovered_subscriptions_fingerprint(&subs));
                        }
                        shell.attach_bot_browser_orchestrator(orch, cx);
                        shell.attach_local_fabushi_host(host);
                        shell.sync_vault_custody_to_settings(cx);

                        let notice = if lang.starts_with("zh") {
                            format!(
                                "端到端加密保险库已启用并同步（密码 {} / Cookie {}）。\n请妥善保存一次性恢复密钥（仅本地显示，绝不上传云端）：\n{}",
                                report.passwords_pushed, report.cookies_pushed, recovery_key.as_str()
                            )
                        } else {
                            format!(
                                "E2EE Vault enabled & synced ({} password(s), {} cookie profile(s)).\nSave your one-time Recovery Key (local-only, NEVER uploaded):\n{}",
                                report.passwords_pushed, report.cookies_pushed, recovery_key.as_str()
                            )
                        };
                        shell.append_local_vault_notice(
                            active_chat_opt,
                            "vault_recovery_key",
                            notice,
                            cx,
                        );
                        shell.window_runtime.enqueue_toast(
                            tr_str(
                                lang,
                                "端到端加密保险库已启用，请保存本地显示的一次性恢复密钥",
                                "E2EE Vault enabled — save the local one-time Recovery Key",
                            ),
                            4500,
                        );
                    }
                    Err(err) => {
                        shell.window_runtime.enqueue_toast(err.clone(), 3500);
                        shell.append_local_vault_notice(active_chat_opt, "vault_err", err, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn execute_local_vault_unlock(
        &mut self,
        user_id: u64,
        passphrase: Option<Zeroizing<String>>,
        recovery_key: Option<Zeroizing<String>>,
        active_chat_opt: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id();
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let prefs_dir = self.preferences_dir.clone();
        let user_browser_dir = user_bot_browser_dir(&prefs_dir, user_id);
        let fabushi_dir = user_local_fabushi_dir(&prefs_dir, user_id);
        let cached_subs = self.magpie_local.discovered_items.clone();
        let base_url = self.client.http_client.base_url.clone();
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        self.window_runtime.enqueue_toast(
            tr_str(lang, "正在本地解锁并解密同步数据…", "Unlocking & decrypting vault locally…"),
            2200,
        );

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let env_res = runtime
                .spawn({
                    let client = client.clone();
                    async move { client.get_user_vault_envelope().await }
                })
                .await
                .map_err(|e| format!("envelope fetch task error: {e}"))
                .and_then(|r| r.map_err(|e| format!("Failed fetching cloud vault envelope: {e}")));

            let envelope_dto = match env_res {
                Ok(resp) => match resp.envelope {
                    Some(env) => env,
                    None => {
                        let _ = weak.update(app, |shell, cx| {
                            let msg = tr_str(
                                shell.active_lang_id(),
                                "云端尚未创建加密保险库，将在下次密码或扫码登录时自动初始化，也可在「设置 → 隐私与安全」查看状态",
                                "No cloud vault envelope found yet. It will initialize automatically on password sign-in, or inspect status in Settings → Privacy & Security.",
                            )
                            .to_string();
                            shell.window_runtime.enqueue_toast(msg.clone(), 3200);
                            shell.append_local_vault_notice(active_chat_opt, "vault_no_env", msg, cx);
                            cx.notify();
                        });
                        return;
                    }
                },
                Err(err) => {
                    let _ = weak.update(app, |shell, cx| {
                        shell.window_runtime.enqueue_toast(err.clone(), 3200);
                        shell.append_local_vault_notice(active_chat_opt, "vault_err", err, cx);
                        cx.notify();
                    });
                    return;
                }
            };

            let envelope = user_vault_envelope_from_proto(&envelope_dto);
            let unlock_res = runtime
                .spawn_blocking(move || -> Result<_, String> {
                let unlocked = if let Some(pw) = passphrase {
                    let res = UserVaultCrypto::unlock_with_passphrase(user_id, pw.as_str(), &envelope);
                    drop(pw);
                    res
                } else if let Some(rk) = recovery_key {
                    let res =
                        UserVaultCrypto::unlock_with_recovery_key(user_id, rk.as_str(), &envelope);
                    drop(rk);
                    res
                } else {
                    return Err("Missing passphrase or recovery key".to_string());
                }
                .map_err(|e| format!("Vault unlock failed: {e}"))?;

                let keychain_saved =
                    save_device_vault_root_key(user_id, &user_browser_dir, &unlocked);

                let vault_db_path = user_browser_dir.join("vault.db");
                let vault = Arc::new(
                    open_vault_with_e2ee_key_preserving(
                        &vault_db_path,
                        &unlocked,
                        Vec::new(),
                        false,
                    )
                    .map_err(|e| format!("Failed opening unlocked vault DB: {e}"))?,
                );
                let orch = Arc::new(BotBrowserOrchestrator::new(
                    OrchestratorConfig::new(&user_browser_dir),
                    Arc::clone(&vault),
                ));
                let host = build_local_fabushi_host(&fabushi_dir, Arc::clone(&orch))?;
                let local_subs = if !cached_subs.is_empty() {
                    cached_subs
                } else {
                    discover_local_subscriptions_for_server(&base_url)
                };
                Ok((
                    Arc::new(unlocked),
                    vault,
                    orch,
                    host,
                    local_subs,
                    user_browser_dir.join("profiles"),
                    keychain_saved,
                ))
            })
            .await
            .map_err(|e| format!("vault unlock task error: {e}"))
            .and_then(|r| r);

            let (unlocked_key, vault, orch, host, local_subs, profiles_root, keychain_saved) = match unlock_res {
                Ok(v) => v,
                Err(err) => {
                    let _ = weak.update(app, |shell, cx| {
                        shell.bot_browser.vault_custody_status_note = Some(err.clone());
                        shell.window_runtime.enqueue_toast(err.clone(), 3500);
                        shell.append_local_vault_notice(active_chat_opt, "vault_err", err, cx);
                        shell.sync_vault_custody_to_settings(cx);
                        cx.notify();
                    });
                    return;
                }
            };

            let pull_res = runtime
                .spawn({
                    let client = client.clone();
                    let unlocked_key = Arc::clone(&unlocked_key);
                    let vault = Arc::clone(&vault);
                    async move {
                        let report = pull_and_apply_user_vault_items(
                            &client,
                            &unlocked_key,
                            user_id,
                            &vault,
                            &profiles_root,
                            0,
                        )
                        .await?;
                        let pending = fetch_pending_device_sync_rows(&client).await;
                        let metas = tokio::task::spawn_blocking(move || {
                            collect_credential_metas_from_vault(&vault)
                        })
                        .await
                        .unwrap_or_default();
                        Ok::<_, String>((report, metas, pending))
                    }
                })
                .await
                .map_err(|e| format!("vault pull task error: {e}"))
                .and_then(|r| r);

            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.bot_browser.vault_bootstrap_generation != boot_gen
                {
                    return;
                }
                let lang = shell.active_lang_id();
                match pull_res {
                    Ok((report, credential_metas, pending_approvals)) => {
                        let uid_str = user_id.to_string();
                        let mut oauth_hydrated = 0usize;
                        if let Some(ref bundle) = report.ai_oauth_bundle {
                            oauth_hydrated = hydrate_ai_oauth_vault_bundle_into_fabushi_for_user(
                                &host,
                                bundle,
                                Some(&uid_str),
                            )
                            .unwrap_or(0);
                        }
                        if !local_subs.is_empty() {
                            let local_cnt = hydrate_local_subscriptions_into_fabushi_for_user(
                                &host,
                                &local_subs,
                                Some(&uid_str),
                            );
                            oauth_hydrated = oauth_hydrated.max(local_cnt);
                            shell.bot_browser.hydrated_subscriptions_fingerprint =
                                Some(discovered_subscriptions_fingerprint(&local_subs));
                        }
                        shell.bot_browser.unlocked_vault_key = Some(unlocked_key);
                        shell.bot_browser.vault_sync_revision = report.latest_revision;
                        shell.bot_browser.vault_locked_needs_passphrase = false;
                        shell.bot_browser.vault_custody_status =
                            VaultCustodyStatus::UnlockedAndSynced;
                        shell.bot_browser.vault_keychain_saved = keychain_saved;
                        shell.bot_browser.vault_last_sync_at_ms = Some(current_epoch_ms());
                        shell.bot_browser.vault_last_sync_count = credential_metas
                            .len()
                            .max(report.passwords_applied + report.cookies_applied);
                        shell.bot_browser.vault_custody_status_note = None;
                        shell.bot_browser.pending_device_sync = None;
                        shell.bot_browser.device_sync_poll_generation =
                            shell.bot_browser.device_sync_poll_generation.wrapping_add(1);
                        shell.bot_browser.pending_device_approvals = pending_approvals;
                        shell.bot_browser.cached_credential_metas = credential_metas;
                        shell.attach_bot_browser_orchestrator(orch, cx);
                        shell.attach_local_fabushi_host(host);
                        shell.sync_vault_custody_to_settings(cx);

                        let msg = if lang.starts_with("zh") {
                            format!(
                                "加密保险库已解锁并同步：导入 {} 条密码、{} 个浏览器 Cookie 会话、{} 个 AI 订阅",
                                report.passwords_applied, report.cookies_applied, oauth_hydrated
                            )
                        } else {
                            format!(
                                "Encrypted vault unlocked & synced: {} password(s), {} cookie profile(s), {} AI subscription(s)",
                                report.passwords_applied, report.cookies_applied, oauth_hydrated
                            )
                        };
                        shell.window_runtime.enqueue_toast(msg.clone(), 3500);
                        shell.append_local_vault_notice(active_chat_opt, "vault_unlocked", msg, cx);
                    }
                    Err(err) => {
                        shell.bot_browser.vault_custody_status_note = Some(err.clone());
                        shell.window_runtime.enqueue_toast(err.clone(), 3500);
                        shell.append_local_vault_notice(active_chat_opt, "vault_err", err, cx);
                        shell.sync_vault_custody_to_settings(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn execute_local_vault_sync(
        &mut self,
        user_id: u64,
        active_chat_opt: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id();
        let Some(unlocked_key) = self.bot_browser.unlocked_vault_key.clone() else {
            let msg = if self.bot_browser.vault_locked_needs_passphrase {
                tr_str(
                    lang,
                    "保险库尚未解锁，请在「设置 → 隐私与安全」核对设备批准或输入恢复密钥",
                    "Vault is locked. Approve sync or enter your Emergency Kit in Settings → Privacy & Security.",
                )
            } else {
                tr_str(
                    lang,
                    "尚未启用云端端到端同步，请在「设置 → 隐私与安全」检查保管库状态",
                    "E2EE cloud sync not unlocked yet. Check Vault status in Settings → Privacy & Security.",
                )
            }
            .to_string();
            self.window_runtime.enqueue_toast(msg.clone(), 3000);
            self.append_local_vault_notice(active_chat_opt, "vault_sync_locked", msg, cx);
            return;
        };

        let Ok(orch) = self.ensure_bot_browser_orchestrator(cx) else {
            return;
        };
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let since_rev = self.bot_browser.vault_sync_revision;
        let profiles_root = user_bot_browser_dir(&self.preferences_dir, user_id).join("profiles");
        let cached_subs = self.magpie_local.discovered_items.clone();
        let base_url = self.client.http_client.base_url.clone();
        let local_fabushi = self.bot_browser.local_fabushi.clone();
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let subs = if !cached_subs.is_empty() {
                cached_subs
            } else {
                runtime
                    .spawn_blocking(move || discover_local_subscriptions_for_server(&base_url))
                    .await
                    .unwrap_or_default()
            };

            let sync_res = runtime
                .spawn({
                    let client = client.clone();
                    let unlocked_key = Arc::clone(&unlocked_key);
                    let orch = Arc::clone(&orch);
                    let local_fabushi = local_fabushi.clone();
                    let subs = subs.clone();
                    async move {
                        let pull_report = pull_and_apply_user_vault_items(
                            &client,
                            &unlocked_key,
                            user_id,
                            orch.vault(),
                            &profiles_root,
                            since_rev,
                        )
                        .await?;
                        let uid_str = user_id.to_string();
                        if let (Some(host), Some(bundle)) =
                            (&local_fabushi, &pull_report.ai_oauth_bundle)
                        {
                            let _ = hydrate_ai_oauth_vault_bundle_into_fabushi_for_user(
                                host,
                                bundle,
                                Some(&uid_str),
                            );
                        }
                        let user_router = local_fabushi
                            .as_ref()
                            .map(|h| h.core_engine().providers_for_user(&uid_str));
                        let ai_bundle = if !subs.is_empty() || user_router.is_some() {
                            Some(build_ai_oauth_vault_bundle(&subs, user_router.as_ref())?)
                        } else {
                            None
                        };
                        let mut rev = pull_report.latest_revision;
                        let push_report = push_all_local_vault_items(
                            &client,
                            &unlocked_key,
                            user_id,
                            orch.vault(),
                            &profiles_root,
                            ai_bundle.as_ref(),
                            &mut rev,
                        )
                        .await?;
                        let pending = fetch_pending_device_sync_rows(&client).await;
                        let orch_metas = Arc::clone(&orch);
                        let metas = tokio::task::spawn_blocking(move || {
                            collect_credential_metas_from_vault(orch_metas.vault())
                        })
                        .await
                        .unwrap_or_default();
                        Ok::<_, String>((pull_report, push_report, metas, pending))
                    }
                })
                .await
                .map_err(|e| format!("vault sync task error: {e}"))
                .and_then(|r| r);

            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.bot_browser.vault_bootstrap_generation != boot_gen
                {
                    return;
                }
                let lang = shell.active_lang_id();
                match sync_res {
                    Ok((pull_r, push_r, metas, pending)) => {
                        shell.bot_browser.vault_sync_revision = push_r.latest_revision;
                        shell.bot_browser.vault_last_sync_at_ms = Some(current_epoch_ms());
                        shell.bot_browser.vault_last_sync_count = metas
                            .len()
                            .max(pull_r.passwords_applied + push_r.passwords_pushed);
                        shell.bot_browser.cached_credential_metas = metas;
                        shell.bot_browser.pending_device_approvals = pending;
                        shell.sync_vault_custody_to_settings(cx);
                        let msg = if lang.starts_with("zh") {
                            format!(
                                "E2EE 双向同步完成：拉取 {} 密码/{} Cookie，上传 {} 密码/{} Cookie (rev {})",
                                pull_r.passwords_applied,
                                pull_r.cookies_applied,
                                push_r.passwords_pushed,
                                push_r.cookies_pushed,
                                push_r.latest_revision
                            )
                        } else {
                            format!(
                                "E2EE vault sync complete: pulled {} pw/{} cookie, pushed {} pw/{} cookie (rev {})",
                                pull_r.passwords_applied,
                                pull_r.cookies_applied,
                                push_r.passwords_pushed,
                                push_r.cookies_pushed,
                                push_r.latest_revision
                            )
                        };
                        shell.window_runtime.enqueue_toast(msg.clone(), 3200);
                        shell.append_local_vault_notice(active_chat_opt, "vault_synced", msg, cx);
                    }
                    Err(err) => {
                        shell.window_runtime.enqueue_toast(err.clone(), 3200);
                        shell.append_local_vault_notice(active_chat_opt, "vault_err", err, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Hydrates discovered local AI subscriptions into `local_fabushi` and pushes `AiOauthVaultBundle`
    /// if the user's E2EE vault is unlocked.
    pub fn sync_discovered_subscriptions_to_local_fabushi_and_vault(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated {
            return;
        }
        let items = self.magpie_local.discovered_items.clone();
        if items.is_empty() {
            return;
        }
        if self.bot_browser.local_fabushi.is_none() && self.bot_browser.unlocked_vault_key.is_none()
        {
            return;
        }
        let uid_str = self.session_sync.user_id.map(|u| u.to_string());
        let fp = discovered_subscriptions_fingerprint(&items);
        let already_usable = self
            .bot_browser
            .local_fabushi
            .as_ref()
            .is_none_or(|h| has_usable_local_fabushi_provider_for_user(h, uid_str.as_deref()));
        if already_usable
            && self
                .bot_browser
                .hydrated_subscriptions_fingerprint
                .as_deref()
                == Some(fp.as_str())
        {
            return;
        }
        self.bot_browser.hydrated_subscriptions_fingerprint = Some(fp);

        if let Some(ref host) = self.bot_browser.local_fabushi {
            hydrate_local_subscriptions_into_fabushi_for_user(host, &items, uid_str.as_deref());
        }

        let (Some(unlocked_key), Some(user_id)) = (
            self.bot_browser.unlocked_vault_key.clone(),
            self.session_sync.user_id,
        ) else {
            return;
        };

        let router_snap = self
            .bot_browser
            .local_fabushi
            .as_ref()
            .map(|h| h.core_engine().providers_for_user(&user_id.to_string()));
        let Ok(bundle) = build_ai_oauth_vault_bundle(&items, router_snap.as_ref()) else {
            return;
        };
        let Ok(bytes) = bundle.to_json_bytes() else {
            return;
        };

        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let mut known_rev = self.bot_browser.vault_sync_revision;
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = runtime
                .spawn(async move {
                    seal_and_put_vault_item(
                        &client,
                        &unlocked_key,
                        user_id,
                        UserVaultCategory::AiOauth,
                        AI_OAUTH_VAULT_ITEM_KEY,
                        &bytes,
                        &mut known_rev,
                    )
                    .await
                })
                .await
                .ok()
                .and_then(|r| r.ok());
            if let Some(new_rev) = res {
                let _ = weak.update(app, |shell, _| {
                    if shell.is_authenticated
                        && shell.session_sync.auth_generation == auth_gen
                        && shell.bot_browser.vault_bootstrap_generation == boot_gen
                    {
                        shell.bot_browser.vault_sync_revision =
                            shell.bot_browser.vault_sync_revision.max(new_rev);
                    }
                });
            }
        })
        .detach();
    }

    /// Auto-pushes a newly saved password record (`cred_id`) to `/api/v1/user-vault/items/password/<cred_id>`
    /// when `unlocked_vault_key` is present.
    pub fn push_password_item_to_vault_if_unlocked(
        &mut self,
        credential_id: String,
        cx: &mut Context<Self>,
    ) {
        let (Some(unlocked_key), Some(user_id), Some(orch)) = (
            self.bot_browser.unlocked_vault_key.clone(),
            self.session_sync.user_id,
            self.bot_browser.orchestrator.clone(),
        ) else {
            return;
        };
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let mut known_rev = self.bot_browser.vault_sync_revision;
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = runtime
                .spawn(async move {
                    let record = orch
                        .vault()
                        .export_for_sync(&credential_id)
                        .map_err(|e| format!("export_for_sync failed: {e}"))?;
                    let bytes = Zeroizing::new(
                        serde_json::to_vec(&record)
                            .map_err(|e| format!("serialize password payload failed: {e}"))?,
                    );
                    let rev = seal_and_put_vault_item(
                        &client,
                        &unlocked_key,
                        user_id,
                        UserVaultCategory::Password,
                        &credential_id,
                        &bytes,
                        &mut known_rev,
                    )
                    .await?;
                    let orch_metas = Arc::clone(&orch);
                    let metas = tokio::task::spawn_blocking(move || {
                        collect_credential_metas_from_vault(orch_metas.vault())
                    })
                    .await
                    .unwrap_or_default();
                    Ok::<_, String>((rev, metas))
                })
                .await
                .ok()
                .and_then(|r| r.ok());
            if let Some((new_rev, metas)) = res {
                let _ = weak.update(app, |shell, cx| {
                    if shell.is_authenticated
                        && shell.session_sync.auth_generation == auth_gen
                        && shell.bot_browser.vault_bootstrap_generation == boot_gen
                    {
                        shell.bot_browser.vault_sync_revision =
                            shell.bot_browser.vault_sync_revision.max(new_rev);
                        shell.bot_browser.vault_last_sync_at_ms = Some(current_epoch_ms());
                        shell.bot_browser.vault_last_sync_count = metas.len();
                        shell.bot_browser.cached_credential_metas = metas;
                        shell.sync_vault_custody_to_settings(cx);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// Auto-packs and pushes Obscura `--storage-dir` profile cookies for `session_id` (or all active profiles)
    /// when `unlocked_vault_key` is present.
    pub fn push_browser_cookies_for_session_if_unlocked(
        &mut self,
        session_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let (Some(unlocked_key), Some(user_id)) = (
            self.bot_browser.unlocked_vault_key.clone(),
            self.session_sync.user_id,
        ) else {
            return;
        };
        let profiles_root = self
            .bot_browser
            .orchestrator
            .as_ref()
            .map(|o| o.config().data_dir.join("profiles"))
            .unwrap_or_else(|| user_bot_browser_dir(&self.preferences_dir, user_id).join("profiles"));
        let auth_gen = self.session_sync.auth_generation;
        let boot_gen = self.bot_browser.vault_bootstrap_generation;
        let mut known_rev = self.bot_browser.vault_sync_revision;
        let client = self.client.clone();
        let runtime = self.client.runtime().clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = runtime
                .spawn(async move {
                    let dirs = collect_nonempty_profile_dirs(&profiles_root, session_id.as_deref());
                    for (profile_key, dir_path) in dirs {
                        if let Ok(packed) = BrowserProfileSnapshot::pack_storage_dir(
                            &dir_path,
                            None,
                            MAX_PROFILE_BUNDLE_BYTES,
                        ) {
                            let _ = seal_and_put_vault_item(
                                &client,
                                &unlocked_key,
                                user_id,
                                UserVaultCategory::BrowserCookie,
                                &profile_key,
                                &packed,
                                &mut known_rev,
                            )
                            .await;
                        }
                    }
                    known_rev
                })
                .await
                .ok();
            if let Some(new_rev) = res {
                let _ = weak.update(app, |shell, _| {
                    if shell.is_authenticated
                        && shell.session_sync.auth_generation == auth_gen
                        && shell.bot_browser.vault_bootstrap_generation == boot_gen
                    {
                        shell.bot_browser.vault_sync_revision =
                            shell.bot_browser.vault_sync_revision.max(new_rev);
                    }
                });
            }
        })
        .detach();
    }

    /// Runs a Fabushi Bot turn locally in-process using `local_fabushi` + `BotBrowserOrchestrator`,
    /// displays the reply in `ConversationStage`, archives the non-secret final reply via
    /// `client.archive_local_bot_reply`, and syncs Obscura cookies E2EE if unlocked.
    pub fn spawn_local_fabushi_turn(
        &mut self,
        chat_id: u64,
        target: LocalFabushiBotTarget,
        explicit_session_id: Option<String>,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.bot_browser.local_fabushi.clone() else {
            return;
        };
        let Some(user_id) = self.session_sync.user_id else {
            return;
        };
        let auth_gen = self.session_sync.auth_generation;
        let session_id = explicit_session_id
            .unwrap_or_else(|| format!("local_chat_{chat_id}_bot_{}", target.bot_user_id));
        self.bot_browser
            .active_browser_sessions
            .insert(session_id.clone(), session_id.clone());

        let lang = self.active_lang_id();
        let thinking_txt =
            tr_str(lang, "AI 助手正在思考...", "Assistant is thinking...").to_string();
        self.sidebar.update(cx, |sb, sb_cx| {
            sb.set_typing(chat_id, Some(thinking_txt.clone()));
            sb_cx.notify();
        });
        self.stage.update(cx, |st, st_cx| {
            if st.active_chat_id == Some(chat_id) {
                st.is_bot_chat = true;
                st.set_bot_activity("thinking", Some(thinking_txt));
                st_cx.notify();
            }
        });

        let client = self.client.clone();
        let runtime = self.client.runtime().clone();
        let bot_user_id = target.bot_user_id;
        let bot_display_name = target.display_name.clone();
        let bot_display_name_err = target.display_name.clone();
        let provision_spec = target.provision_spec.clone();
        let session_id_for_run = session_id.clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let turn_res = runtime
                .spawn(async move {
                    let uid_str = user_id.to_string();
                    let core = host.core_engine();
                    if !has_usable_local_fabushi_provider_for_user(&host, Some(&uid_str))
                        && has_usable_local_fabushi_provider_for_user(&host, None)
                    {
                        core.providers_for_user(&uid_str)
                            .sync_from_router(core.providers());
                        core.persist_user_providers_state(&uid_str);
                        host.sync_all_providers();
                    }
                    let engine = if bot_user_id != OFFICIAL_DEFAULT_BOT_USER_ID {
                        if let Some(spec) = provision_spec {
                            host.provision_or_sync_bot(spec)
                                .map(|i| i.hermes())
                                .unwrap_or_else(|_| host.engine_for_bot(Some(bot_user_id)))
                        } else {
                            host.engine_for_bot(Some(bot_user_id))
                        }
                    } else {
                        core
                    };

                    let (system_prompt, profile_delta) =
                        host.build_turn_system_prompt(bot_user_id, chat_id as i64, "", &[], &[]);
                    let effective_prompt = match profile_delta {
                        Some(prefix) => format!("{prefix}\n\n{prompt}"),
                        None => prompt,
                    };

                    let mut req = HermesTurnRequest::new(effective_prompt)
                        .with_chat_bot(chat_id as i64, bot_user_id.to_string())
                        .with_bot_name(bot_display_name.clone())
                        .with_session(session_id_for_run.clone());
                    req.user_id = Some(user_id.to_string());
                    req.source = "desktop_local_fabushi".to_string();
                    req.preset = Some(ToolsetPreset::Full);
                    if !system_prompt.trim().is_empty() {
                        req.system_prompt = Some(system_prompt);
                    }

                    engine.run_turn(req).await
                })
                .await
                .map_err(|e| format!("Local Fabushi turn join error: {e}"))
                .and_then(|r| r.map_err(|e| e.to_string()));

            match turn_res {
                Ok(res) => {
                    let reply_text = res.response_text.trim().to_string();
                    let chunks = if reply_text.is_empty() {
                        Vec::new()
                    } else {
                        split_text_chunks(&reply_text, 4096)
                    };
                    let mut chunk_pairs = Vec::with_capacity(chunks.len());
                    for chunk in chunks {
                        let nonce = (client.next_client_nonce() & (i64::MAX as u64)).max(1);
                        chunk_pairs.push((nonce, chunk));
                    }

                    let pairs_for_ui = chunk_pairs.clone();
                    let session_for_cookies = session_id.clone();
                    let bot_name_ui = target.display_name.clone();
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.is_authenticated || shell.session_sync.auth_generation != auth_gen {
                            return;
                        }
                        let ts = format_authoritative_timestamp(Some(
                            chrono::Utc::now().timestamp_millis(),
                        ));
                        let initials = {
                            let i = derive_initials(&bot_name_ui);
                            if i.is_empty() {
                                "FA".to_string()
                            } else {
                                i
                            }
                        };
                        let color = peer_avatar_color(bot_user_id.max(1) as u64);
                        shell.sidebar.update(cx, |sb, sb_cx| {
                            sb.set_typing(chat_id, None);
                            if let Some((_, last_txt)) = pairs_for_ui.last() {
                                if let Some(d) = sb.dialogs.iter_mut().find(|d| d.id == chat_id) {
                                    d.last_message =
                                        super::shell_pts_updates::format_sidebar_preview(last_txt);
                                    d.timestamp = ts.clone();
                                    d.delivery_status = DialogDeliveryStatus::Read;
                                }
                            }
                            sb_cx.notify();
                        });
                        shell.stage.update(cx, |st, st_cx| {
                            if st.active_chat_id == Some(chat_id) {
                                st.set_bot_activity("idle", None);
                            }
                            for (nonce, chunk_txt) in &pairs_for_ui {
                                let model = MessageItemModel {
                                    id: *nonce,
                                    sender_id: bot_user_id.max(0) as u64,
                                    sender_name: bot_name_ui.clone(),
                                    avatar_initials: initials.clone(),
                                    avatar_color: color,
                                    direction: MessageDirection::Incoming,
                                    timestamp: ts.clone(),
                                    delivery_status: DeliveryStatus::Read,
                                    reply_to: None,
                                    reactions: Vec::new(),
                                    content: MessageContent::Text {
                                        text: chunk_txt.clone(),
                                        is_edited: false,
                                    },
                                    bot_keyboard: None,
                                    is_pinned: false,
                                };
                                if st.active_chat_id == Some(chat_id) {
                                    st.add_message(model);
                                } else {
                                    st.chat_histories.entry(chat_id).or_default().push(model);
                                }
                            }
                            st_cx.notify();
                        });
                        shell.push_browser_cookies_for_session_if_unlocked(
                            Some(session_for_cookies),
                            cx,
                        );
                        cx.notify();
                    });

                    // Archive non-secret final reply text to cloud history in background.
                    for (nonce, chunk_txt) in chunk_pairs {
                        let req = ArchiveLocalBotReplyRequest {
                            bot_user_id,
                            text: chunk_txt,
                            reply_to_msg_id: None,
                            client_nonce: Some(nonce),
                        };
                        if let Ok(Ok(archived)) = runtime
                            .spawn({
                                let client = client.clone();
                                async move {
                                    client.archive_local_bot_reply(chat_id as i64, &req).await
                                }
                            })
                            .await
                        {
                            let server_id = u64::try_from(archived.message_id).unwrap_or(0);
                            let pts = archived.pts;
                            let _ = weak.update(app, |shell, cx| {
                                if !shell.is_authenticated
                                    || shell.session_sync.auth_generation != auth_gen
                                {
                                    return;
                                }
                                if pts > 0 {
                                    shell
                                        .client
                                        .sync_coordinator
                                        .set_pts(ChatId(chat_id as i64), pts);
                                }
                                if server_id > 0 {
                                    shell.stage.update(cx, |st, st_cx| {
                                        let mut changed = false;
                                        if st.active_chat_id == Some(chat_id)
                                            && reconcile_stage_message_list(
                                                &mut st.messages,
                                                nonce,
                                                server_id,
                                                DeliveryStatus::Read,
                                            )
                                        {
                                            st.remap_message_id(nonce, server_id);
                                            st.row_height_cache.rebuild_prefix_sums(&st.messages);
                                            changed = true;
                                        }
                                        if let Some(hist) = st.chat_histories.get_mut(&chat_id) {
                                            reconcile_stage_message_list(
                                                hist,
                                                nonce,
                                                server_id,
                                                DeliveryStatus::Read,
                                            );
                                        }
                                        if changed {
                                            st_cx.notify();
                                        }
                                    });
                                }
                                cx.notify();
                            });
                        }
                    }
                }
                Err(err) => {
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.is_authenticated || shell.session_sync.auth_generation != auth_gen {
                            return;
                        }
                        shell.apply_local_fabushi_turn_error_to_shell(
                            chat_id,
                            bot_user_id,
                            &bot_display_name_err,
                            &err,
                            cx,
                        );
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Strips ANSI CSI/OSC escape sequences (e.g. `\x1b[31m...\x1b[0m`) so terminal color
    /// codes never leak into inline chat error cards.
    fn strip_ansi_escape_codes(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                match chars.peek().copied() {
                    Some('[') => {
                        chars.next();
                        while let Some(ch) = chars.next() {
                            if ('@'..='~').contains(&ch) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        chars.next();
                        while let Some(ch) = chars.next() {
                            if ch == '\x07' {
                                break;
                            }
                            if ch == '\x1b' && chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                        }
                    }
                    Some(_) => {
                        chars.next();
                    }
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Formats a local Fabushi turn failure into a structured in-chat error card, optional
    /// `hermes:open_ai_providers` inline button, and a concise single-line toast summary (`<= 80` chars).
    pub fn build_local_fabushi_error_card(
        err: &str,
        is_zh: bool,
        needs_provider_cta: bool,
    ) -> (String, Option<Vec<Vec<InlineKeyboardButton>>>, String) {
        let cleaned_ansi = Self::strip_ansi_escape_codes(err);
        let lower = cleaned_ansi.to_ascii_lowercase();
        let is_provider_err = needs_provider_cta
            || lower.contains("provider_error")
            || lower.contains("needs_provider_selection")
            || lower.contains("no ai subscription or provider configured")
            || lower.contains("no configured provider")
            || lower.contains("providerrouter")
            || cleaned_ansi.contains("未检测到已配置的 AI 订阅或模型供应商")
            || cleaned_ansi.contains("未配置可用的 AI 供应商");

        let trimmed = cleaned_ansi.trim();
        let stripped = trimmed
            .strip_prefix("PROVIDER_ERROR:")
            .or_else(|| trimmed.strip_prefix("provider_error:"))
            .unwrap_or(trimmed);
        let compact = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
        let detail: String = if compact.chars().count() > 240 {
            let head: String = compact.chars().take(239).collect();
            format!("{head}…")
        } else if compact.is_empty() {
            if is_zh {
                "未知执行错误，请稍后重试。".to_string()
            } else {
                "Unknown local execution error.".to_string()
            }
        } else {
            compact
        };

        let card_markdown = if is_provider_err {
            if is_zh {
                format!("⚠️ **未配置可用的 AI 供应商**\n\n{detail}")
            } else {
                format!("⚠️ **AI Provider Not Ready**\n\n{detail}")
            }
        } else if is_zh {
            format!("⚠️ **本地 AI 执行失败**\n\n{detail}")
        } else {
            format!("⚠️ **Local AI Turn Failed**\n\n{detail}")
        };

        let keyboard = if is_provider_err {
            Some(vec![vec![InlineKeyboardButton {
                text: if is_zh {
                    "⚙️ 打开 AI 供应商设置".to_string()
                } else {
                    "⚙️ Open AI Provider Settings".to_string()
                },
                button_type: InlineButtonType::Callback("hermes:open_ai_providers".to_string()),
            }]])
        } else {
            None
        };

        let toast_summary = if is_provider_err {
            if is_zh {
                "AI 供应商未就绪，请点击消息按钮配置".to_string()
            } else {
                "AI provider unavailable — tap card button to configure".to_string()
            }
        } else if is_zh {
            "本地 AI 执行失败，详情见会话卡片".to_string()
        } else {
            "Local AI turn failed — see chat message".to_string()
        };

        (card_markdown, keyboard, toast_summary)
    }

    /// Inserts a structured local Bot error card into `ConversationStage`, clears typing/activity
    /// indicators, updates the sidebar preview, and shows only a concise non-blocking toast.
    pub fn apply_local_fabushi_turn_error_to_shell(
        &mut self,
        chat_id: u64,
        bot_user_id: i64,
        bot_display_name: &str,
        err: &str,
        cx: &mut Context<Self>,
    ) {
        let is_zh = self.active_lang_id().starts_with("zh");
        let (card_text, bot_keyboard, toast_summary) =
            Self::build_local_fabushi_error_card(err, is_zh, false);
        let ts = format_authoritative_timestamp(Some(chrono::Utc::now().timestamp_millis()));
        let sender_name = if bot_display_name.trim().is_empty() {
            "Fabushi AI".to_string()
        } else {
            bot_display_name.trim().to_string()
        };
        let initials = {
            let i = derive_initials(&sender_name);
            if i.is_empty() {
                "FA".to_string()
            } else {
                i
            }
        };
        let color = peer_avatar_color(bot_user_id.max(1) as u64);
        let local_id = self.bot_browser.next_ephemeral_local_msg_id();

        self.sidebar.update(cx, |sb, sb_cx| {
            sb.set_typing(chat_id, None);
            if let Some(d) = sb.dialogs.iter_mut().find(|d| d.id == chat_id) {
                d.last_message = super::shell_pts_updates::format_sidebar_preview(&card_text);
                d.timestamp = ts.clone();
                d.delivery_status = DialogDeliveryStatus::Read;
            }
            sb_cx.notify();
        });
        self.stage.update(cx, |st, st_cx| {
            if st.active_chat_id == Some(chat_id) {
                st.set_bot_activity("idle", None);
            }
            let model = MessageItemModel {
                id: local_id,
                sender_id: bot_user_id.max(0) as u64,
                sender_name,
                avatar_initials: initials,
                avatar_color: color,
                direction: MessageDirection::Incoming,
                timestamp: ts,
                delivery_status: DeliveryStatus::Read,
                reply_to: None,
                reactions: Vec::new(),
                content: MessageContent::Text {
                    text: card_text,
                    is_edited: false,
                },
                bot_keyboard,
                is_pinned: false,
            };
            if st.active_chat_id == Some(chat_id) {
                st.add_message(model);
            } else {
                st.chat_histories.entry(chat_id).or_default().push(model);
            }
            st_cx.notify();
        });
        self.show_chat_toast(chat_id, toast_summary, 3200, cx);
    }

    fn append_local_vault_notice(
        &mut self,
        chat_id_opt: Option<u64>,
        notice_prefix: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Some(chat_id) = chat_id_opt else {
            return;
        };
        let local_id = self.bot_browser.next_ephemeral_local_msg_id();
        let _ = notice_prefix;
        let ts = format_authoritative_timestamp(Some(chrono::Utc::now().timestamp_millis()));
        let model = MessageItemModel {
            id: local_id,
            sender_id: 0,
            sender_name: tr_str(self.active_lang_id(), "本地安全保险库", "Local Security Vault")
                .to_string(),
            avatar_initials: "🔐".to_string(),
            avatar_color: 0x0F766E,
            direction: MessageDirection::Incoming,
            timestamp: ts,
            delivery_status: DeliveryStatus::Read,
            reply_to: None,
            reactions: Vec::new(),
            content: MessageContent::Service {
                text,
                service_type: ServiceType::SecurityKeyChanged,
            },
            bot_keyboard: None,
            is_pinned: false,
        };
        self.stage.update(cx, |st, st_cx| {
            if st.active_chat_id == Some(chat_id) {
                st.add_message(model);
            } else {
                st.chat_histories.entry(chat_id).or_default().push(model);
            }
            st_cx.notify();
        });
    }

    fn resolve_bot_card_chat_id(
        &self,
        card_id: &str,
        event_chat_id: Option<i64>,
        cx: &App,
    ) -> Option<u64> {
        event_chat_id
            .and_then(|cid| u64::try_from(cid).ok())
            .filter(|&cid| cid > 0)
            .or_else(|| self.bot_browser.card_locations.get(card_id).map(|&(c, _)| c))
            .or_else(|| {
                resolve_active_target_chat_id(
                    self.stage.read(cx).active_chat_id,
                    self.sidebar.read(cx).active_chat_id,
                )
            })
    }

    fn spawn_bot_browser_event_listener(
        &self,
        orchestrator: Arc<BotBrowserOrchestrator>,
        expected_gen: u64,
        cx: &mut Context<Self>,
    ) {
        let mut rx = orchestrator.subscribe_events();
        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let keep_running = weak_shell
                            .update(app, |shell, cx| {
                                if shell.bot_browser.listener_generation != expected_gen {
                                    return false;
                                }
                                shell.apply_bot_browser_event(ev, cx);
                                true
                            })
                            .unwrap_or(false);
                        if !keep_running {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .detach();
    }

    /// Applies a `BotBrowserEvent` strictly to the local `ConversationStage` transcript without MTProto leakage.
    pub fn apply_bot_browser_event(&mut self, event: BotBrowserEvent, cx: &mut Context<Self>) {
        match event {
            BotBrowserEvent::UpsertCard(payload) => {
                let Some(chat_id) =
                    self.resolve_bot_card_chat_id(payload.card_id(), payload.chat_id(), cx)
                else {
                    return;
                };
                let local_msg_id = self
                    .bot_browser
                    .allocate_local_msg_id(payload.card_id(), chat_id);
                let ts = format_authoritative_timestamp(Some(chrono::Utc::now().timestamp_millis()));
                self.stage.update(cx, |st, st_cx| {
                    st.upsert_bot_assist_card(chat_id, local_msg_id, payload, ts);
                    st_cx.notify();
                });
                cx.notify();
            }
            BotBrowserEvent::UpdateCardStatus {
                card_id,
                chat_id,
                status,
                status_note,
                saved_credential_id,
                saved_account_hint,
                redirected_url,
            } => {
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, chat_id, cx);
                let updated = self.stage.update(cx, |st, st_cx| {
                    let changed = st.update_bot_assist_card_status(
                        cid_u64,
                        &card_id,
                        status,
                        status_note,
                        saved_credential_id,
                        saved_account_hint,
                        redirected_url,
                    );
                    if changed {
                        st_cx.notify();
                    }
                    changed
                });
                if status == AssistCardStatus::Completed {
                    if let Some(AssistCardPayload::QrLogin { session_id, .. }) = self
                        .bot_browser
                        .orchestrator
                        .as_ref()
                        .and_then(|o| o.get_card(&card_id))
                    {
                        self.push_browser_cookies_for_session_if_unlocked(Some(session_id), cx);
                    }
                }
                if updated {
                    cx.notify();
                }
            }
            BotBrowserEvent::RefreshQrImage {
                card_id,
                chat_id,
                png_bytes,
                countdown_secs,
                poll_attempt,
            } => {
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, chat_id, cx);
                let updated = self.stage.update(cx, |st, st_cx| {
                    let changed = st.refresh_bot_assist_qr(
                        cid_u64,
                        &card_id,
                        png_bytes,
                        countdown_secs,
                        poll_attempt,
                    );
                    if changed {
                        st_cx.notify();
                    }
                    changed
                });
                if updated {
                    cx.notify();
                }
            }
            BotBrowserEvent::TaskWakeupTriggered(wakeup) => {
                let lang = self.active_lang_id();
                let msg = format!(
                    "{} ({})",
                    tr_str(
                        lang,
                        "已保存并自动填充登录，正在唤醒 Bot 续跑任务",
                        "Credentials saved & auto-filled; resuming bot task",
                    ),
                    wakeup.origin
                );
                let cid_opt = wakeup
                    .chat_id
                    .and_then(|c| u64::try_from(c).ok())
                    .filter(|&c| c > 0)
                    .or_else(|| {
                        resolve_active_target_chat_id(
                            self.stage.read(cx).active_chat_id,
                            self.sidebar.read(cx).active_chat_id,
                        )
                    });
                if let Some(cid) = cid_opt {
                    if self.stage.read(cx).active_chat_id == Some(cid) {
                        self.show_chat_toast(cid, msg, 3200, cx);
                    }
                } else {
                    self.show_toast(msg, 3200, cx);
                }
                if let Some(cid) = cid_opt {
                    if self.can_run_local_fabushi_turn(cx) {
                        let target = self.resolve_fabushi_bot_target(cid, cx).unwrap_or(
                            LocalFabushiBotTarget {
                                bot_user_id: OFFICIAL_DEFAULT_BOT_USER_ID,
                                display_name: "Fabushi AI".to_string(),
                                provision_spec: None,
                            },
                        );
                        self.spawn_local_fabushi_turn(
                            cid,
                            target,
                            Some(wakeup.session_id),
                            "安全凭据已完成本地填充，请继续原任务".to_string(),
                            cx,
                        );
                    }
                }
                cx.notify();
            }
            BotBrowserEvent::SidecarRestarted {
                session_id,
                restart_count,
                restored_url,
            } => {
                eprintln!(
                    "[BotBrowser] Obscura sidecar restarted (session={session_id}, count={restart_count}, restored_url={restored_url:?})"
                );
            }
        }
    }

    /// Dispatches a user interaction from any of the 4 Bot Assist cards strictly in-process.
    pub fn handle_bot_assist_card_action(
        &mut self,
        action: BotAssistCardAction,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id();
        let runtime = self.client.runtime().clone();
        match action {
            BotAssistCardAction::FocusLoginField { .. }
            | BotAssistCardAction::ToggleShowPassword { .. }
            | BotAssistCardAction::ToggleAutoReuse { .. }
            | BotAssistCardAction::ClickCaptchaPoint { .. }
            | BotAssistCardAction::ResetCaptchaClicks { .. } => {}
            BotAssistCardAction::SubmitLogin { card_id } => {
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, None, cx);
                let sub_res = self.stage.update(cx, |st, st_cx| {
                    let res = st.take_bot_card_login_submission(&card_id);
                    st_cx.notify();
                    res
                });
                let submission = match sub_res {
                    Ok(sub) => sub,
                    Err(err_msg) => {
                        if let Some(cid) = cid_u64.filter(|&c| c != 0) {
                            if self.stage.read(cx).active_chat_id == Some(cid) {
                                self.show_chat_toast(cid, err_msg, 2400, cx);
                            }
                        } else {
                            self.show_toast(err_msg, 2400, cx);
                        }
                        cx.notify();
                        return;
                    }
                };
                let Some(orch) = self.bot_browser.orchestrator.clone() else {
                    let msg = tr_str(
                        lang,
                        "本地安全浏览器服务未初始化",
                        "Local bot browser orchestrator unavailable",
                    );
                    if let Some(cid) = cid_u64.filter(|&c| c != 0) {
                        if self.stage.read(cx).active_chat_id == Some(cid) {
                            self.show_chat_toast(cid, msg, 2600, cx);
                        }
                    } else {
                        self.show_toast(msg, 2600, cx);
                    }
                    cx.notify();
                    return;
                };
                let card_id_for_ui = card_id.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let res = runtime
                        .spawn(async move { orch.submit_login_card(submission).await })
                        .await
                        .map_err(|e| BotBrowserError::Io(format!("task join error: {e}")))
                        .and_then(|inner| inner);
                    let _ = weak.update(app, |shell, cx| {
                        match res {
                            Ok(outcome) => {
                                let saved_cred_id = outcome.credential_id.clone();
                                shell.stage.update(cx, |st, st_cx| {
                                    st.update_bot_assist_card_status(
                                        cid_u64,
                                        &card_id_for_ui,
                                        AssistCardStatus::SavedAndLoggedIn,
                                        None,
                                        Some(saved_cred_id.clone()),
                                        Some(outcome.account_hint.clone()),
                                        None,
                                    );
                                    st_cx.notify();
                                });
                                shell.push_password_item_to_vault_if_unlocked(saved_cred_id, cx);
                                let note = if outcome.resumed_timed_out_task {
                                    tr_str(
                                        shell.active_lang_id(),
                                        "已保存凭据并自动续跑原任务",
                                        "Saved & resumed previous task",
                                    )
                                } else {
                                    tr_str(
                                        shell.active_lang_id(),
                                        "登录凭据已加密保存至本地安全库",
                                        "Credentials encrypted & saved locally",
                                    )
                                };
                                if let Some(cid) = cid_u64.filter(|&c| c != 0) {
                                    if shell.stage.read(cx).active_chat_id == Some(cid) {
                                        shell.show_chat_toast(cid, note, 2600, cx);
                                    }
                                } else {
                                    shell.show_toast(note, 2600, cx);
                                }
                            }
                            Err(e) => {
                                let err_toast = format!("Vault login error: {e}");
                                if let Some(cid) = cid_u64.filter(|&c| c != 0) {
                                    if shell.stage.read(cx).active_chat_id == Some(cid) {
                                        shell.show_chat_toast(cid, err_toast, 3200, cx);
                                    }
                                } else {
                                    shell.show_toast(err_toast, 3200, cx);
                                }
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            BotAssistCardAction::ApproveReuse { card_id, approved } => {
                let Some(orch) = self.bot_browser.orchestrator.clone() else {
                    return;
                };
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, None, cx);
                let next_local_status = if approved {
                    AssistCardStatus::ApprovedAndFilling
                } else {
                    AssistCardStatus::Denied
                };
                self.stage.update(cx, |st, st_cx| {
                    st.update_bot_assist_card_status(
                        cid_u64,
                        &card_id,
                        next_local_status,
                        None,
                        None,
                        None,
                        None,
                    );
                    st_cx.notify();
                });
                let card_id_req = card_id.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let res = runtime
                        .spawn(async move { orch.approve_reuse_card(&card_id_req, approved).await })
                        .await
                        .map_err(|e| BotBrowserError::Io(format!("task join error: {e}")))
                        .and_then(|inner| inner);
                    let _ = weak.update(app, |shell, cx| {
                        match res {
                            Ok(outcome) => {
                                let final_status = if outcome.approved {
                                    AssistCardStatus::Completed
                                } else {
                                    AssistCardStatus::Denied
                                };
                                shell.stage.update(cx, |st, st_cx| {
                                    st.update_bot_assist_card_status(
                                        cid_u64,
                                        &card_id,
                                        final_status,
                                        None,
                                        None,
                                        None,
                                        None,
                                    );
                                    st_cx.notify();
                                });
                            }
                            Err(e) => {
                                let err_toast = format!("Reuse approval failed: {e}");
                                if let Some(cid) = cid_u64.filter(|&c| c != 0) {
                                    if shell.stage.read(cx).active_chat_id == Some(cid) {
                                        shell.show_chat_toast(cid, err_toast, 2800, cx);
                                    }
                                } else {
                                    shell.show_toast(err_toast, 2800, cx);
                                }
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            BotAssistCardAction::SubmitCaptchaClicks { card_id } => {
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, None, cx);
                let points = self
                    .stage
                    .read(cx)
                    .bot_assist_click_drafts
                    .get(&card_id)
                    .map(|d| d.points.clone())
                    .unwrap_or_default();
                if points.is_empty() {
                    let msg = tr_str(
                        lang,
                        "请先在验证图上点击目标位置",
                        "Click target points first",
                    );
                    if let Some(cid) = cid_u64.filter(|&c| c != 0) {
                        if self.stage.read(cx).active_chat_id == Some(cid) {
                            self.show_chat_toast(cid, msg, 2200, cx);
                        }
                    } else {
                        self.show_toast(msg, 2200, cx);
                    }
                    cx.notify();
                    return;
                }
                let Some(orch) = self.bot_browser.orchestrator.clone() else {
                    return;
                };
                let card_id_req = card_id.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let res = runtime
                        .spawn(async move { orch.submit_click_captcha(&card_id_req, points).await })
                        .await
                        .map_err(|e| BotBrowserError::Io(format!("task join error: {e}")))
                        .and_then(|inner| inner);
                    let _ = weak.update(app, |shell, cx| {
                        match res {
                            Ok(_) => {
                                shell.stage.update(cx, |st, st_cx| {
                                    st.update_bot_assist_card_status(
                                        cid_u64,
                                        &card_id,
                                        AssistCardStatus::Completed,
                                        None,
                                        None,
                                        None,
                                        None,
                                    );
                                    st_cx.notify();
                                });
                            }
                            Err(e) => {
                                let err_toast = format!("Click captcha failed: {e}");
                                if let Some(cid) = cid_u64.filter(|&c| c != 0) {
                                    if shell.stage.read(cx).active_chat_id == Some(cid) {
                                        shell.show_chat_toast(cid, err_toast, 2800, cx);
                                    }
                                } else {
                                    shell.show_toast(err_toast, 2800, cx);
                                }
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            BotAssistCardAction::CancelCard { card_id } => {
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, None, cx);
                self.stage.update(cx, |st, st_cx| {
                    st.update_bot_assist_card_status(
                        cid_u64,
                        &card_id,
                        AssistCardStatus::Cancelled,
                        None,
                        None,
                        None,
                        None,
                    );
                    st_cx.notify();
                });
                if let Some(orch) = self.bot_browser.orchestrator.clone() {
                    let card_id_req = card_id.clone();
                    cx.spawn(async move |_: WeakEntity<Self>, _: &mut AsyncApp| {
                        let _ = runtime
                            .spawn(async move { orch.cancel_card(&card_id_req).await })
                            .await;
                    })
                    .detach();
                }
            }
        }
    }
}
