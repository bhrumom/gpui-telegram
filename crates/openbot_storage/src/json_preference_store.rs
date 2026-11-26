//! Atomic `0600` JSON preference stores (`JsonPreferenceStore`, `VersionedJsonStore`,
//! `SealedKeyStore`) and the complete `OPENBOT_PREFERENCE_FILES` catalog (`OB-F02`).
//!
//! Mirrors `src/backend/{atomic-json-file.ts,versioned-json-store.ts}` and
//! `src/main/{preference-file.ts,voice-api-key-store.ts,application-services.ts}`.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::Utc;
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use uuid::Uuid;

use crate::error::{StorageError, StorageResult};

struct TempFileCleanupGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for TempFileCleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn create_dir_mode_700(dir: &Path) -> StorageResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let existed = dir.exists();
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(dir)?;
        if !existed {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// Atomically writes `content` to `path` via `<path>.<uuid>.tmp` (`0o600`), ensuring a
/// trailing `\n`, calling `File::sync_all()`, and renaming into place (`0o700` parent dir
/// when `create_directory` is `true`). Cleans up the temporary file on any failure.
pub fn write_file_atomically(
    path: impl AsRef<Path>,
    content: impl AsRef<[u8]>,
    create_directory: bool,
) -> StorageResult<()> {
    let path = path.as_ref();
    if create_directory
        && let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        create_dir_mode_700(parent)?;
    }

    let temp_path = PathBuf::from(format!("{}.{}.tmp", path.display(), Uuid::new_v4()));
    let mut guard = TempFileCleanupGuard {
        path: temp_path.clone(),
        armed: true,
    };

    {
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp_path)?
        };
        #[cfg(not(unix))]
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;

        let bytes = content.as_ref();
        file.write_all(bytes)?;
        if !bytes.ends_with(b"\n") {
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
    }

    fs::rename(&temp_path, path)?;
    guard.armed = false;
    Ok(())
}

pub fn write_json_file_atomically<T: Serialize>(
    path: impl AsRef<Path>,
    value: &T,
    create_directory: bool,
) -> StorageResult<()> {
    let mut encoded = serde_json::to_string(value)?;
    encoded.push('\n');
    write_file_atomically(path, encoded.as_bytes(), create_directory)
}

#[inline]
pub fn strip_utf8_bom(contents: &str) -> &str {
    contents.strip_prefix('\u{FEFF}').unwrap_or(contents)
}

/// Returns `true` if `contents` (after stripping BOM and `\0` bytes) is empty or whitespace-only.
pub fn is_empty_stored_file(contents: &str) -> bool {
    strip_utf8_bom(contents)
        .chars()
        .filter(|&ch| ch != '\0')
        .all(char::is_whitespace)
}

pub fn parse_stored_json<T, F>(path: &Path, contents: &str, decode: F) -> StorageResult<T>
where
    F: FnOnce(JsonValue) -> StorageResult<T>,
{
    if is_empty_stored_file(contents) {
        return Err(StorageError::validation(format!(
            "Unreadable empty stored JSON file: {}",
            path.display()
        )));
    }
    let parsed: JsonValue = serde_json::from_str(strip_utf8_bom(contents)).map_err(|_| {
        StorageError::validation(format!("Unreadable stored JSON file: {}", path.display()))
    })?;
    decode(parsed)
}

/// Renames a damaged JSON file aside to `<path>.corrupt-<epoch_ms>`.
pub fn set_aside_damaged_file(path: &Path) -> Option<PathBuf> {
    let epoch_ms = Utc::now().timestamp_millis();
    let corrupt_path = PathBuf::from(format!("{}.corrupt-{epoch_ms}", path.display()));
    if fs::rename(path, &corrupt_path).is_ok() {
        Some(corrupt_path)
    } else {
        None
    }
}

fn serialize_with_version<T: Serialize>(version: u32, value: &T) -> StorageResult<JsonValue> {
    let raw = serde_json::to_value(value)?;
    match raw {
        JsonValue::Object(mut obj) => {
            obj.insert("version".to_string(), JsonValue::from(version));
            Ok(JsonValue::Object(obj))
        }
        other => {
            let mut wrapper = JsonMap::new();
            wrapper.insert("version".to_string(), JsonValue::from(version));
            wrapper.insert("value".to_string(), other);
            Ok(JsonValue::Object(wrapper))
        }
    }
}

