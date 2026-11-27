#![forbid(unsafe_code)]

//! Pure-Rust Orca IDE runtime crate (`orca_ide`):
//! - `orchestration_engine` (F11): Durable SQLite WAL multi-agent DAG orchestration,
//!   generation-fenced dispatches, non-guessing worker liveness, 9-type mailbox &
//!   follow-up reopen, auto-teardown/archives, gates, idempotent receipts, and federation.
//! - `chat_journal` (F12): Event-sourced SQLite WAL chat journal, single-current
//!   submission attempt guard, idempotent command receipts, `provider_handle_chain`
//!   Chat<->Terminal handoff, and active-CLI-owner resume protection.
//! - `agent_catalog` (F12): 22 live Bot CLI agents, 44 TUI catalog entries, 16 ACP
//!   dialects, dynamic model/effort launch plans, and shared `/` slash-command registry.
//! - `agent_status_hooks` (F13): 4-state (`Working`/`Blocked`/`Waiting`/`Done` + `Idle`)
//!   engine with 3-layer precedence, loopback HTTP/1.1 hook server with `endpoint.env`/`.cmd`,
//!   13 ReDoS-safe `regex` JSON screen-anchor rule packs, and OSC 7501 facet parser.
//! - `worktree_manager` (F10): FolderWorkspace CRUD, Git worktree lineage, creature naming,
//!   first-work auto-rename guards, removal recovery ledger, sparse presets, `orca.yaml`,
//!   `.worktreeinclude` copy engine, checkpoints, disk analyzer, and guarded cleanup.
//! - `ports_proxy` (F10): `/proc/net/tcp(6)` listening port scanner and pure-Rust
//!   `*.orca.localhost` HTTP/1.1 + RFC 6455 WebSocket reverse proxy.
//! - `pty_multiplex` (F14): Headless VT snapshots, off-UI transient facts, hidden cold-park,
//!   fair credit/ACK round-robin mux with `ack-pending-overflow`, incarnation kill guards,
//!   and durable 30-day closed-tab tombstones.
//! - `hibernation` (F13): Strict 8-condition background agent hibernation and transparent
//!   `--resume <session_handle>` rehydration for 10 resumable agents.
//! - `accounts_usage` (F15): Isolated per-account homes, LKG `config.toml` fallback,
//!   bounded turn-end hot-swap, Codex reset credit ledger, and 9-provider usage roster.
//! - `profile_state` (F16): Domain-partitioned SQLite v3 authority with optimistic
//!   revision fencing, generation-fenced `AtomicSnapshotWriter`, `open_versioned_cache_db`,
//!   and non-destructive `ProtectedSecretPersistence`.
//! - `vault_search` (F16): Cross-agent AI Vault search over SQLite FTS5 v8 with incremental
//!   `(byte_offset, prefix_hash)` indexing, identifier splitting, and typo fuzzy repair.
//! - `diff_review` (F17): Persistent line-shifted diff annotations, local AI-vs-Human line
//!   attribution, conventional commit/PR draft builder, and multi-forge Tasks Hub.
//! - `plugin_host` (F19): 4-tier capability-gated Plugin System V2 (16 contribution points),
//!   MCP config sync, and transactional skill installer with crash recovery.
//! - `system_services` (F18 & F20): Safe Computer Use, Mobile Emulator bridge, STT dictation
//!   with GPU->CPU fallback marker, sealed-token Shareable Artifacts, Ephemeral VM doctor,
//!   and sleep-aware Hang Watchdog.
//! - `rpc_dispatcher` & `facade` (F20): E2EE v2 session manager, Unified Runtime RPC v3
//!   dispatcher across 18+ namespaces (`orca_ide` + `orca_bots`), and `OrcaIdeFacade`.

pub mod accounts_usage;
pub mod agent_catalog;
pub mod agent_status_hooks;
pub mod chat_journal;
pub mod diff_review;
pub mod facade;
pub mod hibernation;
pub mod orchestration_engine;
pub mod plugin_host;
pub mod ports_proxy;
pub mod profile_state;
pub mod pty_multiplex;
pub mod rpc_dispatcher;
pub mod system_services;
pub mod vault_search;
pub mod worktree_manager;

