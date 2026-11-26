//! External connectors security (`ActsFor`, `--leave-thread` anti-injection, `BotAccessHost`),
//! genuine async `SlackMirror` turn-end ordering barrier, 6-adapter zero-leak `WebhookEngine`,
//! 31-event-kind `AutomationTriggerEngine`, `OneShotWaitManager`, `WatchPrCoordinator`, and
//! `ContentFreeWakeFrame`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use orca_core::bot_ecosystem::{NativeChatBotCard, NativeChatBotCardKind};
use orca_core::bot_types::{
    ActsForAsker, BotMember, acts_for_owner, bot_member_source, distinct_askers,
    format_asked_by_line, with_asked_by,
};
use orca_core::triggers_webhooks::{
    AutomationBotTarget, AutomationEventKind, AutomationEventSource, AutomationEventTrigger,
    AutomationRunStatus, AutomationRunTrigger, EVENT_ONLY_SCHEDULE_DATE,
    EVENT_ONLY_SCHEDULE_DTSTART_MS, EVENT_ONLY_SCHEDULE_RRULE, GitHubEventKind,
    OneShotRunSummary, OneShotWait, PendingWaitEntry, WAIT_EXPIRY_MS, WAIT_MAX_DEADLINE_MS,
    WATCH_PR_DEFAULT_MS, WATCH_PR_MAX_FIX_ATTEMPTS, WEBHOOK_CALL_ID_PREFIX,
    WEBHOOK_CALL_ID_RANDOM_LEN, WEBHOOK_ENVELOPE_MAX_BODY_BYTES,
    WEBHOOK_ENVELOPE_MAX_HEADER_VALUE_LEN, WEBHOOK_HOOK_ID_PREFIX, WEBHOOK_HOOK_ID_RANDOM_LEN,
    WEBHOOK_KEY_PREFIX, WEBHOOK_KEY_RANDOM_LEN, WEBHOOK_PATH_PREFIX, WaitEnding, WatchPrPolicy,
    WatchPrStepAction, WatchPrWakeOutcome, WebhookAdapterId, WebhookCallFields,
    WebhookCallStartAnswer, WebhookCallState, WebhookCallView, WebhookCapDecision,
    WebhookCliSummary, WebhookEventKind, WebhookFilters, WebhookHourlyCapTracker, WebhookNotice,
    WebhookReceiptStatus, WebhookRuleWire, WebhookTriggerEvent, advance_webhook_epoch,
    can_transition_webhook_call_state, check_webhook_epoch, claims_the_run,
    evaluate_watch_pr_step, is_valid_webhook_call_id, is_valid_webhook_hook_id,
    is_valid_webhook_key, matches_webhook_call, once_claimed, once_run_ending, once_run_finished,
    parse_watch_duration, pending_waits, redact_webhook_keys, validate_automation_event_triggers,
    validate_webhook_filters, validate_webhook_rule_bytes, webhook_actor_id, webhook_body_fits,
    webhook_call_subject, webhook_rule_id,
};
use orca_core::{BOT_MAX_CONNECTOR_WORKSPACES, BOT_MAX_MEMBERS, BotErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex as AsyncMutex, Notify};
use uuid::Uuid;

use crate::controller::{BotController, BotPatch};
use crate::home_store::atomic_write_bytes;
use crate::{OrcaBotsError, Result, now_epoch_ms};

type HmacSha256 = Hmac<Sha256>;

/// Default TTL for a "Who Can Ask" connector access request (24 hours).
pub const ACCESS_REQUEST_DEFAULT_TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// WebSocket close code sent by the hibernating Wake Durable Object when a newer host socket supersedes an older one.
pub const WS_CLOSE_SUPERSEDED: u16 = 4409;
/// WebSocket close code sent when an inbound wake frame exceeds `CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES`.
pub const WS_CLOSE_MESSAGE_TOO_BIG: u16 = 1009;
/// Maximum allowed byte length of a content-free wake WebSocket frame.
pub const CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES: usize = 4_096;

const ACCESS_REQUESTS_FILE: &str = "access_requests.json";
const ALNUM_CHARSET: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

// ---------------------------------------------------------------------------
// 1. ActsFor Attribution, Owner-Action Guard & `--leave-thread` Anti-Injection
// ---------------------------------------------------------------------------

/// Stamps a new asker onto a conversation's deduplicated asker roster via `orca_core::bot_types::distinct_askers`.
pub fn stamp_conversation_askers(
    existing: &[ActsForAsker],
    incoming: ActsForAsker,
) -> Vec<ActsForAsker> {
    let mut combined = Vec::with_capacity(existing.len() + 1);
    combined.extend_from_slice(existing);
    combined.push(incoming);
    distinct_askers(&combined)
}

/// Formats the 140-char bounded `"asked by <name> via <Bot>"` attribution line for external actions.
pub fn format_external_action_attribution(
    askers: &[ActsForAsker],
    bot_name: &str,
    typed_by_owner: bool,
) -> Option<String> {
    format_asked_by_line(askers, bot_name, typed_by_owner)
}

/// Appends `"asked by <name> via <Bot>"` onto an external action body when acting for non-owner askers.
pub fn decorate_external_action_body(
    body: &str,
    askers: &[ActsForAsker],
    bot_name: &str,
    typed_by_owner: bool,
) -> String {
    with_asked_by(body, askers, bot_name, typed_by_owner)
}

/// Enforces owner-only authorization for approvals, merges, and privileged external actions.
/// Returns `BotErrorCode::NotOwner` unless `orca_core::bot_types::acts_for_owner(askers)` holds.
pub fn require_owner_asker(askers: &[ActsForAsker], action: &str) -> Result<()> {
    if acts_for_owner(askers) {
        return Ok(());
    }
    Err(OrcaBotsError::bot(
        BotErrorCode::NotOwner,
        format!(
            "Action '{action}' requires the bot owner; refusing because the active asker is not the owner."
        ),
    ))
}

/// Input context evaluated by [`evaluate_leave_thread_request`] to prevent `--leave-thread` prompt injection.
#[derive(Debug, Clone)]
pub struct LeaveThreadContext<'a> {
    pub text: &'a str,
    pub bot_name: &'a str,
    pub other_bot_names: &'a [&'a str],
    pub is_forwarded: bool,
    pub is_quoted_reply: bool,
    pub addressed_bot_name: Option<&'a str>,
}

/// Reason a `--leave-thread` request was blocked by the anti-injection evaluator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaveThreadBlockReason {
    ForwardedMessage,
    QuotedOrPastedText,
    InsideCodeBlock,
    MetaQuestion,
    DirectedAtAnotherBot,
    NotRequested,
}

/// Outcome of evaluating whether a bot may execute `bot reply --leave-thread`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum LeaveThreadDecision {
    Allow,
    Block {
        reason: LeaveThreadBlockReason,
        explanation: String,
    },
}

impl LeaveThreadDecision {
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// Evaluates whether a `--leave-thread` request is a genuine direct instruction to `ctx.bot_name`,
/// blocking quoted/pasted/forwarded/code-block text, meta questions, and requests directed at another bot.
pub fn evaluate_leave_thread_request(ctx: &LeaveThreadContext<'_>) -> LeaveThreadDecision {
    if ctx.is_forwarded {
        return LeaveThreadDecision::Block {
            reason: LeaveThreadBlockReason::ForwardedMessage,
            explanation: "Refusing --leave-thread because the message is forwarded.".to_string(),
        };
    }
    if ctx.is_quoted_reply {
        return LeaveThreadDecision::Block {
            reason: LeaveThreadBlockReason::QuotedOrPastedText,
            explanation: "Refusing --leave-thread because the text comes from a quoted reply."
                .to_string(),
        };
    }
    if let Some(addressed) = ctx.addressed_bot_name
        && !addressed.trim().is_empty()
        && !addressed.trim().eq_ignore_ascii_case(ctx.bot_name.trim())
    {
        return LeaveThreadDecision::Block {
            reason: LeaveThreadBlockReason::DirectedAtAnotherBot,
            explanation: format!(
                "Refusing --leave-thread because the request is directed at '{}', not '{}'.",
                addressed.trim(),
                ctx.bot_name.trim()
            ),
        };
    }

    let raw = ctx.text;
    let had_leave_phrase_raw = contains_leave_phrase(&raw.to_ascii_lowercase());
    let without_code = strip_code_regions(raw);
    let lower_no_code = without_code.to_ascii_lowercase();
    if had_leave_phrase_raw && !contains_leave_phrase(&lower_no_code) {
        return LeaveThreadDecision::Block {
            reason: LeaveThreadBlockReason::InsideCodeBlock,
            explanation: "Refusing --leave-thread because the leave phrase only appears inside code."
                .to_string(),
        };
    }

    let without_quotes = strip_quoted_and_pasted_regions(&without_code);
    let lower_direct = without_quotes.to_ascii_lowercase();
    if contains_leave_phrase(&lower_no_code) && !contains_leave_phrase(&lower_direct) {
        return LeaveThreadDecision::Block {
            reason: LeaveThreadBlockReason::QuotedOrPastedText,
            explanation:
                "Refusing --leave-thread because the leave phrase appears inside quoted, pasted, or reported speech."
                    .to_string(),
        };
    }
    if is_meta_leave_question(&lower_direct) {
        return LeaveThreadDecision::Block {
            reason: LeaveThreadBlockReason::MetaQuestion,
            explanation: "Refusing --leave-thread because the message is a meta question."
                .to_string(),
        };
    }
    if !contains_leave_phrase(&lower_direct) {
        return LeaveThreadDecision::Block {
            reason: LeaveThreadBlockReason::NotRequested,
            explanation: "No direct instruction to leave this thread was found.".to_string(),
        };
    }
    if is_directed_at_other_bot(&lower_direct, ctx.bot_name, ctx.other_bot_names) {
        return LeaveThreadDecision::Block {
            reason: LeaveThreadBlockReason::DirectedAtAnotherBot,
            explanation: "Refusing --leave-thread because the request addresses another bot."
                .to_string(),
        };
    }

    LeaveThreadDecision::Allow
}

fn contains_leave_phrase(lower: &str) -> bool {
    const PHRASES: &[&str] = &[
        "--leave-thread",
        "leave this thread",
        "leave the thread",
        "leave thread",
        "stop following this thread",
        "stop following the thread",
        "stop following",
        "unfollow this thread",
        "mute this thread",
        "please leave",
        "bow out of this thread",
        "bow out",
    ];
    PHRASES.iter().any(|p| lower.contains(p))
}

fn strip_code_regions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push('\n');
            continue;
        }
        if in_fence {
            out.push('\n');
            continue;
        }
        let mut in_inline = false;
        for ch in line.chars() {
            if ch == '`' {
                in_inline = !in_inline;
                out.push(' ');
            } else if !in_inline {
                out.push(ch);
            }
        }
        out.push('\n');
    }
    out
}

fn strip_quoted_and_pasted_regions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('>') {
            continue;
        }
        let lower_line = trimmed.to_ascii_lowercase();
        let is_pasted_or_reported = lower_line.starts_with("forwarded:")
            || lower_line.starts_with("pasted:")
            || lower_line.starts_with("quote:")
            || lower_line.starts_with("quoted:")
            || lower_line.contains(" said:")
            || lower_line.contains(" wrote:")
            || lower_line.contains(" pasted:")
            || lower_line.contains(" forwarded:");
        if is_pasted_or_reported {
            continue;
        }
        let mut in_double = false;
        let mut in_curly = false;
        for ch in trimmed.chars() {
            match ch {
                '"' => in_double = !in_double,
                '\u{201C}' => in_curly = true,
                '\u{201D}' => in_curly = false,
                _ if !in_double && !in_curly => out.push(ch),
                _ => {}
            }
        }
        out.push('\n');
    }
    out
}

fn is_meta_leave_question(lower: &str) -> bool {
    const META_PREFIXES: &[&str] = &[
        "how do you leave",
        "how can you leave",
        "how do i make you leave",
        "how does --leave-thread",
        "what does --leave-thread",
        "what happens if",
        "what happens when",
        "why would you leave",
        "when do you leave",
        "do you know how to leave",
        "explain --leave-thread",
        "explain how to leave",
    ];
    if META_PREFIXES.iter().any(|p| lower.contains(p)) {
        return true;
    }
    let mentions_leave_topic = lower.contains("leave")
        || lower.contains("unfollow")
        || lower.contains("stop following")
        || lower.contains("mute");
    mentions_leave_topic
        && lower.trim_end().ends_with('?')
        && (lower.contains("how ")
            || lower.contains("what ")
            || lower.contains("why ")
            || lower.contains("when "))
}

fn is_directed_at_other_bot(lower: &str, bot_name: &str, other_bot_names: &[&str]) -> bool {
    let self_lower = bot_name.trim().to_ascii_lowercase();
    let mentions_self = !self_lower.is_empty() && lower_contains_bot_mention(lower, &self_lower);
    if mentions_self {
        return false;
    }
    other_bot_names.iter().any(|other| {
        let other_lower = other.trim().to_ascii_lowercase();
        !other_lower.is_empty()
            && other_lower != self_lower
            && lower_contains_bot_mention(lower, &other_lower)
    })
}

fn lower_contains_bot_mention(lower: &str, name_lower: &str) -> bool {
    if lower.contains(&format!("@{name_lower}")) {
        return true;
    }
    for prefix in [
        format!("{name_lower},"),
        format!("{name_lower}:"),
        format!("{name_lower} please"),
        format!("{name_lower} leave"),
        format!("{name_lower} stop"),
    ] {
        if lower.contains(&prefix) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// 2. BotAccessHost ("Who Can Ask" approval / decline / redeem / recovery)
// ---------------------------------------------------------------------------

/// Lifecycle state of a cloud/host external user access request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorAccessRequestStatus {
    Pending,
    Granted,
    Declined,
}

/// Persisted "Who Can Ask" connector access request record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorAccessRequest {
    pub request_id: String,
    pub host_id: String,
    pub bot_id: String,
    pub member: BotMember,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_workspace_id: Option<String>,
    pub status: ConnectorAccessRequestStatus,
    pub redeem_token: String,
    pub redeem_token_used: bool,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at_ms: Option<i64>,
    #[serde(default)]
    pub granted_not_added: bool,
}

/// Input to create a pending "Who Can Ask" access request.
#[derive(Debug, Clone)]
pub struct CreateAccessRequestInput {
    pub bot_id: String,
    pub member: BotMember,
    pub connector_workspace_id: Option<String>,
    pub ttl_ms: Option<i64>,
    pub now_ms: i64,
}

/// Result of deciding or redeeming a connector access request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessDecisionOutcome {
    pub request: ConnectorAccessRequest,
    pub added_locally: bool,
}