fn default_decode_json<T: DeserializeOwned>(parsed: &JsonValue) -> Option<T> {
    if let Ok(direct) = serde_json::from_value::<T>(parsed.clone()) {
        return Some(direct);
    }
    if let Some(obj) = parsed.as_object() {
        if let Some(inner) = obj.get("value")
            && let Ok(unwrapped) = serde_json::from_value::<T>(inner.clone())
        {
            return Some(unwrapped);
        }
        let mut stripped = obj.clone();
        stripped.remove("version");
        if let Ok(without_ver) = serde_json::from_value::<T>(JsonValue::Object(stripped)) {
            return Some(without_ver);
        }
    }
    None
}

type PreferenceDecoder<T> = Arc<dyn Fn(&JsonValue) -> Option<T> + Send + Sync>;

pub struct JsonPreferenceStore<T> {
    path: PathBuf,
    default_value: T,
    decode: PreferenceDecoder<T>,
    value: Mutex<T>,
}

pub type PreferenceStore<T> = JsonPreferenceStore<T>;

impl<T: Clone + Serialize + DeserializeOwned + Send + 'static> JsonPreferenceStore<T> {
    pub fn new(path: impl Into<PathBuf>, default_value: T) -> Self {
        Self::with_decoder(path, default_value, default_decode_json::<T>)
    }
}

impl<T: Clone + Serialize> JsonPreferenceStore<T> {
    pub fn with_decoder<F>(path: impl Into<PathBuf>, default_value: T, decode: F) -> Self
    where
        F: Fn(&JsonValue) -> Option<T> + Send + Sync + 'static,
    {
        let initial = default_value.clone();
        Self {
            path: path.into(),
            default_value,
            decode: Arc::new(decode),
            value: Mutex::new(initial),
        }
    }

