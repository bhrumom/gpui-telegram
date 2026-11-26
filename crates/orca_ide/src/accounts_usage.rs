//! Multi-Account Isolated Runtime Homes, LKG `config.toml` Fallback, Bounded Turn-End
//! Hot-Swap Coordinator, Idempotent Codex Rate-Limit Reset Credit Ledger, and
//! 9-Provider Usage / Rate-Limit Roster with `UsageLimitContinue` Integration (`F15`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use orca_core::bot_ecosystem::UsageLimitContinue;
use parking_lot::{Mutex, RwLock};
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{OrcaIdeRuntimeError, Result, USAGE_PROVIDERS_9};

/// Exact 9 providers with isolated per-account runtime home directories.
pub const ACCOUNT_HOME_PROVIDERS_9: [&str; 9] = [
    "codex",
    "claude",
    "cursor",
    "grok",
    "antigravity",
    "opencode",
    "minimax",
    "muse",
    "zcode",
];

/// Maximum concurrent turn-end hot-swap restarts allowed across accounts.
pub const MAX_IN_FLIGHT_RESTARTS: usize = 4;

/// Cooldown window (ms) before re-prompting/re-attempting a hot-swap on the same session.
pub const REASK_COOLDOWN_MS: i64 = 90_000;

/// Utilization percentage threshold at or above which `warning_chip` turns `true`.
pub const WARNING_CHIP_THRESHOLD_PERCENT: f64 = 80.0;

/// Paths for one isolated per-account runtime home directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountHomePaths {
    pub provider: String,
    pub account_id: String,
    pub home_dir: PathBuf,
    pub config_toml_path: PathBuf,
    pub config_lkg_path: PathBuf,
}

/// Result of reading `config.toml` with automatic `config.toml.lkg` fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigLoadOutcome {
    pub content: String,
    pub used_lkg_fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_error: Option<String>,
}

/// Request to hot-swap the active account for a running session.
#[derive(Debug, Clone)]
pub struct AccountHotSwapRequest {
    pub session_id: String,
    pub provider: String,
    pub target_account_id: String,
    pub turn_in_progress: bool,
    pub force: bool,
}

/// Decision returned by the bounded turn-end hot-swap restart coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum HotSwapDecision {
    StartRestart {
        restart_id: String,
        target_account_id: String,
        target_home_dir: PathBuf,
    },
    QueuedForTurnEnd {
        target_account_id: String,
    },
    DeferredConcurrencyCap {
        in_flight: usize,
        max_in_flight: usize,
    },
    DeferredCooldown {
        retry_after_ms: i64,
    },
}

/// One idempotent record in the Codex rate-limit reset credit attempt ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexResetCreditEntry {
    pub account_id: String,
    pub reset_window_key: String,
    pub credit_amount_usd_cents: u64,
    pub recorded_at_ms: i64,
    pub attempt_id: String,
}

/// Outcome of [`AccountHomeManager::record_codex_reset_credit`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CodexResetCreditOutcome {
    Recorded(CodexResetCreditEntry),
    AlreadyRecorded(CodexResetCreditEntry),
}

#[derive(Debug, Clone)]
struct InFlightRestart {
    provider: String,
    target_account_id: String,
}

#[derive(Debug, Default)]
struct AccountCoordinatorState {
    active_account_by_provider: BTreeMap<String, String>,
    in_flight_restarts: BTreeMap<String, InFlightRestart>,
    last_reask_at_ms: BTreeMap<String, i64>,
    codex_reset_credits: BTreeMap<(String, String), CodexResetCreditEntry>,
}

/// Manages isolated per-account runtime homes, atomic `config.toml` + `config.toml.lkg`
/// fallback, bounded turn-end hot-swap restarts, and the idempotent Codex rate-limit
/// reset credit ledger (`Send + Sync`).
pub struct AccountHomeManager {
    base_dir: PathBuf,
    state: Mutex<AccountCoordinatorState>,
}

fn validate_home_provider(provider: &str) -> Result<String> {
    let norm = provider.trim().to_ascii_lowercase();
    if !ACCOUNT_HOME_PROVIDERS_9.contains(&norm.as_str()) {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unsupported account home provider '{provider}'"
        )));
    }
    Ok(norm)
}

fn validate_account_slug(account_id: &str) -> Result<String> {
    let trimmed = account_id.trim();
    if trimmed.is_empty()
        || trimmed.len() > 128
        || trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.chars().any(char::is_control)
    {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "invalid account_id '{account_id}'"
        )));
    }
    Ok(trimmed.to_string())
}

