//! 4-Tier Capability-Gated Plugin System V2, MCP Config Sync & Transactional Skill
//! Installer (`F19`).
//!
//! Enforces all 4 [`PluginTier`] levels (`data`, `declarative`, `sandboxed-ui`,
//! `trusted-worker`), all 16 [`PluginContributionPoint`] types, reserved publisher/ID
//! checks, package file/size caps (`<= 2_000` files, `<= 50 MiB` total, `<= 1 MiB` per file),
//! SHA-256 `capability_fingerprint()` approvals (`ApprovalRequired` exit code `3`),
//! `2_000ms` command-replacement fallback, `onError: pass|deny` hooks, and non-themable
//! [`SecuritySurface`] protection.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use orca_core::OrcaIdeErrorCode;
use orca_core::orchestration_protocol::{
    PLUGIN_ACTION_FALLBACK_TIMEOUT_MS, PLUGIN_APPROVAL_REQUIRED_EXIT_CODE, PLUGIN_MAX_FILES,
    PLUGIN_MAX_TOTAL_BYTES, PluginExecutionDecision, PluginHookErrorPolicy, PluginHookSpec,
    PluginManifestV2, PluginSystemConfig, PluginTier, PluginValidationError,
    evaluate_plugin_execution,
};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{OrcaIdeRuntimeError, Result};

/// Maximum permitted size for a single file inside a Plugin V2 package (1 MiB).
pub const PLUGIN_MAX_SINGLE_FILE_BYTES: u64 = 1_048_576;

/// Exact count of Plugin System V2 contribution points.
pub const PLUGIN_CONTRIBUTION_POINT_COUNT: usize = 16;

/// All 16 canonical Plugin System V2 contribution points (`F19`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginContributionPoint {
    Panels,
    CustomEditors,
    Themes,
    TerminalThemes,
    IconThemes,
    Fonts,
    TextmateSyntaxes,
    LanguagePacks,
    Commands,
    Keybindings,
    VmRecipes,
    Settings,
    StatusBarItems,
    McpServers,
    Events,
    Hooks,
}

impl PluginContributionPoint {
    pub const ALL: [Self; PLUGIN_CONTRIBUTION_POINT_COUNT] = [
        Self::Panels,
        Self::CustomEditors,
        Self::Themes,
        Self::TerminalThemes,
        Self::IconThemes,
        Self::Fonts,
        Self::TextmateSyntaxes,
        Self::LanguagePacks,
        Self::Commands,
        Self::Keybindings,
        Self::VmRecipes,
        Self::Settings,
        Self::StatusBarItems,
        Self::McpServers,
        Self::Events,
        Self::Hooks,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Panels => "panels",
            Self::CustomEditors => "custom_editors",
            Self::Themes => "themes",
            Self::TerminalThemes => "terminal_themes",
            Self::IconThemes => "icon_themes",
            Self::Fonts => "fonts",
            Self::TextmateSyntaxes => "textmate_syntaxes",
            Self::LanguagePacks => "language_packs",
            Self::Commands => "commands",
            Self::Keybindings => "keybindings",
            Self::VmRecipes => "vm_recipes",
            Self::Settings => "settings",
            Self::StatusBarItems => "status_bar_items",
            Self::McpServers => "mcp_servers",
            Self::Events => "events",
            Self::Hooks => "hooks",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let norm = raw.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|p| p.as_str() == norm)
    }

    /// Minimum [`PluginTier`] required to register this contribution point.
    pub const fn min_required_tier(self) -> PluginTier {
        match self {
            Self::Themes
            | Self::TerminalThemes
            | Self::IconThemes
            | Self::Fonts
            | Self::TextmateSyntaxes
            | Self::LanguagePacks => PluginTier::Data,
            Self::Commands
            | Self::Keybindings
            | Self::VmRecipes
            | Self::Settings
            | Self::McpServers => PluginTier::Declarative,
            Self::Panels | Self::CustomEditors | Self::StatusBarItems => PluginTier::SandboxedUi,
            Self::Events | Self::Hooks => PluginTier::TrustedWorker,
        }
    }

    pub const fn allowed_for_tier(self, tier: PluginTier) -> bool {
        tier_rank(tier) >= tier_rank(self.min_required_tier())
    }
}

const fn tier_rank(tier: PluginTier) -> u8 {
    match tier {
        PluginTier::Data => 0,
        PluginTier::Declarative => 1,
        PluginTier::SandboxedUi => 2,
        PluginTier::TrustedWorker => 3,
    }
}

/// Anti-spoofing surface descriptor guaranteeing security-critical UI surfaces
/// (approval dialogs, E2EE pairing screens, permission sheets) cannot be themed by plugins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecuritySurface {
    pub surface_id: String,
    pub theme_override_allowed: bool,
}

impl SecuritySurface {
    /// Creates a security-critical surface with `theme_override_allowed: false`.
    pub fn protected(surface_id: impl Into<String>) -> Self {
        Self {
            surface_id: surface_id.into(),
            theme_override_allowed: false,
        }
    }
}

/// Canonical audit actor string for plugin actions: `"plugin:<publisher>.<id>"`.
pub fn audit_actor_for_manifest(manifest: &PluginManifestV2) -> String {
    let publisher = manifest
        .publisher
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("local");
    format!("plugin:{publisher}.{}", manifest.id.trim())
}

