//! Error types and result alias for `openbot_storage` (`OB-F02`).

use openbot_contracts::ContractError;

pub type StorageResult<T> = Result<T, StorageError>;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database schema version {found} is newer than supported version {supported}")]
    NewerDatabase { found: i64, supported: i64 },
    #[error("OpenBot database migration history is missing version {missing_version}.")]
    MigrationHistoryGap { missing_version: i64 },
    #[error("foreign key violation in migration {migration}: {details}")]
    ForeignKeyViolation { migration: i64, details: String },
    #[error("database quick_check failed: {0}")]
    IntegrityCheckFailed(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("limit exceeded: {0}")]
    LimitExceeded(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("not found: {0}")]
    NotFound(String),
}

impl StorageError {
    #[inline]
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    #[inline]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    #[inline]
    pub fn limit(message: impl Into<String>) -> Self {
        Self::LimitExceeded(message.into())
    }

    #[inline]
    pub fn limit_exceeded(message: impl Into<String>) -> Self {
        Self::LimitExceeded(message.into())
    }

    #[inline]
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden(message.into())
    }

    #[inline]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    #[inline]
    pub fn integrity(message: impl Into<String>) -> Self {
        Self::IntegrityCheckFailed(message.into())
    }

    #[inline]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }

    #[inline]
    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::Conflict(_))
    }

    #[inline]
    pub fn is_newer_database(&self) -> bool {
        matches!(self, Self::NewerDatabase { .. })
    }
}

impl From<ContractError> for StorageError {
    fn from(err: ContractError) -> Self {
        Self::Validation(err.to_string())
    }
}
