#![forbid(unsafe_code)]

//! Pure-Rust Orca Bots runtime crate (`orca_bots`):
//! - `home_store`: Bot home directory provisioning, `bot.json` ownership guard, atomic CAS file ops, skill symlinks.
//! - `controller`: Serialized async 22-agent CRUD, rollback-safe split persistence, functional patches, CAS avatars.
//! - `memory_service`: Live preamble, SHA-256 delta sync, provenance governance, 14-day Re-Add Guard, journal, overnight Tidy-Up.
//! - `items_store`: SQLite WAL 6-state Bot Items store, forward-only `user_version` read-only guard, multi-decision asks.
//! - `look_around`: Read-only daytime proactive Look Around scheduler & persistent Up Next suggestion store.
//! - `group_runtime`: Multi-bot group rooms, SQLite WAL 7 tables + rollback-safe `AFTER UPDATE` triggers, sub-threads, remote members/asks, paused mail hold, 7-code request policy.
//! - `tool_oauth`: Host-managed MCP OAuth 2.1 (PKCE S256, DCR, RFC 8707/9207/7009, SSRF private/loopback IP guard).
//! - `tool_relay`: Loopback credential-isolating HTTP/1.1 streaming relay (`127.0.0.1:0`) & atomic `scrub_bot_tool_secrets`.
//! - `shared_mcp`: Shared stdio MCP child process multiplexer with SHA-256 deduplication, env stripping, and 10s failed-start debounce.
//! - `connectors_triggers`: `ActsFor` & `--leave-thread` anti-injection, `BotAccessHost`, async `SlackMirror` barrier, 6-adapter zero-leak `WebhookEngine`, 31-event `AutomationTriggerEngine`, `OneShotWaitManager`, `WatchPrCoordinator`, and `ContentFreeWakeFrame`.
//! - `ecosystem_handover`: 24-template + 13-skill `BotTemplateInstaller`, Deflate zip `BotPackageEngine`, E2EE `BotTeamShareEngine`, cross-app `BotImportEngine`, transactional `WorkspaceHandoverCoordinator`, and `BotChatTreeBuilder`.
//! - `facade`: Unified `Send + Sync` `OrcaBotsFacade` bridging all M2 subsystems for M4/M5.

pub mod connectors_triggers;
pub mod controller;
pub mod ecosystem_handover;
pub mod facade;
pub mod group_runtime;
pub mod home_store;
pub mod items_store;
pub mod look_around;
pub mod memory_service;
pub mod shared_mcp;
pub mod tool_oauth;
pub mod tool_relay;

use orca_core::bot_memory::{BotMemoryError, TidyUpError, TidyUpErrorCode};
use orca_core::{BotError, BotErrorCode};
use thiserror::Error;