/// Result of the startup recovery sweep for cloud-granted requests not yet added locally.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessRecoveryReport {
    pub recovered_request_ids: Vec<String>,
    pub still_pending_request_ids: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AccessHostPersisted {
    requests: BTreeMap<String, ConnectorAccessRequest>,
}

/// Host manager for "Who Can Ask" connector access requests, single-use CSPRNG redeem tokens,
/// host binding, member/workspace cap checks, cloud-first approval, and startup recovery.
#[derive(Debug, Clone)]
pub struct BotAccessHost {
    host_id: String,
    state_path: Option<PathBuf>,
    inner: Arc<StdMutex<AccessHostPersisted>>,
}

impl BotAccessHost {
    /// Creates an in-memory `BotAccessHost` bound to `host_id`.
    pub fn new(host_id: impl Into<String>) -> Self {
        Self {
            host_id: host_id.into(),
            state_path: None,
            inner: Arc::new(StdMutex::new(AccessHostPersisted::default())),
        }
    }

    /// Opens a file-persisted `BotAccessHost` storing requests under `state_dir/access_requests.json`.
    pub fn open(host_id: impl Into<String>, state_dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = state_dir.into();
        fs::create_dir_all(&dir)?;
        let path = dir.join(ACCESS_REQUESTS_FILE);
        let persisted = match fs::read_to_string(&path) {
            Ok(raw) if raw.trim().is_empty() => AccessHostPersisted::default(),
            Ok(raw) => serde_json::from_str(&raw)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => AccessHostPersisted::default(),
            Err(err) => return Err(OrcaBotsError::Io(err)),
        };
        Ok(Self {
            host_id: host_id.into(),
            state_path: Some(path),
            inner: Arc::new(StdMutex::new(persisted)),
        })
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    /// Creates a new pending connector access request bound to this host with a CSPRNG redeem token.
    pub fn create_request(&self, input: CreateAccessRequestInput) -> Result<ConnectorAccessRequest> {
        let host_id = self.host_id.clone();
        self.create_request_for_host(&host_id, input)
    }

    /// Creates a pending connector access request bound to an explicit `target_host_id`
    /// (enabling deterministic cross-host `NotThisHost` verification).
    pub fn create_request_for_host(
        &self,
        target_host_id: &str,
        input: CreateAccessRequestInput,
    ) -> Result<ConnectorAccessRequest> {
        if target_host_id.trim().is_empty()
            || input.bot_id.trim().is_empty()
            || input.member.id.trim().is_empty()
            || input.member.name.trim().is_empty()
        {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Access request requires non-empty host_id, bot_id, member.id, and member.name.",
            ));
        }
        let ttl = input.ttl_ms.unwrap_or(ACCESS_REQUEST_DEFAULT_TTL_MS);
        if ttl <= 0 {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Access request ttl_ms must be positive.",
            ));
        }
        let req = ConnectorAccessRequest {
            request_id: format!("acc_{}", Uuid::new_v4().simple()),
            host_id: target_host_id.trim().to_string(),
            bot_id: input.bot_id.trim().to_string(),
            member: input.member,
            connector_workspace_id: input.connector_workspace_id,
            status: ConnectorAccessRequestStatus::Pending,
            redeem_token: generate_csprng_redeem_token()?,
            redeem_token_used: false,
            created_at_ms: input.now_ms,
            expires_at_ms: input.now_ms.saturating_add(ttl),
            decided_at_ms: None,
            granted_not_added: false,
        };
        let mut guard = self.lock_state()?;
        guard.requests.insert(req.request_id.clone(), req.clone());
        self.persist_locked(&guard)?;
        Ok(req)
    }

    /// Lists stored access requests, optionally filtered by `bot_id` and/or `status`.
    pub fn list_requests(
        &self,
        bot_id: Option<&str>,
        status: Option<ConnectorAccessRequestStatus>,
    ) -> Result<Vec<ConnectorAccessRequest>> {
        let guard = self.lock_state()?;
        let mut out: Vec<ConnectorAccessRequest> = guard
            .requests
            .values()
            .filter(|r| bot_id.is_none_or(|b| r.bot_id == b))
            .filter(|r| status.is_none_or(|s| r.status == s))
            .cloned()
            .collect();
        out.sort_by_key(|r| (r.created_at_ms, r.request_id.clone()));
        Ok(out)
    }

    /// Approves (`approve = true`) or declines (`approve = false`) an access request.
    /// Enforces host binding (`NotThisHost`), expiry (`Expired`), member cap 200 & connector
    /// workspace cap 8 (`Full`), and cloud-first approval with `GrantedNotAdded` tracking when
    /// local member addition fails.
    pub async fn decide_request(
        &self,
        request_id: &str,
        approve: bool,
        now_ms: i64,
        controller: &BotController,
    ) -> Result<AccessDecisionOutcome> {
        let snapshot = {
            let guard = self.lock_state()?;
            guard.requests.get(request_id).cloned().ok_or_else(|| {
                OrcaBotsError::bot(
                    BotErrorCode::NotFound,
                    format!("Access request '{request_id}' not found."),
                )
            })?
        };

        if snapshot.host_id != self.host_id {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotThisHost,
                format!(
                    "Access request '{request_id}' belongs to host '{}', not '{}'.",
                    snapshot.host_id, self.host_id
                ),
            ));
        }
        if now_ms > snapshot.expires_at_ms
            && snapshot.status == ConnectorAccessRequestStatus::Pending
        {
            return Err(OrcaBotsError::bot(
                BotErrorCode::Expired,
                format!("Access request '{request_id}' has expired."),
            ));
        }
        if snapshot.status == ConnectorAccessRequestStatus::Declined && approve {
            return Err(OrcaBotsError::bot(
                BotErrorCode::Expired,
                format!("Access request '{request_id}' was already declined."),
            ));
        }
        if snapshot.status == ConnectorAccessRequestStatus::Granted && !approve {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                format!("Access request '{request_id}' was already granted."),
            ));
        }

        if !approve {
            let mut guard = self.lock_state()?;
            let entry = guard.requests.get_mut(request_id).ok_or_else(|| {
                OrcaBotsError::bot(BotErrorCode::NotFound, "Access request disappeared.")
            })?;
            entry.status = ConnectorAccessRequestStatus::Declined;
            entry.redeem_token_used = true;
            entry.decided_at_ms = Some(now_ms);
            entry.granted_not_added = false;
            let updated = entry.clone();
            self.persist_locked(&guard)?;
            return Ok(AccessDecisionOutcome {
                request: updated,
                added_locally: false,
            });
        }

        // Enforce member cap (200) and connector workspace cap (8) when the bot exists locally.
        if let Ok(bot) = controller.get_bot(&snapshot.bot_id).await {
            check_bot_access_capacity(&bot, &snapshot.member, snapshot.connector_workspace_id.as_deref())?;
        }

        // Cloud-first authoritative approval commit.
        {
            let mut guard = self.lock_state()?;
            let entry = guard.requests.get_mut(request_id).ok_or_else(|| {
                OrcaBotsError::bot(BotErrorCode::NotFound, "Access request disappeared.")
            })?;
            entry.status = ConnectorAccessRequestStatus::Granted;
            entry.redeem_token_used = true;
            entry.decided_at_ms = Some(now_ms);
            entry.granted_not_added = true;
            self.persist_locked(&guard)?;
        }

        // Attempt local idempotent member addition via BotController.
        let mut member_to_add = snapshot.member.clone();
        if member_to_add.workspace_id.is_none() {
            member_to_add.workspace_id = snapshot.connector_workspace_id.clone();
        }
        let patch = BotPatch {
            add_members: vec![member_to_add],
            ..Default::default()
        };
        match controller.update_bot(&snapshot.bot_id, patch).await {
            Ok(_) => {
                let mut guard = self.lock_state()?;
                let entry = guard.requests.get_mut(request_id).ok_or_else(|| {
                    OrcaBotsError::bot(BotErrorCode::NotFound, "Access request disappeared.")
                })?;
                entry.granted_not_added = false;
                let updated = entry.clone();
                self.persist_locked(&guard)?;
                Ok(AccessDecisionOutcome {
                    request: updated,
                    added_locally: true,
                })
            }
            Err(err) if err.code() == BotErrorCode::Full => {
                let mut guard = self.lock_state()?;
                if let Some(entry) = guard.requests.get_mut(request_id) {
                    entry.status = ConnectorAccessRequestStatus::Pending;
                    entry.redeem_token_used = false;
                    entry.decided_at_ms = None;
                    entry.granted_not_added = false;
                    let _ = self.persist_locked(&guard);
                }
                Err(err)
            }
            Err(err) => Err(OrcaBotsError::bot(
                BotErrorCode::GrantedNotAdded,
                format!(
                    "Access request '{request_id}' granted cloud-first, but local member addition failed: {err}"
                ),
            )),
        }
    }

    /// Redeems a single-use CSPRNG access token using constant-time comparison,
    /// enforcing host binding, single-use consumption, and expiry before approving.
    pub async fn redeem_token(
        &self,
        token: &str,
        now_ms: i64,
        controller: &BotController,
    ) -> Result<AccessDecisionOutcome> {
        let matched = {
            let guard = self.lock_state()?;
            guard
                .requests
                .values()
                .find(|req| constant_time_str_eq(&req.redeem_token, token))
                .cloned()
        };
        let Some(req) = matched else {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotFound,
                "Invalid or unknown access redeem token.",
            ));
        };
        if req.host_id != self.host_id {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotThisHost,
                format!(
                    "Redeem token belongs to host '{}', not '{}'.",
                    req.host_id, self.host_id
                ),
            ));
        }
        if req.redeem_token_used || req.status != ConnectorAccessRequestStatus::Pending {
            return Err(OrcaBotsError::bot(
                BotErrorCode::Expired,
                format!("Redeem token for '{}' has already been used.", req.request_id),
            ));
        }
        if now_ms > req.expires_at_ms {
            return Err(OrcaBotsError::bot(
                BotErrorCode::Expired,
                format!("Redeem token for '{}' has expired.", req.request_id),
            ));
        }
        self.decide_request(&req.request_id, true, now_ms, controller)
            .await
    }

    /// Startup recovery sweep ensuring all cloud-granted requests with `granted_not_added == true`
    /// are idempotently added to their local bot records via `BotController`.
    pub async fn recover_granted_not_added(
        &self,
        controller: &BotController,
    ) -> Result<AccessRecoveryReport> {
        let pending_recovery: Vec<ConnectorAccessRequest> = {
            let guard = self.lock_state()?;
            guard
                .requests
                .values()
                .filter(|r| {
                    r.host_id == self.host_id
                        && r.status == ConnectorAccessRequestStatus::Granted
                        && r.granted_not_added
                })
                .cloned()
                .collect()
        };

        let mut report = AccessRecoveryReport::default();
        for req in pending_recovery {
            let mut member_to_add = req.member.clone();
            if member_to_add.workspace_id.is_none() {
                member_to_add.workspace_id = req.connector_workspace_id.clone();
            }
            let patch = BotPatch {
                add_members: vec![member_to_add],
                ..Default::default()
            };
            match controller.update_bot(&req.bot_id, patch).await {
                Ok(_) => {
                    let mut guard = self.lock_state()?;
                    if let Some(entry) = guard.requests.get_mut(&req.request_id) {
                        entry.granted_not_added = false;
                        self.persist_locked(&guard)?;
                    }
                    report.recovered_request_ids.push(req.request_id);
                }
                Err(_) => {
                    report.still_pending_request_ids.push(req.request_id);
                }
            }
        }
        Ok(report)
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, AccessHostPersisted>> {
        self.inner.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "BotAccessHost mutex poisoned.")
        })
    }

    fn persist_locked(&self, state: &AccessHostPersisted) -> Result<()> {
        let Some(ref path) = self.state_path else {
            return Ok(());
        };
        let mut json = serde_json::to_string_pretty(state)?;
        json.push('\n');
        atomic_write_bytes(path, json.as_bytes())
    }
}

fn check_bot_access_capacity(
    bot: &orca_core::bot_types::OrcaBotRecord,
    incoming: &BotMember,
    connector_workspace_id: Option<&str>,
) -> Result<()> {
    let source = bot_member_source(incoming);
    let eff_workspace = incoming.workspace_id.as_deref().or(connector_workspace_id);
    let already_member = bot.members.iter().any(|m| {
        bot_member_source(m) == source
            && m.id == incoming.id
            && m.workspace_id.as_deref() == eff_workspace
    });
    if !already_member && bot.members.len() >= BOT_MAX_MEMBERS {
        return Err(OrcaBotsError::bot(
            BotErrorCode::Full,
            format!(
                "Bot '{}' already has the maximum of {BOT_MAX_MEMBERS} members.",
                bot.name.as_str()
            ),
        ));
    }

    if let Some(ws) = eff_workspace.filter(|w| !w.trim().is_empty()) {
        let mut workspaces: HashSet<String> = HashSet::new();
        for m in &bot.members {
            if bot_member_source(m) == source
                && let Some(ref existing_ws) = m.workspace_id
                && !existing_ws.trim().is_empty()
            {
                workspaces.insert(existing_ws.clone());
            }
        }
        if source == "slack" {
            for link in bot.connectors.effective_slack_apps() {
                if let Some(ref team) = link.team_id {
                    workspaces.insert(team.clone());
                } else {
                    workspaces.insert(link.app_id.clone());
                }
            }
        } else if source == "linear" {
            for link in bot.connectors.effective_linear_workspaces() {
                workspaces.insert(link.workspace_id.clone());
            }
        }
        if !workspaces.contains(ws) && workspaces.len() >= BOT_MAX_CONNECTOR_WORKSPACES {
            return Err(OrcaBotsError::bot(
                BotErrorCode::Full,
                format!(
                    "Bot '{}' already reached the maximum of {BOT_MAX_CONNECTOR_WORKSPACES} '{source}' workspaces.",
                    bot.name.as_str()
                ),
            ));
        }
    }
    Ok(())
}

fn generate_csprng_redeem_token() -> Result<String> {
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|err| {
        OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            format!("CSPRNG failed generating access redeem token: {err}"),
        )
    })?;
    Ok(format!("orca_acc_{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn constant_time_str_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    bool::from(ab.ct_eq(bb))
}

// ---------------------------------------------------------------------------
// 3. SlackMirror (Genuine Async Turn-End Ordering Barrier)
// ---------------------------------------------------------------------------

/// Lifecycle state of a mirrored owner user message in a Slack-linked chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SlackMirrorEntryState {
    Posting,
    Posted { slack_ts: String },
    Failed { error: String },
    Dismissed,
    Deleted,
}