    #[inline]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> StorageResult<T> {
        let mut guard = self.value.lock();
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                *guard = self.default_value.clone();
                return Ok(guard.clone());
            }
            Err(err) => return Err(StorageError::Io(err)),
        };

        let Ok(contents) = std::str::from_utf8(&bytes) else {
            set_aside_damaged_file(&self.path);
            *guard = self.default_value.clone();
            return Ok(guard.clone());
        };

        if is_empty_stored_file(contents) {
            set_aside_damaged_file(&self.path);
            *guard = self.default_value.clone();
            return Ok(guard.clone());
        }

        match serde_json::from_str::<JsonValue>(strip_utf8_bom(contents)) {
            Ok(parsed) => match (self.decode)(&parsed) {
                Some(decoded) => *guard = decoded,
                None => {
                    set_aside_damaged_file(&self.path);
                    *guard = self.default_value.clone();
                }
            },
            Err(_) => {
                set_aside_damaged_file(&self.path);
                *guard = self.default_value.clone();
            }
        }
        Ok(guard.clone())
    }

    pub fn get(&self) -> T {
        self.value.lock().clone()
    }

    pub fn update<F: FnOnce(T) -> T>(&self, change: F) -> StorageResult<T> {
        let mut guard = self.value.lock();
        let next = change(guard.clone());
        let payload = serialize_with_version(1, &next)?;
        write_json_file_atomically(&self.path, &payload, true)?;
        *guard = next.clone();
        Ok(next)
    }

    pub fn set(&self, value: T) -> StorageResult<T> {
        self.update(|_| value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionedJsonFileState {
    Missing,
    Loaded,
    Damaged,
    Kept,
    Unreadable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionedJsonLoad<T> {
    pub value: Option<T>,
    pub write_path: PathBuf,
    pub state: VersionedJsonFileState,
}

pub fn versioned_json_fallback_path(path: &Path, version: u32) -> PathBuf {
    PathBuf::from(format!("{}.v{version}", path.display()))
}

type ReleaseDecoder<T> =
    Arc<dyn Fn(&JsonMap<String, JsonValue>) -> StorageResult<T> + Send + Sync>;

struct VersionedState<T> {
    value: T,
    write_path: PathBuf,
    last_state: VersionedJsonFileState,
}

pub struct VersionedJsonStore<T> {
    path: PathBuf,
    current_version: u32,
    default_value: T,
    releases: BTreeMap<u32, ReleaseDecoder<T>>,
    state: Mutex<VersionedState<T>>,
}

impl<T: Clone + Serialize + DeserializeOwned + Send + 'static> VersionedJsonStore<T> {
    pub fn new(path: impl Into<PathBuf>, current_version: u32, default_value: T) -> Self {
        let path = path.into();
        let initial = default_value.clone();
        let mut releases: BTreeMap<u32, ReleaseDecoder<T>> = BTreeMap::new();
        releases.insert(
            current_version,
            Arc::new(|obj| {
                default_decode_json::<T>(&JsonValue::Object(obj.clone())).ok_or_else(|| {
                    StorageError::validation("Failed to decode current versioned JSON object.")
                })
            }),
        );
        write_path_init(&path, initial, default_value, current_version, releases)
    }
}

fn write_path_init<T>(
    path: &Path,
    initial: T,
    default_value: T,
    current_version: u32,
    releases: BTreeMap<u32, ReleaseDecoder<T>>,
) -> VersionedJsonStore<T> {
    VersionedJsonStore {
        path: path.to_path_buf(),
        current_version,
        default_value,
        releases,
        state: Mutex::new(VersionedState {
            value: initial,
            write_path: path.to_path_buf(),
            last_state: VersionedJsonFileState::Missing,
        }),
    }
}

impl<T: Clone + Serialize> VersionedJsonStore<T> {
    pub fn with_migration<F>(mut self, version: u32, decode: F) -> Self
    where
        F: Fn(&JsonMap<String, JsonValue>) -> StorageResult<T> + Send + Sync + 'static,
    {
        self.releases.insert(version, Arc::new(decode));
        self
    }

    #[inline]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[inline]
    pub fn current_version(&self) -> u32 {
        self.current_version
    }

    pub fn write_path(&self) -> PathBuf {
        self.state.lock().write_path.clone()
    }

    pub fn last_state(&self) -> VersionedJsonFileState {
        self.state.lock().last_state
    }

    pub fn load(&self) -> StorageResult<VersionedJsonLoad<T>> {
        let mut guard = self.state.lock();
        let main = read_one_versioned_file(&self.path, self.current_version, &self.releases);
        let resolved = if main.state != VersionedJsonFileState::Kept
            && main.state != VersionedJsonFileState::Unreadable
        {
            VersionedJsonLoad {
                value: main.value,
                write_path: self.path.clone(),
                state: main.state,
            }
        } else {
            let fallback_path = versioned_json_fallback_path(&self.path, self.current_version);
            let fallback =
                read_one_versioned_file(&fallback_path, self.current_version, &self.releases);
            VersionedJsonLoad {
                value: fallback.value,
                write_path: fallback_path,
                state: main.state,
            }
        };

        guard.write_path = resolved.write_path.clone();
        guard.last_state = resolved.state;
        guard.value = resolved
            .value
            .clone()
            .unwrap_or_else(|| self.default_value.clone());
        Ok(resolved)
    }

    pub fn get(&self) -> T {
        self.state.lock().value.clone()
    }

    pub fn update<F: FnOnce(T) -> T>(&self, change: F) -> StorageResult<T> {
        let mut guard = self.state.lock();
        let next = change(guard.value.clone());
        let payload = serialize_with_version(self.current_version, &next)?;
        write_json_file_atomically(&guard.write_path, &payload, true)?;
        guard.value = next.clone();
        Ok(next)
    }

    pub fn set(&self, value: T) -> StorageResult<T> {
        self.update(|_| value)
    }
}

struct OneFileResult<T> {
    value: Option<T>,
    state: VersionedJsonFileState,
}

fn read_one_versioned_file<T>(
    path: &Path,
    current_version: u32,
    releases: &BTreeMap<u32, ReleaseDecoder<T>>,
) -> OneFileResult<T> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return OneFileResult {
                value: None,
                state: VersionedJsonFileState::Missing,
            };
        }
        Err(_) => {
            return OneFileResult {
                value: None,
                state: VersionedJsonFileState::Unreadable,
            };
        }
    };

    let Ok(contents) = std::str::from_utf8(&bytes) else {
        set_aside_damaged_file(path);
        return OneFileResult {
            value: None,
            state: VersionedJsonFileState::Damaged,
        };
    };

    if is_empty_stored_file(contents) {
        set_aside_damaged_file(path);
        return OneFileResult {
            value: None,
            state: VersionedJsonFileState::Damaged,
        };
    }

    let Ok(JsonValue::Object(obj)) = serde_json::from_str::<JsonValue>(strip_utf8_bom(contents))
    else {
        set_aside_damaged_file(path);
        return OneFileResult {
            value: None,
            state: VersionedJsonFileState::Damaged,
        };
    };

    let version_opt = obj.get("version").and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
    });
    if let Some(version) = version_opt {
        if version > u64::from(current_version) {
            return OneFileResult {
                value: None,
                state: VersionedJsonFileState::Kept,
            };
        }
        if let Ok(ver_u32) = u32::try_from(version)
            && let Some(decoder) = releases.get(&ver_u32)
            && let Ok(decoded) = decoder(&obj)
        {
            return OneFileResult {
                value: Some(decoded),
                state: VersionedJsonFileState::Loaded,
            };
        }
    }

    set_aside_damaged_file(path);
    OneFileResult {
        value: None,
        state: VersionedJsonFileState::Damaged,
    }
}