/// Unified error type for all `orca_bots` runtime operations.
#[derive(Debug, Error)]
pub enum OrcaBotsError {
    #[error("{0}")]
    Bot(#[from] BotError),

    #[error("{0}")]
    Memory(#[from] BotMemoryError),

    #[error("{0}")]
    TidyUp(#[from] TidyUpError),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("url parse error: {0}")]
    Url(#[from] url::ParseError),

    #[error("oauth error ({code}): {message}")]
    OAuth { code: &'static str, message: String },

    #[error("relay error ({status} {code}): {message}")]
    Relay {
        status: u16,
        code: &'static str,
        message: String,
    },

    #[error("shared mcp error ({code}): {message}")]
    Mcp { code: &'static str, message: String },

    #[error("CAS precondition mismatch on {path}")]
    PreconditionFailed { path: String },
}

impl From<BotErrorCode> for OrcaBotsError {
    fn from(code: BotErrorCode) -> Self {
        Self::Bot(BotError::from(code))
    }
}

impl OrcaBotsError {
    /// Constructs a structured `OrcaBotsError::Bot` error.
    pub fn bot(code: BotErrorCode, message: impl Into<String>) -> Self {
        Self::Bot(BotError::new(code, message))
    }

    /// Constructs a structured `OrcaBotsError::OAuth` error.
    pub fn oauth(code: &'static str, message: impl Into<String>) -> Self {
        Self::OAuth {
            code,
            message: message.into(),
        }
    }

    /// Constructs a structured `OrcaBotsError::Relay` error.
    pub fn relay(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self::Relay {
            status,
            code,
            message: message.into(),
        }
    }

    /// Constructs a structured `OrcaBotsError::Mcp` error.
    pub fn mcp(code: &'static str, message: impl Into<String>) -> Self {
        Self::Mcp {
            code,
            message: message.into(),
        }
    }

    /// Maps any runtime error into its canonical `BotErrorCode`.
    pub fn code(&self) -> BotErrorCode {
        match self {
            Self::Bot(err) => err.code,
            Self::Memory(err) => err.code,
            Self::TidyUp(_) => BotErrorCode::InvalidArgument,
            Self::Sqlite(_) => BotErrorCode::ItemsUnavailable,
            Self::OAuth { code, .. } => match *code {
                "attempt_expired" => BotErrorCode::Expired,
                "credential_not_found" => BotErrorCode::NotFound,
                _ => BotErrorCode::InvalidArgument,
            },
            Self::Relay { status, .. } => match *status {
                404 => BotErrorCode::NotFound,
                429 => BotErrorCode::Full,
                _ => BotErrorCode::InvalidArgument,
            },
            Self::Mcp { .. }
            | Self::Http(_)
            | Self::Url(_)
            | Self::Io(_)
            | Self::Json(_)
            | Self::PreconditionFailed { .. } => BotErrorCode::InvalidArgument,
        }
    }

    /// Extracts the structured `TidyUpErrorCode` when this error originated from Tidy-Up.
    pub fn tidy_up_code(&self) -> Option<TidyUpErrorCode> {
        match self {
            Self::TidyUp(err) => Some(err.code),
            _ => None,
        }
    }
}

/// Convenient crate-wide result alias.
pub type Result<T> = std::result::Result<T, OrcaBotsError>;

/// Returns current UTC epoch time in milliseconds.
pub(crate) fn now_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub use connectors_triggers::{
    ACCESS_REQUEST_DEFAULT_TTL_MS, AccessDecisionOutcome, AccessRecoveryReport, ActiveWatchPr,
    AutomationTriggerEngine, BotAccessHost, BotAutomationRecord,
    CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES, ConnectorAccessRequest, ConnectorAccessRequestStatus,
    ContentFreeWakeFrame, CreateAccessRequestInput, CreateAutomationRequest, CreateWebhookRequest,
    EVENT_ONLY_SCHEDULE, EventOnlySchedule, IngestWebhookCallRequest, LeaveThreadBlockReason,
    LeaveThreadContext, LeaveThreadDecision, ManagedOneShotWait, OneShotWaitManager,
    RecordWaitRunOutcome, SlackLinkedChat, SlackMirror, SlackMirrorEntry, SlackMirrorEntryState,
    SpooledWebhookPayload, WS_CLOSE_MESSAGE_TOO_BIG, WS_CLOSE_SUPERSEDED, WakeFrameKind,
    WatchPrCoordinator, WebhookCallRecord, WebhookCliView, WebhookCopyUrlCard, WebhookEngine,
    WebhookIngestOutcome, all_supported_event_kinds, compute_hmac_sha256_hex,
    decorate_external_action_body, evaluate_leave_thread_request,
    format_external_action_attribution, is_local_only_source, require_owner_asker,
    stamp_conversation_askers, verify_adapter_signature,
};
pub use controller::{
    BotConnectorPatch, BotController, BotMemberRemoval, BotPatch, CreateBotRequest,
    UpdateAvatarStyleRequest, UploadAvatarPngRequest,
};
pub use ecosystem_handover::{
    ActiveTeamShare, BUNDLED_SKILLS, BotChatTreeBuilder, BotChatTreeView, BotImportDiscoveryRoots,
    BotImportEngine, BotPackageEngine, BotTeamShareEngine, BotTemplateInstaller,
    ExportPackageOptions, ExportedBotPackage, HandOverTarget, HandoverPlacement,
    ImportedBotPackage, InMemoryWorkspaceLineage, InstalledTemplateResult,
    InstalledTemplateSchedule, WorkspaceHandoverCoordinator, WorkspaceHandoverNotification,
    WorkspaceHandoverOutcome, WorkspaceLineageAdapter, is_forbidden_foreign_file,
    suggested_bot_name, suggested_bot_purpose,
};
pub use facade::{OrcaBotsConfig, OrcaBotsFacade};
pub use group_runtime::{
    BotGroupRuntime, CreateBotGroupRequest, GroupMessageRecord, GroupMessageRequester,
    GroupMessageSender, GroupOperationRecord, GroupPendingMail, GroupSidebarEntry,
    GroupWorkerOperation, PostGroupMessageRequest, PostedGroupMessage,
};
pub use home_store::{BotHomeStore, WorktreeReleaseBotHome};
pub use items_store::{
    AnswerQuestionRequest, BotItemsStore, CreateBotItemRequest, ITEMS_SCHEMA_VERSION,
    ItemCloseJournalTarget, ListBotItemsFilter, UpdateBotItemFieldsRequest,
};
pub use look_around::{
    LookAroundEngine, LookAroundProvider, LookAroundRunInput, LookAroundRunOutcome,
    PersistedUpNextState, StartedSuggestion, UpNextStore,
};
pub use memory_service::{
    BotMemoryService, BotTurnMemorySync, ExtractedOwnerChats, TidyUpChatTranscript,
    TidyUpChatTurn, TidyUpDraftProvider, TidyUpDraftResponse, TidyUpPromptInput,
    TidyUpProviderOutput, TidyUpRunOutcome, TidyUpRunRequest, TidyUpRuntimeState,
    TidyUpSchedule, TurnEndMemoryGovernanceResult, TurnMemorySyncResult,
};
pub use shared_mcp::{
    DEFAULT_MCP_RPC_TIMEOUT_MS, FAILED_START_MS, SharedMcpChildInfo, SharedMcpPool,
    SharedMcpServerConfig, build_child_environment, compute_server_fingerprint,
    drop_inherited_orca_identity_env,
};
pub use tool_oauth::{
    ATTEMPT_TTL_SECS, BotToolOAuth, DynamicClientRegistrationRequest,
    DynamicClientRegistrationResponse, OAuthAuthorizationStart, OAuthCallbackParams,
    OAuthProviderConfig, OAuthSecurityPolicy, OAuthTokenRecord, PkcePair, REFRESH_MARGIN_SECS,
    REVOKE_DEADLINE_SECS, TOKEN_DEADLINE_SECS, generate_pkce_pair, is_blocked_ip,
    resolve_and_validate_oauth_url, validate_oauth_endpoint_url, validate_resource_indicator,
};
pub use tool_relay::{
    BotRelayGrant, BotToolRelay, BotToolRelayConfig, BoundedMcpSessionMap,
    DEFAULT_HEADER_READ_TIMEOUT, DEFAULT_OWNER_MAX_CONCURRENT, DEFAULT_OWNER_RATE_LIMIT,
    DEFAULT_OWNER_RATE_WINDOW, MCP_SESSION_MAP_CAP, McpSessionBinding, RelayToolAuth,
    RelayToolRegistration, ScrubbedSecretHeader, SecretScrubReport, is_allowed_loopback_host,
    scrub_bot_tool_secrets,
};
