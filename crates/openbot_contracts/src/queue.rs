//! Queue deliveries (`QueueItem`), steer/edit holds (`QUEUE_EDIT_HOLD_MS = 600_000`),
//! attachment summaries, and queue edit protocol requests.
//! Mirrors `packages/contracts/src/{ipc-queue,ipc-attachments}.ts` and `team-protocol/queue-edit-v1.ts`.

use serde::{Deserialize, Serialize};

pub use crate::agents::{BusyMessageMode, DEFAULT_BUSY_MESSAGE_MODE};
use crate::limits::{
    ContractError, INPUT_LIMITS, double_option, is_bounded_string, is_identifier, non_null_option,
};
use crate::providers::AgentProviderRef;

pub const QUEUE_EDIT_HOLD_MS: u64 = 10 * 60_000;
pub const QUEUE_EDIT_SWEEP_MS: u64 = QUEUE_EDIT_HOLD_MS + 1_000;
pub const TEAM_QUEUE_EDIT_CAPABILITY: &str = "queue-edit-v1";
pub const CONVERSATION_SNAPSHOT_MESSAGE_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    Image,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentPreviewKind {
    Image,
    Pdf,
    Text,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentSummary {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub kind: AttachmentKind,
    pub mime_type: String,
    pub preview_kind: AttachmentPreviewKind,
    #[serde(deserialize_with = "Option::deserialize")]
    pub preview_url: Option<String>,
}

pub type DraftAttachment = AttachmentSummary;

impl AttachmentSummary {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.id)
            && is_bounded_string(&self.name, INPUT_LIMITS.attachment_name)
            && is_bounded_string(&self.mime_type, INPUT_LIMITS.mime_type)
            && self
                .preview_url
                .as_deref()
                .is_none_or(|u| is_bounded_string(u, INPUT_LIMITS.avatar_url))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueDeliveryStatus {
    Queued,
    Starting,
    Running,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

impl QueueDeliveryStatus {
    pub const ALL: [Self; 7] = [
        Self::Queued,
        Self::Starting,
        Self::Running,
        Self::Completed,
        Self::Failed,
        Self::Interrupted,
        Self::Cancelled,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueSteerFallback {
    ProviderUnsupported,
    SteerFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum QueueSender {
    User,
    Agent {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    Routine {
        #[serde(rename = "routineId")]
        routine_id: String,
        #[serde(rename = "runId")]
        run_id: String,
        #[serde(rename = "routineName")]
        routine_name: String,
        #[serde(rename = "scheduledFor")]
        scheduled_for: String,
    },
}

impl QueueSender {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::User => true,
            Self::Agent { agent_id } => is_identifier(agent_id),
            Self::Routine {
                routine_id,
                run_id,
                routine_name,
                scheduled_for,
            } => {
                is_identifier(routine_id)
                    && is_identifier(run_id)
                    && is_bounded_string(routine_name, INPUT_LIMITS.routine_name)
                    && is_bounded_string(scheduled_for, 160)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueDelivery {
    pub id: String,
    pub message_id: String,
    pub recipient_agent_id: String,
    pub sender: QueueSender,
    pub text: String,
    pub attachments: Vec<AttachmentSummary>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub reply_to_message_id: Option<String>,
    pub status: QueueDeliveryStatus,
    #[serde(deserialize_with = "Option::deserialize")]
    pub position: Option<u32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub turn_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub error: Option<String>,
    pub created_at: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub editing: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub expects_reply: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub steer_fallback: Option<QueueSteerFallback>,
}

/// Alias matching `PROJECT.md § Interface Contracts § 1` (`QueueItem`).
pub type QueueItem = QueueDelivery;

impl QueueDelivery {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.id)
            && is_identifier(&self.message_id)
            && is_identifier(&self.recipient_agent_id)
            && self.sender.is_valid()
            && is_bounded_string(&self.text, INPUT_LIMITS.message_text)
            && self.attachments.len() <= INPUT_LIMITS.attachments
            && self.attachments.iter().all(AttachmentSummary::is_valid)
            && self.reply_to_message_id.as_deref().is_none_or(is_identifier)
            && self.position.is_none_or(|p| p >= 1)
            && self.turn_id.as_deref().is_none_or(is_identifier)
            && self
                .error
                .as_deref()
                .is_none_or(|e| is_bounded_string(e, INPUT_LIMITS.message_text))
            && is_bounded_string(&self.created_at, 160)
    }

    pub fn is_queued_agent_reply(&self) -> bool {
        self.status == QueueDeliveryStatus::Queued
            && matches!(self.sender, QueueSender::Agent { .. })
            && self.reply_to_message_id.is_some()
            && self.expects_reply == Some(false)
    }
}

pub fn routine_runs_turn(deliveries: &[QueueDelivery], turn_id: Option<&str>) -> bool {
    let Some(tid) = turn_id.filter(|s| !s.is_empty()) else {
        return false;
    };
    deliveries.iter().any(|d| {
        matches!(d.sender, QueueSender::Routine { .. })
            && matches!(
                d.status,
                QueueDeliveryStatus::Starting | QueueDeliveryStatus::Running
            )
            && d.turn_id.as_deref() == Some(tid)
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum QueueHold {
    ChannelTask {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "channelName")]
        channel_name: String,
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    ProviderRestart {
        provider: AgentProviderRef,
    },
    SignIn {
        provider: AgentProviderRef,
    },
}

impl QueueHold {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::ChannelTask {
                channel_id,
                channel_name,
                agent_id,
            } => {
                is_identifier(channel_id)
                    && is_bounded_string(channel_name, INPUT_LIMITS.agent_title)
                    && is_identifier(agent_id)
            }
            Self::ProviderRestart { .. } | Self::SignIn { .. } => true,
        }
    }

    pub fn worker_agent_id(&self) -> Option<&str> {
        match self {
            Self::ChannelTask { agent_id, .. } => Some(agent_id.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSnapshot {
    pub agent_id: String,
    pub deliveries: Vec<QueueDelivery>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub hold: Option<Option<QueueHold>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub welcome: Option<bool>,
}

impl QueueSnapshot {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.agent_id)
            && self.deliveries.iter().all(QueueDelivery::is_valid)
            && self
                .hold
                .as_ref()
                .is_none_or(|h| h.as_ref().is_none_or(QueueHold::is_valid))
            && self.welcome.is_none_or(|w| w)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum QueueEditRequest {
    Begin {
        #[serde(rename = "deliveryId")]
        delivery_id: String,
        #[serde(rename = "editId")]
        edit_id: String,
    },
    Cancel {
        #[serde(rename = "deliveryId")]
        delivery_id: String,
        #[serde(rename = "editId")]
        edit_id: String,
    },
    RetainAttachments {
        #[serde(rename = "deliveryId")]
        delivery_id: String,
        #[serde(rename = "editId")]
        edit_id: String,
        #[serde(rename = "attachmentDraftIds")]
        attachment_draft_ids: Vec<String>,
    },
    Save {
        #[serde(rename = "deliveryId")]
        delivery_id: String,
        #[serde(rename = "editId")]
        edit_id: String,
        text: String,
        #[serde(rename = "keepAttachmentIds")]
        keep_attachment_ids: Vec<String>,
        #[serde(rename = "attachmentDraftIds")]
        attachment_draft_ids: Vec<String>,
    },
}

impl QueueEditRequest {
    pub fn validate(&self) -> Result<(), ContractError> {
        match self {
            Self::Begin { delivery_id, edit_id } | Self::Cancel { delivery_id, edit_id } => {
                if !is_identifier(delivery_id) || !is_identifier(edit_id) {
                    return Err(ContractError::InvalidValue("Invalid queue edit request.".to_string()));
                }
            }
            Self::RetainAttachments {
                delivery_id,
                edit_id,
                attachment_draft_ids,
            } => {
                if !is_identifier(delivery_id)
                    || !is_identifier(edit_id)
                    || attachment_draft_ids.len() > INPUT_LIMITS.attachments
                    || !attachment_draft_ids.iter().all(|id| is_identifier(id))
                {
                    return Err(ContractError::InvalidValue("Invalid queue edit request.".to_string()));
                }
            }
            Self::Save {
                delivery_id,
                edit_id,
                text,
                keep_attachment_ids,
                attachment_draft_ids,
            } => {
                if !is_identifier(delivery_id)
                    || !is_identifier(edit_id)
                    || !is_bounded_string(text, INPUT_LIMITS.message_text)
                    || !keep_attachment_ids.iter().all(|id| is_identifier(id))
                    || !attachment_draft_ids.iter().all(|id| is_identifier(id))
                    || keep_attachment_ids.len() + attachment_draft_ids.len() > INPUT_LIMITS.attachments
                {
                    return Err(ContractError::InvalidValue("Invalid queue edit request.".to_string()));
                }
            }
        }
        Ok(())
    }
}