/// Validates that `raw` is a non-empty, non-corrupt TOML config document.
pub fn is_valid_config_toml(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() || raw.contains('\0') {
        return false;
    }
    let mut has_entry = false;
    for line in trimmed.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if l.starts_with('[') && l.ends_with(']') && l.len() >= 3 {
            has_entry = true;
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let key = k.trim();
            let val = v.trim();
            if key.is_empty() || val.is_empty() {
                return false;
            }
            if val.starts_with('"') && (!val.ends_with('"') || val.len() < 2) {
                return false;
            }
            has_entry = true;
        } else {
            return false;
        }
    }
    has_entry
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_name = format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file"),
        Uuid::new_v4()
    );
    let tmp_path = path.with_file_name(tmp_name);
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

impl AccountHomeManager {
    /// Opens (or initializes) multi-account runtime storage under `base_dir`, loading
    /// any persisted idempotent Codex rate-limit reset credit ledger.
    pub fn open(base_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = base_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let ledger_path = dir.join("codex_reset_credits.json");
        let mut codex_reset_credits = BTreeMap::new();
        if ledger_path.is_file() {
            let raw = std::fs::read_to_string(&ledger_path)?;
            let entries: Vec<CodexResetCreditEntry> = serde_json::from_str(&raw)?;
            for entry in entries {
                codex_reset_credits.insert(
                    (entry.account_id.clone(), entry.reset_window_key.clone()),
                    entry,
                );
            }
        }
        Ok(Self {
            base_dir: dir,
            state: Mutex::new(AccountCoordinatorState {
                codex_reset_credits,
                ..AccountCoordinatorState::default()
            }),
        })
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Creates and returns isolated runtime home paths `<base_dir>/accounts/<provider>/<account_id>`.
    pub fn ensure_account_home(
        &self,
        provider: &str,
        account_id: &str,
    ) -> Result<AccountHomePaths> {
        let prov = validate_home_provider(provider)?;
        let acct = validate_account_slug(account_id)?;
        let home_dir = self.base_dir.join("accounts").join(&prov).join(&acct);
        std::fs::create_dir_all(&home_dir)?;
        let config_toml_path = home_dir.join("config.toml");
        let config_lkg_path = home_dir.join("config.toml.lkg");
        Ok(AccountHomePaths {
            provider: prov,
            account_id: acct,
            home_dir,
            config_toml_path,
            config_lkg_path,
        })
    }

    /// Atomically writes `config.toml` and preserves the last-known-good `config.toml.lkg`.
    pub fn write_config_toml(
        &self,
        provider: &str,
        account_id: &str,
        content: &str,
    ) -> Result<AccountHomePaths> {
        if !is_valid_config_toml(content) {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "refusing to write empty or malformed config.toml",
            ));
        }
        let paths = self.ensure_account_home(provider, account_id)?;
        if paths.config_toml_path.is_file()
            && let Ok(existing) = std::fs::read_to_string(&paths.config_toml_path)
            && is_valid_config_toml(&existing)
        {
            atomic_write_bytes(&paths.config_lkg_path, existing.as_bytes())?;
        }
        atomic_write_bytes(&paths.config_toml_path, content.as_bytes())?;
        if !paths.config_lkg_path.is_file() {
            atomic_write_bytes(&paths.config_lkg_path, content.as_bytes())?;
        }
        Ok(paths)
    }

    /// Reads `config.toml`, falling back automatically to `config.toml.lkg` and healing the
    /// primary file if `config.toml` is missing, empty, or corrupt.
    pub fn read_config_toml_with_lkg(
        &self,
        provider: &str,
        account_id: &str,
    ) -> Result<ConfigLoadOutcome> {
        let paths = self.ensure_account_home(provider, account_id)?;
        let primary_err = match std::fs::read_to_string(&paths.config_toml_path) {
            Ok(raw) if is_valid_config_toml(&raw) => {
                return Ok(ConfigLoadOutcome {
                    content: raw,
                    used_lkg_fallback: false,
                    primary_error: None,
                });
            }
            Ok(_) => "primary config.toml is empty or corrupt".to_string(),
            Err(e) => format!("failed reading primary config.toml: {e}"),
        };

        let lkg_raw = std::fs::read_to_string(&paths.config_lkg_path).map_err(|_| {
            OrcaIdeRuntimeError::not_found(format!(
                "{primary_err}; no valid config.toml.lkg available"
            ))
        })?;
        if !is_valid_config_toml(&lkg_raw) {
            return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "{primary_err}; config.toml.lkg is also corrupt"
            )));
        }
        atomic_write_bytes(&paths.config_toml_path, lkg_raw.as_bytes())?;
        Ok(ConfigLoadOutcome {
            content: lkg_raw,
            used_lkg_fallback: true,
            primary_error: Some(primary_err),
        })
    }

    /// Evaluates bounded turn-end hot-swap restart rules (`MAX_IN_FLIGHT_RESTARTS = 4`,
    /// `REASK_COOLDOWN_MS = 90_000`) and either starts, queues, or defers the restart.
    pub fn request_account_hotswap(
        &self,
        req: &AccountHotSwapRequest,
        now_ms: i64,
    ) -> Result<HotSwapDecision> {
        let paths = self.ensure_account_home(&req.provider, &req.target_account_id)?;
        let mut st = self.state.lock();

        if !req.force
            && let Some(&last_ask) = st.last_reask_at_ms.get(&req.session_id)
        {
            let retry_at = last_ask.saturating_add(REASK_COOLDOWN_MS);
            if now_ms < retry_at {
                return Ok(HotSwapDecision::DeferredCooldown {
                    retry_after_ms: retry_at.saturating_sub(now_ms),
                });
            }
        }
        st.last_reask_at_ms.insert(req.session_id.clone(), now_ms);

        if req.turn_in_progress {
            return Ok(HotSwapDecision::QueuedForTurnEnd {
                target_account_id: paths.account_id,
            });
        }

        if st.in_flight_restarts.len() >= MAX_IN_FLIGHT_RESTARTS {
            return Ok(HotSwapDecision::DeferredConcurrencyCap {
                in_flight: st.in_flight_restarts.len(),
                max_in_flight: MAX_IN_FLIGHT_RESTARTS,
            });
        }

        let restart_id = Uuid::new_v4().to_string();
        st.in_flight_restarts.insert(
            restart_id.clone(),
            InFlightRestart {
                provider: paths.provider,
                target_account_id: paths.account_id.clone(),
            },
        );
        Ok(HotSwapDecision::StartRestart {
            restart_id,
            target_account_id: paths.account_id,
            target_home_dir: paths.home_dir,
        })
    }

    /// Marks an in-flight hot-swap restart completed, freeing its concurrency slot.
    pub fn complete_hotswap_restart(&self, restart_id: &str, succeeded: bool) -> Result<()> {
        let mut st = self.state.lock();
        let entry = st.in_flight_restarts.remove(restart_id).ok_or_else(|| {
            OrcaIdeRuntimeError::not_found(format!("hot-swap restart '{restart_id}' not found"))
        })?;
        if succeeded {
            st.active_account_by_provider
                .insert(entry.provider, entry.target_account_id);
        }
        Ok(())
    }

    pub fn active_account(&self, provider: &str) -> Option<String> {
        let norm = provider.trim().to_ascii_lowercase();
        self.state.lock().active_account_by_provider.get(&norm).cloned()
    }

    /// Idempotently records a Codex rate-limit reset credit attempt keyed by
    /// `(account_id, reset_window_key)` and persists the ledger atomically to disk.
    pub fn record_codex_reset_credit(
        &self,
        account_id: &str,
        reset_window_key: &str,
        credit_amount_usd_cents: u64,
        now_ms: i64,
    ) -> Result<CodexResetCreditOutcome> {
        let acct = validate_account_slug(account_id)?;
        let win = reset_window_key.trim();
        if win.is_empty() || win.chars().any(char::is_control) {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "reset_window_key must be non-empty without control characters",
            ));
        }

        let mut st = self.state.lock();
        let key = (acct.clone(), win.to_string());
        if let Some(existing) = st.codex_reset_credits.get(&key) {
            return Ok(CodexResetCreditOutcome::AlreadyRecorded(existing.clone()));
        }

        let entry = CodexResetCreditEntry {
            account_id: acct,
            reset_window_key: win.to_string(),
            credit_amount_usd_cents,
            recorded_at_ms: now_ms,
            attempt_id: Uuid::new_v4().to_string(),
        };
        st.codex_reset_credits.insert(key, entry.clone());
        let all: Vec<&CodexResetCreditEntry> = st.codex_reset_credits.values().collect();
        let bytes = serde_json::to_vec_pretty(&all)?;
        atomic_write_bytes(&self.base_dir.join("codex_reset_credits.json"), &bytes)?;
        Ok(CodexResetCreditOutcome::Recorded(entry))
    }

    pub fn list_codex_reset_credits(&self) -> Vec<CodexResetCreditEntry> {
        self.state
            .lock()
            .codex_reset_credits
            .values()
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 9-Provider Usage & Rate-Limit Roster (`AccountQuotaProvider` + `UsageRosterManager`)
// ---------------------------------------------------------------------------

/// Rolling rate-limit window bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindowKind {
    FiveHour,
    Daily,
    Weekly,
    Monthly,
}

