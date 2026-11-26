//! Pure-Rust domain models, contracts, parsers, and security invariants for Orca.
//!
//! `orca_core` is a strict leaf crate with zero SQLite, filesystem, network,
//! or async runtime dependencies.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod bot_ecosystem;
pub mod bot_groups;
pub mod bot_items;
pub mod bot_look_around;
pub mod bot_memory;
pub mod bot_preamble;
pub mod bot_types;
pub mod orchestration_protocol;
pub mod triggers_webhooks;
pub mod workspace_layout;

// ---------------------------------------------------------------------------
// Canonical Constants (`PROJECT.md §1` & `SCOPE.md`)
// ---------------------------------------------------------------------------

/// All 22 live Orca Bot agent wire identifiers in canonical order.
pub const BOT_AGENTS: [&str; 22] = [
    "claude",
    "codex",
    "grok",
    "opencode",
    "omp",
    "pi",
    "antigravity",
    "cursor",
    "hermes",
    "copilot",
    "opencode2",
    "kimi",
    "kiro",
    "aug",
    "goose",
    "kilo",
    "mimo-code",
    "droid",
    "qwen-code",
    "cline",
    "devin",
    "codebuddy",
];

pub const LOCAL_EXECUTION_HOST_ID: &str = "local";
pub const BOT_QUIET_MARKER: &str = "NO_REPLY";
pub const BOT_NAME_MAX_LENGTH: usize = 64;
pub const UNNAMED_BOT_NAME: &str = "New bot";

pub const BOT_MEMORY_INDEX_LINE_CAP: usize = 200;
pub const READD_GUARD_DAYS: u64 = 14;
pub const READD_GUARD_MS: u64 = READD_GUARD_DAYS * 24 * 60 * 60 * 1000;
pub const TIDY_UP_TARGET_FACTS: usize = 60;
pub const TIDY_UP_MAX_NEW_FACTS: usize = 6;
pub const TIDY_UP_MAX_FILED: usize = 20;

pub const BOT_MAX_MEMBERS: usize = 200;
pub const BOT_MAX_CONNECTOR_WORKSPACES: usize = 8;
pub const BOT_MAX_SKILL_PROJECTS: usize = 20;
pub const BOT_HANDED_PAIRED_WORKSPACES_MAX: usize = 50;

pub const WEBHOOK_HOOK_HOURLY_CAP: u32 = 200;
pub const WEBHOOK_ACCOUNT_HOURLY_CAP: u32 = 300;
pub const ORCA_YAML_MAX_BYTES: usize = 262_144;
pub const WORKTREE_INCLUDE_MAX_BYTES: u64 = 2_147_483_648;
pub const WORKTREE_INCLUDE_MAX_ENTRIES: usize = 50_000;
pub const CLOSED_TAB_TOMBSTONE_DAYS: u64 = 30;
pub const RUNTIME_PROTOCOL_VERSION: u32 = 3;
pub const MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION: u32 = 2;
pub const MIN_COMPATIBLE_RUNTIME_SERVER_VERSION: u32 = 2;
pub const E2EE_V2_KEY_SCHEDULE_BYTES: usize = 96;

// ---------------------------------------------------------------------------
// Bot Error Contract (29 Exact Variants)
// ---------------------------------------------------------------------------

/// Stable `snake_case` wire error codes for Orca Bot operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
#[error("{}", self.as_str())]
pub enum BotErrorCode {
    BotNameTaken,
    BotNotFound,
    BotFolderNotOwned,
    BotAvatarChanged,
    BotAvatarInvalid,
    InvalidArgument,
    BotsDisabled,
    BotChatNotFound,
    BotPaused,
    BotSelf,
    BotRequestType,
    BotRequestWorker,
    NotFound,
    ItemClosed,
    NotOwner,
    ItemsReadOnly,
    ItemsUnavailable,
    ItemChanged,
    QuestionClosed,
    BotPackageNewerSchema,
    BotPackageDuplicatePart,
    BotPackagePathNotAllowed,
    SecretBlocked,
    LineageParentChanged,
    BotOfferAlreadyAnswered,
    Expired,
    NotThisHost,
    Full,
    GrantedNotAdded,
}

