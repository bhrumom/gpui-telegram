//! Core input limits, bounded string/integer validators, RequestId, and tri-state serde helpers.
//! Mirrors `packages/contracts/src/input-limits.ts` and `ipc-bounded-values.ts`.

use chrono::{DateTime, NaiveDate};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ContractError {
    #[error("invalid contract value: {0}")]
    InvalidValue(String),
    #[error("json serialization error: {0}")]
    Json(String),
    #[error("webhook verification failed: {0}")]
    Webhook(String),
    #[error("team protocol error: {0}")]
    TeamProtocol(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputLimits {
    pub identifier: usize,
    pub vault_login_id: usize,
    pub email: usize,
    pub account_name: usize,
    pub profile_name_min: usize,
    pub profile_name: usize,
    pub server_name_min: usize,
    pub server_name: usize,
    pub invite_url: usize,
    pub hostname: usize,
    pub browser_url: usize,
    pub avatar_url: usize,
    pub browser_action_text: usize,
    pub agent_name: usize,
    pub model_name: usize,
    pub service_tier: usize,
    pub agent_title: usize,
    pub agent_description: usize,
    pub stored_channel_instructions: usize,
    pub agent_instructions_file: usize,
    pub agent_memories: usize,
    pub agent_skills: usize,
    pub shared_tables: usize,
    pub shared_table_name: usize,
    pub channel_memories: usize,
    pub messaging_memories: usize,
    pub agent_memory_text: usize,
    pub agent_routines: usize,
    pub routine_name: usize,
    pub routine_instruction: usize,
    pub routine_runs_page: usize,
    pub routine_flow_links: usize,
    pub workflow_nodes: usize,
    pub workflow_edges: usize,
    pub workflow_node_fields: usize,
    pub workflow_node_text: usize,
    pub workflow_strokes: usize,
    pub workflow_stroke_points: usize,
    pub workflow_title: usize,
    pub workflow_graph_bytes: usize,
    pub workflow_runs_page: usize,
    pub workflow_step_input: usize,
    pub workflow_webhook_payload: usize,
    pub routine_failure_reason: usize,
    pub agent_waits: usize,
    pub agent_wait_note: usize,
    pub agent_tasks: usize,
    pub agent_task_title: usize,
    pub agent_task_progress: usize,
    pub agent_task_error: usize,
    pub automation_payload: usize,
    pub routine_calendar_days: usize,
    pub routine_cron: usize,
    pub message_text: usize,
    pub pasted_text: usize,
    pub direct_message_text: usize,
    pub side_question_turns: usize,
    pub side_question_answer: usize,
    pub side_question_earlier_text: usize,
    pub prompt_questions: usize,
    pub prompt_options: usize,
    pub prompt_header: usize,
    pub prompt_question: usize,
    pub prompt_option_label: usize,
    pub prompt_option_description: usize,
    pub prompt_answers_per_question: usize,
    pub prompt_answer_text: usize,
    pub prompt_answers_total_text: usize,
    pub attachments: usize,
    pub draft_attachments: usize,
    pub message_recipients: usize,
    pub queue_reorder: usize,
    pub attachment_name: usize,
    pub mime_type: usize,
    pub path: usize,
    pub agents: usize,
    pub sidebar_sections: usize,
    pub sidebar_section_name: usize,
    pub browser_tabs: usize,
    pub browser_coordinate: usize,
    pub browser_dimension: usize,
    pub mcp_servers: usize,
    pub mcp_server_name: usize,
    pub mcp_command: usize,
    pub mcp_args: usize,
    pub mcp_arg_value: usize,
    pub mcp_env_variables: usize,
    pub mcp_env_name: usize,
    pub mcp_env_value: usize,
    pub mcp_headers: usize,
    pub mcp_header_value: usize,
    pub mcp_url: usize,
    pub mcp_tool_count: usize,
    pub mcp_error_text: usize,
    pub mcp_client_id: usize,
    pub mcp_client_secret: usize,
    pub mcp_scope: usize,
    pub sign_in_relay_address: usize,
    pub team_members: usize,
    pub active_invites: usize,
    pub max_permanent_invites: usize,
    pub sessions_per_member: usize,
}

pub const INPUT_LIMITS: InputLimits = InputLimits {
    identifier: 128,
    vault_login_id: 520,
    email: 254,
    account_name: 120,
    profile_name_min: 3,
    profile_name: 20,
    server_name_min: 6,
    server_name: 32,
    invite_url: 4_096,
    hostname: 253,
    browser_url: 8_192,
    avatar_url: 2_048,
    browser_action_text: 50_000,
    agent_name: 80,
    model_name: 160,
    service_tier: 64,
    agent_title: 120,
    agent_description: 2_000,
    stored_channel_instructions: 32_000,
    agent_instructions_file: 32_000,
    agent_memories: 64,
    agent_skills: 32,
    shared_tables: 64,
    shared_table_name: 64,
    channel_memories: 32,
    messaging_memories: 32,
    agent_memory_text: 500,
    agent_routines: 64,
    routine_name: 80,
    routine_instruction: 100_000,
    routine_runs_page: 100,
    routine_flow_links: 50,
    workflow_nodes: 200,
    workflow_edges: 400,
    workflow_node_fields: 32,
    workflow_node_text: 20_000,
    workflow_strokes: 500,
    workflow_stroke_points: 2_000,
    workflow_title: 160,
    workflow_graph_bytes: 1_024 * 1_024,
    workflow_runs_page: 50,
    workflow_step_input: 32_000,
    workflow_webhook_payload: 64_000,
    routine_failure_reason: 500,
    agent_waits: 20,
    agent_wait_note: 500,
    agent_tasks: 50,
    agent_task_title: 120,
    agent_task_progress: 500,
    agent_task_error: 2_000,
    automation_payload: 4_000,
    routine_calendar_days: 44,
    routine_cron: 255,
    message_text: 100_000,
    pasted_text: 4_000,
    direct_message_text: 20_000,
    side_question_turns: 20,
    side_question_answer: 32_000,
    side_question_earlier_text: 4_000,
    prompt_questions: 32,
    prompt_options: 5,
    prompt_header: 120,
    prompt_question: 2_000,
    prompt_option_label: 120,
    prompt_option_description: 2_000,
    prompt_answers_per_question: 32,
    prompt_answer_text: 100_000,
    prompt_answers_total_text: 100_000,
    attachments: 10,
    draft_attachments: 50,
    message_recipients: 32,
    queue_reorder: 100,
    attachment_name: 255,
    mime_type: 255,
    path: 4_096,
    agents: 100,
    sidebar_sections: 100,
    sidebar_section_name: 40,
    browser_tabs: 25,
    browser_coordinate: 100_000,
    browser_dimension: 16_384,
    mcp_servers: 100,
    mcp_server_name: 80,
    mcp_command: 4_096,
    mcp_args: 64,
    mcp_arg_value: 4_096,
    mcp_env_variables: 64,
    mcp_env_name: 255,
    mcp_env_value: 8_192,
    mcp_headers: 32,
    mcp_header_value: 8_192,
    mcp_url: 2_048,
    mcp_tool_count: 10_000,
    mcp_error_text: 2_000,
    mcp_client_id: 1_024,
    mcp_client_secret: 4_096,
    mcp_scope: 2_048,
    sign_in_relay_address: 8_192,
    team_members: 100,
    active_invites: 100,
    max_permanent_invites: 5,
    sessions_per_member: 10,
};

pub const DEFAULT_TEAM_MEMBER_LIMIT: usize = 3;
pub const AGENT_INSTRUCTIONS_READ_LIMIT: usize = 256_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachmentLimits {
    pub file_bytes: usize,
    pub total_bytes: usize,
}

pub const ATTACHMENT_LIMITS: AttachmentLimits = AttachmentLimits {
    file_bytes: 100 * 1024 * 1024,
    total_bytes: 250 * 1024 * 1024,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvatarImageLimits {
    pub source_bytes: usize,
    pub stored_bytes: usize,
    pub output_pixels: u32,
}

pub const AVATAR_IMAGE_LIMITS: AvatarImageLimits = AvatarImageLimits {
    source_bytes: 10 * 1024 * 1024,
    stored_bytes: 512 * 1024,
    output_pixels: 512,
};

/// Returns exact JavaScript `string.length` in UTF-16 code units.
#[inline]
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[inline]
pub fn is_bounded_string(s: &str, maximum: usize) -> bool {
    utf16_len(s) <= maximum
}

#[inline]
pub fn is_nullable_bounded_string(s: Option<&str>, maximum: usize) -> bool {
    s.is_none_or(|v| is_bounded_string(v, maximum))
}

#[inline]
pub fn is_identifier(s: &str) -> bool {
    !s.is_empty() && is_bounded_string(s, INPUT_LIMITS.identifier)
}

#[inline]
pub fn integer_in_range(value: i64, minimum: i64, maximum: i64) -> bool {
    value >= minimum && value <= maximum
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(i64),
}

impl RequestId {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::String(s) => is_identifier(s),
            Self::Number(_) => true,
        }
    }

    pub fn display_key(&self) -> String {
        match self {
            Self::String(s) => s.clone(),
            Self::Number(n) => n.to_string(),
        }
    }
}

pub fn is_uuid_v4(value: &str) -> bool {
    utf16_len(value) <= 36
        && Uuid::parse_str(value)
            .map(|u| {
                u.get_version_num() == 4
                    && u.get_variant() == uuid::Variant::RFC4122
                    && u.to_string().eq_ignore_ascii_case(value)
            })
            .unwrap_or(false)
}

pub fn is_calendar_date(value: &str) -> bool {
    if value.len() != 10 {
        return false;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return false;
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|d| d.format("%Y-%m-%d").to_string() == value)
        .unwrap_or(false)
}

pub fn is_iso_timestamp(value: &str) -> bool {
    if value.trim().is_empty() {
        return false;
    }
    DateTime::parse_from_rfc3339(value).is_ok()
        || DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f%z").is_ok()
        || is_calendar_date(value)
}

/// Clips by UTF-16 code units matching JS `text.length <= max ? text : text.slice(0, max - 1) + '…'`.
pub fn clip_utf16(text: &str, max: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max {
        return text.to_string();
    }
    if max == 0 {
        return "…".to_string();
    }
    let prefix = String::from_utf16_lossy(&units[..max.saturating_sub(1)]);
    format!("{prefix}…")
}

/// Clips by Unicode scalar values matching JS `Array.from(text)` truncation.
pub fn clip_chars(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    if max == 0 {
        return "…".to_string();
    }
    let prefix: String = chars[..max.saturating_sub(1)].iter().collect();
    format!("{}…", prefix.trim_end())
}

/// Serde helper for TypeScript tri-state fields `field?: T | null`:
/// - Omitted (`undefined`) <-> `None` (pair with `#[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]`)
/// - Explicit `null` <-> `Some(None)`
/// - Value `v` <-> `Some(Some(v))`
pub mod double_option {
    use super::*;

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Some)
    }

    pub fn serialize<T, S>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        match value {
            None => serializer.serialize_none(),
            Some(None) => serializer.serialize_none(),
            Some(Some(v)) => v.serialize(serializer),
        }
    }
}

/// Serde helper for TypeScript optional non-nullable fields `field?: T`:
/// - Omitted (`undefined`) <-> `None` (pair with `#[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]`)
/// - Explicit `null` -> rejected (`Err`)
/// - Value `v` <-> `Some(v)`
pub fn non_null_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Serde helper that rejects a specific forbidden key if present in a map (even when `null`),
/// while keeping the surrounding struct permissive of benign unknown keys.
pub fn forbid_field<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Err(serde::de::Error::custom("forbidden field present"))
}

