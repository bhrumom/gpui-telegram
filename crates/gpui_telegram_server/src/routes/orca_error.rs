//! Canonical Orca REST error envelope helpers and exhaustive status/code mappers
//! for `orca_bots::OrcaBotsError` and `orca_ide::OrcaIdeRuntimeError`.

use axum::{
    body::Bytes,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use gpui_telegram_proto::orca_contracts::OrcaErrorEnvelope;
use orca_bots::OrcaBotsError;
use orca_core::bot_memory::TidyUpErrorCode;
use orca_core::orchestration_protocol::{DiffAnnotationError, E2eeV2Error, VaultError};
use orca_core::{BotErrorCode, OrcaIdeErrorCode};
use orca_ide::OrcaIdeRuntimeError;
use serde::de::DeserializeOwned;

/// Emits the canonical Orca REST error JSON:
/// `{"error": {"code": "<snake_case>", "message": "<msg>"}}`.
pub fn orca_err(status: StatusCode, code: &str, msg: impl Into<String>) -> Response {
    (status, Json(OrcaErrorEnvelope::new(code, msg))).into_response()
}

/// Parses a required JSON request body into `T`, returning canonical `400 invalid_argument`
/// instead of Axum's plain-text extractor rejection.
pub fn parse_json_body<T: DeserializeOwned>(bytes: &Bytes) -> Result<T, Response> {
    if bytes.is_empty() {
        return Err(orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Request body cannot be empty",
        ));
    }
    serde_json::from_slice(bytes).map_err(|err| {
        orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("Malformed JSON payload: {err}"),
        )
    })
}

/// Parses an optional JSON request body into `T` (returning `T::default()` on empty body).
pub fn parse_optional_json_body<T: DeserializeOwned + Default>(
    bytes: &Bytes,
) -> Result<T, Response> {
    if bytes.is_empty() || bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    parse_json_body(bytes)
}

/// Maps every one of the 29 `BotErrorCode` variants to its canonical HTTP status code.
pub const fn bot_error_code_status(code: BotErrorCode) -> StatusCode {
    match code {
        BotErrorCode::BotNameTaken => StatusCode::CONFLICT,
        BotErrorCode::BotNotFound => StatusCode::NOT_FOUND,
        BotErrorCode::BotFolderNotOwned => StatusCode::FORBIDDEN,
        BotErrorCode::BotAvatarChanged => StatusCode::CONFLICT,
        BotErrorCode::BotAvatarInvalid => StatusCode::BAD_REQUEST,
        BotErrorCode::InvalidArgument => StatusCode::BAD_REQUEST,
        BotErrorCode::BotsDisabled => StatusCode::FORBIDDEN,
        BotErrorCode::BotChatNotFound => StatusCode::NOT_FOUND,
        BotErrorCode::BotPaused => StatusCode::CONFLICT,
        BotErrorCode::BotSelf => StatusCode::BAD_REQUEST,
        BotErrorCode::BotRequestType => StatusCode::BAD_REQUEST,
        BotErrorCode::BotRequestWorker => StatusCode::FORBIDDEN,
        BotErrorCode::NotFound => StatusCode::NOT_FOUND,
        BotErrorCode::ItemClosed => StatusCode::CONFLICT,
        BotErrorCode::NotOwner => StatusCode::FORBIDDEN,
        BotErrorCode::ItemsReadOnly => StatusCode::CONFLICT,
        BotErrorCode::ItemsUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        BotErrorCode::ItemChanged => StatusCode::CONFLICT,
        BotErrorCode::QuestionClosed => StatusCode::CONFLICT,
        BotErrorCode::BotPackageNewerSchema => StatusCode::UNPROCESSABLE_ENTITY,
        BotErrorCode::BotPackageDuplicatePart => StatusCode::BAD_REQUEST,
        BotErrorCode::BotPackagePathNotAllowed => StatusCode::BAD_REQUEST,
        BotErrorCode::SecretBlocked => StatusCode::UNPROCESSABLE_ENTITY,
        BotErrorCode::LineageParentChanged => StatusCode::CONFLICT,
        BotErrorCode::BotOfferAlreadyAnswered => StatusCode::CONFLICT,
        BotErrorCode::Expired => StatusCode::GONE,
        BotErrorCode::NotThisHost => StatusCode::FORBIDDEN,
        BotErrorCode::Full => StatusCode::CONFLICT,
        BotErrorCode::GrantedNotAdded => StatusCode::CONFLICT,
    }
}