fn is_reserved_or_invalid_slug(raw: &str) -> bool {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    let reserved = matches!(lower.as_str(), "constructor" | "prototype" | "__proto__")
        || lower.starts_with("orca-");
    let valid_chars = !trimmed.is_empty()
        && trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    reserved || !valid_chars
}

/// Validates manifest, reserved publisher/ID rules, and tier-gating for every contribution point.
pub fn validate_plugin_manifest_and_contributions(
    manifest: &PluginManifestV2,
    contributions: &[PluginContributionPoint],
) -> Result<()> {
    manifest.validate()?;
    if let Some(pub_name) = &manifest.publisher
        && is_reserved_or_invalid_slug(pub_name)
    {
        return Err(OrcaIdeRuntimeError::PluginValidation(
            PluginValidationError::ReservedOrInvalidId(pub_name.clone()),
        ));
    }
    for &point in contributions {
        if !point.allowed_for_tier(manifest.tier) {
            return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "contribution '{}' requires tier '{}' or higher (manifest tier is '{}')",
                point.as_str(),
                point.min_required_tier().as_str(),
                manifest.tier.as_str()
            )));
        }
    }
    Ok(())
}

/// Package filesystem scan summary returned by [`scan_plugin_package_dir`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginPackageScanReport {
    pub file_count: usize,
    pub total_bytes: u64,
    pub max_file_bytes: u64,
    pub package_sha256: String,
}

/// Scans a plugin package directory enforcing hard caps:
/// - `<= PLUGIN_MAX_FILES` (`2_000`) files
/// - `<= PLUGIN_MAX_TOTAL_BYTES` (`50 MiB`) total
/// - `<= PLUGIN_MAX_SINGLE_FILE_BYTES` (`1 MiB`) per individual file
/// - Rejects symlinks to prevent directory traversal outside the package root.
pub fn scan_plugin_package_dir(dir: &Path) -> Result<PluginPackageScanReport> {
    if !dir.is_dir() {
        return Err(OrcaIdeRuntimeError::not_found(format!(
            "plugin package directory '{}' does not exist",
            dir.display()
        )));
    }
    let mut files: Vec<(String, PathBuf, u64)> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut max_file_bytes: u64 = 0;

    for entry in WalkDir::new(dir).follow_links(false) {
        let entry = entry.map_err(|e| {
            OrcaIdeRuntimeError::invalid_argument(format!("plugin package scan error: {e}"))
        })?;
        let path = entry.path();
        if path == dir {
            continue;
        }
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            return Err(OrcaIdeRuntimeError::permission_denied(format!(
                "symlinks are forbidden inside plugin packages: '{}'",
                path.display()
            )));
        }
        if meta.is_file() {
            let size = meta.len();
            if size > PLUGIN_MAX_SINGLE_FILE_BYTES {
                return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                    "plugin file '{}' exceeds 1 MiB single-file cap ({size} > {PLUGIN_MAX_SINGLE_FILE_BYTES})",
                    path.display()
                )));
            }
            if files.len() >= PLUGIN_MAX_FILES {
                return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                    "plugin package exceeds file count cap ({PLUGIN_MAX_FILES})"
                )));
            }
            total_bytes = total_bytes.saturating_add(size);
            if total_bytes > PLUGIN_MAX_TOTAL_BYTES {
                return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                    "plugin package exceeds total size cap ({total_bytes} > {PLUGIN_MAX_TOTAL_BYTES})"
                )));
            }
            max_file_bytes = max_file_bytes.max(size);
            let rel = path
                .strip_prefix(dir)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
            files.push((rel, path.to_path_buf(), size));
        }
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (rel, path, _) in &files {
        hasher.update(rel.as_bytes());
        hasher.update([0]);
        let data = fs::read(path)?;
        hasher.update(&data);
        hasher.update([0xFF]);
    }
    Ok(PluginPackageScanReport {
        file_count: files.len(),
        total_bytes,
        max_file_bytes,
        package_sha256: hex::encode(hasher.finalize()),
    })
}

const PLUGIN_HOST_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS installed_plugins (
    plugin_id TEXT PRIMARY KEY,
    manifest_json TEXT NOT NULL,
    contributions_json TEXT NOT NULL DEFAULT '[]',
    package_dir TEXT,
    fingerprint TEXT NOT NULL,
    audit_actor TEXT NOT NULL,
    installed_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS plugin_approvals (
    plugin_id TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    approved_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS plugin_system_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// Validation summary returned by [`PluginHostV2::validate_manifest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginValidationSummary {
    pub plugin_id: String,
    pub tier: PluginTier,
    pub fingerprint: String,
    pub audit_actor: String,
    pub requires_approval: bool,
    pub approval_exit_code: i32,
}

/// Persisted installed plugin record inside `PluginHostV2`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPluginRecord {
    pub manifest: PluginManifestV2,
    pub contributions: Vec<PluginContributionPoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_dir: Option<String>,
    pub fingerprint: String,
    pub audit_actor: String,
    pub installed_at_ms: i64,
}

/// Result of executing a plugin command override with `2_000ms` built-in fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCommandExecution<T> {
    pub value: T,
    pub used_fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    pub audit_actor: String,
}