impl SlackMirrorEntryState {
    pub const fn is_in_flight(&self) -> bool {
        matches!(self, Self::Posting)
    }
}

/// A single owner-typed message mirrored into a Slack thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackMirrorEntry {
    pub entry_id: String,
    pub session_id: String,
    pub channel_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    pub text: String,
    pub state: SlackMirrorEntryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_slack_ts: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Configuration for a Slack-linked chat session participating in Slack Mirror.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackLinkedChat {
    pub session_id: String,
    pub channel_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    pub opened_by_owner: bool,
    /// `None` or `Some(true)` enables mirroring on owner-opened chats (`slack_mirror != false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slack_mirror: Option<bool>,
}

impl SlackLinkedChat {
    pub const fn mirroring_enabled(&self) -> bool {
        self.opened_by_owner && !matches!(self.slack_mirror, Some(false))
    }
}

#[derive(Debug, Default)]
struct SlackMirrorState {
    chats: HashMap<String, SlackLinkedChat>,
    entries: BTreeMap<String, SlackMirrorEntry>,
    notifiers: HashMap<String, Arc<Notify>>,
}

impl SlackMirrorState {
    fn session_notify(&mut self, session_id: &str) -> Arc<Notify> {
        self.notifiers
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    fn has_in_flight_posting(&self, session_id: &str) -> bool {
        self.entries
            .values()
            .any(|e| e.session_id == session_id && e.state.is_in_flight())
    }
}

/// Async Slack Mirror coordinator holding outbound turn replies until all in-flight
/// `Posting` mirrored owner messages for the session settle.
#[derive(Debug, Clone, Default)]
pub struct SlackMirror {
    inner: Arc<AsyncMutex<SlackMirrorState>>,
}

impl SlackMirror {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or updates a Slack-linked chat session.
    pub async fn register_chat(&self, chat: SlackLinkedChat) -> Result<()> {
        if chat.session_id.trim().is_empty() || chat.channel_id.trim().is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "SlackLinkedChat requires non-empty session_id and channel_id.",
            ));
        }
        let mut guard = self.inner.lock().await;
        guard.session_notify(&chat.session_id);
        guard.chats.insert(chat.session_id.clone(), chat);
        Ok(())
    }

    /// Returns `true` iff `session_id` is an owner-opened Slack-linked chat with `slack_mirror != false`.
    pub async fn should_mirror(&self, session_id: &str) -> bool {
        let guard = self.inner.lock().await;
        guard
            .chats
            .get(session_id)
            .is_some_and(SlackLinkedChat::mirroring_enabled)
    }

    /// Begins mirroring a user message to Slack in state `Posting`, returning `Ok(None)` when
    /// mirroring is disabled or chat is not owner-opened.
    pub async fn begin_mirror_post(
        &self,
        session_id: &str,
        text: impl Into<String>,
        now_ms: i64,
    ) -> Result<Option<SlackMirrorEntry>> {
        let mut guard = self.inner.lock().await;
        let Some(chat) = guard.chats.get(session_id).cloned() else {
            return Ok(None);
        };
        if !chat.mirroring_enabled() {
            return Ok(None);
        }
        let entry = SlackMirrorEntry {
            entry_id: format!("sm_{}", Uuid::new_v4().simple()),
            session_id: chat.session_id.clone(),
            channel_id: chat.channel_id,
            thread_ts: chat.thread_ts,
            text: text.into(),
            state: SlackMirrorEntryState::Posting,
            last_slack_ts: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        guard.session_notify(&chat.session_id);
        guard.entries.insert(entry.entry_id.clone(), entry.clone());
        Ok(Some(entry))
    }

    /// Marks an in-flight mirrored message as `Posted` with its Slack `ts` and wakes waiting turn-ends.
    pub async fn mark_posted(
        &self,
        entry_id: &str,
        slack_ts: impl Into<String>,
        now_ms: i64,
    ) -> Result<SlackMirrorEntry> {
        let ts = slack_ts.into();
        if ts.trim().is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Slack ts must be non-empty.",
            ));
        }
        let (updated, notify) = {
            let mut guard = self.inner.lock().await;
            let entry = guard.entries.get_mut(entry_id).ok_or_else(|| {
                OrcaBotsError::bot(
                    BotErrorCode::NotFound,
                    format!("Slack mirror entry '{entry_id}' not found."),
                )
            })?;
            entry.state = SlackMirrorEntryState::Posted {
                slack_ts: ts.clone(),
            };
            entry.last_slack_ts = Some(ts);
            entry.updated_at_ms = now_ms;
            let sid = entry.session_id.clone();
            let cloned = entry.clone();
            let notify = guard.session_notify(&sid);
            (cloned, notify)
        };
        notify.notify_waiters();
        Ok(updated)
    }

    /// Marks a mirrored message as `Failed` and wakes waiting turn-ends so the turn does not deadlock.
    pub async fn mark_failed(
        &self,
        entry_id: &str,
        error: impl Into<String>,
        now_ms: i64,
    ) -> Result<SlackMirrorEntry> {
        let (updated, notify) = {
            let mut guard = self.inner.lock().await;
            let entry = guard.entries.get_mut(entry_id).ok_or_else(|| {
                OrcaBotsError::bot(
                    BotErrorCode::NotFound,
                    format!("Slack mirror entry '{entry_id}' not found."),
                )
            })?;
            entry.state = SlackMirrorEntryState::Failed {
                error: error.into(),
            };
            entry.updated_at_ms = now_ms;
            let sid = entry.session_id.clone();
            let cloned = entry.clone();
            let notify = guard.session_notify(&sid);
            (cloned, notify)
        };
        notify.notify_waiters();
        Ok(updated)
    }

    /// Transitions a `Failed` mirror entry back to `Posting` for retry.
    pub async fn retry_post(&self, entry_id: &str, now_ms: i64) -> Result<SlackMirrorEntry> {
        let mut guard = self.inner.lock().await;
        let entry = guard.entries.get_mut(entry_id).ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Slack mirror entry '{entry_id}' not found."),
            )
        })?;
        if !matches!(entry.state, SlackMirrorEntryState::Failed { .. }) {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                format!("Cannot retry mirror entry '{entry_id}' because it is not in Failed state."),
            ));
        }
        entry.state = SlackMirrorEntryState::Posting;
        entry.updated_at_ms = now_ms;
        Ok(entry.clone())
    }

    /// Dismisses a `Posting` or `Failed` mirror entry and wakes waiting turn-ends.
    pub async fn dismiss_post(&self, entry_id: &str, now_ms: i64) -> Result<SlackMirrorEntry> {
        let (updated, notify) = {
            let mut guard = self.inner.lock().await;
            let entry = guard.entries.get_mut(entry_id).ok_or_else(|| {
                OrcaBotsError::bot(
                    BotErrorCode::NotFound,
                    format!("Slack mirror entry '{entry_id}' not found."),
                )
            })?;
            entry.state = SlackMirrorEntryState::Dismissed;
            entry.updated_at_ms = now_ms;
            let sid = entry.session_id.clone();
            let cloned = entry.clone();
            let notify = guard.session_notify(&sid);
            (cloned, notify)
        };
        notify.notify_waiters();
        Ok(updated)
    }

    /// Marks a mirror entry as `Deleted` from Slack (preserving `last_slack_ts` for upstream deletion)
    /// and wakes waiting turn-ends if it was still posting.
    pub async fn delete_from_slack(&self, entry_id: &str, now_ms: i64) -> Result<SlackMirrorEntry> {
        let (updated, notify) = {
            let mut guard = self.inner.lock().await;
            let entry = guard.entries.get_mut(entry_id).ok_or_else(|| {
                OrcaBotsError::bot(
                    BotErrorCode::NotFound,
                    format!("Slack mirror entry '{entry_id}' not found."),
                )
            })?;
            entry.state = SlackMirrorEntryState::Deleted;
            entry.updated_at_ms = now_ms;
            let sid = entry.session_id.clone();
            let cloned = entry.clone();
            let notify = guard.session_notify(&sid);
            (cloned, notify)
        };
        notify.notify_waiters();
        Ok(updated)
    }

    /// Lists all mirror entries tracked for `session_id` ordered by creation timestamp.
    pub async fn list_entries(&self, session_id: &str) -> Vec<SlackMirrorEntry> {
        let guard = self.inner.lock().await;
        let mut out: Vec<SlackMirrorEntry> = guard
            .entries
            .values()
            .filter(|e| e.session_id == session_id)
            .cloned()
            .collect();
        out.sort_by_key(|e| (e.created_at_ms, e.entry_id.clone()));
        out
    }

    /// Returns the number of currently in-flight `Posting` mirror entries for `session_id`.
    pub async fn in_flight_count(&self, session_id: &str) -> usize {
        let guard = self.inner.lock().await;
        guard
            .entries
            .values()
            .filter(|e| e.session_id == session_id && e.state.is_in_flight())
            .count()
    }

    /// Genuine async turn-end ordering barrier: holds execution of `send_reply` until ALL
    /// in-flight `Posting` mirrored messages for `session_id` have settled (`Posted`, `Failed`,
    /// `Dismissed`, or `Deleted`), preventing Slack from showing an answer before the question.
    pub async fn with_held_turn_end<F, Fut, T>(
        &self,
        session_id: &str,
        send_reply: F,
    ) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let notify = {
            let mut guard = self.inner.lock().await;
            guard.session_notify(session_id)
        };
        loop {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let guard = self.inner.lock().await;
                if !guard.has_in_flight_posting(session_id) {
                    break;
                }
            }
            notified.await;
        }
        send_reply().await
    }
}

// ---------------------------------------------------------------------------
// 4. WebhookEngine (6 adapters, 6 filter ops, 200/300 caps, zero-leak, spool)
// ---------------------------------------------------------------------------

/// Secret-free CLI view of a webhook (`fabushi webhooks list/show`).
/// STRUCTURAL INVARIANT: NEVER contains ingest URL or secret key!
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookCliView {
    pub hook_id: String,
    pub bot_id: String,
    pub automation_id: String,
    pub adapter: WebhookAdapterId,
    pub rev: u32,
    pub epoch: u64,
    pub paused: bool,
    pub filters: WebhookFilters,
    pub hourly_cap: u32,
    pub require_signature: bool,
    pub created_at_ms: i64,
}

impl WebhookCliView {
    pub fn to_cli_summary(&self) -> WebhookCliSummary {
        WebhookCliSummary {
            hook_id: self.hook_id.clone(),
            adapter: self.adapter,
            rev: self.rev,
            epoch: self.epoch,
            paused: self.paused,
            filters: self.filters.clone(),
        }
    }
}

/// UI-only host card revealing the ingest URL and signing secret for one-time copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookCopyUrlCard {
    pub hook_id: String,
    pub ingest_url: String,
    pub secret_key: String,
    pub adapter: WebhookAdapterId,
    pub epoch: u64,
}

/// Input parameters for registering a new host-managed webhook.
#[derive(Debug, Clone)]
pub struct CreateWebhookRequest {
    pub bot_id: String,
    pub automation_id: String,
    pub adapter: WebhookAdapterId,
    pub filters: WebhookFilters,
    pub hourly_cap: Option<u32>,
    pub grace_minutes: Option<u32>,
    pub require_signature: bool,
    pub now_ms: i64,
}

/// Inbound HTTP webhook delivery envelope evaluated by `WebhookEngine::ingest_call`.
#[derive(Debug, Clone)]
pub struct IngestWebhookCallRequest {
    pub hook_id: String,
    pub headers: BTreeMap<String, String>,
    pub raw_body: Vec<u8>,
    pub delivery_epoch: Option<u64>,
    pub idempotency_key: Option<String>,
    pub now_ms: u64,
}