impl BotErrorCode {
    pub const ALL: [Self; 29] = [
        Self::BotNameTaken,
        Self::BotNotFound,
        Self::BotFolderNotOwned,
        Self::BotAvatarChanged,
        Self::BotAvatarInvalid,
        Self::InvalidArgument,
        Self::BotsDisabled,
        Self::BotChatNotFound,
        Self::BotPaused,
        Self::BotSelf,
        Self::BotRequestType,
        Self::BotRequestWorker,
        Self::NotFound,
        Self::ItemClosed,
        Self::NotOwner,
        Self::ItemsReadOnly,
        Self::ItemsUnavailable,
        Self::ItemChanged,
        Self::QuestionClosed,
        Self::BotPackageNewerSchema,
        Self::BotPackageDuplicatePart,
        Self::BotPackagePathNotAllowed,
        Self::SecretBlocked,
        Self::LineageParentChanged,
        Self::BotOfferAlreadyAnswered,
        Self::Expired,
        Self::NotThisHost,
        Self::Full,
        Self::GrantedNotAdded,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BotNameTaken => "bot_name_taken",
            Self::BotNotFound => "bot_not_found",
            Self::BotFolderNotOwned => "bot_folder_not_owned",
            Self::BotAvatarChanged => "bot_avatar_changed",
            Self::BotAvatarInvalid => "bot_avatar_invalid",
            Self::InvalidArgument => "invalid_argument",
            Self::BotsDisabled => "bots_disabled",
            Self::BotChatNotFound => "bot_chat_not_found",
            Self::BotPaused => "bot_paused",
            Self::BotSelf => "bot_self",
            Self::BotRequestType => "bot_request_type",
            Self::BotRequestWorker => "bot_request_worker",
            Self::NotFound => "not_found",
            Self::ItemClosed => "item_closed",
            Self::NotOwner => "not_owner",
            Self::ItemsReadOnly => "items_read_only",
            Self::ItemsUnavailable => "items_unavailable",
            Self::ItemChanged => "item_changed",
            Self::QuestionClosed => "question_closed",
            Self::BotPackageNewerSchema => "bot_package_newer_schema",
            Self::BotPackageDuplicatePart => "bot_package_duplicate_part",
            Self::BotPackagePathNotAllowed => "bot_package_path_not_allowed",
            Self::SecretBlocked => "secret_blocked",
            Self::LineageParentChanged => "lineage_parent_changed",
            Self::BotOfferAlreadyAnswered => "bot_offer_already_answered",
            Self::Expired => "expired",
            Self::NotThisHost => "not_this_host",
            Self::Full => "full",
            Self::GrantedNotAdded => "granted_not_added",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.as_str() == raw)
    }
}

/// Structured error envelope for Orca Bot APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("{code}: {message}")]
pub struct BotError {
    pub code: BotErrorCode,
    pub message: String,
}