/// Maps every one of the 17 `OrcaIdeErrorCode` variants to its canonical HTTP status code.
pub const fn ide_error_code_status(code: OrcaIdeErrorCode) -> StatusCode {
    match code {
        OrcaIdeErrorCode::UnknownTask => StatusCode::NOT_FOUND,
        OrcaIdeErrorCode::UnknownDispatch => StatusCode::NOT_FOUND,
        OrcaIdeErrorCode::StaleDispatch => StatusCode::CONFLICT,
        OrcaIdeErrorCode::MailboxIsLocal => StatusCode::BAD_REQUEST,
        OrcaIdeErrorCode::ServerMailUnsupported => StatusCode::BAD_REQUEST,
        OrcaIdeErrorCode::TabNotFound => StatusCode::NOT_FOUND,
        OrcaIdeErrorCode::SystemShortcutBlocked => StatusCode::FORBIDDEN,
        OrcaIdeErrorCode::ApprovalRequired => StatusCode::FORBIDDEN,
        OrcaIdeErrorCode::PluginSystemDisabled => StatusCode::FORBIDDEN,
        OrcaIdeErrorCode::EmulatorUnsupported => StatusCode::BAD_REQUEST,
        OrcaIdeErrorCode::EmulatorInputBlocked => StatusCode::FORBIDDEN,
        OrcaIdeErrorCode::WaitTimeout => StatusCode::REQUEST_TIMEOUT,
        OrcaIdeErrorCode::YamlDuplicateKey => StatusCode::BAD_REQUEST,
        OrcaIdeErrorCode::YamlSizeExceeded => StatusCode::PAYLOAD_TOO_LARGE,
        OrcaIdeErrorCode::YamlAliasLimit => StatusCode::BAD_REQUEST,
        OrcaIdeErrorCode::CopyBudgetExceeded => StatusCode::PAYLOAD_TOO_LARGE,
        OrcaIdeErrorCode::HibernationBlocked => StatusCode::CONFLICT,
    }
}

/// Exhaustively maps any `orca_bots::OrcaBotsError` to its canonical REST status and error envelope.
pub fn map_bots_err(err: OrcaBotsError) -> Response {
    match err {
        OrcaBotsError::Bot(e) => orca_err(bot_error_code_status(e.code), e.code.as_str(), e.message),
        OrcaBotsError::Memory(e) => {
            orca_err(bot_error_code_status(e.code), e.code.as_str(), e.message)
        }
        OrcaBotsError::TidyUp(e) => {
            let status = match e.code {
                TidyUpErrorCode::Changed => StatusCode::CONFLICT,
                TidyUpErrorCode::TimedOut => StatusCode::REQUEST_TIMEOUT,
                TidyUpErrorCode::ProtectedLineModified
                | TidyUpErrorCode::ProtectedLineRemoved
                | TidyUpErrorCode::TooManyRemoved
                | TidyUpErrorCode::TooManyAdded
                | TidyUpErrorCode::TooManyFiled => StatusCode::UNPROCESSABLE_ENTITY,
                _ => StatusCode::BAD_REQUEST,
            };
            orca_err(status, e.code.as_str(), e.message)
        }
        OrcaBotsError::OAuth { code, message } => match code {
            "attempt_expired" => orca_err(StatusCode::GONE, "expired", message),
            "credential_not_found" => orca_err(StatusCode::NOT_FOUND, "not_found", message),
            other => orca_err(StatusCode::BAD_REQUEST, other, message),
        },
        OrcaBotsError::Relay {
            status,
            code,
            message,
        } => {
            let http_status =
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            orca_err(http_status, code, message)
        }
        OrcaBotsError::Mcp { code, message } => orca_err(StatusCode::BAD_REQUEST, code, message),
        OrcaBotsError::PreconditionFailed { path } => orca_err(
            StatusCode::CONFLICT,
            "conflict",
            format!("CAS precondition mismatch on {path}"),
        ),
        OrcaBotsError::Sqlite(e) => orca_err(
            StatusCode::SERVICE_UNAVAILABLE,
            "items_unavailable",
            format!("sqlite error: {e}"),
        ),
        OrcaBotsError::Json(e) => orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("json error: {e}"),
        ),
        OrcaBotsError::Url(e) => orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("url parse error: {e}"),
        ),
        OrcaBotsError::Io(e) => orca_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "io_error",
            format!("io error: {e}"),
        ),
        OrcaBotsError::Http(e) => orca_err(
            StatusCode::BAD_GATEWAY,
            "io_error",
            format!("http error: {e}"),
        ),
    }
}