/// Tracked call record spanning all 14 `WebhookCallState` lifecycle states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookCallRecord {
    pub call_id: String,
    pub hook_id: String,
    pub bot_id: String,
    pub adapter: WebhookAdapterId,
    pub epoch: u64,
    pub state: WebhookCallState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spool_path: Option<PathBuf>,
    pub received_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Raw inbound webhook payload persisted strictly to a local host spool file (`<spool_dir>/<call_id>.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpooledWebhookPayload {
    pub call_id: String,
    pub hook_id: String,
    pub adapter: WebhookAdapterId,
    pub received_at_ms: u64,
    pub headers: BTreeMap<String, String>,
    pub body: JsonValue,
}

/// Outcome of processing an inbound webhook call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookIngestOutcome {
    pub call_id: String,
    pub receipt_status: WebhookReceiptStatus,
    pub state: WebhookCallState,
    pub start_answer: Option<WebhookCallStartAnswer>,
    pub trigger_event: Option<WebhookTriggerEvent>,
    pub spool_path: Option<PathBuf>,
    pub notice: Option<WebhookNotice>,
}

#[derive(Debug, Clone)]
struct StoredWebhook {
    hook_id: String,
    bot_id: String,
    automation_id: String,
    adapter: WebhookAdapterId,
    rev: u32,
    epoch: u64,
    paused: bool,
    filters: WebhookFilters,
    secret_key: String,
    ingest_url: String,
    grace_minutes: Option<u32>,
    require_signature: bool,
    created_at_ms: i64,
}

impl StoredWebhook {
    fn to_cli_view(&self, hourly_cap: u32) -> WebhookCliView {
        WebhookCliView {
            hook_id: self.hook_id.clone(),
            bot_id: self.bot_id.clone(),
            automation_id: self.automation_id.clone(),
            adapter: self.adapter,
            rev: self.rev,
            epoch: self.epoch,
            paused: self.paused,
            filters: self.filters.clone(),
            hourly_cap,
            require_signature: self.require_signature,
            created_at_ms: self.created_at_ms,
        }
    }

    fn to_copy_url_card(&self) -> WebhookCopyUrlCard {
        WebhookCopyUrlCard {
            hook_id: self.hook_id.clone(),
            ingest_url: self.ingest_url.clone(),
            secret_key: self.secret_key.clone(),
            adapter: self.adapter,
            epoch: self.epoch,
        }
    }
}

#[derive(Debug, Default)]
struct WebhookEngineState {
    hooks: BTreeMap<String, StoredWebhook>,
    calls: BTreeMap<String, WebhookCallRecord>,
    seen_idempotency: HashMap<(String, String), String>,
    cap_tracker: WebhookHourlyCapTracker,
    host_gen: u64,
}

/// Host-side webhook engine supporting all 6 adapters, 6 JSON-path filter ops, rolling-hour caps
/// (200/hook, 300/account), HMAC-SHA256 verification, local payload spooling, 14 call states,
/// epoch fencing, secret-free CLI views, and zero-leak `WebhookTriggerEvent` frames.
#[derive(Debug, Clone)]
pub struct WebhookEngine {
    spool_dir: PathBuf,
    base_ingest_url: String,
    inner: Arc<StdMutex<WebhookEngineState>>,
}

impl WebhookEngine {
    /// Creates a `WebhookEngine` spooling raw payloads under `spool_dir`.
    pub fn new(spool_dir: impl Into<PathBuf>, base_ingest_url: impl Into<String>) -> Result<Self> {
        let spool_dir = spool_dir.into();
        fs::create_dir_all(&spool_dir)?;
        let base_ingest_url = base_ingest_url.into().trim_end_matches('/').to_string();
        if base_ingest_url.is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "WebhookEngine base_ingest_url must be non-empty.",
            ));
        }
        Ok(Self {
            spool_dir,
            base_ingest_url,
            inner: Arc::new(StdMutex::new(WebhookEngineState {
                host_gen: 1,
                ..Default::default()
            })),
        })
    }

    pub fn spool_dir(&self) -> &Path {
        &self.spool_dir
    }

    /// Creates a new webhook with CSPRNG hook id & signing secret, validating filter bounds
    /// (`<=8 where`, `<=10 values`, `<=12 pick`, `<=2560 rule bytes`).
    pub fn create_hook(
        &self,
        req: CreateWebhookRequest,
    ) -> Result<(WebhookCliView, WebhookCopyUrlCard)> {
        if req.bot_id.trim().is_empty() || req.automation_id.trim().is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Webhook requires non-empty bot_id and automation_id.",
            ));
        }
        validate_webhook_filters(&req.filters)?;

        let hook_id = generate_webhook_hook_id()?;
        let rule_id = webhook_rule_id(&hook_id)?;
        let rule_wire = WebhookRuleWire {
            id: rule_id,
            automation_id: req.automation_id.clone(),
            bot_id: req.bot_id.clone(),
            source: AutomationEventSource::Webhook,
            event: WebhookEventKind::Call,
            rev: 1,
            filters: req.filters.clone(),
            grace_minutes: req.grace_minutes,
        };
        validate_webhook_rule_bytes(&rule_wire)?;

        let secret_key = generate_webhook_secret_key()?;
        let ingest_url = format!("{}{WEBHOOK_PATH_PREFIX}{hook_id}", self.base_ingest_url);

        let mut guard = self.lock_state()?;
        if let Some(cap) = req.hourly_cap {
            guard.cap_tracker.set_hook_cap(&hook_id, cap)?;
        }
        let effective_cap = guard.cap_tracker.effective_hook_cap(&hook_id);
        let stored = StoredWebhook {
            hook_id: hook_id.clone(),
            bot_id: req.bot_id,
            automation_id: req.automation_id,
            adapter: req.adapter,
            rev: 1,
            epoch: 1,
            paused: false,
            filters: req.filters,
            secret_key,
            ingest_url,
            grace_minutes: req.grace_minutes,
            require_signature: req.require_signature,
            created_at_ms: req.now_ms,
        };
        let cli_view = stored.to_cli_view(effective_cap);
        let copy_card = stored.to_copy_url_card();
        guard.hooks.insert(hook_id, stored);
        Ok((cli_view, copy_card))
    }

    /// Updates webhook JSON-path filters, re-checking `<= 2560` rule byte cap and advancing `rev` & `epoch`.
    pub fn update_hook_filters(
        &self,
        hook_id: &str,
        filters: WebhookFilters,
    ) -> Result<WebhookCliView> {
        validate_webhook_filters(&filters)?;
        let mut guard = self.lock_state()?;
        let existing = guard.hooks.get(hook_id).cloned().ok_or_else(|| {
            OrcaBotsError::bot(BotErrorCode::NotFound, format!("Webhook '{hook_id}' not found."))
        })?;
        let next_rev = existing.rev.saturating_add(1);
        let rule_wire = WebhookRuleWire {
            id: webhook_rule_id(hook_id)?,
            automation_id: existing.automation_id.clone(),
            bot_id: existing.bot_id.clone(),
            source: AutomationEventSource::Webhook,
            event: WebhookEventKind::Call,
            rev: next_rev,
            filters: filters.clone(),
            grace_minutes: existing.grace_minutes,
        };
        validate_webhook_rule_bytes(&rule_wire)?;

        let cap = guard.cap_tracker.effective_hook_cap(hook_id);
        let hook = guard.hooks.get_mut(hook_id).ok_or_else(|| {
            OrcaBotsError::bot(BotErrorCode::NotFound, format!("Webhook '{hook_id}' not found."))
        })?;
        hook.filters = filters;
        hook.rev = next_rev;
        hook.epoch = advance_webhook_epoch(hook.epoch);
        Ok(hook.to_cli_view(cap))
    }

    /// Pauses or resumes a webhook, advancing its epoch when paused.
    pub fn set_hook_paused(&self, hook_id: &str, paused: bool) -> Result<WebhookCliView> {
        let mut guard = self.lock_state()?;
        let cap = guard.cap_tracker.effective_hook_cap(hook_id);
        let hook = guard.hooks.get_mut(hook_id).ok_or_else(|| {
            OrcaBotsError::bot(BotErrorCode::NotFound, format!("Webhook '{hook_id}' not found."))
        })?;
        hook.paused = paused;
        if paused {
            hook.epoch = advance_webhook_epoch(hook.epoch);
        }
        Ok(hook.to_cli_view(cap))
    }

    /// Rotates a webhook's CSPRNG signing secret and advances its `epoch` to fence stale deliveries.
    pub fn rotate_hook_secret(&self, hook_id: &str) -> Result<WebhookCopyUrlCard> {
        let next_secret = generate_webhook_secret_key()?;
        let mut guard = self.lock_state()?;
        let hook = guard.hooks.get_mut(hook_id).ok_or_else(|| {
            OrcaBotsError::bot(BotErrorCode::NotFound, format!("Webhook '{hook_id}' not found."))
        })?;
        hook.secret_key = next_secret;
        hook.epoch = advance_webhook_epoch(hook.epoch);
        Ok(hook.to_copy_url_card())
    }

    /// Lists secret-free `WebhookCliView` records (NEVER exposes URL or signing secret).
    pub fn list_cli_views(&self, bot_id: Option<&str>) -> Result<Vec<WebhookCliView>> {
        let guard = self.lock_state()?;
        let out = guard
            .hooks
            .values()
            .filter(|h| bot_id.is_none_or(|b| h.bot_id == b))
            .map(|h| h.to_cli_view(guard.cap_tracker.effective_hook_cap(&h.hook_id)))
            .collect();
        Ok(out)
    }

    /// Shows a single secret-free `WebhookCliView` (NEVER exposes URL or signing secret).
    pub fn show_cli_view(&self, hook_id: &str) -> Result<WebhookCliView> {
        let guard = self.lock_state()?;
        let hook = guard.hooks.get(hook_id).ok_or_else(|| {
            OrcaBotsError::bot(BotErrorCode::NotFound, format!("Webhook '{hook_id}' not found."))
        })?;
        Ok(hook.to_cli_view(guard.cap_tracker.effective_hook_cap(hook_id)))
    }

    /// Ingests an external webhook call: checks epoch fencing, envelope size, HMAC-SHA256 adapter
    /// signature, pause status, JSON-path filter match, deduplication, rolling-hour 200/300 caps,
    /// spools raw payload locally, and emits a strictly zero-leak `WebhookTriggerEvent`.
    pub fn ingest_call(&self, req: IngestWebhookCallRequest) -> Result<WebhookIngestOutcome> {
        if req.raw_body.len() > WEBHOOK_ENVELOPE_MAX_BODY_BYTES {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                format!(
                    "Webhook payload exceeds max size ({WEBHOOK_ENVELOPE_MAX_BODY_BYTES} bytes)."
                ),
            ));
        }
        let mut normalized_headers = BTreeMap::new();
        for (k, v) in &req.headers {
            if v.len() > WEBHOOK_ENVELOPE_MAX_HEADER_VALUE_LEN {
                return Err(OrcaBotsError::bot(
                    BotErrorCode::InvalidArgument,
                    format!(
                        "Webhook header '{k}' exceeds max length ({WEBHOOK_ENVELOPE_MAX_HEADER_VALUE_LEN})."
                    ),
                ));
            }
            normalized_headers.insert(k.to_ascii_lowercase(), v.clone());
        }

        let call_id = generate_webhook_call_id()?;
        let mut guard = self.lock_state()?;
        let hook = guard.hooks.get(&req.hook_id).cloned().ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Webhook '{}' not found.", req.hook_id),
            )
        })?;

        if let Some(delivery_epoch) = req.delivery_epoch {
            check_webhook_epoch(delivery_epoch, hook.epoch)?;
        }

        // Verify HMAC-SHA256 adapter signature when required or when a signature header is present.
        if (hook.require_signature || has_adapter_signature_header(hook.adapter, &normalized_headers))
            && !verify_adapter_signature(
                hook.adapter,
                &hook.secret_key,
                &normalized_headers,
                &req.raw_body,
            )?
        {
            let notice =
                guard
                    .cap_tracker
                    .record_refusal(&hook.hook_id, &hook.bot_id, req.now_ms);
            let rec = WebhookCallRecord {
                call_id: call_id.clone(),
                hook_id: hook.hook_id,
                bot_id: hook.bot_id,
                adapter: hook.adapter,
                epoch: hook.epoch,
                state: WebhookCallState::Refused,
                spool_path: None,
                received_at_ms: req.now_ms,
                updated_at_ms: req.now_ms,
            };
            guard.calls.insert(call_id.clone(), rec);
            return Ok(WebhookIngestOutcome {
                call_id,
                receipt_status: WebhookReceiptStatus::Ignored,
                state: WebhookCallState::Refused,
                start_answer: None,
                trigger_event: None,
                spool_path: None,
                notice,
            });
        }

        if hook.paused {
            let rec = WebhookCallRecord {
                call_id: call_id.clone(),
                hook_id: hook.hook_id,
                bot_id: hook.bot_id,
                adapter: hook.adapter,
                epoch: hook.epoch,
                state: WebhookCallState::Paused,
                spool_path: None,
                received_at_ms: req.now_ms,
                updated_at_ms: req.now_ms,
            };
            guard.calls.insert(call_id.clone(), rec);
            return Ok(WebhookIngestOutcome {
                call_id,
                receipt_status: WebhookReceiptStatus::Paused,
                state: WebhookCallState::Paused,
                start_answer: None,
                trigger_event: None,
                spool_path: None,
                notice: None,
            });
        }

        if let Some(ref idem_key) = req.idempotency_key
            && let Some(prev_call_id) = guard
                .seen_idempotency
                .get(&(hook.hook_id.clone(), idem_key.clone()))
                .cloned()
        {
            return Ok(WebhookIngestOutcome {
                call_id: prev_call_id,
                receipt_status: WebhookReceiptStatus::Duplicate,
                state: WebhookCallState::Folded,
                start_answer: None,
                trigger_event: None,
                spool_path: None,
                notice: None,
            });
        }

        let body: JsonValue = if req.raw_body.is_empty() {
            JsonValue::Object(serde_json::Map::new())
        } else {
            serde_json::from_slice(&req.raw_body)?
        };
        if !webhook_body_fits(&body) {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Webhook JSON payload exceeds depth/node structural bounds.",
            ));
        }

        let view = WebhookCallView {
            headers: &normalized_headers,
            body: &body,
        };
        if !matches_webhook_call(&hook.filters, &view) {
            let rec = WebhookCallRecord {
                call_id: call_id.clone(),
                hook_id: hook.hook_id,
                bot_id: hook.bot_id,
                adapter: hook.adapter,
                epoch: hook.epoch,
                state: WebhookCallState::Ignored,
                spool_path: None,
                received_at_ms: req.now_ms,
                updated_at_ms: req.now_ms,
            };
            guard.calls.insert(call_id.clone(), rec);
            return Ok(WebhookIngestOutcome {
                call_id,
                receipt_status: WebhookReceiptStatus::Ignored,
                state: WebhookCallState::Ignored,
                start_answer: None,
                trigger_event: None,
                spool_path: None,
                notice: None,
            });
        }

        match guard
            .cap_tracker
            .try_admit_start(&hook.hook_id, &hook.bot_id, req.now_ms)
        {
            WebhookCapDecision::Capped {
                retry_at_ms,
                notice,
                ..
            } => {
                let rec = WebhookCallRecord {
                    call_id: call_id.clone(),
                    hook_id: hook.hook_id,
                    bot_id: hook.bot_id,
                    adapter: hook.adapter,
                    epoch: hook.epoch,
                    state: WebhookCallState::WaitingCapacity,
                    spool_path: None,
                    received_at_ms: req.now_ms,
                    updated_at_ms: req.now_ms,
                };
                guard.calls.insert(call_id.clone(), rec);
                return Ok(WebhookIngestOutcome {
                    call_id,
                    receipt_status: WebhookReceiptStatus::Queued,
                    state: WebhookCallState::WaitingCapacity,
                    start_answer: Some(WebhookCallStartAnswer::WaitingCapacity {
                        retry_at: retry_at_ms,
                    }),
                    trigger_event: None,
                    spool_path: None,
                    notice,
                });
            }
            WebhookCapDecision::Allowed { .. } => {}
        }

        // Write raw webhook payload to a dedicated local spool file (`<spool_dir>/<call_id>.json`).
        let spool_path = self.spool_dir.join(format!("{call_id}.json"));
        let spooled = SpooledWebhookPayload {
            call_id: call_id.clone(),
            hook_id: hook.hook_id.clone(),
            adapter: hook.adapter,
            received_at_ms: req.now_ms,
            headers: normalized_headers.clone(),
            body: body.clone(),
        };
        let spooled_bytes = serde_json::to_vec_pretty(&spooled)?;
        atomic_write_bytes(&spool_path, &spooled_bytes)?;

        // Derive optional subject and redact any embedded webhook keys so zero secrets leak.
        let raw_subject = hook
            .filters
            .subject
            .clone()
            .or_else(|| webhook_call_subject(hook.filters.subject_from.as_deref(), &view));
        let safe_subject = raw_subject
            .map(|s| redact_webhook_keys(&s))
            .filter(|s| !s.trim().is_empty());

        let event = WebhookTriggerEvent {
            key: call_id.clone(),
            actor_id: webhook_actor_id(&hook.hook_id)?,
            subject: safe_subject,
            occurred_at: req.now_ms,
            webhook: WebhookCallFields {
                call_id: call_id.clone(),
                epoch: hook.epoch,
                host_gen: Some(guard.host_gen),
                sample: None,
            },
            adapter: Some(hook.adapter),
            matched_rule_id: Some(webhook_rule_id(&hook.hook_id)?),
        };
        event.validate()?;

        if let Some(idem_key) = req.idempotency_key {
            guard
                .seen_idempotency
                .insert((hook.hook_id.clone(), idem_key), call_id.clone());
        }
        let rec = WebhookCallRecord {
            call_id: call_id.clone(),
            hook_id: hook.hook_id,
            bot_id: hook.bot_id,
            adapter: hook.adapter,
            epoch: hook.epoch,
            state: WebhookCallState::Delivered,
            spool_path: Some(spool_path.clone()),
            received_at_ms: req.now_ms,
            updated_at_ms: req.now_ms,
        };
        guard.calls.insert(call_id.clone(), rec);
        Ok(WebhookIngestOutcome {
            call_id,
            receipt_status: WebhookReceiptStatus::Queued,
            state: WebhookCallState::Delivered,
            start_answer: Some(WebhookCallStartAnswer::Started { run_id: None }),
            trigger_event: Some(event),
            spool_path: Some(spool_path),
            notice: None,
        })
    }

    /// Transitions a tracked webhook call across the 14-state machine using
    /// `orca_core::triggers_webhooks::can_transition_webhook_call_state`.
    pub fn transition_call_state(
        &self,
        call_id: &str,
        to: WebhookCallState,
        now_ms: u64,
    ) -> Result<WebhookCallRecord> {
        let mut guard = self.lock_state()?;
        let rec = guard.calls.get_mut(call_id).ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Webhook call '{call_id}' not found."),
            )
        })?;
        if !can_transition_webhook_call_state(rec.state, to) {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                format!(
                    "Illegal webhook call transition from '{}' to '{}'.",
                    rec.state.as_str(),
                    to.as_str()
                ),
            ));
        }
        rec.state = to;
        rec.updated_at_ms = now_ms;
        Ok(rec.clone())
    }

    /// Fetches a webhook call record by `call_id`.
    pub fn get_call(&self, call_id: &str) -> Result<WebhookCallRecord> {
        let guard = self.lock_state()?;
        guard.calls.get(call_id).cloned().ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Webhook call '{call_id}' not found."),
            )
        })
    }

    /// Reads the locally spooled raw webhook envelope for `call_id`.
    pub fn read_spooled_payload(&self, call_id: &str) -> Result<SpooledWebhookPayload> {
        if !is_valid_webhook_call_id(call_id) {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                format!("Invalid webhook call_id '{call_id}'."),
            ));
        }
        let path = self.spool_dir.join(format!("{call_id}.json"));
        let raw = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, WebhookEngineState>> {
        self.inner.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "WebhookEngine mutex poisoned.")
        })
    }
}

/// Computes an HMAC-SHA256 lowercase hex digest using `hmac` + `sha2`.
pub fn compute_hmac_sha256_hex(secret: &str, payload: &[u8]) -> Result<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|err| {
        OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            format!("Failed initializing HMAC-SHA256 key: {err}"),
        )
    })?;
    mac.update(payload);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn has_adapter_signature_header(
    adapter: WebhookAdapterId,
    headers: &BTreeMap<String, String>,
) -> bool {
    match adapter {
        WebhookAdapterId::Standard => {
            headers.contains_key("x-orca-signature-256") || headers.contains_key("x-orca-webhook-key")
        }
        WebhookAdapterId::Github => headers.contains_key("x-hub-signature-256"),
        WebhookAdapterId::Sentry => headers.contains_key("sentry-hook-signature"),
        WebhookAdapterId::Linear => headers.contains_key("linear-signature"),
        WebhookAdapterId::Pagerduty => headers.contains_key("x-pagerduty-signature"),
        WebhookAdapterId::Stripe => headers.contains_key("stripe-signature"),
    }
}

/// Verifies HMAC-SHA256 adapter signatures across all 6 supported webhook adapters
/// using `hmac` + `sha2` + `subtle::ConstantTimeEq`.
pub fn verify_adapter_signature(
    adapter: WebhookAdapterId,
    secret: &str,
    headers: &BTreeMap<String, String>,
    raw_body: &[u8],
) -> Result<bool> {
    match adapter {
        WebhookAdapterId::Standard => {
            if let Some(sig) = headers.get("x-orca-signature-256") {
                let Some(hex_part) = sig.strip_prefix("sha256=") else {
                    return Ok(false);
                };
                let expected = compute_hmac_sha256_hex(secret, raw_body)?;
                return Ok(constant_time_hex_eq(hex_part, &expected));
            }
            if let Some(raw_key) = headers.get("x-orca-webhook-key") {
                return Ok(constant_time_str_eq(raw_key.trim(), secret));
            }
            Ok(false)
        }
        WebhookAdapterId::Github => {
            let Some(sig) = headers.get("x-hub-signature-256") else {
                return Ok(false);
            };
            let Some(hex_part) = sig.strip_prefix("sha256=") else {
                return Ok(false);
            };
            let expected = compute_hmac_sha256_hex(secret, raw_body)?;
            Ok(constant_time_hex_eq(hex_part, &expected))
        }
        WebhookAdapterId::Sentry => {
            let Some(sig) = headers.get("sentry-hook-signature") else {
                return Ok(false);
            };
            let expected = compute_hmac_sha256_hex(secret, raw_body)?;
            Ok(constant_time_hex_eq(sig.trim(), &expected))
        }
        WebhookAdapterId::Linear => {
            let Some(sig) = headers.get("linear-signature") else {
                return Ok(false);
            };
            let expected = compute_hmac_sha256_hex(secret, raw_body)?;
            Ok(constant_time_hex_eq(sig.trim(), &expected))
        }
        WebhookAdapterId::Pagerduty => {
            let Some(sig_header) = headers.get("x-pagerduty-signature") else {
                return Ok(false);
            };
            let expected = compute_hmac_sha256_hex(secret, raw_body)?;
            let matched = sig_header
                .split(',')
                .filter_map(|part| part.trim().strip_prefix("v1="))
                .any(|candidate| constant_time_hex_eq(candidate, &expected));
            Ok(matched)
        }
        WebhookAdapterId::Stripe => {
            let Some(sig_header) = headers.get("stripe-signature") else {
                return Ok(false);
            };
            let mut timestamp: Option<&str> = None;
            let mut v1_sigs = Vec::new();
            for part in sig_header.split(',') {
                let trimmed = part.trim();
                if let Some(t) = trimmed.strip_prefix("t=") {
                    timestamp = Some(t);
                } else if let Some(v1) = trimmed.strip_prefix("v1=") {
                    v1_sigs.push(v1);
                }
            }
            let Some(ts) = timestamp.filter(|t| !t.is_empty()) else {
                return Ok(false);
            };
            if v1_sigs.is_empty() {
                return Ok(false);
            }
            let mut signed_payload = Vec::with_capacity(ts.len() + 1 + raw_body.len());
            signed_payload.extend_from_slice(ts.as_bytes());
            signed_payload.push(b'.');
            signed_payload.extend_from_slice(raw_body);
            let expected = compute_hmac_sha256_hex(secret, &signed_payload)?;
            Ok(v1_sigs
                .into_iter()
                .any(|candidate| constant_time_hex_eq(candidate, &expected)))
        }
    }
}

fn constant_time_hex_eq(candidate: &str, expected_lower_hex: &str) -> bool {
    let lower = candidate.trim().to_ascii_lowercase();
    constant_time_str_eq(&lower, expected_lower_hex)
}

fn generate_csprng_alnum(len: usize) -> Result<String> {
    let mut raw = vec![0u8; len * 2];
    let mut out = String::with_capacity(len);
    while out.len() < len {
        getrandom::getrandom(&mut raw).map_err(|err| {
            OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                format!("CSPRNG failure generating webhook token: {err}"),
            )
        })?;
        for &b in &raw {
            // Rejection sampling over 62*4 = 248 removes modulo bias.
            if b < 248 {
                let idx = usize::from(b % 62);
                out.push(char::from(ALNUM_CHARSET[idx]));
                if out.len() == len {
                    break;
                }
            }
        }
    }
    Ok(out)
}

fn generate_webhook_hook_id() -> Result<String> {
    let suffix = generate_csprng_alnum(WEBHOOK_HOOK_ID_RANDOM_LEN)?;
    let hook_id = format!("{WEBHOOK_HOOK_ID_PREFIX}{suffix}");
    if !is_valid_webhook_hook_id(&hook_id) {
        return Err(OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            "Generated invalid webhook hook_id.",
        ));
    }
    Ok(hook_id)
}

fn generate_webhook_call_id() -> Result<String> {
    let suffix = generate_csprng_alnum(WEBHOOK_CALL_ID_RANDOM_LEN)?;
    let call_id = format!("{WEBHOOK_CALL_ID_PREFIX}{suffix}");
    if !is_valid_webhook_call_id(&call_id) {
        return Err(OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            "Generated invalid webhook call_id.",
        ));
    }
    Ok(call_id)
}

fn generate_webhook_secret_key() -> Result<String> {
    let suffix = generate_csprng_alnum(WEBHOOK_KEY_RANDOM_LEN)?;
    let key = format!("{WEBHOOK_KEY_PREFIX}{suffix}");
    if !is_valid_webhook_key(&key) {
        return Err(OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            "Generated invalid webhook secret key.",
        ));
    }
    Ok(key)
}

// ---------------------------------------------------------------------------
// 5. ContentFreeWakeFrame, OneShotWaitManager, WatchPrCoordinator & AutomationTriggerEngine
// ---------------------------------------------------------------------------

/// Event wake category emitted over the hibernating Wake Durable Object WebSocket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeFrameKind {
    Delivery,
    AccessRequest,
    WebhookCall,
    TeamShare,
    Ping,
}

/// Content-free wake frame carrying ZERO message body, secrets, actor names, or URLs.
/// `deny_unknown_fields` guarantees unexpected sensitive payload fields fail closed on decode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentFreeWakeFrame {
    pub host_id: String,
    pub kind: WakeFrameKind,
    pub sequence: u64,
    pub timestamp_ms: u64,
}

impl ContentFreeWakeFrame {
    pub fn new(
        host_id: impl Into<String>,
        kind: WakeFrameKind,
        sequence: u64,
        timestamp_ms: u64,
    ) -> Result<Self> {
        let host_id = host_id.into();
        if host_id.trim().is_empty() || host_id.chars().any(char::is_control) {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "ContentFreeWakeFrame host_id must be non-empty without control chars.",
            ));
        }
        Ok(Self {
            host_id: host_id.trim().to_string(),
            kind,
            sequence,
            timestamp_ms,
        })
    }

    /// Serializes the content-free wake frame to JSON and verifies it fits within
    /// `CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES`.
    pub fn encode_bounded_json(&self) -> Result<String> {
        let encoded = serde_json::to_string(self)?;
        if encoded.len() > CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                format!(
                    "Wake frame exceeds {CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES} bytes (close code {WS_CLOSE_MESSAGE_TOO_BIG})."
                ),
            ));
        }
        Ok(encoded)
    }

    /// Decodes a content-free wake frame, rejecting oversized payloads (`WS_CLOSE_MESSAGE_TOO_BIG`)
    /// and any extra/leaked fields (`deny_unknown_fields`).
    pub fn decode_bounded_json(raw: &str) -> Result<Self> {
        if raw.len() > CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                format!(
                    "Inbound wake frame exceeds {CONNECTOR_WAKE_MAX_INBOUND_FRAME_BYTES} bytes (close code {WS_CLOSE_MESSAGE_TOO_BIG})."
                ),
            ));
        }
        let frame: Self = serde_json::from_str(raw)?;
        if frame.host_id.trim().is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Wake frame host_id cannot be empty.",
            ));
        }
        Ok(frame)
    }
}

/// Active `--once [--by <ISO>]` wait tracked by `OneShotWaitManager`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedOneShotWait {
    pub wait_id: String,
    pub automation_id: String,
    pub bot_id: String,
    pub wait: OneShotWait,
    pub created_at_ms: u64,
    pub due_at_ms: u64,
    pub runs: Vec<OneShotRunSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending: Option<WaitEnding>,
}

/// Outcome returned when recording a run attempt on a managed one-shot wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordWaitRunOutcome {
    pub claimed: bool,
    pub consumed_and_deleted: bool,
    pub ending: Option<WaitEnding>,
    pub notice_card: Option<NativeChatBotCard>,
}

/// Manages `--once [--by <ISO>]` waits with 30-day default expiry (`WAIT_EXPIRY_MS`),
/// 1-year maximum deadline (`WAIT_MAX_DEADLINE_MS`), skipped-run immunity (`claims_the_run`),
/// automatic deletion after consumption, and `WaitEnding` notice/card emission.
#[derive(Debug, Clone, Default)]
pub struct OneShotWaitManager {
    waits: Arc<StdMutex<BTreeMap<String, ManagedOneShotWait>>>,
}

impl OneShotWaitManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a validated one-shot wait (30-day default expiry, 1-year max deadline).
    pub fn register_wait(
        &self,
        automation_id: impl Into<String>,
        bot_id: impl Into<String>,
        wait: OneShotWait,
        created_at_ms: u64,
        has_events: bool,
    ) -> Result<ManagedOneShotWait> {
        let automation_id = automation_id.into();
        let bot_id = bot_id.into();
        if automation_id.trim().is_empty() || bot_id.trim().is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "One-shot wait requires non-empty automation_id and bot_id.",
            ));
        }
        wait.validate(created_at_ms, true, false, has_events)?;
        let due_at_ms = wait.due_at(created_at_ms);
        let managed = ManagedOneShotWait {
            wait_id: format!("wait_{}", Uuid::new_v4().simple()),
            automation_id,
            bot_id,
            wait,
            created_at_ms,
            due_at_ms,
            runs: Vec::new(),
            ending: None,
        };
        let mut guard = self.lock_waits()?;
        guard.insert(managed.wait_id.clone(), managed.clone());
        Ok(managed)
    }

    /// Records a run attempt against a one-shot wait.
    /// Skipped runs (`skipped_*`) and `Manual` runs NEVER consume or delete the wait!
    /// Once a non-manual claiming run finishes, emits `WaitEnding` + notice card and deletes the wait.
    #[allow(clippy::too_many_arguments)]
    pub fn record_run_outcome(
        &self,
        wait_id: &str,
        trigger: AutomationRunTrigger,
        status: AutomationRunStatus,
        event_at_ms: u64,
        actor: Option<String>,
        session_id: &str,
    ) -> Result<RecordWaitRunOutcome> {
        let mut guard = self.lock_waits()?;
        let wait_entry = guard.get_mut(wait_id).ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("One-shot wait '{wait_id}' not found."),
            )
        })?;
        wait_entry
            .runs
            .push(OneShotRunSummary { trigger, status });

        let claimed = once_claimed(&wait_entry.runs);
        let consumed = once_run_finished(&wait_entry.runs);
        let this_claims = trigger != AutomationRunTrigger::Manual && claims_the_run(status);

        if consumed {
            let scheduled_for = (trigger == AutomationRunTrigger::Scheduled)
                .then_some(wait_entry.due_at_ms);
            let event_occurred_at = (trigger == AutomationRunTrigger::Event).then_some(event_at_ms);
            let ending = once_run_ending(true, trigger, scheduled_for, event_occurred_at, actor);
            let card = if let Some(ref end) = ending {
                Some(build_wait_ending_card(
                    &wait_entry.bot_id,
                    session_id,
                    wait_id,
                    end,
                    event_at_ms,
                )?)
            } else {
                None
            };
            guard.remove(wait_id);
            return Ok(RecordWaitRunOutcome {
                claimed,
                consumed_and_deleted: true,
                ending,
                notice_card: card,
            });
        }

        Ok(RecordWaitRunOutcome {
            claimed: claimed || this_claims,
            consumed_and_deleted: false,
            ending: None,
            notice_card: None,
        })
    }

    /// Sweeps expired or deadline-reached unclaimed waits, emitting `WaitEnding::Deadline`
    /// (when `--by` was set) or `WaitEnding::Expired` (when default 30-day expiry elapsed)
    /// and removing them from the active registry.
    pub fn sweep_expired(&self, now_ms: u64) -> Result<Vec<(String, WaitEnding)>> {
        let mut guard = self.lock_waits()?;
        let mut expired = Vec::new();
        let ids: Vec<String> = guard
            .iter()
            .filter(|(_, w)| !once_claimed(&w.runs) && now_ms >= w.due_at_ms)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(entry) = guard.remove(&id) {
                let ending = if entry.wait.by.is_some() {
                    WaitEnding::Deadline { at: entry.due_at_ms }
                } else {
                    WaitEnding::Expired { at: entry.due_at_ms }
                };
                expired.push((id, ending));
            }
        }
        Ok(expired)
    }

    /// Returns active unclaimed waits sorted soonest-deadline-first via `orca_core::triggers_webhooks::pending_waits`.
    pub fn pending(&self) -> Result<Vec<PendingWaitEntry>> {
        let guard = self.lock_waits()?;
        let all: Vec<PendingWaitEntry> = guard
            .values()
            .map(|w| PendingWaitEntry {
                id: w.wait_id.clone(),
                wait: w.wait,
                runs: w.runs.clone(),
            })
            .collect();
        Ok(pending_waits(&all))
    }

    pub fn get(&self, wait_id: &str) -> Result<Option<ManagedOneShotWait>> {
        let guard = self.lock_waits()?;
        Ok(guard.get(wait_id).cloned())
    }

    pub const fn default_expiry_ms() -> u64 {
        WAIT_EXPIRY_MS
    }

    pub const fn max_deadline_ms() -> u64 {
        WAIT_MAX_DEADLINE_MS
    }

    fn lock_waits(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, ManagedOneShotWait>>> {
        self.waits.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "OneShotWaitManager mutex poisoned.")
        })
    }
}

fn build_wait_ending_card(
    bot_id: &str,
    session_id: &str,
    wait_id: &str,
    ending: &WaitEnding,
    now_ms: u64,
) -> Result<NativeChatBotCard> {
    let title = match ending {
        WaitEnding::Event { .. } => "Wait completed by event",
        WaitEnding::Deadline { .. } => "Wait reached deadline",
        WaitEnding::Expired { .. } => "Wait expired after 30 days",
    };
    let card = NativeChatBotCard {
        card_id: format!("wait_end_{wait_id}"),
        kind: NativeChatBotCardKind::BotRequest,
        bot_id: bot_id.to_string(),
        session_id: if session_id.trim().is_empty() {
            "home".to_string()
        } else {
            session_id.to_string()
        },
        title: title.to_string(),
        summary: Some(format!("One-shot wait {wait_id} settled.")),
        created_at_ms: i64::try_from(now_ms).unwrap_or(now_epoch_ms()),
        payload: serde_json::json!({
            "waitId": wait_id,
            "waitEnding": ending,
        }),
    };
    card.validate()?;
    Ok(card)
}

/// Active `watch-pr <owner/repo#n>` state tracked by `WatchPrCoordinator`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveWatchPr {
    pub watch_id: String,
    pub bot_id: String,
    pub session_id: String,
    pub policy: WatchPrPolicy,
    /// 1-indexed next worker fix attempt (hard capped by `WATCH_PR_MAX_FIX_ATTEMPTS = 2`).
    pub current_attempt: u8,
    pub fix_dispatches_used: u8,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    pub created_at_ms: u64,
}