/// Utilization metrics for one rolling quota window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindowUsage {
    pub window_kind: QuotaWindowKind,
    pub percent_used: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at_ms: Option<i64>,
}

/// Telemetry source that produced a quota snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaSource {
    StructuredApi,
    ClaudeStatusLine,
    LocalEstimate,
}

/// Structured quota snapshot ingested for one `(provider, account_id)` pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderQuotaSnapshot {
    pub provider: String,
    pub account_id: String,
    pub source: QuotaSource,
    pub windows: Vec<QuotaWindowUsage>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    /// Explicit USD cost when known; when `None`, inferred via [`infer_provider_cost_usd`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explicit_cost_usd: Option<f64>,
    pub captured_at_ms: i64,
}

/// Normalized usage/rate-limit roster row ready for UI & RPC surfaces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRosterEntry {
    pub provider: String,
    pub account_id: String,
    pub percent_used: f64,
    pub percent_remaining: f64,
    pub warning_chip: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tightest_reset_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour_reset_in_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_reset_in_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_reset_in_ms: Option<i64>,
    pub inferred_cost_usd: f64,
    pub source: QuotaSource,
    pub updated_at_ms: i64,
}

/// Due auto-continue item produced by [`UsageRosterManager::evaluate_auto_continues`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoContinueDue {
    pub session_key: String,
    pub state: UsageLimitContinue,
    pub wait_notice: String,
}

