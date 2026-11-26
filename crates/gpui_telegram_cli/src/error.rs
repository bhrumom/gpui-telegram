use crate::envelope::{err_envelope, now_timestamp};
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("Usage error: {0}")]
    Usage(String),

    #[error("Unknown action or subcommand: {0}")]
    UnknownCommand(String),

    #[error("Input validation error: {0}")]
    Validation(String),

    #[error("Authentication required or rejected: {0}")]
    Unauthorized(String),

    #[error("Approval required for '{action}' (gate_id={gate_id}): {message}")]
    ApprovalRequired {
        action: String,
        gate_id: String,
        expires_at_ms: u64,
        message: String,
    },

    #[error("Action denied or forbidden: {0}")]
    Forbidden(String),

    #[error("HTTP/network error: {0}")]
    Http(String),

    #[error("Server returned {status}: {body}")]
    ServerError { status: u16, body: Value },

    #[error("Execution error: {0}")]
    Execution(String),

    #[error("Check error: {0}")]
    CheckError(String),

    #[error("{code}: {message}")]
    OrcaError {
        code: String,
        message: String,
        exit_code: i32,
    },
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_)
            | Self::UnknownCommand(_)
            | Self::Validation(_)
            | Self::CheckError(_) => 2,
            Self::Unauthorized(_) => 3,
            Self::ApprovalRequired { .. } | Self::Forbidden(_) => 4,
            Self::Http(_) | Self::ServerError { .. } | Self::Execution(_) => 1,
            Self::OrcaError { exit_code, .. } => *exit_code,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "usage_error",
            Self::UnknownCommand(_) => "unknown_command",
            Self::Validation(_) => "validation_error",
            Self::Unauthorized(_) => "unauthorized",
            Self::ApprovalRequired { .. } => "approval_required",
            Self::Forbidden(_) => "forbidden",
            Self::Http(_) => "http_error",
            Self::ServerError { .. } => "server_error",
            Self::Execution(_) => "execution_error",
            Self::CheckError(_) => "check_error",
            Self::OrcaError { .. } => "orca_error",
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Self::OrcaError { code, message, .. } => json!({
                "error": {
                    "code": code,
                    "message": message,
                },
            }),
            Self::ApprovalRequired {
                action,
                gate_id,
                expires_at_ms,
                message,
            } => json!({
                "success": false,
                "data": Value::Null,
                "error": "Approval required",
                "error_detail": {
                    "code": "approval_required",
                    "message": message,
                },
                "action": action,
                "gate_id": gate_id,
                "expires_at_ms": expires_at_ms,
                "message": message,
                "timestamp": now_timestamp(),
            }),
            Self::ServerError { status, body } => {
                let code = body
                    .get("error")
                    .and_then(|v| v.get("code"))
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        body.get("error_detail")
                            .and_then(|v| v.get("code"))
                            .and_then(|v| v.as_str())
                    })
                    .or_else(|| body.get("error").and_then(|v| v.as_str()))
                    .or_else(|| body.get("code").and_then(|v| v.as_str()))
                    .unwrap_or("server_error");
                let message = body
                    .get("error")
                    .and_then(|v| v.get("message"))
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        body.get("error_detail")
                            .and_then(|v| v.get("message"))
                            .and_then(|v| v.as_str())
                    })
                    .or_else(|| body.get("message").and_then(|v| v.as_str()))
                    .map(String::from)
                    .unwrap_or_else(|| format!("Server returned status {status}"));
                json!({
                    "success": false,
                    "data": Value::Null,
                    "status": status,
                    "error": {
                        "code": code,
                        "message": message,
                        "status": status,
                        "body": body,
                    },
                    "timestamp": now_timestamp(),
                })
            }
            other => err_envelope(other.code(), &other.to_string()),
        }
    }
}