/// Coordinates `watch-pr <owner/repo#n>` across CI finish, merge conflict, merge/close,
/// review/comment, and deadline (default 2h) triggers with a hard cap of 2 autonomous worker fixes.
#[derive(Debug, Clone, Default)]
pub struct WatchPrCoordinator {
    watches: Arc<StdMutex<BTreeMap<String, ActiveWatchPr>>>,
}

impl WatchPrCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn default_duration_ms() -> u64 {
        WATCH_PR_DEFAULT_MS
    }

    pub const fn max_fix_attempts() -> u8 {
        WATCH_PR_MAX_FIX_ATTEMPTS
    }

    /// Starts watching a GitHub PR (`owner/repo#n`), defaulting to 2h deadline and max 2 fix attempts.
    pub fn start_watch(
        &self,
        bot_id: impl Into<String>,
        session_id: impl Into<String>,
        pr_raw: &str,
        now_ms: u64,
        duration_raw: Option<&str>,
        ci_finished_supported: bool,
    ) -> Result<ActiveWatchPr> {
        let bot_id = bot_id.into();
        let session_id = session_id.into();
        if bot_id.trim().is_empty() || session_id.trim().is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Watch-PR requires non-empty bot_id and session_id.",
            ));
        }
        let duration_ms = match duration_raw {
            Some(raw) => Some(parse_watch_duration(raw)?),
            None => None,
        };
        let policy = WatchPrPolicy::new(
            pr_raw,
            now_ms,
            duration_ms,
            Some(WATCH_PR_MAX_FIX_ATTEMPTS),
            ci_finished_supported,
        )?;
        let watch = ActiveWatchPr {
            watch_id: format!("wpr_{}", Uuid::new_v4().simple()),
            bot_id,
            session_id,
            policy,
            current_attempt: 1,
            fix_dispatches_used: 0,
            active: true,
            stop_reason: None,
            created_at_ms: now_ms,
        };
        let mut guard = self.lock_watches()?;
        guard.insert(watch.watch_id.clone(), watch.clone());
        Ok(watch)
    }

    /// Returns the GitHub event triggers required to watch this PR (CI finish, conflict, close/merge, review/comment).
    pub fn required_github_events(&self, watch_id: &str) -> Result<Vec<GitHubEventKind>> {
        let guard = self.lock_watches()?;
        let watch = guard.get(watch_id).ok_or_else(|| {
            OrcaBotsError::bot(BotErrorCode::NotFound, format!("Watch '{watch_id}' not found."))
        })?;
        Ok(watch.policy.events())
    }

    /// Processes a PR wake outcome using `evaluate_watch_pr_step`, strictly enforcing the
    /// 2-attempt worker-fix ceiling and deadline expiry.
    pub fn handle_wake(
        &self,
        watch_id: &str,
        outcome: WatchPrWakeOutcome,
        now_ms: u64,
    ) -> Result<WatchPrStepAction> {
        let mut guard = self.lock_watches()?;
        let watch = guard.get_mut(watch_id).ok_or_else(|| {
            OrcaBotsError::bot(BotErrorCode::NotFound, format!("Watch '{watch_id}' not found."))
        })?;
        if !watch.active {
            return Err(OrcaBotsError::bot(
                BotErrorCode::Expired,
                format!("Watch '{watch_id}' is already stopped."),
            ));
        }
        if now_ms >= watch.policy.deadline_ms {
            watch.active = false;
            watch.stop_reason = Some("deadline_reached".to_string());
            return Ok(WatchPrStepAction::EscalateToOwnerAndStop {
                attempt: watch.current_attempt,
            });
        }

        let action = evaluate_watch_pr_step(
            watch.current_attempt,
            outcome,
            watch.policy.max_fix_attempts,
        );
        match action {
            WatchPrStepAction::DispatchWorkerFix { next_attempt, .. } => {
                watch.fix_dispatches_used = watch.fix_dispatches_used.saturating_add(1);
                watch.current_attempt = next_attempt;
            }
            WatchPrStepAction::AnnounceAndStop { reason } => {
                watch.active = false;
                watch.stop_reason = Some(format!("{reason:?}"));
            }
            WatchPrStepAction::EscalateToOwnerAndStop { .. } => {
                watch.active = false;
                watch.stop_reason = Some("max_fix_attempts_reached".to_string());
            }
            WatchPrStepAction::NotifyOwnerAndRearmSameAttempt { .. }
            | WatchPrStepAction::ForwardQuestionAndRearmSameAttempt { .. } => {}
        }
        Ok(action)
    }

    pub fn get_watch(&self, watch_id: &str) -> Result<ActiveWatchPr> {
        let guard = self.lock_watches()?;
        guard.get(watch_id).cloned().ok_or_else(|| {
            OrcaBotsError::bot(BotErrorCode::NotFound, format!("Watch '{watch_id}' not found."))
        })
    }

    fn lock_watches(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, ActiveWatchPr>>> {
        self.watches.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "WatchPrCoordinator mutex poisoned.")
        })
    }
}