/// Clean pure-Rust quota provider abstraction bridged to concrete gateways in M4/M5.
pub trait AccountQuotaProvider: Send + Sync {
    fn provider_id(&self) -> &'static str;
    fn fetch_snapshot(&self, account_id: &str, now_ms: i64) -> Result<ProviderQuotaSnapshot>;
}

/// Canonicalizes a provider wire identifier against [`USAGE_PROVIDERS_9`]
/// (accepting `"claude-code"` as an alias for `"claude"`).
pub fn canonicalize_usage_provider(raw: &str) -> Result<&'static str> {
    let norm = raw.trim().to_ascii_lowercase();
    let mapped = if norm == "claude-code" {
        "claude"
    } else {
        norm.as_str()
    };
    USAGE_PROVIDERS_9
        .iter()
        .copied()
        .find(|&p| p == mapped)
        .ok_or_else(|| {
            OrcaIdeRuntimeError::invalid_argument(format!(
                "unknown usage provider '{raw}'; expected one of {:?}",
                USAGE_PROVIDERS_9
            ))
        })
}

/// Infers estimated USD cost from token counts using deterministic per-provider pricing tiers.
pub fn infer_provider_cost_usd(
    provider: &str,
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
) -> f64 {
    // Rates in USD per 1M tokens: (input, output, cached_input)
    let (in_rate, out_rate, cache_rate) = match provider {
        "claude" => (3.0, 15.0, 0.30),
        "codex" => (2.5, 10.0, 0.25),
        "grok" => (3.0, 15.0, 0.75),
        "cursor" => (2.0, 8.0, 0.20),
        "antigravity" => (1.25, 5.0, 0.15),
        "opencode-go" => (1.0, 4.0, 0.10),
        "kimi-code" => (0.60, 2.50, 0.10),
        "minimax" => (0.40, 1.60, 0.05),
        "glm-coding-plan" => (0.50, 2.00, 0.08),
        _ => (1.0, 4.0, 0.10),
    };
    let in_cost = (input_tokens as f64 / 1_000_000.0) * in_rate;
    let out_cost = (output_tokens as f64 / 1_000_000.0) * out_rate;
    let cache_cost = (cached_input_tokens as f64 / 1_000_000.0) * cache_rate;
    ((in_cost + out_cost + cache_cost) * 10_000.0).round() / 10_000.0
}

/// Manages quota snapshots across all 9 canonical usage providers (`USAGE_PROVIDERS_9`),
/// tightest-limit-first sorting, 80% warning chips, zero-network Claude status-line parsing,
/// and `orca_core::bot_ecosystem::UsageLimitContinue` auto-resume scheduling (`Send + Sync`).
#[derive(Default)]
pub struct UsageRosterManager {
    snapshots: RwLock<BTreeMap<(String, String), ProviderQuotaSnapshot>>,
    pauses: RwLock<BTreeMap<String, UsageLimitContinue>>,
}