/// Result of dispatching a `TrustedWorker` hook honoring `PluginHookErrorPolicy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum HookDispatchOutcome {
    Allowed { audit_actor: String },
    PassedOnError { reason: String, audit_actor: String },
    DeniedOnError { reason: String, audit_actor: String },
}

struct PluginHostInner {
    conn: Connection,
    config: PluginSystemConfig,
    plugins: BTreeMap<String, InstalledPluginRecord>,
}

/// Capability-gated 4-tier Plugin System V2 host (`Send + Sync`, SQLite WAL backed).
pub struct PluginHostV2 {
    inner: Mutex<PluginHostInner>,
}

impl fmt::Debug for PluginHostV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self.inner.lock();
        f.debug_struct("PluginHostV2")
            .field("enabled", &guard.config.enabled)
            .field("installed_plugins", &guard.plugins.len())
            .finish()
    }
}

fn init_plugin_host_inner(conn: Connection, default_enabled: bool) -> Result<PluginHostInner> {
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(PLUGIN_HOST_SCHEMA_SQL)?;

    let enabled: bool = match conn
        .query_row(
            "SELECT value FROM plugin_system_settings WHERE key = 'enabled'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        Some(v) => v == "true",
        None => {
            conn.execute(
                "INSERT INTO plugin_system_settings (key, value) VALUES ('enabled', ?1)",
                params![if default_enabled { "true" } else { "false" }],
            )?;
            default_enabled
        }
    };

    let mut approved_plugin_ids = BTreeSet::new();
    let mut approved_fingerprints = BTreeMap::new();
    {
        let mut stmt = conn.prepare("SELECT plugin_id, fingerprint FROM plugin_approvals")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (pid, fp) = row?;
            approved_plugin_ids.insert(pid.clone());
            approved_fingerprints.insert(pid, fp);
        }
    }

    let mut plugins = BTreeMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT manifest_json, contributions_json, package_dir, fingerprint, audit_actor, installed_at_ms
             FROM installed_plugins",
        )?;
        let rows = stmt.query_map([], |r| {
            let m_raw: String = r.get(0)?;
            let c_raw: String = r.get(1)?;
            Ok((
                m_raw,
                c_raw,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        for row in rows {
            let (m_raw, c_raw, package_dir, fingerprint, audit_actor, installed_at_ms) = row?;
            let manifest: PluginManifestV2 = serde_json::from_str(&m_raw)?;
            let contributions: Vec<PluginContributionPoint> =
                serde_json::from_str(&c_raw).unwrap_or_default();
            plugins.insert(
                manifest.id.clone(),
                InstalledPluginRecord {
                    manifest,
                    contributions,
                    package_dir,
                    fingerprint,
                    audit_actor,
                    installed_at_ms,
                },
            );
        }
    }

    Ok(PluginHostInner {
        conn,
        config: PluginSystemConfig {
            enabled,
            approved_plugin_ids,
            approved_fingerprints,
        },
        plugins,
    })
}

impl PluginHostV2 {
    pub fn open(db_path: impl AsRef<Path>, default_enabled: bool) -> Result<Self> {
        let path = db_path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Ok(Self {
            inner: Mutex::new(init_plugin_host_inner(conn, default_enabled)?),
        })
    }

    pub fn in_memory(default_enabled: bool) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Ok(Self {
            inner: Mutex::new(init_plugin_host_inner(conn, default_enabled)?),
        })
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        let mut guard = self.inner.lock();
        guard.conn.execute(
            "INSERT INTO plugin_system_settings (key, value) VALUES ('enabled', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![if enabled { "true" } else { "false" }],
        )?;
        guard.config.enabled = enabled;
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.lock().config.enabled
    }

    pub fn system_config(&self) -> PluginSystemConfig {
        self.inner.lock().config.clone()
    }

    /// Always returns `false` for security-critical UI surfaces (`theme_override_allowed == false`).
    pub fn can_apply_plugin_theme(&self, surface: &SecuritySurface) -> bool {
        surface.theme_override_allowed
    }

    /// Validates a manifest + contribution list without installing it.
    pub fn validate_manifest(
        &self,
        manifest: &PluginManifestV2,
        contributions: &[PluginContributionPoint],
    ) -> Result<PluginValidationSummary> {
        validate_plugin_manifest_and_contributions(manifest, contributions)?;
        let requires_approval = manifest.tier.requires_approval()
            || !manifest.permissions.is_empty()
            || !manifest.tools.is_empty()
            || !manifest.hooks.is_empty();
        Ok(PluginValidationSummary {
            plugin_id: manifest.id.clone(),
            tier: manifest.tier,
            fingerprint: manifest.capability_fingerprint(),
            audit_actor: audit_actor_for_manifest(manifest),
            requires_approval,
            approval_exit_code: if requires_approval {
                PLUGIN_APPROVAL_REQUIRED_EXIT_CODE
            } else {
                0
            },
        })
    }

    /// Installs (or upgrades) a validated Plugin V2 package into SQLite WAL storage.
    pub fn install_plugin(
        &self,
        manifest: PluginManifestV2,
        contributions: Vec<PluginContributionPoint>,
        package_dir: Option<&Path>,
        now_ms: i64,
    ) -> Result<InstalledPluginRecord> {
        validate_plugin_manifest_and_contributions(&manifest, &contributions)?;
        let package_dir_str = if let Some(dir) = package_dir {
            let _scan = scan_plugin_package_dir(dir)?;
            Some(dir.to_string_lossy().to_string())
        } else {
            None
        };
        let fingerprint = manifest.capability_fingerprint();
        let audit_actor = audit_actor_for_manifest(&manifest);
        let manifest_json = serde_json::to_string(&manifest)?;
        let contributions_json = serde_json::to_string(&contributions)?;

        let mut guard = self.inner.lock();
        guard.conn.execute(
            "INSERT INTO installed_plugins
                (plugin_id, manifest_json, contributions_json, package_dir, fingerprint, audit_actor, installed_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(plugin_id) DO UPDATE SET
                manifest_json = excluded.manifest_json,
                contributions_json = excluded.contributions_json,
                package_dir = excluded.package_dir,
                fingerprint = excluded.fingerprint,
                audit_actor = excluded.audit_actor,
                installed_at_ms = excluded.installed_at_ms",
            params![
                &manifest.id,
                &manifest_json,
                &contributions_json,
                &package_dir_str,
                &fingerprint,
                &audit_actor,
                now_ms,
            ],
        )?;
        let record = InstalledPluginRecord {
            manifest: manifest.clone(),
            contributions,
            package_dir: package_dir_str,
            fingerprint,
            audit_actor,
            installed_at_ms: now_ms,
        };
        guard.plugins.insert(manifest.id, record.clone());
        Ok(record)
    }

    /// Records explicit SHA-256 capability fingerprint approval for an installed plugin.
    pub fn approve_plugin(
        &self,
        plugin_id: &str,
        expected_fingerprint: &str,
        now_ms: i64,
    ) -> Result<PluginExecutionDecision> {
        let mut guard = self.inner.lock();
        let record = guard
            .plugins
            .get(plugin_id)
            .cloned()
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("plugin '{plugin_id}'")))?;
        if record.fingerprint != expected_fingerprint.trim() {
            return Err(OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::ApprovalRequired,
                format!(
                    "fingerprint mismatch for plugin '{plugin_id}': expected '{}', got '{}'",
                    record.fingerprint,
                    expected_fingerprint.trim()
                ),
            ));
        }
        guard.conn.execute(
            "INSERT INTO plugin_approvals (plugin_id, fingerprint, approved_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(plugin_id) DO UPDATE SET
                fingerprint = excluded.fingerprint,
                approved_at_ms = excluded.approved_at_ms",
            params![plugin_id, &record.fingerprint, now_ms],
        )?;
        guard.config.approved_plugin_ids.insert(plugin_id.to_string());
        guard
            .config
            .approved_fingerprints
            .insert(plugin_id.to_string(), record.fingerprint.clone());
        let decision = evaluate_plugin_execution(&record.manifest, &guard.config)?;
        Ok(decision)
    }

    /// Evaluates whether `plugin_id` is currently permitted to execute under `PluginSystemConfig`.
    pub fn evaluate_plugin(&self, plugin_id: &str) -> Result<PluginExecutionDecision> {
        let guard = self.inner.lock();
        let record = guard
            .plugins
            .get(plugin_id)
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("plugin '{plugin_id}'")))?;
        Ok(evaluate_plugin_execution(&record.manifest, &guard.config)?)
    }

    pub fn list_plugins(&self) -> Vec<InstalledPluginRecord> {
        self.inner.lock().plugins.values().cloned().collect()
    }

    pub fn get_plugin(&self, plugin_id: &str) -> Option<InstalledPluginRecord> {
        self.inner.lock().plugins.get(plugin_id).cloned()
    }

    /// Executes a plugin command override with a hard `<= 2_000ms`
    /// (`PLUGIN_ACTION_FALLBACK_TIMEOUT_MS`) timeout, falling back to `builtin_fallback`
    /// if the plugin times out or errors.
    pub async fn execute_command_with_fallback<F, Fut, B, T>(
        &self,
        plugin_id: &str,
        timeout_override_ms: Option<u32>,
        plugin_action: F,
        builtin_fallback: B,
    ) -> Result<PluginCommandExecution<T>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
        B: FnOnce() -> T,
    {
        let _decision = self.evaluate_plugin(plugin_id)?;
        let audit_actor = self
            .get_plugin(plugin_id)
            .map(|r| r.audit_actor)
            .unwrap_or_else(|| format!("plugin:local.{plugin_id}"));

        let timeout_ms = timeout_override_ms
            .unwrap_or(PLUGIN_ACTION_FALLBACK_TIMEOUT_MS)
            .clamp(1, PLUGIN_ACTION_FALLBACK_TIMEOUT_MS);

        match tokio::time::timeout(Duration::from_millis(u64::from(timeout_ms)), plugin_action())
            .await
        {
            Ok(Ok(value)) => Ok(PluginCommandExecution {
                value,
                used_fallback: false,
                fallback_reason: None,
                audit_actor,
            }),
            Ok(Err(err)) => Ok(PluginCommandExecution {
                value: builtin_fallback(),
                used_fallback: true,
                fallback_reason: Some(format!("plugin command failed: {err}")),
                audit_actor,
            }),
            Err(_) => Ok(PluginCommandExecution {
                value: builtin_fallback(),
                used_fallback: true,
                fallback_reason: Some(format!(
                    "plugin command exceeded {timeout_ms}ms fallback timeout"
                )),
                audit_actor,
            }),
        }
    }

    /// Dispatches a `TrustedWorker` lifecycle hook honoring `PluginHookErrorPolicy::{Pass, Deny}`.
    pub fn dispatch_hook<F>(
        &self,
        plugin_id: &str,
        event: &str,
        handler: F,
    ) -> Result<HookDispatchOutcome>
    where
        F: FnOnce(&PluginHookSpec) -> std::result::Result<(), String>,
    {
        let _decision = self.evaluate_plugin(plugin_id)?;
        let record = self
            .get_plugin(plugin_id)
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("plugin '{plugin_id}'")))?;
        if record.manifest.tier != PluginTier::TrustedWorker {
            return Err(OrcaIdeRuntimeError::PluginValidation(
                PluginValidationError::HooksNotAllowedForTier(record.manifest.tier.as_str()),
            ));
        }
        let spec = record
            .manifest
            .hooks
            .iter()
            .find(|h| h.event == event)
            .ok_or_else(|| {
                OrcaIdeRuntimeError::not_found(format!(
                    "plugin '{plugin_id}' has no hook registered for event '{event}'"
                ))
            })?;

        match handler(spec) {
            Ok(()) => Ok(HookDispatchOutcome::Allowed {
                audit_actor: record.audit_actor,
            }),
            Err(reason) => match spec.on_error {
                PluginHookErrorPolicy::Pass => Ok(HookDispatchOutcome::PassedOnError {
                    reason,
                    audit_actor: record.audit_actor,
                }),
                PluginHookErrorPolicy::Deny => Ok(HookDispatchOutcome::DeniedOnError {
                    reason,
                    audit_actor: record.audit_actor,
                }),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// MCP Config Sync (Atomic JSON Merge + Secret Stripping for Bot Homes)
// ---------------------------------------------------------------------------

/// MCP server definition synchronized into agent config files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerEntry {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub source: String,
}

/// Summary returned by [`McpConfigSync::sync_to_agent_config`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSyncReport {
    pub target_path: String,
    pub merged_servers: usize,
    pub secrets_stripped: usize,
}