/// Sentinel schedule attached to event-only automations (`dtstart: 9999-01-01`) so older hosts
/// never fire them on a recurring timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EventOnlySchedule {
    pub date: &'static str,
    pub rrule: &'static str,
    pub dtstart_ms: i64,
}

pub const EVENT_ONLY_SCHEDULE: EventOnlySchedule = EventOnlySchedule {
    date: EVENT_ONLY_SCHEDULE_DATE,
    rrule: EVENT_ONLY_SCHEDULE_RRULE,
    dtstart_ms: EVENT_ONLY_SCHEDULE_DTSTART_MS,
};

/// Input to create an automation backed by recurring schedule and/or any of the 31 event trigger kinds.
#[derive(Debug, Clone)]
pub struct CreateAutomationRequest {
    pub name: String,
    pub prompt: String,
    pub enabled: bool,
    pub bot_target: AutomationBotTarget,
    /// `None` attaches `EVENT_ONLY_SCHEDULE` (`dtstart: 9999-01-01`).
    pub dtstart_ms: Option<i64>,
    pub rrule: Option<String>,
    pub triggers: Vec<AutomationEventTrigger>,
    pub once: Option<OneShotWait>,
    pub now_ms: u64,
}

/// Persisted automation definition managed by `AutomationTriggerEngine`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotAutomationRecord {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub enabled: bool,
    pub bot_target: AutomationBotTarget,
    pub dtstart_ms: i64,
    pub rrule: String,
    pub triggers: Vec<AutomationEventTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once: Option<OneShotWait>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_id: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl BotAutomationRecord {
    pub fn is_event_only(&self) -> bool {
        self.dtstart_ms == EVENT_ONLY_SCHEDULE_DTSTART_MS
    }

    /// Returns `true` when any trigger uses a strictly local-host source (`gmail` or `calendar`).
    pub fn has_local_only_triggers(&self) -> bool {
        self.triggers.iter().any(|t| is_local_only_source(t.source()))
    }
}

/// Returns all 31 supported automation event kinds across all 6 sources
/// (`slack` 3, `github` 21, `linear` 4, `gmail` 1, `calendar` 1, `webhook` 1).
pub const fn all_supported_event_kinds() -> &'static [AutomationEventKind; 31] {
    &AutomationEventKind::ALL
}

/// Returns `true` iff `source` must be evaluated strictly on the local host (`gmail` & `calendar`)
/// and NEVER forwarded to the cloud trigger router.
pub const fn is_local_only_source(source: AutomationEventSource) -> bool {
    source.is_host_local_only()
}

/// Manages bot automations across all 31 event kinds / 6 sources, `OneShotWaitManager`,
/// and `WatchPrCoordinator`.
#[derive(Debug, Clone, Default)]
pub struct AutomationTriggerEngine {
    automations: Arc<StdMutex<BTreeMap<String, BotAutomationRecord>>>,
    waits: OneShotWaitManager,
    watch_pr: WatchPrCoordinator,
}

