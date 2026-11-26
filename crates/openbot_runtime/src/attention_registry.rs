//! Interactive attention registry for approvals, `ask_user`, `ask_ui`, and browser takeovers (`OB-F07`).
//!
//! Mirrors `src/backend/agent/attention-registry.ts`: enforces the `10m` (`600_000 ms`)
//! interactive TTL across all pending gates, validates all four `UiBlockSpec` kinds
//! (`Confirm`, `QuickReplies`, `Choice`, `Form`), strictly redacts `UiFormField` /
//! `AgentPromptQuestion` secrets to `[REDACTED]` before persisting to conversation logs
//! or disk while issuing one-time ephemeral `SecretTokenHandle`s to callers, and tracks
//! active browser/human takeovers per tab.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use openbot_contracts::conversation::{
    AgentPromptQuestion, AgentPromptQuestionResolution, AgentPromptResolution, ConversationMessage,
    ConversationMessageAuthor, ConversationMessageSender, ConversationMessageSource,
    ConversationMessageStatus, ConversationQuestionPrompt,
};
use openbot_contracts::limits::{INPUT_LIMITS, RequestId, clip_utf16, is_identifier};
use openbot_contracts::ui_blocks::{
    ConversationUiBlock, UI_BLOCK_VERSION, UI_VAULT_SECRET_FIELDS, UiBlockResponse, UiBlockSpec,
    UiBlockState, UiBlockStatus, UiBlockValue, is_ui_block_id, ui_block_action_is_privileged,
    ui_block_outcome_text, validate_ui_block_response,
};
use openbot_storage::{
    AgentRoster, AppendConversationMessageInput, OpenBotDatabase, PersistConversationChangesInput,
    ThreadStore, now_rfc3339_millis, redact_secrets, register_secret_value,
    write_json_file_atomically,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::error::{OpenBotRuntimeError, RuntimeResult};

/// Standard interactive gate TTL (`10 minutes` = `600_000 ms`), matching upstream
/// `USER_ANSWER_LIMIT_MS = 10 * 60_000`.
pub const ATTENTION_TTL_MS: u64 = 10 * 60_000;
/// Redacted marker written to conversation logs and transcripts in place of raw secrets.
pub const REDACTED_PLACEHOLDER: &str = "[REDACTED]";
/// Maximum active attention entries retained per category.
pub const AGENT_RUNTIME_ATTENTION_LIMIT: usize = 64;
/// Persisted non-secret attention state file inside `<data_dir>`.
pub const ATTENTION_STATE_FILE: &str = "openbot-attention-v1.json";

/// Sanitized secret handle handed to callers when a user submits a secret `ask_ui` or
/// `ask_user` field. Raw secret bytes never leave ephemeral memory or enter SQLite/JSON logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretTokenHandle {
    pub handle_id: String,
    pub field_id: String,
    pub redaction_marker: String,
    pub sha256_prefix: String,
    pub expires_at_ms: u64,
}

struct StoredSecretEntry {
    value: String,
    expires_at_ms: u64,
}