/// Pure-Rust MCP JSON config synchronizer preserving non-MCP keys and scrubbing
/// raw secrets when writing into bot home directories.
#[derive(Debug, Default, Clone, Copy)]
pub struct McpConfigSync;

fn looks_like_secret_env(key: &str, value: &str) -> bool {
    let k = key.to_ascii_uppercase();
    let sensitive_key = k.contains("SECRET")
        || k.contains("TOKEN")
        || k.contains("PASSWORD")
        || k.contains("PASSWD")
        || k.contains("API_KEY")
        || k.contains("PRIVATE_KEY")
        || k.contains("CREDENTIAL")
        || k.contains("AUTH")
        || k.ends_with("_KEY");
    let v = value.trim();
    let sensitive_val = v.starts_with("sk-")
        || v.starts_with("ghp_")
        || v.starts_with("gho_")
        || v.starts_with("xoxb-")
        || v.starts_with("xoxp-")
        || v.starts_with("Bearer ")
        || v.starts_with("bearer ");
    sensitive_key || sensitive_val
}

impl McpConfigSync {
    pub fn sync_to_agent_config(
        target_json_path: &Path,
        host_servers: &[McpServerEntry],
        plugin_servers: &[McpServerEntry],
        strip_secrets_for_bot_home: bool,
    ) -> Result<McpSyncReport> {
        let mut root = if target_json_path.is_file() {
            let raw = fs::read_to_string(target_json_path)?;
            serde_json::from_str::<serde_json::Value>(&raw)?
        } else {
            serde_json::json!({})
        };
        if !root.is_object() {
            return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "agent MCP config '{}' root must be a JSON object",
                target_json_path.display()
            )));
        }

        let mut merged = serde_json::Map::new();
        let mut secrets_stripped = 0usize;
        for entry in host_servers.iter().chain(plugin_servers.iter()) {
            let name = entry.name.trim();
            if name.is_empty() || entry.command.trim().is_empty() {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "MCP server entry name and command must be non-empty",
                ));
            }
            let mut env_map = serde_json::Map::new();
            for (k, v) in &entry.env {
                if strip_secrets_for_bot_home && looks_like_secret_env(k, v) {
                    secrets_stripped += 1;
                    continue;
                }
                env_map.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            merged.insert(
                name.to_string(),
                serde_json::json!({
                    "command": entry.command,
                    "args": entry.args,
                    "env": env_map,
                }),
            );
        }

        let count = merged.len();
        if let Some(obj) = root.as_object_mut() {
            obj.insert("mcpServers".to_string(), serde_json::Value::Object(merged));
        }

        if let Some(parent) = target_json_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(&root)?;
        let tmp_name = format!(
            ".{}.tmp-{}",
            target_json_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("mcp.json"),
            Uuid::new_v4()
        );
        let tmp_path = target_json_path.with_file_name(tmp_name);
        {
            let mut f = File::create(&tmp_path)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        fs::rename(&tmp_path, target_json_path)?;

        Ok(McpSyncReport {
            target_path: target_json_path.to_string_lossy().to_string(),
            merged_servers: count,
            secrets_stripped,
        })
    }
}