use orca_core::orchestration_protocol::{
    DiffAnnotationError, E2eeV2Error, PluginValidationError, ProtocolNegotiationError, VaultError,
};
use orca_core::workspace_layout::OrcaYamlError;
use orca_core::{BotError, BotErrorCode, OrcaIdeError, OrcaIdeErrorCode};
use sha2::{Digest, Sha256};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Canonical F10–F20 Parity Constants
// ---------------------------------------------------------------------------

/// WebSocket close code when a newer connection supersedes an existing session.
pub const WS_CLOSE_SUPERSEDED: u16 = 4409;

/// RFC 6455 close code when an inbound wake/control frame exceeds max payload size.
pub const WS_CLOSE_MESSAGE_TOO_BIG: u16 = 1009;

/// Default wait/gate expiration window in days when no explicit deadline is supplied.
pub const WAIT_DEFAULT_EXPIRY_DAYS: u64 = 30;

/// Maximum permitted wait/gate deadline horizon in days.
pub const WAIT_MAX_DEADLINE_DAYS: u64 = 365;

/// Canonical 9 usage/rate-limit provider wire identifiers (`F15`).
pub const USAGE_PROVIDERS_9: [&str; 9] = [
    "claude",
    "codex",
    "opencode-go",
    "kimi-code",
    "minimax",
    "grok",
    "antigravity",
    "cursor",
    "glm-coding-plan",
];

// ---------------------------------------------------------------------------
// Unified Runtime Error Envelope
// ---------------------------------------------------------------------------