pub trait SecretCipher: Send + Sync {
    fn can_persist(&self) -> bool {
        true
    }
    fn encrypt(&self, plaintext: &str) -> StorageResult<Vec<u8>>;
    fn decrypt(&self, ciphertext: &[u8]) -> StorageResult<String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PassthroughSecretCipher;

impl SecretCipher for PassthroughSecretCipher {
    fn encrypt(&self, plaintext: &str) -> StorageResult<Vec<u8>> {
        Ok(plaintext.as_bytes().to_vec())
    }

    fn decrypt(&self, ciphertext: &[u8]) -> StorageResult<String> {
        String::from_utf8(ciphertext.to_vec())
            .map_err(|_| StorageError::validation("Invalid UTF-8 in sealed secret payload."))
    }
}

#[derive(Serialize, Deserialize)]
struct SealedKeyPayload {
    version: u32,
    key: String,
}

/// Atomic `0600` sealed API-key store (`{"version":1,"key":"<base64>"}`), deleting the file
/// on disk whenever the key is cleared (`set(None)`).
pub struct SealedKeyStore {
    path: PathBuf,
    cipher: Arc<dyn SecretCipher>,
    key: Mutex<Option<String>>,
}

impl SealedKeyStore {
    pub fn new(path: impl Into<PathBuf>, cipher: Arc<dyn SecretCipher>) -> Self {
        Self {
            path: path.into(),
            cipher,
            key: Mutex::new(None),
        }
    }

    pub fn new_passthrough(path: impl Into<PathBuf>) -> Self {
        Self::new(path, Arc::new(PassthroughSecretCipher))
    }