impl UsageRosterManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingests a structured [`ProviderQuotaSnapshot`] for one of the 9 supported providers.
    pub fn ingest_snapshot(&self, snapshot: ProviderQuotaSnapshot) -> Result<UsageRosterEntry> {
        let canonical = canonicalize_usage_provider(&snapshot.provider)?;
        let acct = validate_account_slug(&snapshot.account_id)?;
        let normalized = ProviderQuotaSnapshot {
            provider: canonical.to_string(),
            account_id: acct.clone(),
            ..snapshot
        };
        let entry = build_roster_entry(&normalized, normalized.captured_at_ms);
        self.snapshots
            .write()
            .insert((canonical.to_string(), acct), normalized);
        Ok(entry)
    }

    /// Polls a pure-Rust [`AccountQuotaProvider`] implementation and stores its snapshot.
    pub fn poll_provider(
        &self,
        provider: &dyn AccountQuotaProvider,
        account_id: &str,
        now_ms: i64,
    ) -> Result<UsageRosterEntry> {
        let _ = canonicalize_usage_provider(provider.provider_id())?;
        let snapshot = provider.fetch_snapshot(account_id, now_ms)?;
        self.ingest_snapshot(snapshot)
    }

    /// Parses a zero-network Claude Code status line (e.g.,
    /// `"Claude Code · 5h: 84% (resets in 2h 15m) · 7d: 42% (resets in 3d 4h) · $1.85"`)
    /// into a structured `"claude"` quota snapshot and ingests it.
    pub fn ingest_claude_status_line(
        &self,
        account_id: &str,
        status_line: &str,
        now_ms: i64,
    ) -> Result<UsageRosterEntry> {
        let parsed = parse_claude_status_line(account_id, status_line, now_ms)?;
        self.ingest_snapshot(parsed)
    }

    /// Returns all tracked usage entries sorted **tightest limit first**:
    /// highest `percent_used` first, tie-broken by earliest `tightest_reset_at_ms`,
    /// then lexicographically by `(provider, account_id)`.
    pub fn sorted_roster(&self, now_ms: i64) -> Vec<UsageRosterEntry> {
        let mut entries: Vec<UsageRosterEntry> = self
            .snapshots
            .read()
            .values()
            .map(|snap| build_roster_entry(snap, now_ms))
            .collect();

        entries.sort_by(|a, b| {
            b.percent_used
                .total_cmp(&a.percent_used)
                .then_with(|| match (a.tightest_reset_at_ms, b.tightest_reset_at_ms) {
                    (Some(ta), Some(tb)) => ta.cmp(&tb),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                })
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.account_id.cmp(&b.account_id))
        });
        entries
    }

    /// Registers a usage-limit pause backed directly by `orca_core::bot_ecosystem::UsageLimitContinue`.
    pub fn register_usage_limit_pause(
        &self,
        session_key: impl Into<String>,
        provider: &str,
        paused_at_ms: i64,
        reset_at_ms: i64,
        auto_continue_enabled: bool,
    ) -> Result<UsageLimitContinue> {
        let canonical = canonicalize_usage_provider(provider)?;
        let key = session_key.into();
        if key.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "session_key must be non-empty",
            ));
        }
        let state = UsageLimitContinue::new(
            canonical,
            paused_at_ms,
            reset_at_ms,
            auto_continue_enabled,
        )?;
        self.pauses.write().insert(key, state.clone());
        Ok(state)
    }

    /// Evaluates all paused sessions and returns every entry currently due for auto-continue.
    pub fn evaluate_auto_continues(&self, now_ms: i64) -> Vec<AutoContinueDue> {
        self.pauses
            .read()
            .iter()
            .filter(|(_, st)| st.should_auto_continue(now_ms))
            .map(|(session_key, st)| AutoContinueDue {
                session_key: session_key.clone(),
                state: st.clone(),
                wait_notice: st.format_wait_notice(now_ms),
            })
            .collect()
    }

    /// Marks a paused session as resumed via `UsageLimitContinue::mark_resumed(now_ms)`.
    /// Propagates `BotErrorCode::Full` once `max_attempts` is exhausted.
    pub fn mark_auto_continued(
        &self,
        session_key: &str,
        now_ms: i64,
    ) -> Result<UsageLimitContinue> {
        let mut guard = self.pauses.write();
        let entry = guard.get_mut(session_key).ok_or_else(|| {
            OrcaIdeRuntimeError::not_found(format!(
                "no usage-limit pause registered for session '{session_key}'"
            ))
        })?;
        entry.mark_resumed(now_ms)?;
        Ok(entry.clone())
    }

    pub fn get_usage_limit_pause(&self, session_key: &str) -> Option<UsageLimitContinue> {
        self.pauses.read().get(session_key).cloned()
    }

    pub fn clear_usage_limit_pause(&self, session_key: &str) -> Option<UsageLimitContinue> {
        self.pauses.write().remove(session_key)
    }
}