/// Unified error type for all `orca_ide` runtime subsystems.
#[derive(Debug, Error)]
pub enum OrcaIdeRuntimeError {
    #[error("{0}")]
    Ide(#[from] OrcaIdeError),

    #[error("{0}")]
    Bot(#[from] BotError),

    #[error("{0}")]
    Yaml(#[from] OrcaYamlError),

    #[error("{0}")]
    Vault(#[from] VaultError),

    #[error("{0}")]
    DiffAnnotation(#[from] DiffAnnotationError),

    #[error("{0}")]
    PluginValidation(#[from] PluginValidationError),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("regex error: {0}")]
    Regex(#[from] regex::Error),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("active CLI owner: {0}")]
    ActiveCliOwner(String),

    #[error("protocol version error: {0}")]
    ProtocolVersion(#[from] ProtocolNegotiationError),

    #[error("hook server error: {0}")]
    HookServer(String),

    #[error("{0}")]
    OrcaBots(#[from] orca_bots::OrcaBotsError),

    #[error("E2EE v2 error: {0}")]
    E2ee(#[from] E2eeV2Error),

    #[error(
        "profile state revision conflict for domain '{domain}': expected {expected}, current {current}"
    )]
    ProfileStateRevisionConflict {
        domain: String,
        expected: u64,
        current: u64,
    },

    #[error("http client error: {0}")]
    Http(#[from] reqwest::Error),
}

impl From<OrcaIdeErrorCode> for OrcaIdeRuntimeError {
    fn from(code: OrcaIdeErrorCode) -> Self {
        Self::Ide(OrcaIdeError::from(code))
    }
}

impl From<BotErrorCode> for OrcaIdeRuntimeError {
    fn from(code: BotErrorCode) -> Self {
        Self::Bot(BotError::from(code))
    }
}

impl OrcaIdeRuntimeError {
    pub fn ide(code: OrcaIdeErrorCode, message: impl Into<String>) -> Self {
        Self::Ide(OrcaIdeError::new(code, message))
    }

    pub fn bot(code: BotErrorCode, message: impl Into<String>) -> Self {
        Self::Bot(BotError::new(code, message))
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::InvalidArgument(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::PermissionDenied(message.into())
    }

    pub fn active_cli_owner(message: impl Into<String>) -> Self {
        Self::ActiveCliOwner(message.into())
    }

    pub fn hook_server(message: impl Into<String>) -> Self {
        Self::HookServer(message.into())
    }

    /// Extracts the underlying `OrcaIdeErrorCode` when present.
    pub fn ide_error_code(&self) -> Option<OrcaIdeErrorCode> {
        match self {
            Self::Ide(err) => Some(err.code),
            Self::Yaml(err) => err.ide_error_code(),
            _ => None,
        }
    }

    /// Maps any runtime error to its canonical `snake_case` wire error code.
    pub fn code_str(&self) -> &'static str {
        match self {
            Self::Ide(err) => err.code.as_str(),
            Self::Bot(err) => err.code.as_str(),
            Self::Yaml(err) => err
                .ide_error_code()
                .map_or("invalid_argument", OrcaIdeErrorCode::as_str),
            Self::Vault(err) => match err {
                VaultError::VersionConflict { .. } | VaultError::StaleGeneration { .. } => {
                    "conflict"
                }
                _ => "invalid_argument",
            },
            Self::DiffAnnotation(err) => match err {
                DiffAnnotationError::NotFound(_) => "not_found",
                DiffAnnotationError::DuplicateId(_) => "conflict",
                _ => "invalid_argument",
            },
            Self::PluginValidation(_) => "invalid_argument",
            Self::Conflict(_) => "conflict",
            Self::InvalidArgument(_) | Self::Json(_) | Self::Regex(_) => "invalid_argument",
            Self::NotFound(_) => "not_found",
            Self::PermissionDenied(_) => "permission_denied",
            Self::ActiveCliOwner(_) => "active_cli_owner",
            Self::ProtocolVersion(_) => "protocol_version",
            Self::HookServer(_) => "hook_server_error",
            Self::OrcaBots(err) => err.code().as_str(),
            Self::E2ee(err) => match err {
                E2eeV2Error::ReplayDetected { .. } => "replay_detected",
                E2eeV2Error::InvalidSignature => "invalid_signature",
                _ => "e2ee_error",
            },
            Self::ProfileStateRevisionConflict { .. } => "revision_conflict",
            Self::Http(_) => "io_error",
            Self::Sqlite(_) => "storage_error",
            Self::Io(_) => "io_error",
        }
    }
}

/// Crate-wide result alias for `orca_ide`.
pub type Result<T> = std::result::Result<T, OrcaIdeRuntimeError>;

/// Current UTC epoch timestamp in milliseconds.
pub(crate) fn now_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Computes lowercase hex-encoded SHA-256 digest of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Computes deterministic canonical SHA-256 hex digest of a JSON value by sorting object keys.
pub(crate) fn canonical_json_sha256(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_canonical_json(value, &mut out);
    sha256_hex(out.as_bytes())
}

fn write_canonical_json(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => out.push_str(&n.to_string()),
        serde_json::Value::String(s) => {
            if let Ok(encoded) = serde_json::to_string(s) {
                out.push_str(&encoded);
            }
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                write_canonical_json(item, out);
            }
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (idx, key) in keys.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                if let Ok(encoded_key) = serde_json::to_string(key) {
                    out.push_str(&encoded_key);
                }
                out.push(':');
                if let Some(val) = map.get(*key) {
                    write_canonical_json(val, out);
                }
            }
            out.push('}');
        }
    }
}

pub use agent_catalog::{
    ACP_DIALECT_COUNT, AcpDialect, AcpDialectSpec, AgentCatalog, AgentCatalogEntry,
    AgentLaunchPlan, AgentModelOption, BOT_AGENT_CATALOG_COUNT, NativeDriverKind,
    SlashCommandCategory, SlashCommandEntry, SlashCommandSurface, TUI_AGENT_CATALOG_COUNT,
};
pub use agent_status_hooks::{
    AgentHookServer, AgentHookServerConfig, AgentRunState, AgentSessionStatus,
    AgentStatusEngine, AgentStatusTransition, AskUserQuestionOption, AskUserQuestionPrompt,
    BUILTIN_SCREEN_ANCHOR_PACK_COUNT, BUILTIN_SCREEN_ANCHOR_PACKS_JSON, CompiledScreenAnchorPack,
    DEFAULT_PRECEDENCE_WINDOW_MS, HookEventPayload, Osc7501ProgramStatus, ScreenAnchorRulePack,
    StatusSignalLayer, SubagentStatus, compile_rule_pack_json, is_loopback_host,
    load_builtin_rule_packs, parse_osc7501_sequences,
};
pub use chat_journal::{
    AgentSessionRecord, AgentSessionSnapshot, AppendJournalItemRequest, ChatJournalStore,
    CreateAgentSessionRequest, EnqueueSubmissionRequest, JOURNAL_DOMAIN_SCHEMA_ID,
    JOURNAL_SCHEMA_VERSION, JournalChatState, JournalEventRecord, JournalItemRecord,
    SendReceiptRecord, SubmissionAttemptRecord, SubmissionControlAction, SubmissionStatus,
};
pub use orchestration_engine::{
    CheckMailboxRequest, CompleteWorkerOutcome, CompleteWorkerRequest, CreateGateRequest,
    CreateRunRequest, CreateTaskRequest, DecisionGateKind, DecisionGatePayload,
    DecisionGateRecord, DeliveryWithMessage, DispatchContextRecord, DispatchFederatedRequest,
    FederatedDispatchRecord, FederationRelayDirection, FederationRelayItem,
    FollowUpReopenOutcome, LivenessReportOutcome, ORCHESTRATION_SCHEMA_VERSION, ObservationFact,
    OrchestrationEngine, OrchestrationQuestion, OrchestrationRunRecord, OrchestrationRunStatus,
    OrchestrationWakeEvent, PeerMailConversation, PeerMailMessage, PeerRunRecord,
    RemoteDispatchAttachment, RemoteQuestionRecord, ResolveGateAction, SendMessageOutcome,
    SendMessageRequest, SendPeerMailRequest, StartWorkerRequest, StartWorkerResult,
    WorkerTerminalArchive, WorkerTerminalResource, parse_mailbox_target,
};
pub use accounts_usage::{
    ACCOUNT_HOME_PROVIDERS_9, AccountHomeManager, AccountHomePaths, AccountHotSwapRequest,
    AccountQuotaProvider, AutoContinueDue, CodexResetCreditEntry, CodexResetCreditOutcome,
    ConfigLoadOutcome, HotSwapDecision, MAX_IN_FLIGHT_RESTARTS, ProviderQuotaSnapshot,
    QuotaSource, QuotaWindowKind, QuotaWindowUsage, REASK_COOLDOWN_MS, UsageRosterEntry,
    UsageRosterManager, WARNING_CHIP_THRESHOLD_PERCENT, canonicalize_usage_provider,
    infer_provider_cost_usd, is_valid_config_toml, parse_claude_status_line,
};
pub use hibernation::{
    HibernatedTabRecord, HibernationCandidateState, HibernationController, HibernationOutcome,
    MAX_HIBERNATION_IDLE_MS, MIN_HIBERNATION_IDLE_MS, RESUMABLE_AGENTS_10, RehydrationPlan,
    is_resumable_agent,
};
pub use ports_proxy::{
    ListeningPortScanner, ListeningSocketEntry, LocalhostWorktreeLabelProxy,
    ORCA_LOCALHOST_SUFFIX, ProcNetSocket, ProxyHostError, WEBSOCKET_ACCEPT_GUID,
    WorktreePortAttribution, compute_websocket_accept, parse_proc_net_tcp,
};
pub use pty_multiplex::{
    AckStats, ClosedTerminalTabTombstone, DEFAULT_VT_INCREMENTAL_RING_BYTES, DaemonTransientFact,
    HeadlessVtBuffer, KillSessionRequest, MultiplexConfig, MuxFrame, MuxFrameKind,
    PendingOutputChunk, PtyIncarnationId, PtyMultiplexController, TransientFactExtractor,
    tombstone_expires_at_ms,
};
pub use worktree_manager::{
    CleanupWorktreeOutcome, CleanupWorktreeRequest, CreateWorktreeRequest,
    DEFAULT_FIRST_WORK_RENAME_BACKOFF_MS, DiskUsageEntry, DiskUsageReport, ExecutedScriptStep,
    FirstWorkRenameContext, FirstWorkRenameOutcome, FirstWorkRenameSkipReason,
    MAX_FIRST_WORK_RENAME_BACKOFF_MS, OrcaYamlExecutionReport, OrcaYamlRunRequest,
    OrcaYamlRunnerPlan, OrcaYamlScriptPhase, PlannedScriptStep, ProjectGroup,
    RemovalLedgerState, RemovalRecoveryReport, ScriptOverrideMode, ScriptPlatform,
    SharedDirectoryLinkResult, SparseCheckoutPreset, TrustedFloatingCwd,
    WorktreeIncludeCopyOptions, WorktreeIncludeCopyReport, WorktreeManager, WorktreeRecord,
    WorktreeRemovalEntry, WorkspaceStatus, execute_orca_yaml_scripts,
    execute_worktree_include_copy, is_creature_branch_name, link_shared_directories,
    parse_orca_yaml_text, plan_orca_yaml_scripts, slugify_semantic_branch,
};
pub use diff_review::{
    AttributionExport, CommitPrDraft, CommitPrDraftRequest, DiffFileChange, HubTaskRecord,
    PersistentDiffReviewStore, TaskForge, TasksHubFilter, TasksHubStore,
    compose_commit_and_pr_draft, diff_annotation_to_wire_json, expand_issue_command_template,
    normalize_task_key,
};
pub use facade::{IdeStartupSyncReport, OrcaIdeConfig, OrcaIdeFacade};
pub use plugin_host::{
    HookDispatchOutcome, InstalledPluginRecord, McpConfigSync, McpServerEntry, McpSyncReport,
    PLUGIN_CONTRIBUTION_POINT_COUNT, PLUGIN_MAX_SINGLE_FILE_BYTES, PluginCommandExecution,
    PluginContributionPoint, PluginHostV2, PluginPackageScanReport, PluginValidationSummary,
    SecuritySurface, SkillInstallRecord, SkillInstallState, SkillRecoveryReport,
    TransactionalSkillInstaller, audit_actor_for_manifest, installed_plugin_to_wire_json,
    plugin_validation_to_wire_json, scan_plugin_package_dir,
    validate_plugin_manifest_and_contributions,
};
pub use profile_state::{
    AtomicSnapshotWriter, DirtyDomainMutation, InMemoryKeyringProvider, KeyringMasterKeyProvider,
    KeyringState, PROFILE_STATE_SCHEMA_VERSION, PersistedSecretEnvelope, ProfileStateAuthority,
    ProfileStateAutomationRun, ProfileStateDocument, ProfileStateDocumentPart,
    ProtectedSecretPersistence, SecretPersistenceOutcome, StagedAsyncSnapshot,
    open_versioned_cache_db,
};
pub use rpc_dispatcher::{
    DEFAULT_RPC_DEDUP_CAPACITY, E2eeHandshakeInitRequest, E2eeHandshakeServerResponse,
    E2eeV2SessionManager, RpcSubsystemHandles, UnifiedRpcDispatcher,
};
pub use system_services::{
    ArtifactKind, ArtifactPublicView, AxDiffOp, AxNode, AxTreeDiffEntry, AxTreeDiffReport,
    BASE_SNAPSHOT_CONTAMINATION_FILES, CappedAxTree, ComputerUseService, EMULATOR_AX_MAX_NODES,
    EphemeralVmService, HangBreadcrumb, MobileEmulatorService, PublishArtifactRequest,
    STT_GPU_FAILURE_MARKER_FILE, ShareableArtifactStore, SleepAwareHangWatchdog, StallSeverity,
    SttBackendMode, SttDictationService, SttStatus, VmDoctorReport, VmLifecycleOp, VmRecipePlan,
    WaitCondition, WaitProbeSnapshot, WatchdogStatus, WatchdogThresholds, WatchdogTickOutcome,
    bp_to_norm_float, cap_emulator_ax_tree, cleanup_dictation_text, compute_ax_tree_diff,
    flatten_ax_tree_value, norm_float_to_bp, redact_stdin_secret_args,
};
pub use vault_search::{
    AiVaultSearchEngine, VAULT_SCHEMA_VERSION, VaultIndexFileReport, VaultMessageInput,
    VaultMessageRecord, VaultSearchCursor, VaultSearchHit, VaultSearchRequest, VaultSearchResponse,
    damerau_levenshtein_distance, split_code_identifiers,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parity_constants_and_error_codes_match_wire_contract() {
        assert_eq!(WS_CLOSE_SUPERSEDED, 4409);
        assert_eq!(WS_CLOSE_MESSAGE_TOO_BIG, 1009);
        assert_eq!(WAIT_DEFAULT_EXPIRY_DAYS, 30);
        assert_eq!(WAIT_MAX_DEADLINE_DAYS, 365);
        assert_eq!(
            USAGE_PROVIDERS_9,
            [
                "claude",
                "codex",
                "opencode-go",
                "kimi-code",
                "minimax",
                "grok",
                "antigravity",
                "cursor",
                "glm-coding-plan",
            ]
        );
        assert_eq!(
            OrcaIdeRuntimeError::from(OrcaIdeErrorCode::StaleDispatch).code_str(),
            "stale_dispatch"
        );
        assert_eq!(
            OrcaIdeRuntimeError::from(BotErrorCode::BotNotFound).code_str(),
            "bot_not_found"
        );
        assert_eq!(OrcaIdeRuntimeError::conflict("x").code_str(), "conflict");
        assert_eq!(
            OrcaIdeRuntimeError::active_cli_owner("x").code_str(),
            "active_cli_owner"
        );
    }
}