impl Drop for StoredSecretEntry {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalKind {
    Command,
    FileChange,
    Permissions,
    Tool,
    HostedSite,
    Payment,
    CardFill,
    AgentDelete,
    BrowserProfile,
    CommitCoAuthor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Declined,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestInput {
    pub agent_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub kind: ApprovalKind,
    pub action: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub owner_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApprovalRecord {
    pub request_id: String,
    pub gate_id: String,
    pub agent_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub kind: ApprovalKind,
    pub action: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub owner_only: bool,
    pub status: ApprovalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ApprovalDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responder: Option<ConversationMessageSender>,
    pub created_at: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskUserInput {
    pub agent_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub questions: Vec<AgentPromptQuestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingQuestionRecord {
    pub request_id: String,
    pub agent_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub prompt: ConversationQuestionPrompt,
    pub created_at: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionResolutionOutcome {
    pub request_id: String,
    pub resolution: AgentPromptResolution,
    pub secret_handles: BTreeMap<String, SecretTokenHandle>,
    pub resolved_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskUiInput {
    pub agent_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    pub spec: UiBlockSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingUiBlockRecord {
    pub request_id: String,
    pub block_id: String,
    pub agent_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub block: ConversationUiBlock,
    pub created_at: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiBlockResolutionOutcome {
    pub request_id: String,
    pub block_id: String,
    pub status: UiBlockStatus,
    pub response: UiBlockResponse,
    pub outcome: String,
    pub secret_handles: BTreeMap<String, SecretTokenHandle>,
    pub responded_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TakeoverReason {
    Captcha,
    Login,
    TwoFactor,
    Passkey,
    OAuth,
    SignInBlocked,
    Payment,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TakeoverStatus {
    Active,
    Resolved,
    Released,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterTakeoverInput {
    pub agent_id: String,
    pub thread_id: String,
    pub tab_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub reason: TakeoverReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTakeoverRecord {
    pub request_id: String,
    pub agent_id: String,
    pub thread_id: String,
    pub tab_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub reason: TakeoverReason,
    pub message: String,
    pub status: TakeoverStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub created_at: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveAttentionSnapshot {
    pub approvals: Vec<PendingApprovalRecord>,
    pub questions: Vec<PendingQuestionRecord>,
    pub ui_blocks: Vec<PendingUiBlockRecord>,
    pub takeovers: Vec<BrowserTakeoverRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedAttentionState {
    version: u32,
    approvals: BTreeMap<String, PendingApprovalRecord>,
    questions: BTreeMap<String, PendingQuestionRecord>,
    ui_blocks: BTreeMap<String, PendingUiBlockRecord>,
    takeovers: BTreeMap<String, BrowserTakeoverRecord>,
}

/// Thread-safe registry for interactive human-in-the-loop gates with `10m` TTLs
/// and strict zero-leak secret redaction.
pub struct AttentionRegistry {
    database: OpenBotDatabase,
    state_path: PathBuf,
    state: Mutex<PersistedAttentionState>,
    secrets: Mutex<HashMap<String, StoredSecretEntry>>,
}

impl AttentionRegistry {
    pub fn open(database: OpenBotDatabase, data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let state_path = data_dir.as_ref().join(ATTENTION_STATE_FILE);
        let mut persisted = PersistedAttentionState {
            version: 1,
            ..Default::default()
        };
        if state_path.exists()
            && let Ok(raw) = fs::read_to_string(&state_path)
            && let Ok(loaded) = serde_json::from_str::<PersistedAttentionState>(&raw)
        {
            persisted = loaded;
            persisted.version = 1;
        }
        Ok(Arc::new(Self {
            database,
            state_path,
            state: Mutex::new(persisted),
            secrets: Mutex::new(HashMap::new()),
        }))
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    pub fn request_approval(
        &self,
        input: ApprovalRequestInput,
    ) -> RuntimeResult<PendingApprovalRecord> {
        self.request_approval_at(input, current_unix_ms())
    }

    pub fn request_approval_at(
        &self,
        input: ApprovalRequestInput,
        now_ms: u64,
    ) -> RuntimeResult<PendingApprovalRecord> {
        if !is_identifier(&input.agent_id) || !is_identifier(&input.thread_id) {
            return Err(OpenBotRuntimeError::Validation(
                "Approval requires valid agentId and threadId.".to_string(),
            ));
        }
        if input.action.trim().is_empty() || input.title.trim().is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Approval action and title are required.".to_string(),
            ));
        }
        let ttl_ms = input.ttl_ms.unwrap_or(ATTENTION_TTL_MS).max(1);
        let request_id = format!("apr-{}", Uuid::new_v4());
        let record = PendingApprovalRecord {
            request_id: request_id.clone(),
            gate_id: request_id.clone(),
            agent_id: input.agent_id,
            thread_id: input.thread_id,
            turn_id: input.turn_id,
            kind: input.kind,
            action: input.action.trim().to_string(),
            title: clip_utf16(input.title.trim(), 200),
            detail: input.detail.map(|d| redact_secrets(&d)),
            command: input.command.map(|c| redact_secrets(&c)),
            cwd: input.cwd,
            reason: input.reason.map(|r| redact_secrets(&r)),
            owner_only: input.owner_only,
            status: ApprovalStatus::Pending,
            decision: None,
            responder: None,
            created_at: now_rfc3339_millis(),
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
            resolved_at: None,
        };
        let mut state = self.state.lock();
        expire_in_state(&mut state, now_ms);
        state.approvals.insert(request_id, record.clone());
        self.persist_locked(&state)?;
        Ok(record)
    }

    pub fn respond_approval(
        &self,
        request_id: &str,
        decision: ApprovalDecision,
        responder: Option<ConversationMessageSender>,
        is_owner: bool,
    ) -> RuntimeResult<PendingApprovalRecord> {
        self.respond_approval_at(request_id, decision, responder, is_owner, current_unix_ms())
    }

    pub fn respond_approval_at(
        &self,
        request_id: &str,
        decision: ApprovalDecision,
        responder: Option<ConversationMessageSender>,
        is_owner: bool,
        now_ms: u64,
    ) -> RuntimeResult<PendingApprovalRecord> {
        let mut state = self.state.lock();
        expire_in_state(&mut state, now_ms);
        let record = state.approvals.get_mut(request_id).ok_or_else(|| {
            OpenBotRuntimeError::NotFound(format!("Approval request not found: {request_id}"))
        })?;
        if record.status == ApprovalStatus::Expired {
            self.persist_locked(&state)?;
            return Err(OpenBotRuntimeError::Conflict(format!(
                "Approval request {request_id} has expired."
            )));
        }
        if record.status != ApprovalStatus::Pending {
            return Err(OpenBotRuntimeError::Conflict(format!(
                "Approval request {request_id} is no longer pending."
            )));
        }
        if record.owner_only && !is_owner {
            return Err(OpenBotRuntimeError::Forbidden(
                "Only the owner can decide this approval request.".to_string(),
            ));
        }
        record.status = match decision {
            ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => {
                ApprovalStatus::Approved
            }
            ApprovalDecision::Decline => ApprovalStatus::Declined,
            ApprovalDecision::Cancel => ApprovalStatus::Cancelled,
        };
        record.decision = Some(decision);
        record.responder = responder;
        record.resolved_at = Some(now_rfc3339_millis());
        let updated = record.clone();
        self.persist_locked(&state)?;
        Ok(updated)
    }

    pub fn get_approval(&self, request_id: &str) -> Option<PendingApprovalRecord> {
        self.state.lock().approvals.get(request_id).cloned()
    }

    pub fn approval_required_error(record: &PendingApprovalRecord) -> OpenBotRuntimeError {
        OpenBotRuntimeError::ApprovalRequired {
            action: record.action.clone(),
            gate_id: record.gate_id.clone(),
            expires_at_ms: record.expires_at_ms,
            message: format!("Human approval required for '{}': {}", record.action, record.title),
        }
    }

    pub fn ask_user(&self, input: AskUserInput) -> RuntimeResult<PendingQuestionRecord> {
        self.ask_user_at(input, current_unix_ms())
    }

    pub fn ask_user_at(
        &self,
        input: AskUserInput,
        now_ms: u64,
    ) -> RuntimeResult<PendingQuestionRecord> {
        if !is_identifier(&input.agent_id) || !is_identifier(&input.thread_id) {
            return Err(OpenBotRuntimeError::Validation(
                "ask_user requires valid agentId and threadId.".to_string(),
            ));
        }
        if input.questions.is_empty() || input.questions.len() > INPUT_LIMITS.prompt_questions {
            return Err(OpenBotRuntimeError::Validation(format!(
                "ask_user requires between 1 and {} questions.",
                INPUT_LIMITS.prompt_questions
            )));
        }
        let request_id = format!("ask-{}", Uuid::new_v4());
        let prompt = ConversationQuestionPrompt {
            request_id: RequestId::String(request_id.clone()),
            questions: input.questions,
            resolution: None,
        };
        if !prompt.is_valid() {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid ask_user prompt specification.".to_string(),
            ));
        }

        let created_at = now_rfc3339_millis();
        let ttl_ms = input.ttl_ms.unwrap_or(ATTENTION_TTL_MS).max(1);
        let msg_id = format!("question-{}", Uuid::new_v4());

        let mut msg = ConversationMessage::new_simple(
            msg_id.clone(),
            ConversationMessageAuthor::Assistant,
            prompt
                .questions
                .iter()
                .map(|q| q.question.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            created_at.clone(),
            ConversationMessageStatus::Completed,
        );
        msg.turn_id = input.turn_id.clone();
        msg.item_type = Some("question_prompt".to_string());
        msg.source = Some(ConversationMessageSource::Assistant);
        msg.question_prompt = Some(prompt.clone());
        let stored_msg_id = self.persist_thread_message_if_agent_exists(
            &input.agent_id,
            &input.thread_id,
            input.turn_id.as_deref(),
            msg,
            "prompt.created",
        )?;

        let record = PendingQuestionRecord {
            request_id: request_id.clone(),
            agent_id: input.agent_id,
            thread_id: input.thread_id,
            turn_id: input.turn_id,
            message_id: stored_msg_id,
            prompt,
            created_at,
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
            resolved_at: None,
        };
        let mut state = self.state.lock();
        expire_in_state(&mut state, now_ms);
        state.questions.insert(request_id, record.clone());
        self.persist_locked(&state)?;
        Ok(record)
    }

    pub fn respond_question(
        &self,
        request_id: &str,
        answers: BTreeMap<String, Vec<String>>,
        cancelled: bool,
    ) -> RuntimeResult<QuestionResolutionOutcome> {
        self.respond_question_at(request_id, answers, cancelled, current_unix_ms())
    }

    pub fn respond_question_at(
        &self,
        request_id: &str,
        answers: BTreeMap<String, Vec<String>>,
        cancelled: bool,
        now_ms: u64,
    ) -> RuntimeResult<QuestionResolutionOutcome> {
        let mut state = self.state.lock();
        expire_in_state(&mut state, now_ms);
        let record = state.questions.get_mut(request_id).ok_or_else(|| {
            OpenBotRuntimeError::NotFound(format!("Question prompt not found: {request_id}"))
        })?;
        if matches!(
            record.prompt.resolution,
            Some(AgentPromptResolution::Expired)
        ) {
            self.persist_locked(&state)?;
            return Err(OpenBotRuntimeError::Conflict(format!(
                "Question prompt {request_id} has expired."
            )));
        }
        if record.prompt.resolution.is_some() {
            return Err(OpenBotRuntimeError::Conflict(format!(
                "Question prompt {request_id} is already resolved."
            )));
        }

        let mut secret_handles = BTreeMap::new();
        let resolution = if cancelled {
            AgentPromptResolution::Cancelled
        } else {
            let mut responses = BTreeMap::new();
            for q in &record.prompt.questions {
                let raw_list = answers.get(&q.id).cloned().unwrap_or_default();
                let non_empty: Vec<String> = raw_list
                    .into_iter()
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                if non_empty.is_empty() {
                    responses.insert(q.id.clone(), AgentPromptQuestionResolution::Skipped);
                } else if q.is_secret {
                    let joined = non_empty.join("\n");
                    let handle = self.mint_secret_handle(&q.id, &joined, now_ms);
                    secret_handles.insert(q.id.clone(), handle);
                    responses.insert(
                        q.id.clone(),
                        AgentPromptQuestionResolution::Answered {
                            answers: Some(vec![REDACTED_PLACEHOLDER.to_string()]),
                        },
                    );
                } else {
                    responses.insert(
                        q.id.clone(),
                        AgentPromptQuestionResolution::Answered {
                            answers: Some(non_empty),
                        },
                    );
                }
            }
            AgentPromptResolution::Answered { responses }
        };

        let resolved_at = now_rfc3339_millis();
        record.prompt.resolution = Some(resolution.clone());
        record.resolved_at = Some(resolved_at.clone());
        let snapshot_record = record.clone();
        self.persist_locked(&state)?;
        drop(state);

        if let Some(msg_id) = &snapshot_record.message_id {
            let mut updated_msg = ConversationMessage::new_simple(
                msg_id.clone(),
                ConversationMessageAuthor::Assistant,
                snapshot_record
                    .prompt
                    .questions
                    .iter()
                    .map(|q| q.question.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                snapshot_record.created_at.clone(),
                ConversationMessageStatus::Completed,
            );
            updated_msg.turn_id = snapshot_record.turn_id.clone();
            updated_msg.item_type = Some("question_prompt".to_string());
            updated_msg.source = Some(ConversationMessageSource::Assistant);
            updated_msg.question_prompt = Some(snapshot_record.prompt.clone());
            self.update_thread_message_if_agent_exists(
                &snapshot_record.agent_id,
                &snapshot_record.thread_id,
                snapshot_record.turn_id.as_deref(),
                updated_msg,
                "prompt.resolved",
            )?;
        }

        Ok(QuestionResolutionOutcome {
            request_id: request_id.to_string(),
            resolution,
            secret_handles,
            resolved_at,
        })
    }

    pub fn ask_ui(&self, input: AskUiInput) -> RuntimeResult<PendingUiBlockRecord> {
        self.ask_ui_at(input, current_unix_ms())
    }

    pub fn ask_ui_at(
        &self,
        input: AskUiInput,
        now_ms: u64,
    ) -> RuntimeResult<PendingUiBlockRecord> {
        if !is_identifier(&input.agent_id) || !is_identifier(&input.thread_id) {
            return Err(OpenBotRuntimeError::Validation(
                "ask_ui requires valid agentId and threadId.".to_string(),
            ));
        }
        if !input.spec.validate() {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid ask_ui block specification.".to_string(),
            ));
        }
        let block_id = match input.block_id {
            Some(id) if is_ui_block_id(&id) => id,
            Some(_) => {
                return Err(OpenBotRuntimeError::Validation(
                    "Invalid ask_ui blockId.".to_string(),
                ));
            }
            None => format!("blk-{}", Uuid::new_v4()),
        };
        let block = ConversationUiBlock {
            version: UI_BLOCK_VERSION,
            block_id: block_id.clone(),
            spec: input.spec,
            state: UiBlockState {
                status: UiBlockStatus::Pending,
                response: None,
                responded_by: None,
                responded_at: None,
                outcome: None,
                auto_answer: None,
            },
            vault: None,
            api_key: None,
            location: None,
        };
        if !block.validate() {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid ConversationUiBlock payload.".to_string(),
            ));
        }

        let request_id = format!("ui-{}", Uuid::new_v4());
        let created_at = now_rfc3339_millis();
        let ttl_ms = input.ttl_ms.unwrap_or(ATTENTION_TTL_MS).max(1);
        let msg_id = format!("uiblock-{}", Uuid::new_v4());

        let mut msg = ConversationMessage::new_simple(
            msg_id,
            ConversationMessageAuthor::Assistant,
            ui_block_title(&block.spec),
            created_at.clone(),
            ConversationMessageStatus::Completed,
        );
        msg.turn_id = input.turn_id.clone();
        msg.item_type = Some("question_prompt".to_string());
        msg.source = Some(ConversationMessageSource::Assistant);
        msg.ui_block = Some(block.clone());
        let stored_msg_id = self.persist_thread_message_if_agent_exists(
            &input.agent_id,
            &input.thread_id,
            input.turn_id.as_deref(),
            msg,
            "ui-block.created",
        )?;

        let record = PendingUiBlockRecord {
            request_id: request_id.clone(),
            block_id,
            agent_id: input.agent_id,
            thread_id: input.thread_id,
            turn_id: input.turn_id,
            message_id: stored_msg_id,
            block,
            created_at,
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
        };
        let mut state = self.state.lock();
        expire_in_state(&mut state, now_ms);
        state.ui_blocks.insert(request_id, record.clone());
        self.persist_locked(&state)?;
        Ok(record)
    }

    pub fn respond_ui_block(
        &self,
        request_id: &str,
        response: UiBlockResponse,
        responder: Option<ConversationMessageSender>,
        privileged: bool,
    ) -> RuntimeResult<UiBlockResolutionOutcome> {
        self.respond_ui_block_at(request_id, response, responder, privileged, current_unix_ms())
    }

    pub fn respond_ui_block_at(
        &self,
        request_id: &str,
        response: UiBlockResponse,
        responder: Option<ConversationMessageSender>,
        privileged: bool,
        now_ms: u64,
    ) -> RuntimeResult<UiBlockResolutionOutcome> {
        let mut state = self.state.lock();
        expire_in_state(&mut state, now_ms);
        let record = state.ui_blocks.get_mut(request_id).ok_or_else(|| {
            OpenBotRuntimeError::NotFound(format!("UI block prompt not found: {request_id}"))
        })?;
        if record.block.state.status == UiBlockStatus::Expired {
            self.persist_locked(&state)?;
            return Err(OpenBotRuntimeError::Conflict(format!(
                "UI block prompt {request_id} has expired."
            )));
        }
        if record.block.state.status != UiBlockStatus::Pending {
            return Err(OpenBotRuntimeError::Conflict(format!(
                "UI block prompt {request_id} is no longer pending."
            )));
        }
        if ui_block_action_is_privileged(&record.block.spec, &response.action_id) && !privileged {
            return Err(OpenBotRuntimeError::Forbidden(
                "Privileged confirmation action requires an authorized responder.".to_string(),
            ));
        }
        let validated = validate_ui_block_response(&record.block.spec, &response, false)
            .ok_or_else(|| {
                OpenBotRuntimeError::Validation(
                    "Invalid response for ask_ui block specification.".to_string(),
                )
            })?;

        // Strictly redact all secret fields (`UiFormField::Text { secret: Some(true) }`
        // and vault secret field IDs) before persisting to conversation logs or disk.
        let mut secret_handles = BTreeMap::new();
        let mut sanitized_response = validated.clone();
        if let UiBlockSpec::Form { fields, .. } = &record.block.spec
            && let Some(vals) = sanitized_response.values.as_mut()
        {
            for field in fields {
                let is_secret = field.is_secret()
                    || UI_VAULT_SECRET_FIELDS.contains(&field.id());
                if !is_secret {
                    continue;
                }
                if let Some(UiBlockValue::Text(raw_secret)) = vals.get(field.id()).cloned()
                    && !raw_secret.is_empty()
                {
                    let handle = self.mint_secret_handle(field.id(), &raw_secret, now_ms);
                    secret_handles.insert(field.id().to_string(), handle);
                    vals.insert(
                        field.id().to_string(),
                        UiBlockValue::Text(REDACTED_PLACEHOLDER.to_string()),
                    );
                }
            }
        }

        let outcome = ui_block_outcome_text(&record.block.spec, &sanitized_response);
        let responded_at = now_rfc3339_millis();

        record.block.state = UiBlockState {
            status: UiBlockStatus::Answered,
            response: Some(sanitized_response.clone()),
            responded_by: responder,
            responded_at: Some(responded_at.clone()),
            outcome: if outcome.is_empty() {
                if !secret_handles.is_empty() {
                    Some(REDACTED_PLACEHOLDER.to_string())
                } else {
                    None
                }
            } else {
                Some(outcome.clone())
            },
            auto_answer: None,
        };
        let snapshot_record = record.clone();
        self.persist_locked(&state)?;
        drop(state);

        if let Some(msg_id) = &snapshot_record.message_id {
            let log_text = if !secret_handles.is_empty() {
                format!(
                    "{}: {}",
                    ui_block_title(&snapshot_record.block.spec),
                    REDACTED_PLACEHOLDER
                )
            } else if !outcome.is_empty() {
                format!("{}: {outcome}", ui_block_title(&snapshot_record.block.spec))
            } else {
                ui_block_title(&snapshot_record.block.spec)
            };
            let mut updated_msg = ConversationMessage::new_simple(
                msg_id.clone(),
                ConversationMessageAuthor::Assistant,
                log_text,
                snapshot_record.created_at.clone(),
                ConversationMessageStatus::Completed,
            );
            updated_msg.turn_id = snapshot_record.turn_id.clone();
            updated_msg.item_type = Some("question_prompt".to_string());
            updated_msg.source = Some(ConversationMessageSource::Assistant);
            updated_msg.ui_block = Some(snapshot_record.block.clone());
            self.update_thread_message_if_agent_exists(
                &snapshot_record.agent_id,
                &snapshot_record.thread_id,
                snapshot_record.turn_id.as_deref(),
                updated_msg,
                "ui-block.resolved",
            )?;
        }

        Ok(UiBlockResolutionOutcome {
            request_id: request_id.to_string(),
            block_id: snapshot_record.block_id,
            status: UiBlockStatus::Answered,
            response: sanitized_response,
            outcome: snapshot_record
                .block
                .state
                .outcome
                .unwrap_or_else(|| REDACTED_PLACEHOLDER.to_string()),
            secret_handles,
            responded_at,
        })
    }

    pub fn register_takeover(
        &self,
        input: RegisterTakeoverInput,
    ) -> RuntimeResult<BrowserTakeoverRecord> {
        self.register_takeover_at(input, current_unix_ms())
    }

    pub fn register_takeover_at(
        &self,
        input: RegisterTakeoverInput,
        now_ms: u64,
    ) -> RuntimeResult<BrowserTakeoverRecord> {
        if !is_identifier(&input.agent_id)
            || !is_identifier(&input.thread_id)
            || !is_identifier(&input.tab_id)
        {
            return Err(OpenBotRuntimeError::Validation(
                "Browser takeover requires valid agentId, threadId, and tabId.".to_string(),
            ));
        }
        let mut state = self.state.lock();
        expire_in_state(&mut state, now_ms);
        if state
            .takeovers
            .values()
            .any(|t| t.tab_id == input.tab_id && t.status == TakeoverStatus::Active)
        {
            return Err(OpenBotRuntimeError::Conflict(
                "A takeover of this tab already waits for the user.".to_string(),
            ));
        }
        let ttl_ms = input.ttl_ms.unwrap_or(ATTENTION_TTL_MS).max(1);
        let request_id = format!("tko-{}", Uuid::new_v4());
        let default_message = format!("Human takeover requested for tab {}.", input.tab_id);
        let record = BrowserTakeoverRecord {
            request_id: request_id.clone(),
            agent_id: input.agent_id,
            thread_id: input.thread_id,
            tab_id: input.tab_id,
            url: input.url,
            reason: input.reason,
            message: input
                .message
                .map(|m| redact_secrets(m.trim()))
                .filter(|m| !m.is_empty())
                .unwrap_or(default_message),
            status: TakeoverStatus::Active,
            notes: None,
            created_at: now_rfc3339_millis(),
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
            resolved_at: None,
        };
        state.takeovers.insert(request_id, record.clone());
        self.persist_locked(&state)?;
        Ok(record)
    }

    pub fn resolve_takeover(
        &self,
        request_id: &str,
        notes: Option<String>,
        released: bool,
    ) -> RuntimeResult<BrowserTakeoverRecord> {
        self.resolve_takeover_at(request_id, notes, released, current_unix_ms())
    }

    pub fn resolve_takeover_at(
        &self,
        request_id: &str,
        notes: Option<String>,
        released: bool,
        now_ms: u64,
    ) -> RuntimeResult<BrowserTakeoverRecord> {
        let mut state = self.state.lock();
        expire_in_state(&mut state, now_ms);
        let record = state.takeovers.get_mut(request_id).ok_or_else(|| {
            OpenBotRuntimeError::NotFound(format!("Browser takeover not found: {request_id}"))
        })?;
        if record.status == TakeoverStatus::Expired {
            self.persist_locked(&state)?;
            return Err(OpenBotRuntimeError::Conflict(format!(
                "Browser takeover {request_id} has expired."
            )));
        }
        if record.status != TakeoverStatus::Active {
            return Err(OpenBotRuntimeError::Conflict(format!(
                "Browser takeover {request_id} is no longer active."
            )));
        }
        record.status = if released {
            TakeoverStatus::Released
        } else {
            TakeoverStatus::Resolved
        };
        record.notes = notes.map(|n| redact_secrets(&n));
        record.resolved_at = Some(now_rfc3339_millis());
        let updated = record.clone();
        self.persist_locked(&state)?;
        Ok(updated)
    }

    pub fn expire_stale(&self) -> usize {
        self.expire_stale_at(current_unix_ms())
    }

    pub fn expire_stale_at(&self, now_ms: u64) -> usize {
        let expired = {
            let mut state = self.state.lock();
            let count = expire_in_state(&mut state, now_ms);
            if count > 0 {
                let _ = self.persist_locked(&state);
            }
            count
        };
        self.secrets
            .lock()
            .retain(|_, entry| entry.expires_at_ms > now_ms);
        expired
    }

    pub fn list_active(&self, agent_id: Option<&str>) -> ActiveAttentionSnapshot {
        self.list_active_at(agent_id, current_unix_ms())
    }

    pub fn list_active_at(&self, agent_id: Option<&str>, now_ms: u64) -> ActiveAttentionSnapshot {
        let mut state = self.state.lock();
        if expire_in_state(&mut state, now_ms) > 0 {
            let _ = self.persist_locked(&state);
        }
        let matches_agent = |aid: &str| agent_id.is_none_or(|expected| expected == aid);

        let approvals = state
            .approvals
            .values()
            .filter(|a| a.status == ApprovalStatus::Pending && matches_agent(&a.agent_id))
            .take(AGENT_RUNTIME_ATTENTION_LIMIT)
            .cloned()
            .collect();
        let questions = state
            .questions
            .values()
            .filter(|q| q.prompt.resolution.is_none() && matches_agent(&q.agent_id))
            .take(AGENT_RUNTIME_ATTENTION_LIMIT)
            .cloned()
            .collect();
        let ui_blocks = state
            .ui_blocks
            .values()
            .filter(|b| b.block.state.status == UiBlockStatus::Pending && matches_agent(&b.agent_id))
            .take(AGENT_RUNTIME_ATTENTION_LIMIT)
            .cloned()
            .collect();
        let takeovers = state
            .takeovers
            .values()
            .filter(|t| t.status == TakeoverStatus::Active && matches_agent(&t.agent_id))
            .take(AGENT_RUNTIME_ATTENTION_LIMIT)
            .cloned()
            .collect();

        ActiveAttentionSnapshot {
            approvals,
            questions,
            ui_blocks,
            takeovers,
        }
    }

    pub fn has_pending_for_agent(&self, agent_id: &str) -> bool {
        let snap = self.list_active(Some(agent_id));
        !snap.approvals.is_empty()
            || !snap.questions.is_empty()
            || !snap.ui_blocks.is_empty()
            || !snap.takeovers.is_empty()
    }

    /// Consumes a one-time ephemeral secret handle. Returns `None` if unknown or expired.
    pub fn take_secret_by_handle(&self, handle_id: &str) -> Option<String> {
        let now_ms = current_unix_ms();
        let entry = self.secrets.lock().remove(handle_id)?;
        if entry.expires_at_ms <= now_ms {
            return None;
        }
        Some(entry.value.clone())
    }

    fn mint_secret_handle(
        &self,
        field_id: &str,
        raw_secret: &str,
        now_ms: u64,
    ) -> SecretTokenHandle {
        register_secret_value(raw_secret, false);
        let digest = Sha256::digest(raw_secret.as_bytes());
        let sha256_prefix = hex::encode(&digest[..8]);
        let handle_id = format!("sec-{}", Uuid::new_v4());
        let expires_at_ms = now_ms.saturating_add(ATTENTION_TTL_MS);
        self.secrets.lock().insert(
            handle_id.clone(),
            StoredSecretEntry {
                value: raw_secret.to_string(),
                expires_at_ms,
            },
        );
        SecretTokenHandle {
            handle_id,
            field_id: field_id.to_string(),
            redaction_marker: REDACTED_PLACEHOLDER.to_string(),
            sha256_prefix,
            expires_at_ms,
        }
    }

    fn persist_locked(&self, state: &PersistedAttentionState) -> RuntimeResult<()> {
        write_json_file_atomically(&self.state_path, state, true)?;
        Ok(())
    }

    fn persist_thread_message_if_agent_exists(
        &self,
        agent_id: &str,
        thread_id: &str,
        turn_id: Option<&str>,
        message: ConversationMessage,
        event_type: &str,
    ) -> RuntimeResult<Option<String>> {
        let roster = AgentRoster::new(self.database.clone());
        if roster.get_agent(agent_id)?.is_none() {
            return Ok(None);
        }
        let message_id = message.id.clone();
        let thread_store = ThreadStore::new(self.database.clone());
        thread_store.append_conversation_message(&AppendConversationMessageInput {
            agent_id: agent_id.to_string(),
            thread_id: thread_id.to_string(),
            active_turn_id: turn_id.map(str::to_string),
            message,
            event_type: event_type.to_string(),
            detail: serde_json::json!({}),
            command_id: None,
        })?;
        Ok(Some(message_id))
    }

    fn update_thread_message_if_agent_exists(
        &self,
        agent_id: &str,
        thread_id: &str,
        turn_id: Option<&str>,
        message: ConversationMessage,
        event_type: &str,
    ) -> RuntimeResult<()> {
        let roster = AgentRoster::new(self.database.clone());
        if roster.get_agent(agent_id)?.is_none() {
            return Ok(());
        }
        let thread_store = ThreadStore::new(self.database.clone());
        thread_store.persist_conversation_changes(&PersistConversationChangesInput {
            agent_id: agent_id.to_string(),
            thread_id: thread_id.to_string(),
            active_turn_id: turn_id.map(str::to_string),
            changed_messages: vec![message],
            removed_message_ids: Vec::new(),
            deleted_message_ids: Vec::new(),
            event_type: event_type.to_string(),
            detail: serde_json::json!({}),
            command_id: None,
        })?;
        Ok(())
    }
}

fn expire_in_state(state: &mut PersistedAttentionState, now_ms: u64) -> usize {
    let mut expired = 0usize;
    let now_str = now_rfc3339_millis();
    for approval in state.approvals.values_mut() {
        if approval.status == ApprovalStatus::Pending && approval.expires_at_ms <= now_ms {
            approval.status = ApprovalStatus::Expired;
            approval.resolved_at = Some(now_str.clone());
            expired += 1;
        }
    }
    for question in state.questions.values_mut() {
        if question.prompt.resolution.is_none() && question.expires_at_ms <= now_ms {
            question.prompt.resolution = Some(AgentPromptResolution::Expired);
            question.resolved_at = Some(now_str.clone());
            expired += 1;
        }
    }
    for block in state.ui_blocks.values_mut() {
        if block.block.state.status == UiBlockStatus::Pending && block.expires_at_ms <= now_ms {
            block.block.state.status = UiBlockStatus::Expired;
            block.block.state.responded_at = Some(now_str.clone());
            expired += 1;
        }
    }
    for takeover in state.takeovers.values_mut() {
        if takeover.status == TakeoverStatus::Active && takeover.expires_at_ms <= now_ms {
            takeover.status = TakeoverStatus::Expired;
            takeover.resolved_at = Some(now_str.clone());
            expired += 1;
        }
    }
    expired
}

fn ui_block_title(spec: &UiBlockSpec) -> String {
    match spec {
        UiBlockSpec::Confirm { title, .. }
        | UiBlockSpec::Choice { title, .. }
        | UiBlockSpec::Form { title, .. } => title.clone(),
        UiBlockSpec::QuickReplies { title, options, .. } => title.clone().unwrap_or_else(|| {
            options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        }),
    }
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