    #[inline]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> StorageResult<Option<String>> {
        let mut guard = self.key.lock();
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                *guard = None;
                return Ok(None);
            }
            Err(err) => return Err(StorageError::Io(err)),
        };

        let Ok(contents) = std::str::from_utf8(&bytes) else {
            set_aside_damaged_file(&self.path);
            *guard = None;
            return Ok(None);
        };
        if is_empty_stored_file(contents) {
            set_aside_damaged_file(&self.path);
            *guard = None;
            return Ok(None);
        }

        let Ok(parsed) = serde_json::from_str::<SealedKeyPayload>(strip_utf8_bom(contents)) else {
            set_aside_damaged_file(&self.path);
            *guard = None;
            return Ok(None);
        };
        if parsed.version != 1 {
            *guard = None;
            return Ok(None);
        }
        let decoded = BASE64_STANDARD
            .decode(parsed.key.as_bytes())
            .ok()
            .and_then(|raw| self.cipher.decrypt(&raw).ok())
            .filter(|plain| !plain.is_empty());
        *guard = decoded.clone();
        Ok(decoded)
    }

    pub fn get(&self) -> Option<String> {
        self.key.lock().clone()
    }

    pub fn set(&self, key: Option<&str>) -> StorageResult<()> {
        let mut guard = self.key.lock();
        match key {
            None => {
                if let Err(err) = fs::remove_file(&self.path)
                    && err.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(StorageError::Io(err));
                }
                *guard = None;
            }
            Some(secret) => {
                if !self.cipher.can_persist() {
                    return Err(StorageError::forbidden("Secret storage is unavailable."));
                }
                let encrypted = self.cipher.encrypt(secret)?;
                let payload = SealedKeyPayload {
                    version: 1,
                    key: BASE64_STANDARD.encode(encrypted),
                };
                write_json_file_atomically(&self.path, &payload, true)?;
                *guard = Some(secret.to_string());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreferenceFileKind {
    Preference,
    Versioned,
    SealedKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreferenceFileSpec {
    pub file_name: &'static str,
    pub current_version: u32,
    pub kind: PreferenceFileKind,
}

impl PreferenceFileSpec {
    pub const fn preference(file_name: &'static str) -> Self {
        Self {
            file_name,
            current_version: 1,
            kind: PreferenceFileKind::Preference,
        }
    }

    pub const fn versioned(file_name: &'static str, current_version: u32) -> Self {
        Self {
            file_name,
            current_version,
            kind: PreferenceFileKind::Versioned,
        }
    }

    pub const fn sealed_key(file_name: &'static str) -> Self {
        Self {
            file_name,
            current_version: 1,
            kind: PreferenceFileKind::SealedKey,
        }
    }

    pub fn resolve(&self, data_dir: impl AsRef<Path>) -> PathBuf {
        data_dir.as_ref().join(self.file_name)
    }
}

pub mod preference_files {
    pub const SETUP_V2: &str = "openbot-setup-v2.json";
    pub const APPEARANCE_PREFERENCE_V1: &str = "openbot-appearance-preference-v1.json";
    pub const INTERFACE_SIZE_PREFERENCE_V1: &str = "openbot-interface-size-preference-v1.json";
    pub const LOGO_COLOR_PREFERENCE_V1: &str = "openbot-logo-color-preference-v1.json";
    pub const LANGUAGE_PREFERENCE_V1: &str = "openbot-language-preference-v1.json";
    pub const MAIN_WINDOW_STATE_V1: &str = "openbot-main-window-state-v1.json";
    pub const APP_BEHAVIOR_PREFERENCE_V1: &str = "openbot-app-behavior-preference-v1.json";
    pub const EXTERNAL_LINK_PREFERENCE_V1: &str = "openbot-external-link-preference-v1.json";
    pub const DYNAMIC_ISLAND_PREFERENCE_V1: &str = "openbot-dynamic-island-preference-v1.json";
    pub const SIDEBAR_LAYOUT_V1: &str = "openbot-sidebar-layout-v1.json";
    pub const SIDEBAR_PINS_V1: &str = "openbot-sidebar-pins-v1.json";
    pub const WORKFLOW_FOLDERS_V1: &str = "openbot-workflow-folders-v1.json";
    pub const LIBRARY_FOLDERS_V1: &str = "openbot-library-folders-v1.json";
    pub const AGENT_DEFAULTS_V1: &str = "openbot-agent-defaults-v1.json";
    pub const BUSY_MESSAGE_MODE_V1: &str = "openbot-busy-message-mode-v1.json";
    pub const AGENT_MEMORY_LIMIT_V1: &str = "openbot-agent-memory-limit-v1.json";
    pub const MEMORY_REVIEW_V1: &str = "openbot-memory-review-v1.json";
    pub const SKILL_LEARNING_V1: &str = "openbot-skill-learning-v1.json";
    pub const BROWSING_MEMORY_V1: &str = "openbot-browsing-memory-v1.json";
    pub const BROWSING_STEPS_V1: &str = "openbot-browsing-steps-v1.json";
    pub const HOST_TURN_LIMITS_V1: &str = "openbot-host-turn-limits-v1.json";
    pub const SERVER_PERSONALITY_V1: &str = "openbot-server-personality-v1.json";
    pub const SERVER_TIMEZONE_V1: &str = "openbot-server-timezone-v1.json";
    pub const PROMPT_CACHE_TTL_V1: &str = "openbot-prompt-cache-ttl-v1.json";
    pub const CACHE_EXPIRY_COMPACTION_V1: &str = "openbot-cache-expiry-compaction-v1.json";
    pub const CLAUDE_COMPACT_WINDOW_V1: &str = "openbot-claude-compact-window-v1.json";
    pub const CLAUDE_USER_SETUP_V1: &str = "openbot-claude-user-setup-v1.json";
    pub const PROVIDER_DETECTION_V1: &str = "openbot-provider-detection-v1.json";
    pub const PROVIDER_ENVIRONMENT_V1: &str = "openbot-provider-environment-v1.json";
    pub const MODEL_VISIBILITY_V1: &str = "openbot-model-visibility-v1.json";
    pub const CUSTOM_PROVIDERS_V1: &str = "openbot-custom-providers-v1.json";
    pub const CUSTOM_AGENTS: &str = "custom-agents.json";
    pub const APPROVAL_AUTOMATION_V3: &str = "openbot-approval-automation-v3.json";
    pub const MEDIA_APPROVAL_CREDITS_V1: &str = "openbot-media-approval-credits-v1.json";
    pub const CREDIT_ASKS_MUTED_V1: &str = "openbot-credit-asks-muted-v1.json";
    pub const CRM_V1: &str = "openbot-crm-v1.json";
    pub const HERMES_IMPORTS_V1: &str = "openbot-hermes-imports-v1.json";
    pub const ROUTINE_FEED_V1: &str = "openbot-routine-feed-v1.json";
    pub const NOTIFICATION_PREFERENCE_V1: &str = "openbot-notification-preference-v1.json";
    pub const AGENT_NOTIFICATION_LEVELS_V1: &str = "openbot-agent-notification-levels-v1.json";
    pub const PUSH_NOTIFICATIONS_V1: &str = "openbot-push-notifications-v1.json";
    pub const MACHINE_ALERTS_V1: &str = "openbot-machine-alerts-v1.json";
    pub const TEAM_SERVER_V2: &str = "openbot-team-server-v2.json";
    pub const LOCAL_SERVERS_V1: &str = "openbot-local-servers-v1.json";
    pub const REMOTE_SERVERS_V1: &str = "openbot-remote-servers-v1.json";
    pub const REMOTE_NETWORK_ROUTE_V1: &str = "openbot-remote-network-route-v1.json";
    pub const REMOTE_CONTROL_POLICY_V1: &str = "openbot-remote-control-policy-v1.json";
    pub const REMOTE_SESSION_REUSE_PREFERENCE_V1: &str = "openbot-remote-session-reuse-preference-v1.json";
    pub const SERVER_AUDIT_V1: &str = "openbot-server-audit-v1.json";
    pub const SIGNAL_HOME_V1: &str = "openbot-signal-home-v1.json";
    pub const BROWSER_STATE_V1: &str = "openbot-browser-state-v1.json";
    pub const MCP_ACCESS_V1: &str = "openbot-mcp-access-v1.json";
    pub const MCP_SERVER_OWNERS_V1: &str = "openbot-mcp-server-owners-v1.json";
    pub const MCP_OAUTH_V1: &str = "openbot-mcp-oauth-v1.json";
    pub const MCP_OAUTH_PROXY_V1: &str = "openbot-mcp-oauth-proxy-v1.json";
    pub const PROVIDER_CREDENTIALS_V1: &str = "openbot-provider-credentials-v1.json";
    pub const MESSAGING_CREDENTIALS_V1: &str = "openbot-messaging-credentials-v1.json";
    pub const MESSAGING_STATUS_V1: &str = "openbot-messaging-status-v1.json";
    pub const INGRESS_QUEUE_KEY_V1: &str = "openbot-ingress-queue-key-v1.json";
    pub const GITHUB_CONNECTOR_V1: &str = "openbot-github-connector-v1.json";
    pub const LINK_ACCOUNT_V1: &str = "openbot-link-account-v1.json";
    pub const ONEPASSWORD_CONNECTOR_V1: &str = "openbot-onepassword-connector-v1.json";
    pub const BITWARDEN_CONNECTOR_V1: &str = "openbot-bitwarden-connector-v1.json";
    pub const PROTONPASS_CONNECTOR_V1: &str = "openbot-protonpass-connector-v1.json";
    pub const VAULT_V1: &str = "openbot-vault-v1.json";
    pub const UPDATE_PREFERENCE_V1: &str = "openbot-update-preference-v1.json";
    pub const ANALYTICS_PREFERENCE_V1: &str = "openbot-analytics-preference-v1.json";
    pub const ANALYTICS_INVENTORY_V1: &str = "openbot-analytics-inventory-v1.json";
    pub const ANALYTICS_INSTALL_ID_V1: &str = "openbot-analytics-install-id-v1.json";
    pub const RESOURCE_SUMMARY_V1: &str = "openbot-resource-summary-v1.json";
    pub const RUN_MARKER_V1: &str = "openbot-run-marker-v1.json";
    pub const START_ATTEMPTS_V1: &str = "openbot-start-attempts-v1.json";
    pub const START_HALT_V1: &str = "openbot-start-halt-v1.json";
    pub const ROUTINE_HOLD: &str = "routine-hold.json";
    pub const VOICE_PREFERENCE: &str = "voice-preference.json";
    pub const SPEECH_PREFERENCE: &str = "speech-preference.json";
    pub const VOICE_SERVER: &str = "voice-server.json";
    pub const VOICE_API_KEY: &str = "voice-api-key.json";
    pub const GROQ_API_KEY: &str = "groq-api-key.json";
    pub const XAI_API_KEY: &str = "xai-api-key.json";
    pub const VOICE_SERVER_KEY: &str = "voice-server-key.json";
    pub const CRM_STRIPE_KEY: &str = "crm-stripe-key.json";
}

pub const OPENBOT_PREFERENCE_FILES: &[PreferenceFileSpec] = &[
    PreferenceFileSpec::versioned(preference_files::SETUP_V2, 2),
    PreferenceFileSpec::preference(preference_files::APPEARANCE_PREFERENCE_V1),
    PreferenceFileSpec::preference(preference_files::INTERFACE_SIZE_PREFERENCE_V1),
    PreferenceFileSpec::preference(preference_files::LOGO_COLOR_PREFERENCE_V1),
    PreferenceFileSpec::preference(preference_files::LANGUAGE_PREFERENCE_V1),
    PreferenceFileSpec::preference(preference_files::MAIN_WINDOW_STATE_V1),
    PreferenceFileSpec::preference(preference_files::APP_BEHAVIOR_PREFERENCE_V1),
    PreferenceFileSpec::preference(preference_files::EXTERNAL_LINK_PREFERENCE_V1),
    PreferenceFileSpec::versioned(preference_files::DYNAMIC_ISLAND_PREFERENCE_V1, 3),
    PreferenceFileSpec::versioned(preference_files::SIDEBAR_LAYOUT_V1, 2),
    PreferenceFileSpec::preference(preference_files::SIDEBAR_PINS_V1),
    PreferenceFileSpec::preference(preference_files::WORKFLOW_FOLDERS_V1),
    PreferenceFileSpec::preference(preference_files::LIBRARY_FOLDERS_V1),
    PreferenceFileSpec::preference(preference_files::AGENT_DEFAULTS_V1),
    PreferenceFileSpec::preference(preference_files::BUSY_MESSAGE_MODE_V1),
    PreferenceFileSpec::preference(preference_files::AGENT_MEMORY_LIMIT_V1),
    PreferenceFileSpec::preference(preference_files::MEMORY_REVIEW_V1),
    PreferenceFileSpec::preference(preference_files::SKILL_LEARNING_V1),
    PreferenceFileSpec::preference(preference_files::BROWSING_MEMORY_V1),
    PreferenceFileSpec::preference(preference_files::BROWSING_STEPS_V1),
    PreferenceFileSpec::preference(preference_files::HOST_TURN_LIMITS_V1),
    PreferenceFileSpec::preference(preference_files::SERVER_PERSONALITY_V1),
    PreferenceFileSpec::preference(preference_files::SERVER_TIMEZONE_V1),
    PreferenceFileSpec::preference(preference_files::PROMPT_CACHE_TTL_V1),
    PreferenceFileSpec::preference(preference_files::CACHE_EXPIRY_COMPACTION_V1),
    PreferenceFileSpec::preference(preference_files::CLAUDE_COMPACT_WINDOW_V1),
    PreferenceFileSpec::preference(preference_files::CLAUDE_USER_SETUP_V1),
    PreferenceFileSpec::preference(preference_files::PROVIDER_DETECTION_V1),
    PreferenceFileSpec::preference(preference_files::PROVIDER_ENVIRONMENT_V1),
    PreferenceFileSpec::preference(preference_files::MODEL_VISIBILITY_V1),
    PreferenceFileSpec::preference(preference_files::CUSTOM_PROVIDERS_V1),
    PreferenceFileSpec::preference(preference_files::CUSTOM_AGENTS),
    PreferenceFileSpec::versioned(preference_files::APPROVAL_AUTOMATION_V3, 3),
    PreferenceFileSpec::preference(preference_files::MEDIA_APPROVAL_CREDITS_V1),
    PreferenceFileSpec::preference(preference_files::CREDIT_ASKS_MUTED_V1),
    PreferenceFileSpec::preference(preference_files::CRM_V1),
    PreferenceFileSpec::preference(preference_files::HERMES_IMPORTS_V1),
    PreferenceFileSpec::preference(preference_files::ROUTINE_FEED_V1),
    PreferenceFileSpec::preference(preference_files::NOTIFICATION_PREFERENCE_V1),
    PreferenceFileSpec::preference(preference_files::AGENT_NOTIFICATION_LEVELS_V1),
    PreferenceFileSpec::preference(preference_files::PUSH_NOTIFICATIONS_V1),
    PreferenceFileSpec::preference(preference_files::MACHINE_ALERTS_V1),
    PreferenceFileSpec::versioned(preference_files::TEAM_SERVER_V2, 2),
    PreferenceFileSpec::preference(preference_files::LOCAL_SERVERS_V1),
    PreferenceFileSpec::preference(preference_files::REMOTE_SERVERS_V1),
    PreferenceFileSpec::preference(preference_files::REMOTE_NETWORK_ROUTE_V1),
    PreferenceFileSpec::preference(preference_files::REMOTE_CONTROL_POLICY_V1),
    PreferenceFileSpec::preference(preference_files::REMOTE_SESSION_REUSE_PREFERENCE_V1),
    PreferenceFileSpec::preference(preference_files::SERVER_AUDIT_V1),
    PreferenceFileSpec::preference(preference_files::SIGNAL_HOME_V1),
    PreferenceFileSpec::preference(preference_files::BROWSER_STATE_V1),
    PreferenceFileSpec::preference(preference_files::MCP_ACCESS_V1),
    PreferenceFileSpec::preference(preference_files::MCP_SERVER_OWNERS_V1),
    PreferenceFileSpec::preference(preference_files::MCP_OAUTH_V1),
    PreferenceFileSpec::preference(preference_files::MCP_OAUTH_PROXY_V1),
    PreferenceFileSpec::preference(preference_files::PROVIDER_CREDENTIALS_V1),
    PreferenceFileSpec::preference(preference_files::MESSAGING_CREDENTIALS_V1),
    PreferenceFileSpec::preference(preference_files::MESSAGING_STATUS_V1),
    PreferenceFileSpec::preference(preference_files::INGRESS_QUEUE_KEY_V1),
    PreferenceFileSpec::preference(preference_files::GITHUB_CONNECTOR_V1),
    PreferenceFileSpec::preference(preference_files::LINK_ACCOUNT_V1),
    PreferenceFileSpec::preference(preference_files::ONEPASSWORD_CONNECTOR_V1),
    PreferenceFileSpec::preference(preference_files::BITWARDEN_CONNECTOR_V1),
    PreferenceFileSpec::preference(preference_files::PROTONPASS_CONNECTOR_V1),
    PreferenceFileSpec::preference(preference_files::VAULT_V1),
    PreferenceFileSpec::preference(preference_files::UPDATE_PREFERENCE_V1),
    PreferenceFileSpec::preference(preference_files::ANALYTICS_PREFERENCE_V1),
    PreferenceFileSpec::preference(preference_files::ANALYTICS_INVENTORY_V1),
    PreferenceFileSpec::preference(preference_files::ANALYTICS_INSTALL_ID_V1),
    PreferenceFileSpec::preference(preference_files::RESOURCE_SUMMARY_V1),
    PreferenceFileSpec::preference(preference_files::RUN_MARKER_V1),
    PreferenceFileSpec::preference(preference_files::START_ATTEMPTS_V1),
    PreferenceFileSpec::preference(preference_files::START_HALT_V1),
    PreferenceFileSpec::preference(preference_files::ROUTINE_HOLD),
    PreferenceFileSpec::preference(preference_files::VOICE_PREFERENCE),
    PreferenceFileSpec::preference(preference_files::SPEECH_PREFERENCE),
    PreferenceFileSpec::preference(preference_files::VOICE_SERVER),
    PreferenceFileSpec::sealed_key(preference_files::VOICE_API_KEY),
    PreferenceFileSpec::sealed_key(preference_files::GROQ_API_KEY),
    PreferenceFileSpec::sealed_key(preference_files::XAI_API_KEY),
    PreferenceFileSpec::sealed_key(preference_files::VOICE_SERVER_KEY),
    PreferenceFileSpec::sealed_key(preference_files::CRM_STRIPE_KEY),
];

pub fn find_preference_file_spec(file_name: &str) -> Option<&'static PreferenceFileSpec> {
    OPENBOT_PREFERENCE_FILES
        .iter()
        .find(|spec| spec.file_name == file_name)
}

pub fn resolve_preference_file(data_dir: impl AsRef<Path>, file_name: &str) -> PathBuf {
    data_dir.as_ref().join(file_name)
}