fn build_roster_entry(snap: &ProviderQuotaSnapshot, now_ms: i64) -> UsageRosterEntry {
    let mut tightest_pct = 0.0f64;
    let mut tightest_reset: Option<i64> = None;
    let mut five_hour_reset_in_ms = None;
    let mut daily_reset_in_ms = None;
    let mut weekly_reset_in_ms = None;

    for w in &snap.windows {
        let pct = if w.percent_used.is_finite() {
            w.percent_used.clamp(0.0, 100.0)
        } else {
            0.0
        };
        let reset_in = w.reset_at_ms.map(|r| r.saturating_sub(now_ms).max(0));
        match w.window_kind {
            QuotaWindowKind::FiveHour => five_hour_reset_in_ms = reset_in,
            QuotaWindowKind::Daily => daily_reset_in_ms = reset_in,
            QuotaWindowKind::Weekly => weekly_reset_in_ms = reset_in,
            QuotaWindowKind::Monthly => {}
        }

        let is_tighter = match pct.total_cmp(&tightest_pct) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Equal => match (w.reset_at_ms, tightest_reset) {
                (Some(a), Some(b)) => a < b,
                (Some(_), None) => true,
                _ => false,
            },
            std::cmp::Ordering::Less => false,
        };
        if is_tighter {
            tightest_pct = pct;
            tightest_reset = w.reset_at_ms;
        }
    }

    let inferred_cost_usd = snap.explicit_cost_usd.unwrap_or_else(|| {
        infer_provider_cost_usd(
            &snap.provider,
            snap.input_tokens,
            snap.output_tokens,
            snap.cached_input_tokens,
        )
    });

    UsageRosterEntry {
        provider: snap.provider.clone(),
        account_id: snap.account_id.clone(),
        percent_used: tightest_pct,
        percent_remaining: (100.0 - tightest_pct).clamp(0.0, 100.0),
        warning_chip: tightest_pct >= WARNING_CHIP_THRESHOLD_PERCENT,
        tightest_reset_at_ms: tightest_reset,
        five_hour_reset_in_ms,
        daily_reset_in_ms,
        weekly_reset_in_ms,
        inferred_cost_usd,
        source: snap.source,
        updated_at_ms: snap.captured_at_ms,
    }
}

fn parse_relative_duration_ms(raw: &str) -> Option<i64> {
    static PART_RE: OnceLock<Option<Regex>> = OnceLock::new();
    let re = PART_RE
        .get_or_init(|| Regex::new(r"(?i)(\d+)\s*([dhms])").ok())
        .as_ref()?;
    let mut total_ms = 0i64;
    let mut matched = false;
    for caps in re.captures_iter(raw) {
        let val = caps.get(1)?.as_str().parse::<i64>().ok()?;
        let unit = caps.get(2)?.as_str().to_ascii_lowercase();
        let mult = match unit.as_str() {
            "d" => 86_400_000i64,
            "h" => 3_600_000i64,
            "m" => 60_000i64,
            "s" => 1_000i64,
            _ => continue,
        };
        total_ms = total_ms.saturating_add(val.saturating_mul(mult));
        matched = true;
    }
    if matched { Some(total_ms) } else { None }
}