impl BotError {
    pub fn new(code: BotErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<BotErrorCode> for BotError {
    fn from(code: BotErrorCode) -> Self {
        Self {
            code,
            message: code.as_str().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// IDE / Orchestration / Security Error Contract (17 Exact Variants)
// ---------------------------------------------------------------------------

/// Stable `snake_case` wire error codes for Orca IDE, Orchestration, and Security operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
#[error("{}", self.as_str())]
pub enum OrcaIdeErrorCode {
    UnknownTask,
    UnknownDispatch,
    StaleDispatch,
    MailboxIsLocal,
    ServerMailUnsupported,
    TabNotFound,
    SystemShortcutBlocked,
    ApprovalRequired,
    PluginSystemDisabled,
    EmulatorUnsupported,
    EmulatorInputBlocked,
    WaitTimeout,
    YamlDuplicateKey,
    YamlSizeExceeded,
    YamlAliasLimit,
    CopyBudgetExceeded,
    HibernationBlocked,
}

impl OrcaIdeErrorCode {
    pub const ALL: [Self; 17] = [
        Self::UnknownTask,
        Self::UnknownDispatch,
        Self::StaleDispatch,
        Self::MailboxIsLocal,
        Self::ServerMailUnsupported,
        Self::TabNotFound,
        Self::SystemShortcutBlocked,
        Self::ApprovalRequired,
        Self::PluginSystemDisabled,
        Self::EmulatorUnsupported,
        Self::EmulatorInputBlocked,
        Self::WaitTimeout,
        Self::YamlDuplicateKey,
        Self::YamlSizeExceeded,
        Self::YamlAliasLimit,
        Self::CopyBudgetExceeded,
        Self::HibernationBlocked,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown_task",
            Self::UnknownDispatch => "unknown_dispatch",
            Self::StaleDispatch => "stale_dispatch",
            Self::MailboxIsLocal => "mailbox_is_local",
            Self::ServerMailUnsupported => "server_mail_unsupported",
            Self::TabNotFound => "tab_not_found",
            Self::SystemShortcutBlocked => "system_shortcut_blocked",
            Self::ApprovalRequired => "approval_required",
            Self::PluginSystemDisabled => "plugin_system_disabled",
            Self::EmulatorUnsupported => "emulator_unsupported",
            Self::EmulatorInputBlocked => "emulator_input_blocked",
            Self::WaitTimeout => "wait_timeout",
            Self::YamlDuplicateKey => "yaml_duplicate_key",
            Self::YamlSizeExceeded => "yaml_size_exceeded",
            Self::YamlAliasLimit => "yaml_alias_limit",
            Self::CopyBudgetExceeded => "copy_budget_exceeded",
            Self::HibernationBlocked => "hibernation_blocked",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.as_str() == raw)
    }
}

/// Structured error envelope for Orca IDE / Orchestration APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("{code}: {message}")]
pub struct OrcaIdeError {
    pub code: OrcaIdeErrorCode,
    pub message: String,
}

impl OrcaIdeError {
    pub fn new(code: OrcaIdeErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<OrcaIdeErrorCode> for OrcaIdeError {
    fn from(code: OrcaIdeErrorCode) -> Self {
        Self {
            code,
            message: code.as_str().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_error_codes_exact_wire_contract() {
        let expected = [
            "bot_name_taken",
            "bot_not_found",
            "bot_folder_not_owned",
            "bot_avatar_changed",
            "bot_avatar_invalid",
            "invalid_argument",
            "bots_disabled",
            "bot_chat_not_found",
            "bot_paused",
            "bot_self",
            "bot_request_type",
            "bot_request_worker",
            "not_found",
            "item_closed",
            "not_owner",
            "items_read_only",
            "items_unavailable",
            "item_changed",
            "question_closed",
            "bot_package_newer_schema",
            "bot_package_duplicate_part",
            "bot_package_path_not_allowed",
            "secret_blocked",
            "lineage_parent_changed",
            "bot_offer_already_answered",
            "expired",
            "not_this_host",
            "full",
            "granted_not_added",
        ];
        assert_eq!(BotErrorCode::ALL.len(), 29);
        for (code, wire) in BotErrorCode::ALL.into_iter().zip(expected) {
            assert_eq!(code.as_str(), wire);
            assert_eq!(BotErrorCode::parse(wire), Some(code));
            let json = serde_json::to_string(&code).expect("serialize BotErrorCode");
            assert_eq!(json, format!("\"{wire}\""));
            let back: BotErrorCode = serde_json::from_str(&json).expect("deserialize BotErrorCode");
            assert_eq!(back, code);
        }
        assert_eq!(BotErrorCode::parse("unknown_code"), None);
        let err = BotError::new(BotErrorCode::BotNameTaken, "Scout already exists");
        assert_eq!(err.to_string(), "bot_name_taken: Scout already exists");
        assert_eq!(BotError::from(BotErrorCode::Full).message, "full");
    }

    #[test]
    fn orca_ide_error_codes_exact_wire_contract() {
        let expected = [
            "unknown_task",
            "unknown_dispatch",
            "stale_dispatch",
            "mailbox_is_local",
            "server_mail_unsupported",
            "tab_not_found",
            "system_shortcut_blocked",
            "approval_required",
            "plugin_system_disabled",
            "emulator_unsupported",
            "emulator_input_blocked",
            "wait_timeout",
            "yaml_duplicate_key",
            "yaml_size_exceeded",
            "yaml_alias_limit",
            "copy_budget_exceeded",
            "hibernation_blocked",
        ];
        assert_eq!(OrcaIdeErrorCode::ALL.len(), 17);
        for (code, wire) in OrcaIdeErrorCode::ALL.into_iter().zip(expected) {
            assert_eq!(code.as_str(), wire);
            assert_eq!(OrcaIdeErrorCode::parse(wire), Some(code));
            let json = serde_json::to_string(&code).expect("serialize OrcaIdeErrorCode");
            assert_eq!(json, format!("\"{wire}\""));
            let back: OrcaIdeErrorCode =
                serde_json::from_str(&json).expect("deserialize OrcaIdeErrorCode");
            assert_eq!(back, code);
        }
        assert_eq!(OrcaIdeErrorCode::parse("nope"), None);
        let err = OrcaIdeError::new(OrcaIdeErrorCode::YamlDuplicateKey, "dup key");
        assert_eq!(err.to_string(), "yaml_duplicate_key: dup key");
    }

    #[test]
    fn canonical_constants_match_spec() {
        assert_eq!(BOT_AGENTS.len(), 22);
        assert_eq!(BOT_MEMORY_INDEX_LINE_CAP, 200);
        assert_eq!(READD_GUARD_DAYS, 14);
        assert_eq!(READD_GUARD_MS, 14 * 86_400_000);
        assert_eq!(TIDY_UP_TARGET_FACTS, 60);
        assert_eq!(TIDY_UP_MAX_NEW_FACTS, 6);
        assert_eq!(TIDY_UP_MAX_FILED, 20);
        assert_eq!(BOT_MAX_MEMBERS, 200);
        assert_eq!(BOT_MAX_CONNECTOR_WORKSPACES, 8);
        assert_eq!(WEBHOOK_HOOK_HOURLY_CAP, 200);
        assert_eq!(WEBHOOK_ACCOUNT_HOURLY_CAP, 300);
        assert_eq!(ORCA_YAML_MAX_BYTES, 262_144);
        assert_eq!(WORKTREE_INCLUDE_MAX_BYTES, 2_147_483_648);
        assert_eq!(WORKTREE_INCLUDE_MAX_ENTRIES, 50_000);
        assert_eq!(CLOSED_TAB_TOMBSTONE_DAYS, 30);
        assert_eq!(RUNTIME_PROTOCOL_VERSION, 3);
        assert_eq!(MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION, 2);
        assert_eq!(MIN_COMPATIBLE_RUNTIME_SERVER_VERSION, 2);
        assert_eq!(E2EE_V2_KEY_SCHEDULE_BYTES, 96);
    }
}