impl AutomationTriggerEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn waits(&self) -> &OneShotWaitManager {
        &self.waits
    }

    pub fn watch_pr(&self) -> &WatchPrCoordinator {
        &self.watch_pr
    }

    /// Registers a validated bot automation:
    /// - Enforces all 31 event kinds across 6 sources and `pr_opened` as the ONLY GitHub event
    ///   permitting `from_owner: true`.
    /// - Attaches `EVENT_ONLY_SCHEDULE` (`dtstart: 9999-01-01`) when no recurring schedule is set.
    /// - Registers `--once [--by]` waits automatically inside `OneShotWaitManager`.
    pub fn register_automation(
        &self,
        req: CreateAutomationRequest,
    ) -> Result<BotAutomationRecord> {
        if req.name.trim().is_empty() || req.prompt.trim().is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Automation requires non-empty name and prompt.",
            ));
        }
        req.bot_target.validate()?;

        let (dtstart_ms, rrule) = match req.dtstart_ms {
            Some(ts) => (
                ts,
                req.rrule
                    .filter(|r| !r.trim().is_empty())
                    .unwrap_or_else(|| EVENT_ONLY_SCHEDULE_RRULE.to_string()),
            ),
            None => (
                EVENT_ONLY_SCHEDULE.dtstart_ms,
                EVENT_ONLY_SCHEDULE.rrule.to_string(),
            ),
        };

        validate_automation_event_triggers(
            &req.triggers,
            Some(&req.bot_target),
            dtstart_ms,
            req.once.as_ref(),
        )?;

        let id = format!("auto_{}", Uuid::new_v4().simple());
        let wait_id = if let Some(wait) = req.once {
            let managed = self.waits.register_wait(
                &id,
                &req.bot_target.bot_id,
                wait,
                req.now_ms,
                !req.triggers.is_empty(),
            )?;
            Some(managed.wait_id)
        } else {
            None
        };

        let record = BotAutomationRecord {
            id: id.clone(),
            name: req.name.trim().to_string(),
            prompt: req.prompt,
            enabled: req.enabled,
            bot_target: req.bot_target,
            dtstart_ms,
            rrule,
            triggers: req.triggers,
            once: req.once,
            wait_id,
            created_at_ms: req.now_ms,
            updated_at_ms: req.now_ms,
        };
        let mut guard = self.lock_automations()?;
        guard.insert(id, record.clone());
        Ok(record)
    }

    pub fn list_automations(&self, bot_id: Option<&str>) -> Result<Vec<BotAutomationRecord>> {
        let guard = self.lock_automations()?;
        Ok(guard
            .values()
            .filter(|a| bot_id.is_none_or(|b| a.bot_target.bot_id == b))
            .cloned()
            .collect())
    }

    /// Lists automations that include local-host-only (`gmail` / `calendar`) triggers.
    pub fn list_local_host_automations(&self) -> Result<Vec<BotAutomationRecord>> {
        let guard = self.lock_automations()?;
        Ok(guard
            .values()
            .filter(|a| a.has_local_only_triggers())
            .cloned()
            .collect())
    }

    /// Returns ONLY cloud-syncable triggers for `automation_id`, strictly excluding
    /// local-only (`gmail` / `calendar`) triggers so Google triggers never leave the local host.
    pub fn list_cloud_syncable_triggers(
        &self,
        automation_id: &str,
    ) -> Result<Vec<AutomationEventTrigger>> {
        let guard = self.lock_automations()?;
        let auto = guard.get(automation_id).ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Automation '{automation_id}' not found."),
            )
        })?;
        Ok(auto
            .triggers
            .iter()
            .filter(|t| !is_local_only_source(t.source()))
            .cloned()
            .collect())
    }

    pub fn set_automation_enabled(
        &self,
        automation_id: &str,
        enabled: bool,
        now_ms: u64,
    ) -> Result<BotAutomationRecord> {
        let mut guard = self.lock_automations()?;
        let auto = guard.get_mut(automation_id).ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Automation '{automation_id}' not found."),
            )
        })?;
        auto.enabled = enabled;
        auto.updated_at_ms = now_ms;
        Ok(auto.clone())
    }

    pub fn remove_automation(&self, automation_id: &str) -> Result<bool> {
        let mut guard = self.lock_automations()?;
        Ok(guard.remove(automation_id).is_some())
    }

    fn lock_automations(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, BotAutomationRecord>>> {
        self.automations.lock().map_err(|_| {
            OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "AutomationTriggerEngine mutex poisoned.",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::CreateBotRequest;
    use crate::home_store::BotHomeStore;
    use orca_core::bot_types::{
        BotAgentId, ConnectorAskerPlaceKind, ConnectorAskerRole, ConnectorSource,
    };
    use orca_core::triggers_webhooks::{
        AutomationBotResultsTo, AutomationBotRunsIn, AutomationConnectorEventTrigger,
        AutomationGoogleTrigger, WebhookWhereClause, WebhookWhereOp,
    };
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn acts_for_attribution_owner_guard_and_leave_thread_anti_injection() {
        let dana = ActsForAsker::Person {
            source: ConnectorSource::Slack,
            user_id: "U_DANA".to_string(),
            name: Some("Dana".to_string()),
            workspace_id: Some("T1".to_string()),
            role: Some(ConnectorAskerRole::Member),
            place_kind: Some(ConnectorAskerPlaceKind::PublicChannel),
        };
        let askers = stamp_conversation_askers(&[], dana.clone());
        let askers = stamp_conversation_askers(&askers, dana);
        assert_eq!(askers.len(), 1);

        let line = format_external_action_attribution(&askers, "Release Bot", false)
            .expect("non-owner attribution line");
        assert!(line.contains("asked by Dana via Release Bot"));
        assert!(line.chars().count() <= orca_core::bot_types::ASKED_BY_MAX_CHARS);

        let body = decorate_external_action_body("Merged check", &askers, "Release Bot", false);
        assert!(body.contains("asked by Dana via Release Bot"));

        let err = require_owner_asker(&askers, "merge_pr").expect_err("non-owner blocked");
        assert_eq!(err.code(), BotErrorCode::NotOwner);
        require_owner_asker(&[ActsForAsker::Owner], "merge_pr").expect("owner allowed");

        let direct = evaluate_leave_thread_request(&LeaveThreadContext {
            text: "@Helper please leave this thread",
            bot_name: "Helper",
            other_bot_names: &["Reviewer"],
            is_forwarded: false,
            is_quoted_reply: false,
            addressed_bot_name: Some("Helper"),
        });
        assert!(direct.is_allowed());

        let pasted = evaluate_leave_thread_request(&LeaveThreadContext {
            text: "Alice said: stop following this thread",
            bot_name: "Helper",
            other_bot_names: &["Reviewer"],
            is_forwarded: false,
            is_quoted_reply: false,
            addressed_bot_name: None,
        });
        assert!(matches!(
            pasted,
            LeaveThreadDecision::Block {
                reason: LeaveThreadBlockReason::QuotedOrPastedText,
                ..
            }
        ));

        let in_code = evaluate_leave_thread_request(&LeaveThreadContext {
            text: "Run ```orca bot reply --leave-thread``` in CI",
            bot_name: "Helper",
            other_bot_names: &[],
            is_forwarded: false,
            is_quoted_reply: false,
            addressed_bot_name: None,
        });
        assert!(matches!(
            in_code,
            LeaveThreadDecision::Block {
                reason: LeaveThreadBlockReason::InsideCodeBlock,
                ..
            }
        ));

        let meta_q = evaluate_leave_thread_request(&LeaveThreadContext {
            text: "How do you leave a thread when muted?",
            bot_name: "Helper",
            other_bot_names: &[],
            is_forwarded: false,
            is_quoted_reply: false,
            addressed_bot_name: None,
        });
        assert!(matches!(
            meta_q,
            LeaveThreadDecision::Block {
                reason: LeaveThreadBlockReason::MetaQuestion,
                ..
            }
        ));

        let other_bot = evaluate_leave_thread_request(&LeaveThreadContext {
            text: "@Reviewer please leave this thread",
            bot_name: "Helper",
            other_bot_names: &["Reviewer"],
            is_forwarded: false,
            is_quoted_reply: false,
            addressed_bot_name: None,
        });
        assert!(matches!(
            other_bot,
            LeaveThreadDecision::Block {
                reason: LeaveThreadBlockReason::DirectedAtAnotherBot,
                ..
            }
        ));

        let forwarded = evaluate_leave_thread_request(&LeaveThreadContext {
            text: "leave this thread",
            bot_name: "Helper",
            other_bot_names: &[],
            is_forwarded: true,
            is_quoted_reply: false,
            addressed_bot_name: None,
        });
        assert!(matches!(
            forwarded,
            LeaveThreadDecision::Block {
                reason: LeaveThreadBlockReason::ForwardedMessage,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn bot_access_host_lifecycle_caps_host_binding_and_recovery() {
        let dir = TempDir::new().expect("tempdir");
        let home = BotHomeStore::new(dir.path().join("bots"));
        let controller = BotController::open(dir.path().join("state"), home).expect("controller");
        let bot = controller
            .create_bot(CreateBotRequest {
                name: Some("Support Bot".to_string()),
                agent: BotAgentId::Claude,
                ..Default::default()
            })
            .await
            .expect("create bot");

        let host = BotAccessHost::open("host-a", dir.path().join("access")).expect("access host");

        // Wrong host rejected with NotThisHost
        let foreign = host
            .create_request_for_host(
                "host-b",
                CreateAccessRequestInput {
                    bot_id: bot.id.clone(),
                    member: BotMember {
                        id: "U_FOREIGN".to_string(),
                        name: "Foreign".to_string(),
                        source: Some("slack".to_string()),
                        workspace_id: Some("T1".to_string()),
                        avatar_url: None,
                        kind: None,
                    },
                    connector_workspace_id: Some("T1".to_string()),
                    ttl_ms: Some(60_000),
                    now_ms: 1_000,
                },
            )
            .expect("create foreign");
        let err = host
            .redeem_token(&foreign.redeem_token, 2_000, &controller)
            .await
            .expect_err("wrong host must fail");
        assert_eq!(err.code(), BotErrorCode::NotThisHost);

        // Expired request rejected with Expired
        let exp_req = host
            .create_request(CreateAccessRequestInput {
                bot_id: bot.id.clone(),
                member: BotMember {
                    id: "U_EXP".to_string(),
                    name: "Expired".to_string(),
                    source: Some("slack".to_string()),
                    workspace_id: Some("T1".to_string()),
                    avatar_url: None,
                    kind: None,
                },
                connector_workspace_id: Some("T1".to_string()),
                ttl_ms: Some(1_000),
                now_ms: 10_000,
            })
            .expect("create exp");
        let err = host
            .redeem_token(&exp_req.redeem_token, 12_000, &controller)
            .await
            .expect_err("expired must fail");
        assert_eq!(err.code(), BotErrorCode::Expired);

        // Single-use redeem succeeds once, then second redeem fails Expired
        let ok_req = host
            .create_request(CreateAccessRequestInput {
                bot_id: bot.id.clone(),
                member: BotMember {
                    id: "U_ALICE".to_string(),
                    name: "Alice".to_string(),
                    source: Some("slack".to_string()),
                    workspace_id: Some("T1".to_string()),
                    avatar_url: None,
                    kind: None,
                },
                connector_workspace_id: Some("T1".to_string()),
                ttl_ms: Some(60_000),
                now_ms: 10_000,
            })
            .expect("create alice");
        let outcome = host
            .redeem_token(&ok_req.redeem_token, 11_000, &controller)
            .await
            .expect("redeem alice");
        assert!(outcome.added_locally);
        assert_eq!(outcome.request.status, ConnectorAccessRequestStatus::Granted);
        let second = host
            .redeem_token(&ok_req.redeem_token, 11_500, &controller)
            .await
            .expect_err("single-use token cannot be reused");
        assert_eq!(second.code(), BotErrorCode::Expired);

        // Workspace cap 8 enforcement -> Full
        for i in 2..=8 {
            let ws = format!("T{i}");
            let r = host
                .create_request(CreateAccessRequestInput {
                    bot_id: bot.id.clone(),
                    member: BotMember {
                        id: format!("U_{i}"),
                        name: format!("User {i}"),
                        source: Some("slack".to_string()),
                        workspace_id: Some(ws.clone()),
                        avatar_url: None,
                        kind: None,
                    },
                    connector_workspace_id: Some(ws),
                    ttl_ms: Some(60_000),
                    now_ms: 10_000,
                })
                .expect("req");
            host.decide_request(&r.request_id, true, 12_000, &controller)
                .await
                .expect("admit ws");
        }
        let ninth = host
            .create_request(CreateAccessRequestInput {
                bot_id: bot.id.clone(),
                member: BotMember {
                    id: "U_9".to_string(),
                    name: "User 9".to_string(),
                    source: Some("slack".to_string()),
                    workspace_id: Some("T9".to_string()),
                    avatar_url: None,
                    kind: None,
                },
                connector_workspace_id: Some("T9".to_string()),
                ttl_ms: Some(60_000),
                now_ms: 10_000,
            })
            .expect("ninth");
        let full_err = host
            .decide_request(&ninth.request_id, true, 12_500, &controller)
            .await
            .expect_err("9th workspace must fail Full");
        assert_eq!(full_err.code(), BotErrorCode::Full);

        // Cloud-first GrantedNotAdded + startup recovery sweep once bot home is valid
        let missing_bot_req = host
            .create_request(CreateAccessRequestInput {
                bot_id: "missing-bot".to_string(),
                member: BotMember {
                    id: "U_REC".to_string(),
                    name: "Recovered".to_string(),
                    source: Some("slack".to_string()),
                    workspace_id: Some("T1".to_string()),
                    avatar_url: None,
                    kind: None,
                },
                connector_workspace_id: Some("T1".to_string()),
                ttl_ms: Some(60_000),
                now_ms: 10_000,
            })
            .expect("missing bot req");
        let gna = host
            .decide_request(&missing_bot_req.request_id, true, 13_000, &controller)
            .await
            .expect_err("missing local bot returns GrantedNotAdded");
        assert_eq!(gna.code(), BotErrorCode::GrantedNotAdded);
        let sweep = host
            .recover_granted_not_added(&controller)
            .await
            .expect("sweep");
        assert_eq!(sweep.still_pending_request_ids, vec![missing_bot_req.request_id]);
    }

    #[tokio::test]
    async fn slack_mirror_holds_turn_end_until_posting_settles_and_supports_lifecycle() {
        let mirror = SlackMirror::new();
        mirror
            .register_chat(SlackLinkedChat {
                session_id: "sess-1".to_string(),
                channel_id: "C123".to_string(),
                thread_ts: Some("1700000.001".to_string()),
                opened_by_owner: true,
                slack_mirror: None,
            })
            .await
            .expect("register chat");
        mirror
            .register_chat(SlackLinkedChat {
                session_id: "sess-off".to_string(),
                channel_id: "C999".to_string(),
                thread_ts: None,
                opened_by_owner: true,
                slack_mirror: Some(false),
            })
            .await
            .expect("register off");

        assert!(mirror.should_mirror("sess-1").await);
        assert!(!mirror.should_mirror("sess-off").await);
        assert!(
            mirror
                .begin_mirror_post("sess-off", "ignored", 1_000)
                .await
                .expect("off")
                .is_none()
        );

        let entry = mirror
            .begin_mirror_post("sess-1", "Owner question?", 1_000)
            .await
            .expect("begin post")
            .expect("entry created");
        assert_eq!(mirror.in_flight_count("sess-1").await, 1);

        let timeline = Arc::new(StdMutex::new(Vec::<&'static str>::new()));
        let barrier_mirror = mirror.clone();
        let barrier_timeline = timeline.clone();
        let reply_task = tokio::spawn(async move {
            barrier_mirror
                .with_held_turn_end("sess-1", || {
                    let tl = barrier_timeline.clone();
                    async move {
                        tl.lock().expect("tl").push("bot_reply_sent");
                        Ok("reply_ok")
                    }
                })
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        assert!(
            timeline.lock().expect("tl").is_empty(),
            "bot reply must be held while mirrored question is still Posting"
        );

        timeline.lock().expect("tl").push("owner_question_posted");
        mirror
            .mark_posted(&entry.entry_id, "1700000.002", 1_100)
            .await
            .expect("mark posted");

        let reply_res = reply_task.await.expect("join").expect("reply ok");
        assert_eq!(reply_res, "reply_ok");
        assert_eq!(
            *timeline.lock().expect("tl"),
            vec!["owner_question_posted", "bot_reply_sent"]
        );

        let failed = mirror
            .begin_mirror_post("sess-1", "Follow-up", 2_000)
            .await
            .expect("begin")
            .expect("entry");
        let f = mirror
            .mark_failed(&failed.entry_id, "rate_limited", 2_100)
            .await
            .expect("mark failed");
        assert!(matches!(f.state, SlackMirrorEntryState::Failed { .. }));
        let r = mirror
            .retry_post(&failed.entry_id, 2_200)
            .await
            .expect("retry");
        assert_eq!(r.state, SlackMirrorEntryState::Posting);
        let d = mirror
            .dismiss_post(&failed.entry_id, 2_300)
            .await
            .expect("dismiss");
        assert_eq!(d.state, SlackMirrorEntryState::Dismissed);
        let del = mirror
            .delete_from_slack(&entry.entry_id, 2_400)
            .await
            .expect("delete");
        assert_eq!(del.state, SlackMirrorEntryState::Deleted);
        assert_eq!(del.last_slack_ts.as_deref(), Some("1700000.002"));
    }

    #[test]
    fn webhook_engine_six_adapters_six_ops_caps_spool_zero_leak_and_cli_view() {
        let dir = TempDir::new().expect("tempdir");
        let engine =
            WebhookEngine::new(dir.path().join("spool"), "https://hooks.orca.local").expect("eng");

        // Verify all 6 adapters HMAC-SHA256 signature validation
        let payload = br#"{"action":"opened","repo":{"name":"orca"},"tags":["prod","urgent"],"meta":{"note":"deploy ok"}}"#;
        for adapter in WebhookAdapterId::ALL {
            let (cli_view, card) = engine
                .create_hook(CreateWebhookRequest {
                    bot_id: "bot-1".to_string(),
                    automation_id: format!("auto-{}", adapter.as_str()),
                    adapter,
                    filters: WebhookFilters {
                        where_clauses: vec![
                            WebhookWhereClause {
                                path: "body.action".to_string(),
                                op: WebhookWhereOp::Eq,
                                values: vec!["opened".to_string()],
                            },
                            WebhookWhereClause {
                                path: "body.repo.name".to_string(),
                                op: WebhookWhereOp::NotEq,
                                values: vec!["legacy".to_string()],
                            },
                            WebhookWhereClause {
                                path: "body.action".to_string(),
                                op: WebhookWhereOp::In,
                                values: vec!["opened".to_string(), "reopened".to_string()],
                            },
                            WebhookWhereClause {
                                path: "body.meta.note".to_string(),
                                op: WebhookWhereOp::Contains,
                                values: vec!["deploy".to_string()],
                            },
                            WebhookWhereClause {
                                path: "body.tags[0]".to_string(),
                                op: WebhookWhereOp::Exists,
                                values: vec![],
                            },
                            WebhookWhereClause {
                                path: "body.nonexistent".to_string(),
                                op: WebhookWhereOp::NotExists,
                                values: vec![],
                            },
                        ],
                        text_contains: None,
                        subject_from: Some("body.meta.note".to_string()),
                        subject: None,
                        pick: vec!["body.action".to_string()],
                    },
                    hourly_cap: Some(2),
                    grace_minutes: None,
                    require_signature: true,
                    now_ms: 1_000,
                })
                .expect("create hook");

            // CLI view MUST NEVER serialize URL or secret!
            let cli_json = serde_json::to_string(&cli_view).expect("cli json");
            assert!(!cli_json.contains(&card.secret_key));
            assert!(!cli_json.contains(&card.ingest_url));
            assert!(!cli_json.contains("orcawh_"));

            let mut headers = BTreeMap::new();
            match adapter {
                WebhookAdapterId::Standard => {
                    let sig = compute_hmac_sha256_hex(&card.secret_key, payload).expect("hmac");
                    headers.insert("x-orca-signature-256".to_string(), format!("sha256={sig}"));
                }
                WebhookAdapterId::Github => {
                    let sig = compute_hmac_sha256_hex(&card.secret_key, payload).expect("hmac");
                    headers.insert("x-hub-signature-256".to_string(), format!("sha256={sig}"));
                }
                WebhookAdapterId::Sentry => {
                    let sig = compute_hmac_sha256_hex(&card.secret_key, payload).expect("hmac");
                    headers.insert("sentry-hook-signature".to_string(), sig);
                }
                WebhookAdapterId::Linear => {
                    let sig = compute_hmac_sha256_hex(&card.secret_key, payload).expect("hmac");
                    headers.insert("linear-signature".to_string(), sig);
                }
                WebhookAdapterId::Pagerduty => {
                    let sig = compute_hmac_sha256_hex(&card.secret_key, payload).expect("hmac");
                    headers.insert("x-pagerduty-signature".to_string(), format!("v1={sig}"));
                }
                WebhookAdapterId::Stripe => {
                    let ts = "1700000000";
                    let mut signed = format!("{ts}.").into_bytes();
                    signed.extend_from_slice(payload);
                    let sig = compute_hmac_sha256_hex(&card.secret_key, &signed).expect("hmac");
                    headers.insert(
                        "stripe-signature".to_string(),
                        format!("t={ts},v1={sig}"),
                    );
                }
            }

            let out = engine
                .ingest_call(IngestWebhookCallRequest {
                    hook_id: card.hook_id.clone(),
                    headers: headers.clone(),
                    raw_body: payload.to_vec(),
                    delivery_epoch: Some(1),
                    idempotency_key: None,
                    now_ms: 10_000,
                })
                .expect("ingest valid");
            assert_eq!(out.state, WebhookCallState::Delivered);
            let event = out.trigger_event.expect("trigger event");
            let event_json = serde_json::to_string(&event).expect("event json");
            assert!(!event_json.contains("\"text\""));
            assert!(!event_json.contains("\"url\""));
            assert!(!event_json.contains("\"place\""));
            assert!(!event_json.contains(&card.secret_key));
            let spooled = engine.read_spooled_payload(&out.call_id).expect("spooled");
            assert_eq!(spooled.body["action"], "opened");

            // Valid 14-state lifecycle transition Delivered -> Started -> Completed
            engine
                .transition_call_state(&out.call_id, WebhookCallState::Started, 10_500)
                .expect("started");
            let done = engine
                .transition_call_state(&out.call_id, WebhookCallState::Completed, 11_000)
                .expect("completed");
            assert_eq!(done.state, WebhookCallState::Completed);

            if adapter == WebhookAdapterId::Github {
                // 2nd call allowed, 3rd hits cap (2) and emits CapReached notice
                let _ = engine
                    .ingest_call(IngestWebhookCallRequest {
                        hook_id: card.hook_id.clone(),
                        headers: headers.clone(),
                        raw_body: payload.to_vec(),
                        delivery_epoch: Some(1),
                        idempotency_key: None,
                        now_ms: 10_100,
                    })
                    .expect("2nd");
                let capped = engine
                    .ingest_call(IngestWebhookCallRequest {
                        hook_id: card.hook_id.clone(),
                        headers: headers.clone(),
                        raw_body: payload.to_vec(),
                        delivery_epoch: Some(1),
                        idempotency_key: None,
                        now_ms: 10_200,
                    })
                    .expect("3rd capped");
                assert_eq!(capped.state, WebhookCallState::WaitingCapacity);
                assert!(capped.notice.is_some());

                // Rotate secret advances epoch; stale epoch delivery fails Expired
                let rotated = engine.rotate_hook_secret(&card.hook_id).expect("rotate");
                assert_eq!(rotated.epoch, 2);
                let stale_err = engine
                    .ingest_call(IngestWebhookCallRequest {
                        hook_id: card.hook_id.clone(),
                        headers,
                        raw_body: payload.to_vec(),
                        delivery_epoch: Some(1),
                        idempotency_key: None,
                        now_ms: 10_300,
                    })
                    .expect_err("stale epoch rejected");
                assert_eq!(stale_err.code(), BotErrorCode::Expired);
            }
        }
    }

    #[test]
    fn automation_31_kinds_local_google_once_skipped_watch_pr_and_wake_frame() {
        assert_eq!(all_supported_event_kinds().len(), 31);
        assert!(is_local_only_source(AutomationEventSource::Gmail));
        assert!(is_local_only_source(AutomationEventSource::Calendar));
        assert!(!is_local_only_source(AutomationEventSource::Github));

        let engine = AutomationTriggerEngine::new();
        let target = AutomationBotTarget {
            bot_id: "bot-1".to_string(),
            runs_in: AutomationBotRunsIn::New,
            results_to: AutomationBotResultsTo::Chat,
            agent_setup: None,
            let_bot_react: None,
            precheck: None,
            setup_trust_hash: None,
            missed_run_grace_minutes: None,
        };

        // pr_opened permits from_owner = true; issue_opened rejects from_owner = true
        let ok_pr = engine
            .register_automation(CreateAutomationRequest {
                name: "Review own PRs".to_string(),
                prompt: "Review PR".to_string(),
                enabled: true,
                bot_target: target.clone(),
                dtstart_ms: None,
                rrule: None,
                triggers: vec![AutomationEventTrigger::Connector(
                    AutomationConnectorEventTrigger {
                        id: "gh-pr".to_string(),
                        kind: AutomationEventKind::Github(GitHubEventKind::PrOpened),
                        in_places: vec!["acme/orca".to_string()],
                        from: vec![],
                        text_contains: None,
                        subject: None,
                        label: None,
                        run: None,
                        on_owner_message: None,
                        from_owner: Some(true),
                        names: BTreeMap::new(),
                    },
                )],
                once: None,
                now_ms: 1_000,
            })
            .expect("pr_opened allows from_owner");
        assert!(ok_pr.is_event_only());
        assert_eq!(ok_pr.dtstart_ms, EVENT_ONLY_SCHEDULE_DTSTART_MS);

        let bad_issue = engine.register_automation(CreateAutomationRequest {
            name: "Bad owner issue".to_string(),
            prompt: "Handle issue".to_string(),
            enabled: true,
            bot_target: target.clone(),
            dtstart_ms: None,
            rrule: None,
            triggers: vec![AutomationEventTrigger::Connector(
                AutomationConnectorEventTrigger {
                    id: "gh-issue".to_string(),
                    kind: AutomationEventKind::Github(GitHubEventKind::IssueOpened),
                    in_places: vec!["acme/orca".to_string()],
                    from: vec![],
                    text_contains: None,
                    subject: None,
                    label: None,
                    run: None,
                    on_owner_message: None,
                    from_owner: Some(true),
                    names: BTreeMap::new(),
                },
            )],
            once: None,
            now_ms: 1_000,
        });
        assert!(bad_issue.is_err());

        // Gmail trigger stays strictly local-host-only (never cloud-synced)
        let gmail_auto = engine
            .register_automation(CreateAutomationRequest {
                name: "Local Gmail triage".to_string(),
                prompt: "Check urgent mail".to_string(),
                enabled: true,
                bot_target: target.clone(),
                dtstart_ms: None,
                rrule: None,
                triggers: vec![AutomationEventTrigger::Google(
                    AutomationGoogleTrigger::Gmail {
                        id: "gm-1".to_string(),
                        account: Some("owner@example.com".to_string()),
                        from: vec![],
                        to: vec![],
                        subject_contains: Some("incident".to_string()),
                        label: None,
                    },
                )],
                once: Some(OneShotWait { by: None }),
                now_ms: 5_000,
            })
            .expect("gmail automation");
        assert!(gmail_auto.has_local_only_triggers());
        assert!(
            engine
                .list_cloud_syncable_triggers(&gmail_auto.id)
                .expect("cloud list")
                .is_empty()
        );

        // OneShotWait skipped-run immunity: skipped_* never burns the wait; Completed deletes it!
        let wait_id = gmail_auto.wait_id.expect("wait registered");
        let skipped = engine
            .waits()
            .record_run_outcome(
                &wait_id,
                AutomationRunTrigger::Event,
                AutomationRunStatus::SkippedPrecheck,
                6_000,
                None,
                "home",
            )
            .expect("skipped");
        assert!(!skipped.claimed);
        assert!(!skipped.consumed_and_deleted);
        assert!(engine.waits().get(&wait_id).expect("still there").is_some());

        let finished = engine
            .waits()
            .record_run_outcome(
                &wait_id,
                AutomationRunTrigger::Event,
                AutomationRunStatus::Completed,
                7_000,
                Some("pager@example.com".to_string()),
                "home",
            )
            .expect("completed");
        assert!(finished.consumed_and_deleted);
        assert!(matches!(finished.ending, Some(WaitEnding::Event { .. })));
        assert!(finished.notice_card.is_some());
        assert!(engine.waits().get(&wait_id).expect("deleted").is_none());

        // WatchPrCoordinator 2-attempt ceiling
        let watch = engine
            .watch_pr()
            .start_watch("bot-1", "home", "acme/orca#42", 10_000, None, true)
            .expect("start watch");
        assert_eq!(watch.policy.deadline_ms, 10_000 + WATCH_PR_DEFAULT_MS);
        let a1 = engine
            .watch_pr()
            .handle_wake(
                &watch.watch_id,
                WatchPrWakeOutcome::NeedsChange { authorized: true },
                11_000,
            )
            .expect("attempt 1");
        assert!(matches!(
            a1,
            WatchPrStepAction::DispatchWorkerFix {
                attempt: 1,
                next_attempt: 2,
                ..
            }
        ));
        let a2 = engine
            .watch_pr()
            .handle_wake(
                &watch.watch_id,
                WatchPrWakeOutcome::NeedsChange { authorized: true },
                12_000,
            )
            .expect("attempt 2");
        assert!(matches!(
            a2,
            WatchPrStepAction::DispatchWorkerFix {
                attempt: 2,
                next_attempt: 3,
                ..
            }
        ));
        let a3 = engine
            .watch_pr()
            .handle_wake(
                &watch.watch_id,
                WatchPrWakeOutcome::NeedsChange { authorized: true },
                13_000,
            )
            .expect("attempt 3 escalates");
        assert!(matches!(a3, WatchPrStepAction::EscalateToOwnerAndStop { .. }));

        // ContentFreeWakeFrame & WS close code constants
        assert_eq!(WS_CLOSE_SUPERSEDED, 4409);
        assert_eq!(WS_CLOSE_MESSAGE_TOO_BIG, 1009);
        let frame = ContentFreeWakeFrame::new("host-1", WakeFrameKind::Delivery, 7, 15_000)
            .expect("frame");
        let encoded = frame.encode_bounded_json().expect("encode");
        let decoded = ContentFreeWakeFrame::decode_bounded_json(&encoded).expect("decode");
        assert_eq!(decoded, frame);
        assert!(
            ContentFreeWakeFrame::decode_bounded_json(
                r#"{"hostId":"h","kind":"delivery","sequence":1,"timestampMs":1,"text":"secret"}"#
            )
            .is_err()
        );
    }
}