/// Parses a zero-network Claude Code status line into a [`ProviderQuotaSnapshot`].
pub fn parse_claude_status_line(
    account_id: &str,
    status_line: &str,
    now_ms: i64,
) -> Result<ProviderQuotaSnapshot> {
    let acct = validate_account_slug(account_id)?;
    let trimmed = status_line.trim();
    if trimmed.is_empty() {
        return Err(OrcaIdeRuntimeError::invalid_argument(
            "Claude status line must not be empty",
        ));
    }

    static WIN_RE: OnceLock<Option<Regex>> = OnceLock::new();
    static COST_RE: OnceLock<Option<Regex>> = OnceLock::new();

    let win_re = WIN_RE
        .get_or_init(|| {
            Regex::new(
                r"(?i)\b(5h|session|daily|24h|weekly|7d)\b[^0-9%]{0,12}(\d+(?:\.\d+)?)\s*%(?:\s*(?:used)?\s*\(([^)]*)\))?",
            )
            .ok()
        })
        .as_ref()
        .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("status line regex unavailable"))?;

    let mut windows = Vec::new();
    for caps in win_re.captures_iter(trimmed) {
        let label = caps
            .get(1)
            .map(|m| m.as_str().to_ascii_lowercase())
            .unwrap_or_default();
        let pct = caps
            .get(2)
            .and_then(|m| m.as_str().parse::<f64>().ok())
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
        let reset_at_ms = caps
            .get(3)
            .and_then(|m| parse_relative_duration_ms(m.as_str()))
            .map(|dur| now_ms.saturating_add(dur));

        let kind = match label.as_str() {
            "5h" | "session" => QuotaWindowKind::FiveHour,
            "daily" | "24h" => QuotaWindowKind::Daily,
            "weekly" | "7d" => QuotaWindowKind::Weekly,
            _ => QuotaWindowKind::FiveHour,
        };
        windows.push(QuotaWindowUsage {
            window_kind: kind,
            percent_used: pct,
            reset_at_ms,
        });
    }

    if windows.is_empty() {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "no recognizable usage windows found in Claude status line: '{trimmed}'"
        )));
    }

    let explicit_cost_usd = COST_RE
        .get_or_init(|| Regex::new(r"\$(\d+(?:\.\d+)?)").ok())
        .as_ref()
        .and_then(|re| re.captures(trimmed))
        .and_then(|caps| caps.get(1))
        .and_then(|m| m.as_str().parse::<f64>().ok());

    Ok(ProviderQuotaSnapshot {
        provider: "claude".to_string(),
        account_id: acct,
        source: QuotaSource::ClaudeStatusLine,
        windows,
        input_tokens: 0,
        output_tokens: 0,
        cached_input_tokens: 0,
        explicit_cost_usd,
        captured_at_ms: now_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::BotErrorCode;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    struct StubQuotaProvider {
        id: &'static str,
        pct: f64,
        reset_at_ms: i64,
    }

    impl AccountQuotaProvider for StubQuotaProvider {
        fn provider_id(&self) -> &'static str {
            self.id
        }

        fn fetch_snapshot(&self, account_id: &str, now_ms: i64) -> Result<ProviderQuotaSnapshot> {
            Ok(ProviderQuotaSnapshot {
                provider: self.id.to_string(),
                account_id: account_id.to_string(),
                source: QuotaSource::StructuredApi,
                windows: vec![QuotaWindowUsage {
                    window_kind: QuotaWindowKind::FiveHour,
                    percent_used: self.pct,
                    reset_at_ms: Some(self.reset_at_ms),
                }],
                input_tokens: 1_000_000,
                output_tokens: 100_000,
                cached_input_tokens: 0,
                explicit_cost_usd: None,
                captured_at_ms: now_ms,
            })
        }
    }

    #[test]
    fn isolated_homes_lkg_fallback_hotswap_caps_and_codex_credit_idempotency() {
        assert_send_sync::<AccountHomeManager>();
        assert_send_sync::<UsageRosterManager>();

        let tmp = TempDir::new().expect("tempdir");
        let mgr = AccountHomeManager::open(tmp.path()).expect("open");

        // Isolated homes & atomic config.toml + LKG fallback on corruption
        let paths = mgr
            .write_config_toml("codex", "acct-work", "[auth]\nprofile = \"work\"\n")
            .expect("write initial");
        assert!(paths.home_dir.ends_with("accounts/codex/acct-work"));
        assert!(paths.config_lkg_path.is_file());

        // Corrupt primary config.toml -> reads cleanly from config.toml.lkg and heals primary
        std::fs::write(&paths.config_toml_path, "CORRUPT_NOT_TOML\0").expect("corrupt");
        let loaded = mgr
            .read_config_toml_with_lkg("codex", "acct-work")
            .expect("fallback read");
        assert!(loaded.used_lkg_fallback);
        assert!(loaded.content.contains("profile = \"work\""));
        let healed = mgr
            .read_config_toml_with_lkg("codex", "acct-work")
            .expect("healed read");
        assert!(!healed.used_lkg_fallback);

        // Turn-end queueing, 90s re-ask cooldown, and MAX_IN_FLIGHT_RESTARTS = 4 cap
        let queued = mgr
            .request_account_hotswap(
                &AccountHotSwapRequest {
                    session_id: "sess-1".into(),
                    provider: "claude".into(),
                    target_account_id: "acct-b".into(),
                    turn_in_progress: true,
                    force: false,
                },
                10_000,
            )
            .expect("queue");
        assert!(matches!(queued, HotSwapDecision::QueuedForTurnEnd { .. }));

        let cooldown = mgr
            .request_account_hotswap(
                &AccountHotSwapRequest {
                    session_id: "sess-1".into(),
                    provider: "claude".into(),
                    target_account_id: "acct-b".into(),
                    turn_in_progress: false,
                    force: false,
                },
                20_000,
            )
            .expect("cooldown");
        assert!(matches!(cooldown, HotSwapDecision::DeferredCooldown { .. }));

        let mut restart_ids = Vec::new();
        for idx in 0..MAX_IN_FLIGHT_RESTARTS {
            let dec = mgr
                .request_account_hotswap(
                    &AccountHotSwapRequest {
                        session_id: format!("slot-{idx}"),
                        provider: "codex".into(),
                        target_account_id: "acct-work".into(),
                        turn_in_progress: false,
                        force: false,
                    },
                    10_000,
                )
                .expect("start restart");
            match dec {
                HotSwapDecision::StartRestart { restart_id, .. } => restart_ids.push(restart_id),
                other => panic!("expected StartRestart, got {other:?}"),
            }
        }
        let capped = mgr
            .request_account_hotswap(
                &AccountHotSwapRequest {
                    session_id: "slot-overflow".into(),
                    provider: "codex".into(),
                    target_account_id: "acct-work".into(),
                    turn_in_progress: false,
                    force: false,
                },
                10_000,
            )
            .expect("cap check");
        assert_eq!(
            capped,
            HotSwapDecision::DeferredConcurrencyCap {
                in_flight: MAX_IN_FLIGHT_RESTARTS,
                max_in_flight: MAX_IN_FLIGHT_RESTARTS
            }
        );
        mgr.complete_hotswap_restart(&restart_ids[0], true)
            .expect("complete restart");
        assert_eq!(mgr.active_account("codex").as_deref(), Some("acct-work"));

        // Idempotent Codex rate-limit reset credit ledger survives restart
        let first = mgr
            .record_codex_reset_credit("acct-work", "2026-11-25T15:00Z", 500, 100_000)
            .expect("first credit");
        let first_entry = match first {
            CodexResetCreditOutcome::Recorded(e) => e,
            other => panic!("expected Recorded, got {other:?}"),
        };
        let dup = mgr
            .record_codex_reset_credit("acct-work", "2026-11-25T15:00Z", 500, 105_000)
            .expect("dup credit");
        assert_eq!(dup, CodexResetCreditOutcome::AlreadyRecorded(first_entry.clone()));

        let reopened = AccountHomeManager::open(tmp.path()).expect("reopen");
        assert_eq!(reopened.list_codex_reset_credits(), vec![first_entry]);
    }

    #[test]
    fn nine_provider_tightest_first_sorting_warning_chip_status_line_and_auto_continue() {
        let roster = UsageRosterManager::new();
        let now = 1_700_000_000_000i64;

        // Ingest zero-network Claude status line (84% -> warning_chip = true)
        let claude = roster
            .ingest_claude_status_line(
                "acct-main",
                "Claude Code · 5h: 84% (resets in 2h 15m) · 7d: 42% (resets in 3d 4h) · $1.85",
                now,
            )
            .expect("parse status line");
        assert_eq!(claude.provider, "claude");
        assert!(claude.warning_chip);
        assert!((claude.percent_used - 84.0).abs() < 1e-6);
        assert!((claude.percent_remaining - 16.0).abs() < 1e-6);
        assert_eq!(claude.five_hour_reset_in_ms, Some(2 * 3_600_000 + 15 * 60_000));
        assert!((claude.inferred_cost_usd - 1.85).abs() < 1e-6);

        // Poll remaining 8 canonical providers (testing exact all 9 providers + tie-breaking)
        let specs = [
            ("codex", 92.0, now + 3_600_000),
            ("opencode-go", 92.0, now + 1_200_000), // same 92% as codex, EARLIER reset -> MUST sort FIRST!
            ("kimi-code", 80.0, now + 5_000_000),   // exact 80.0% -> warning_chip = true
            ("minimax", 79.9, now + 5_000_000),     // 79.9% -> warning_chip = false
            ("grok", 55.0, now + 4_000_000),
            ("antigravity", 30.0, now + 8_000_000),
            ("cursor", 15.0, now + 9_000_000),
            ("glm-coding-plan", 5.0, now + 10_000_000),
        ];
        for (prov, pct, reset_at) in specs {
            let stub = StubQuotaProvider {
                id: prov,
                pct,
                reset_at_ms: reset_at,
            };
            roster
                .poll_provider(&stub, "acct-main", now)
                .expect("poll provider");
        }

        let sorted = roster.sorted_roster(now);
        assert_eq!(sorted.len(), USAGE_PROVIDERS_9.len());
        assert_eq!(sorted[0].provider, "opencode-go");
        assert_eq!(sorted[1].provider, "codex");
        assert_eq!(sorted[2].provider, "claude");
        assert_eq!(sorted[3].provider, "kimi-code");
        assert!(sorted[3].warning_chip);
        assert_eq!(sorted[4].provider, "minimax");
        assert!(!sorted[4].warning_chip);

        // UsageLimitContinue integration
        let reset_at = now + 60_000;
        roster
            .register_usage_limit_pause("bot-session-1", "codex", now, reset_at, true)
            .expect("register pause");
        assert!(roster.evaluate_auto_continues(now + 30_000).is_empty());

        let due = roster.evaluate_auto_continues(reset_at + 1);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].session_key, "bot-session-1");

        // Exhaust max_attempts (3) and verify BotErrorCode::Full
        for step in 0..3 {
            roster
                .mark_auto_continued("bot-session-1", reset_at + 10 + step)
                .expect("mark resumed");
        }
        let full_err = roster
            .mark_auto_continued("bot-session-1", reset_at + 100)
            .expect_err("4th attempt must fail with Full");
        assert_eq!(full_err.code_str(), BotErrorCode::Full.as_str());
    }
}