// ---------------------------------------------------------------------------
// Transactional Skill Installer (pending -> staged -> committed + recovery)
// ---------------------------------------------------------------------------

const SKILL_INSTALL_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS skill_install_journal (
    install_id TEXT PRIMARY KEY,
    skill_name TEXT NOT NULL,
    source_path TEXT NOT NULL,
    staging_dir TEXT NOT NULL,
    target_dir TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillInstallState {
    Pending,
    Staged,
    Committed,
    RolledBack,
}

impl SkillInstallState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Staged => "staged",
            Self::Committed => "committed",
            Self::RolledBack => "rolled_back",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw {
            "staged" => Self::Staged,
            "committed" => Self::Committed,
            "rolled_back" => Self::RolledBack,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInstallRecord {
    pub install_id: String,
    pub skill_name: String,
    pub source_path: String,
    pub staging_dir: String,
    pub target_dir: String,
    pub state: SkillInstallState,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SkillRecoveryReport {
    pub healed_install_ids: Vec<String>,
    pub cleaned_staging_dirs: usize,
}

/// Journaled transactional skill installer (`Send + Sync`, SQLite WAL backed)
/// supporting crash recovery on startup (`pending -> staged -> committed`).
pub struct TransactionalSkillInstaller {
    conn: Mutex<Connection>,
}

impl fmt::Debug for TransactionalSkillInstaller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransactionalSkillInstaller").finish()
    }
}

fn copy_dir_recursively(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in WalkDir::new(src).follow_links(false) {
        let entry = entry.map_err(|e| {
            OrcaIdeRuntimeError::invalid_argument(format!("skill copy error: {e}"))
        })?;
        let rel = entry
            .path()
            .strip_prefix(src)
            .map_err(|e| OrcaIdeRuntimeError::invalid_argument(e.to_string()))?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

impl TransactionalSkillInstaller {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let path = db_path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SKILL_INSTALL_SCHEMA_SQL)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SKILL_INSTALL_SCHEMA_SQL)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Begins a journaled install (`pending -> staged`) without committing yet.
    pub fn begin_staged_install(
        &self,
        skill_name: &str,
        source_dir: &Path,
        target_root: &Path,
        now_ms: i64,
    ) -> Result<SkillInstallRecord> {
        let name = skill_name.trim();
        if is_reserved_or_invalid_slug(name) {
            return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "invalid skill_name '{skill_name}'"
            )));
        }
        if !source_dir.is_dir() {
            return Err(OrcaIdeRuntimeError::not_found(format!(
                "skill source directory '{}' does not exist",
                source_dir.display()
            )));
        }
        fs::create_dir_all(target_root)?;
        let install_id = format!("skill-{}", Uuid::new_v4());
        let staging_dir = target_root.join(format!(".staging-{name}-{install_id}"));
        let target_dir = target_root.join(name);

        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO skill_install_journal
                (install_id, skill_name, source_path, staging_dir, target_dir, state, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
            params![
                &install_id,
                name,
                source_dir.to_string_lossy().as_ref(),
                staging_dir.to_string_lossy().as_ref(),
                target_dir.to_string_lossy().as_ref(),
                now_ms,
            ],
        )?;
        drop(conn);

        copy_dir_recursively(source_dir, &staging_dir)?;

        let conn = self.conn.lock();
        conn.execute(
            "UPDATE skill_install_journal SET state = 'staged', updated_at_ms = ?1 WHERE install_id = ?2",
            params![now_ms, &install_id],
        )?;
        Ok(SkillInstallRecord {
            install_id,
            skill_name: name.to_string(),
            source_path: source_dir.to_string_lossy().to_string(),
            staging_dir: staging_dir.to_string_lossy().to_string(),
            target_dir: target_dir.to_string_lossy().to_string(),
            state: SkillInstallState::Staged,
            updated_at_ms: now_ms,
        })
    }

    /// Commits a previously staged install (`staged -> committed`), atomically swapping
    /// `staging_dir` into `target_dir`.
    pub fn commit_staged_install(
        &self,
        install_id: &str,
        now_ms: i64,
    ) -> Result<SkillInstallRecord> {
        let record = self
            .get_record(install_id)?
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("skill install '{install_id}'")))?;
        if record.state != SkillInstallState::Staged {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "skill install '{install_id}' is in state {:?}, expected staged",
                record.state
            )));
        }
        let staging = PathBuf::from(&record.staging_dir);
        let target = PathBuf::from(&record.target_dir);
        if target.exists() {
            fs::remove_dir_all(&target)?;
        }
        fs::rename(&staging, &target)?;

        let conn = self.conn.lock();
        conn.execute(
            "UPDATE skill_install_journal SET state = 'committed', updated_at_ms = ?1 WHERE install_id = ?2",
            params![now_ms, install_id],
        )?;
        Ok(SkillInstallRecord {
            state: SkillInstallState::Committed,
            updated_at_ms: now_ms,
            ..record
        })
    }

    /// Full atomic skill installation (`pending -> staged -> committed`).
    pub fn install_skill(
        &self,
        skill_name: &str,
        source_dir: &Path,
        target_root: &Path,
        now_ms: i64,
    ) -> Result<SkillInstallRecord> {
        let staged = self.begin_staged_install(skill_name, source_dir, target_root, now_ms)?;
        self.commit_staged_install(&staged.install_id, now_ms)
    }

    /// Auto-heals any interrupted (`pending` / `staged`) installs left behind by a crash:
    /// removes orphan staging directories and marks journal rows `rolled_back`.
    pub fn recover_interrupted_installs(&self, now_ms: i64) -> Result<SkillRecoveryReport> {
        let incomplete: Vec<SkillInstallRecord> = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT install_id, skill_name, source_path, staging_dir, target_dir, state, updated_at_ms
                 FROM skill_install_journal
                 WHERE state IN ('pending', 'staged')",
            )?;
            stmt.query_map([], |r| {
                let st: String = r.get(5)?;
                Ok(SkillInstallRecord {
                    install_id: r.get(0)?,
                    skill_name: r.get(1)?,
                    source_path: r.get(2)?,
                    staging_dir: r.get(3)?,
                    target_dir: r.get(4)?,
                    state: SkillInstallState::parse(&st),
                    updated_at_ms: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut report = SkillRecoveryReport::default();
        let conn = self.conn.lock();
        for rec in incomplete {
            let staging = PathBuf::from(&rec.staging_dir);
            if staging.exists() {
                let _ = fs::remove_dir_all(&staging);
                report.cleaned_staging_dirs += 1;
            }
            conn.execute(
                "UPDATE skill_install_journal SET state = 'rolled_back', updated_at_ms = ?1 WHERE install_id = ?2",
                params![now_ms, &rec.install_id],
            )?;
            report.healed_install_ids.push(rec.install_id);
        }
        Ok(report)
    }

    pub fn get_record(&self, install_id: &str) -> Result<Option<SkillInstallRecord>> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT install_id, skill_name, source_path, staging_dir, target_dir, state, updated_at_ms
                 FROM skill_install_journal WHERE install_id = ?1",
                params![install_id],
                |r| {
                    let st: String = r.get(5)?;
                    Ok(SkillInstallRecord {
                        install_id: r.get(0)?,
                        skill_name: r.get(1)?,
                        source_path: r.get(2)?,
                        staging_dir: r.get(3)?,
                        target_dir: r.get(4)?,
                        state: SkillInstallState::parse(&st),
                        updated_at_ms: r.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    fn sample_sha() -> String {
        "a".repeat(64)
    }

    fn make_manifest(id: &str, publisher: Option<&str>, tier: PluginTier) -> PluginManifestV2 {
        PluginManifestV2 {
            id: id.to_string(),
            publisher: publisher.map(ToOwned::to_owned),
            version: "1.0.0".to_string(),
            schema_version: 2,
            tier,
            permissions: vec![],
            tools: vec![],
            hooks: vec![],
            sha256: sample_sha(),
        }
    }

    #[tokio::test]
    async fn plugin_host_four_tiers_sixteen_points_caps_approval_fallback_and_hooks() {
        assert_send_sync::<PluginHostV2>();
        assert_send_sync::<TransactionalSkillInstaller>();
        assert_eq!(PluginContributionPoint::ALL.len(), 16);

        let dir = TempDir::new().unwrap();
        let host = PluginHostV2::open(dir.path().join("plugins.db"), true).unwrap();

        // SecuritySurface cannot be themed by plugins.
        let surface = SecuritySurface::protected("e2ee-pairing-modal");
        assert!(!surface.theme_override_allowed);
        assert!(!host.can_apply_plugin_theme(&surface));

        // Reserved ID & publisher rejection.
        let bad_id = make_manifest("orca-spoof", Some("acme"), PluginTier::Data);
        assert!(host.validate_manifest(&bad_id, &[PluginContributionPoint::Themes]).is_err());
        let bad_pub = make_manifest("cool-theme", Some("__proto__"), PluginTier::Data);
        assert!(host.validate_manifest(&bad_pub, &[PluginContributionPoint::Themes]).is_err());

        // Single-file > 1 MiB cap rejection.
        let oversized_pkg = dir.path().join("oversized_pkg");
        fs::create_dir_all(&oversized_pkg).unwrap();
        fs::write(
            oversized_pkg.join("huge.bin"),
            vec![0u8; (PLUGIN_MAX_SINGLE_FILE_BYTES + 1) as usize],
        )
        .unwrap();
        assert!(scan_plugin_package_dir(&oversized_pkg).is_err());

        // Tier 1 (Data): zero approval required.
        let data_pkg = dir.path().join("theme_pkg");
        fs::create_dir_all(&data_pkg).unwrap();
        fs::write(data_pkg.join("theme.json"), r#"{"name":"nord"}"#).unwrap();
        let data_manifest = make_manifest("nord-theme", Some("acme"), PluginTier::Data);
        let rec_data = host
            .install_plugin(
                data_manifest,
                vec![PluginContributionPoint::Themes, PluginContributionPoint::Fonts],
                Some(&data_pkg),
                1000,
            )
            .unwrap();
        assert_eq!(rec_data.audit_actor, "plugin:acme.nord-theme");
        assert!(host.evaluate_plugin("nord-theme").unwrap().allowed);

        // Tier gating: Data cannot register Hooks or Panels.
        let bad_tier = make_manifest("bad-panel", Some("acme"), PluginTier::Data);
        assert!(host.validate_manifest(&bad_tier, &[PluginContributionPoint::Panels]).is_err());

        // Tier 4 (TrustedWorker) with Pass & Deny hooks -> requires SHA-256 approval!
        let mut worker_manifest = make_manifest("guard-worker", Some("sec"), PluginTier::TrustedWorker);
        worker_manifest.permissions = vec!["fs:read".to_string()];
        worker_manifest.hooks = vec![
            PluginHookSpec {
                event: "pre_commit".to_string(),
                on_error: PluginHookErrorPolicy::Deny,
                timeout_ms: 500,
            },
            PluginHookSpec {
                event: "post_open".to_string(),
                on_error: PluginHookErrorPolicy::Pass,
                timeout_ms: 500,
            },
        ];
        let rec_worker = host
            .install_plugin(
                worker_manifest,
                vec![
                    PluginContributionPoint::Hooks,
                    PluginContributionPoint::Events,
                    PluginContributionPoint::Commands,
                ],
                None,
                1100,
            )
            .unwrap();
        let err = host.evaluate_plugin("guard-worker").unwrap_err();
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::ApprovalRequired));
        assert!(err.to_string().contains("exit 3"));

        // Approve using exact SHA-256 capability fingerprint.
        let decision = host
            .approve_plugin("guard-worker", &rec_worker.fingerprint, 1200)
            .unwrap();
        assert!(decision.allowed);

        // Hook onError: deny vs pass.
        let denied = host
            .dispatch_hook("guard-worker", "pre_commit", |_| Err("lint failed".to_string()))
            .unwrap();
        assert!(matches!(denied, HookDispatchOutcome::DeniedOnError { .. }));
        let passed = host
            .dispatch_hook("guard-worker", "post_open", |_| Err("telemetry offline".to_string()))
            .unwrap();
        assert!(matches!(passed, HookDispatchOutcome::PassedOnError { .. }));

        // Command replacement timeout fallback (using short timeout cap within 2000ms limit).
        let exec = host
            .execute_command_with_fallback(
                "guard-worker",
                Some(25),
                || async {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    Ok("plugin-result")
                },
                || "builtin-fallback",
            )
            .await
            .unwrap();
        assert!(exec.used_fallback);
        assert_eq!(exec.value, "builtin-fallback");

        // Disabling plugin system returns PluginSystemDisabled.
        host.set_enabled(false).unwrap();
        let dis_err = host.evaluate_plugin("guard-worker").unwrap_err();
        assert_eq!(dis_err.ide_error_code(), Some(OrcaIdeErrorCode::PluginSystemDisabled));
    }

    #[test]
    fn mcp_sync_strips_bot_home_secrets_and_transactional_skill_recovers_crash() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("agent_mcp.json");
        fs::write(&cfg_path, r#"{"theme":"dark"}"#).unwrap();

        let mut env = BTreeMap::new();
        env.insert("API_TOKEN".to_string(), "sk-live-secret-123".to_string());
        env.insert("SAFE_MODE".to_string(), "strict".to_string());
        let report = McpConfigSync::sync_to_agent_config(
            &cfg_path,
            &[McpServerEntry {
                name: "github".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@mcp/github".to_string()],
                env,
                source: "host".to_string(),
            }],
            &[],
            true,
        )
        .unwrap();
        assert_eq!(report.merged_servers, 1);
        assert_eq!(report.secrets_stripped, 1);
        let raw = fs::read_to_string(&cfg_path).unwrap();
        assert!(!raw.contains("sk-live-secret-123"));
        assert!(raw.contains("SAFE_MODE"));
        assert!(raw.contains("\"theme\": \"dark\""));

        // Transactional skill installer crash recovery.
        let installer = TransactionalSkillInstaller::open(dir.path().join("skills.db")).unwrap();
        let src_skill = dir.path().join("src_skill");
        fs::create_dir_all(&src_skill).unwrap();
        fs::write(src_skill.join("SKILL.md"), "# My Skill\n").unwrap();
        let target_root = dir.path().join("installed_skills");

        // Simulate interrupted staged install.
        let interrupted = installer
            .begin_staged_install("broken-skill", &src_skill, &target_root, 1000)
            .unwrap();
        assert!(Path::new(&interrupted.staging_dir).is_dir());

        let recov = installer.recover_interrupted_installs(2000).unwrap();
        assert_eq!(recov.healed_install_ids, vec![interrupted.install_id.clone()]);
        assert_eq!(recov.cleaned_staging_dirs, 1);
        assert!(!Path::new(&interrupted.staging_dir).exists());

        // Normal full commit succeeds cleanly.
        let committed = installer
            .install_skill("clean-skill", &src_skill, &target_root, 3000)
            .unwrap();
        assert_eq!(committed.state, SkillInstallState::Committed);
        assert!(target_root.join("clean-skill").join("SKILL.md").is_file());
    }
}