/// Exhaustively maps any `orca_ide::OrcaIdeRuntimeError` to its canonical REST status and envelope.
pub fn map_ide_err(err: OrcaIdeRuntimeError) -> Response {
    match err {
        OrcaIdeRuntimeError::Ide(e) => {
            orca_err(ide_error_code_status(e.code), e.code.as_str(), e.message)
        }
        OrcaIdeRuntimeError::Bot(e) => {
            orca_err(bot_error_code_status(e.code), e.code.as_str(), e.message)
        }
        OrcaIdeRuntimeError::OrcaBots(e) => map_bots_err(e),
        OrcaIdeRuntimeError::Yaml(e) => match e.ide_error_code() {
            Some(code) => orca_err(ide_error_code_status(code), code.as_str(), e.to_string()),
            None => orca_err(StatusCode::BAD_REQUEST, "invalid_argument", e.to_string()),
        },
        OrcaIdeRuntimeError::Vault(e) => match e {
            VaultError::VersionConflict { .. } | VaultError::StaleGeneration { .. } => {
                orca_err(StatusCode::CONFLICT, "conflict", e.to_string())
            }
            other => orca_err(StatusCode::BAD_REQUEST, "invalid_argument", other.to_string()),
        },
        OrcaIdeRuntimeError::DiffAnnotation(e) => match e {
            DiffAnnotationError::NotFound(_) => {
                orca_err(StatusCode::NOT_FOUND, "not_found", e.to_string())
            }
            DiffAnnotationError::DuplicateId(_) => {
                orca_err(StatusCode::CONFLICT, "conflict", e.to_string())
            }
            other => orca_err(StatusCode::BAD_REQUEST, "invalid_argument", other.to_string()),
        },
        OrcaIdeRuntimeError::PluginValidation(e) => {
            orca_err(StatusCode::BAD_REQUEST, "invalid_argument", e.to_string())
        }
        OrcaIdeRuntimeError::Conflict(m) => orca_err(StatusCode::CONFLICT, "conflict", m),
        OrcaIdeRuntimeError::ProfileStateRevisionConflict { .. } => {
            orca_err(StatusCode::CONFLICT, "conflict", err.to_string())
        }
        OrcaIdeRuntimeError::InvalidArgument(m) => {
            orca_err(StatusCode::BAD_REQUEST, "invalid_argument", m)
        }
        OrcaIdeRuntimeError::Json(e) => {
            orca_err(StatusCode::BAD_REQUEST, "invalid_argument", e.to_string())
        }
        OrcaIdeRuntimeError::Regex(e) => {
            orca_err(StatusCode::BAD_REQUEST, "invalid_argument", e.to_string())
        }
        OrcaIdeRuntimeError::NotFound(m) => orca_err(StatusCode::NOT_FOUND, "not_found", m),
        OrcaIdeRuntimeError::PermissionDenied(m) => {
            orca_err(StatusCode::FORBIDDEN, "permission_denied", m)
        }
        OrcaIdeRuntimeError::ActiveCliOwner(m) => {
            orca_err(StatusCode::CONFLICT, "active_cli_owner", m)
        }
        OrcaIdeRuntimeError::ProtocolVersion(e) => {
            orca_err(StatusCode::BAD_REQUEST, "protocol_version", e.to_string())
        }
        OrcaIdeRuntimeError::HookServer(m) => {
            orca_err(StatusCode::INTERNAL_SERVER_ERROR, "hook_server_error", m)
        }
        OrcaIdeRuntimeError::E2ee(e) => match e {
            E2eeV2Error::ReplayDetected { .. } => {
                orca_err(StatusCode::CONFLICT, "replay_detected", e.to_string())
            }
            E2eeV2Error::InvalidSignature => {
                orca_err(StatusCode::UNAUTHORIZED, "invalid_signature", e.to_string())
            }
            other => orca_err(StatusCode::BAD_REQUEST, "e2ee_error", other.to_string()),
        },
        OrcaIdeRuntimeError::Sqlite(e) => {
            orca_err(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", e.to_string())
        }
        OrcaIdeRuntimeError::Io(e) => {
            orca_err(StatusCode::INTERNAL_SERVER_ERROR, "io_error", e.to_string())
        }
        OrcaIdeRuntimeError::Http(e) => {
            orca_err(StatusCode::BAD_GATEWAY, "io_error", e.to_string())
        }
    }
}
