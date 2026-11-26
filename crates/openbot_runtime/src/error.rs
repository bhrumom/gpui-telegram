//! Unified runtime error types, failover causes, and soft-refusal payloads (`OB-F06`..`OB-F10`).

use std::fmt;

use openbot_contracts::ContractError;
use openbot_storage::StorageError;
use serde::{Deserialize, Serialize};

/// Canonical causes that can trigger automatic backup-model failover or emit a `model-switch:*` marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailoverCause {
    UsageLimit,
    RateLimit,
    ProviderUnavailable,
    Billing,
    Authentication,
    Network,
    Timeout,
    EmptyAnswer,
    ModelNotFound,
    InvalidRequest,
    ProviderInternalError,
}

impl FailoverCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UsageLimit => "usage-limit",
            Self::RateLimit => "rate-limit",
            Self::ProviderUnavailable => "provider-unavailable",
            Self::Billing => "billing",
            Self::Authentication => "authentication",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::EmptyAnswer => "empty-answer",
            Self::ModelNotFound => "model-not-found",
            Self::InvalidRequest => "invalid-request",
            Self::ProviderInternalError => "provider-internal-error",
        }
    }

    pub fn model_switch_item_type(self) -> String {
        format!("model-switch:{}", self.as_str())
    }

    pub const fn is_usage_or_rate_limit(self) -> bool {
        matches!(self, Self::UsageLimit | Self::RateLimit)
    }
}

impl fmt::Display for FailoverCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Structured soft-refusal response returned by in-turn coordination and tool handlers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoftRefusal {
    pub accepted: bool,
    pub code: String,
    pub reason: String,
}

impl SoftRefusal {
    pub fn new(code: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            accepted: false,
            code: code.into(),
            reason: reason.into(),
        }
    }
}

/// Unified runtime error enum across all `openbot_runtime` services.
#[derive(Debug, thiserror::Error)]
pub enum OpenBotRuntimeError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    LimitExceeded(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("approval required for {action}: {message}")]
    ApprovalRequired {
        action: String,
        gate_id: String,
        expires_at_ms: u64,
        message: String,
    },
    #[error("provider {provider} unavailable: {message}")]
    ProviderUnavailable { provider: String, message: String },
    #[error("provider {provider} rate limited: {message}")]
    RateLimited {
        provider: String,
        retry_after_ms: Option<u64>,
        message: String,
    },
    #[error("{0}")]
    Http(String),
    #[error("{0}")]
    Execution(String),
}

pub type RuntimeResult<T> = Result<T, OpenBotRuntimeError>;

impl OpenBotRuntimeError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Contract(_) => "validation_error",
            Self::Storage(err) => match err {
                StorageError::Validation(_) => "validation_error",
                StorageError::NotFound(_) => "not_found",
                StorageError::Conflict(_) => "conflict",
                StorageError::LimitExceeded(_) => "limit_exceeded",
                StorageError::Forbidden(_) => "forbidden",
                StorageError::NewerDatabase { .. } => "newer_database",
                StorageError::MigrationHistoryGap { .. } => "migration_history_gap",
                StorageError::ForeignKeyViolation { .. } => "foreign_key_violation",
                StorageError::IntegrityCheckFailed(_) => "integrity_check_failed",
                StorageError::Sqlite(_) | StorageError::Json(_) | StorageError::Io(_) => {
                    "storage_error"
                }
            },
            Self::Io(_) => "io_error",
            Self::Json(_) => "validation_error",
            Self::Validation(_) => "validation_error",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::LimitExceeded(_) => "limit_exceeded",
            Self::Unauthorized(_) => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::ApprovalRequired { .. } => "approval_required",
            Self::ProviderUnavailable { .. } => "provider_unavailable",
            Self::RateLimited { .. } => "rate_limited",
            Self::Http(_) => "http_error",
            Self::Execution(_) => "execution_error",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Contract(_)
            | Self::Json(_)
            | Self::Validation(_)
            | Self::NotFound(_)
            | Self::Conflict(_)
            | Self::LimitExceeded(_) => 2,
            Self::Storage(err) => match err {
                StorageError::Validation(_)
                | StorageError::NotFound(_)
                | StorageError::Conflict(_)
                | StorageError::LimitExceeded(_) => 2,
                StorageError::Forbidden(_) => 4,
                StorageError::Sqlite(_)
                | StorageError::Json(_)
                | StorageError::Io(_)
                | StorageError::NewerDatabase { .. }
                | StorageError::MigrationHistoryGap { .. }
                | StorageError::ForeignKeyViolation { .. }
                | StorageError::IntegrityCheckFailed(_) => 1,
            },
            Self::Unauthorized(_) => 3,
            Self::Forbidden(_) | Self::ApprovalRequired { .. } => 4,
            Self::Io(_)
            | Self::ProviderUnavailable { .. }
            | Self::RateLimited { .. }
            | Self::Http(_)
            | Self::Execution(_) => 1,
        }
    }
}
