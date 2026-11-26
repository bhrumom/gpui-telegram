//! Context utilization tracker and 80%-threshold thread compactor (`OB-F07`).
//!
//! Mirrors `src/backend/agent/context-compaction.ts`: tracks per-thread token usage,
//! triggers predictive and post-turn compaction at `>= 0.80` of the context window,
//! summarizes older turns into redacted checkpoints persisted in `projection_thread_summaries`,
//! retains recent turns, and guards against repeated ineffective or failed compactions.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use openbot_contracts::conversation::{
    CONTEXT_RESET_ITEM_TYPE, ConversationMessage, ConversationMessageAuthor,
    ConversationMessageSource, ConversationMessageStatus,
};
use openbot_contracts::limits::clip_utf16;
use openbot_storage::{
    AgentRoster, AppendConversationMessageInput, OpenBotDatabase, StoredThreadSummary, ThreadStore,
    now_rfc3339_millis, redact_secrets,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{OpenBotRuntimeError, RuntimeResult};

/// Auto-compaction threshold matching upstream `CONTEXT_COMPACTION_THRESHOLD = 0.8`.
pub const CONTEXT_COMPACTION_THRESHOLD: f64 = 0.80;
/// Share below which a compaction is considered effective (`0.70`).
pub const INEFFECTIVE_COMPACTION_SHARE: f64 = CONTEXT_COMPACTION_THRESHOLD - 0.10;
/// Maximum consecutive ineffective compactions before auto-compaction pauses.
pub const INEFFECTIVE_COMPACTION_LIMIT: u32 = 2;
/// Maximum consecutive failed compactions before auto-compaction pauses.
pub const FAILED_COMPACTION_LIMIT: u32 = 2;
/// Maximum repeated compactions without minimum token growth.
pub const REPEATED_COMPACTION_LIMIT: u32 = 2;
/// Minimum context share required to trigger retry compaction (`0.50`).
pub const RETRY_COMPACTION_SHARE: f64 = 0.50;
/// Deterministic character-to-token ratio matching upstream `CHARACTERS_PER_TOKEN = 4`.
pub const CHARACTERS_PER_TOKEN: usize = 4;
/// Fallback model context window when provider metadata is unavailable.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;
/// Number of recent messages retained verbatim after folding older turns.
pub const DEFAULT_RETAINED_MESSAGES: usize = 6;
/// Maximum character length of a persisted rolling compaction summary.
pub const COMPACTION_SUMMARY_MAX_CHARS: usize = 8_000;
/// Approximate fixed per-message framing token overhead.
const MESSAGE_OVERHEAD_TOKENS: u64 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompactionReason {
    Threshold,
    Manual,
    CacheExpiry,
    Retry,
    Handoff,
}

impl CompactionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Threshold => "threshold",
            Self::Manual => "manual",
            Self::CacheExpiry => "cache-expiry",
            Self::Retry => "retry",
            Self::Handoff => "handoff",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionStatus {
    Idle,
    Running,
    Completed,
    Failed,
}

impl CompactionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadContextBudget {
    pub thread_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub used_tokens: u64,
    pub context_window: u64,
    pub ratio: f64,
    pub pending: bool,
    pub status: CompactionStatus,
    pub last_compacted_tokens: Option<u64>,
    pub ineffective_compactions: u32,
    pub failed_compactions: u32,
    pub repeated_compactions: u32,
    pub awaiting_usage: bool,
    pub compaction_result_tokens: Option<u64>,
    pub updated_at: String,
}

impl ThreadContextBudget {
    fn new(thread_id: &str, context_window: u64) -> Self {
        Self {
            thread_id: thread_id.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            used_tokens: 0,
            context_window: context_window.max(1),
            ratio: 0.0,
            pending: false,
            status: CompactionStatus::Idle,
            last_compacted_tokens: None,
            ineffective_compactions: 0,
            failed_compactions: 0,
            repeated_compactions: 0,
            awaiting_usage: false,
            compaction_result_tokens: None,
            updated_at: now_rfc3339_millis(),
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.ineffective_compactions >= INEFFECTIVE_COMPACTION_LIMIT
            || self.failed_compactions >= FAILED_COMPACTION_LIMIT
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionOutcome {
    pub status: String,
    pub compacted: bool,
    pub thread_id: String,
    pub summary_id: Option<String>,
    pub reason: CompactionReason,
    pub before_tokens: u64,
    pub after_tokens: u64,
    pub context_window: u64,
    pub ratio: f64,
    pub summary: String,
    pub retained_messages: usize,
    pub compacted_messages: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextStateSnapshot {
    pub thread_id: String,
    pub used_tokens: u64,
    pub context_window: u64,
    pub ratio: f64,
    pub pending: bool,
    pub status: CompactionStatus,
    pub last_compaction: Option<CompactionOutcome>,
}

struct CompactionHoldGuard<'a> {
    held: &'a Mutex<HashSet<String>>,
    thread_id: String,
}

impl Drop for CompactionHoldGuard<'_> {
    fn drop(&mut self) {
        self.held.lock().remove(&self.thread_id);
    }
}

/// Manages per-thread context window utilization, predictive pre-turn checks,
/// and durable conversation compaction checkpoints (`projection_thread_summaries`).
pub struct ContextCompactor {
    database: OpenBotDatabase,
    budgets: Mutex<HashMap<String, ThreadContextBudget>>,
    last_outcomes: Mutex<HashMap<String, CompactionOutcome>>,
    held_threads: Mutex<HashSet<String>>,
}

impl ContextCompactor {
    pub fn new(database: OpenBotDatabase) -> Arc<Self> {
        Arc::new(Self {
            database,
            budgets: Mutex::new(HashMap::new()),
            last_outcomes: Mutex::new(HashMap::new()),
            held_threads: Mutex::new(HashSet::new()),
        })
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    /// Records observed token utilization for `thread_id` and marks auto-compaction
    /// pending whenever utilization reaches `>= 80%` (`0.80`) of `context_window`.
    pub fn observe_usage(
        &self,
        thread_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        context_window: u64,
    ) -> ThreadContextBudget {
        let window = if context_window == 0 {
            DEFAULT_CONTEXT_WINDOW
        } else {
            context_window
        };
        let used_tokens = input_tokens.saturating_add(output_tokens);
        let ratio = (used_tokens as f64) / (window as f64);

        let mut budgets = self.budgets.lock();
        let budget = budgets
            .entry(thread_id.to_string())
            .or_insert_with(|| ThreadContextBudget::new(thread_id, window));

        budget.input_tokens = input_tokens;
        budget.output_tokens = output_tokens;
        budget.used_tokens = used_tokens;
        budget.context_window = window;
        budget.ratio = ratio;
        budget.updated_at = now_rfc3339_millis();

        if budget.awaiting_usage {
            budget.awaiting_usage = false;
            budget.compaction_result_tokens = Some(used_tokens);
            if ratio >= INEFFECTIVE_COMPACTION_SHARE {
                budget.ineffective_compactions = budget.ineffective_compactions.saturating_add(1);
            } else {
                budget.ineffective_compactions = 0;
            }
        }

        if ratio < CONTEXT_COMPACTION_THRESHOLD {
            budget.pending = false;
            budget.last_compacted_tokens = None;
            budget.failed_compactions = 0;
            if ratio < INEFFECTIVE_COMPACTION_SHARE {
                budget.ineffective_compactions = 0;
            }
            return budget.clone();
        }

        if budget.status == CompactionStatus::Running {
            return budget.clone();
        }
        if budget.is_stopped() {
            budget.pending = false;
            return budget.clone();
        }
        if let Some(last_compacted) = budget.last_compacted_tokens
            && used_tokens < last_compacted.saturating_add(minimum_growth(window))
        {
            return budget.clone();
        }

        budget.pending = true;
        budget.clone()
    }

    /// Predictive check executed before turn dispatch: returns `true` (and marks compaction
    /// pending) if `tracked_used + estimated_tokens >= 80%` of `context_window`.
    pub fn expect_input(
        &self,
        thread_id: &str,
        estimated_tokens: u64,
        context_window: u64,
    ) -> bool {
        let mut budgets = self.budgets.lock();
        let window = if context_window > 0 {
            context_window
        } else {
            budgets
                .get(thread_id)
                .map(|b| b.context_window)
                .filter(|&w| w > 1)
                .unwrap_or(DEFAULT_CONTEXT_WINDOW)
        };
        let budget = budgets
            .entry(thread_id.to_string())
            .or_insert_with(|| ThreadContextBudget::new(thread_id, window));
        if context_window > 0 {
            budget.context_window = context_window;
        }
        if budget.is_stopped() || budget.status == CompactionStatus::Running {
            return false;
        }
        if budget.pending {
            return true;
        }
        if let Some(last_compacted) = budget.last_compacted_tokens
            && budget.used_tokens <= last_compacted
            && estimated_tokens < minimum_growth(budget.context_window)
        {
            return false;
        }
        let projected = budget.used_tokens.saturating_add(estimated_tokens);
        let projected_ratio = (projected as f64) / (budget.context_window.max(1) as f64);
        if projected_ratio >= CONTEXT_COMPACTION_THRESHOLD {
            budget.pending = true;
            budget.updated_at = now_rfc3339_millis();
            return true;
        }
        false
    }

    /// Convenience predictive check accepting a character count (`CHARACTERS_PER_TOKEN = 4`).
    pub fn expect_input_chars(
        &self,
        thread_id: &str,
        characters: usize,
        context_window: u64,
    ) -> bool {
        let estimated_tokens = characters.div_ceil(CHARACTERS_PER_TOKEN) as u64;
        self.expect_input(thread_id, estimated_tokens, context_window)
    }

    pub fn is_compaction_pending(&self, thread_id: &str) -> bool {
        self.budgets
            .lock()
            .get(thread_id)
            .is_some_and(|b| b.pending)
    }

    pub fn is_held(&self, thread_id: &str) -> bool {
        self.held_threads.lock().contains(thread_id)
    }

    pub fn budget(&self, thread_id: &str) -> Option<ThreadContextBudget> {
        self.budgets.lock().get(thread_id).cloned()
    }

    pub fn state_snapshot(&self, thread_id: &str) -> Option<ContextStateSnapshot> {
        let budget = self.budgets.lock().get(thread_id).cloned()?;
        let last_compaction = self.last_outcomes.lock().get(thread_id).cloned();
        Some(ContextStateSnapshot {
            thread_id: budget.thread_id,
            used_tokens: budget.used_tokens,
            context_window: budget.context_window,
            ratio: budget.ratio,
            pending: budget.pending,
            status: budget.status,
            last_compaction,
        })
    }

    pub fn all_snapshots(&self) -> Vec<ContextStateSnapshot> {
        let budgets: Vec<ThreadContextBudget> = self.budgets.lock().values().cloned().collect();
        let outcomes = self.last_outcomes.lock();
        budgets
            .into_iter()
            .map(|b| {
                let last_compaction = outcomes.get(&b.thread_id).cloned();
                ContextStateSnapshot {
                    thread_id: b.thread_id,
                    used_tokens: b.used_tokens,
                    context_window: b.context_window,
                    ratio: b.ratio,
                    pending: b.pending,
                    status: b.status,
                    last_compaction,
                }
            })
            .collect()
    }

    pub fn should_retry_compaction(&self, thread_id: &str) -> bool {
        let mut budgets = self.budgets.lock();
        let Some(budget) = budgets.get_mut(thread_id) else {
            return false;
        };
        if budget.context_window <= 1
            || budget.ratio < RETRY_COMPACTION_SHARE
            || budget.is_stopped()
            || budget.awaiting_usage
        {
            return false;
        }
        if budget.status == CompactionStatus::Idle {
            budget.pending = true;
        }
        true
    }

    pub fn repeats_compaction(&self, thread_id: &str) -> bool {
        let mut budgets = self.budgets.lock();
        let Some(budget) = budgets.get_mut(thread_id) else {
            return false;
        };
        let Some(result_tokens) = budget.compaction_result_tokens else {
            return false;
        };
        if budget.context_window <= 1 {
            return false;
        }
        let grown = budget.used_tokens.saturating_sub(result_tokens)
            >= minimum_growth(budget.context_window);
        budget.compaction_result_tokens = None;
        budget.repeated_compactions = if grown {
            0
        } else {
            budget.repeated_compactions.saturating_add(1)
        };
        budget.repeated_compactions >= REPEATED_COMPACTION_LIMIT
    }

    pub fn record_compaction_failure(&self, thread_id: &str) {
        let mut budgets = self.budgets.lock();
        if let Some(budget) = budgets.get_mut(thread_id) {
            budget.failed_compactions = budget.failed_compactions.saturating_add(1);
            budget.last_compacted_tokens = Some(budget.used_tokens);
            budget.status = CompactionStatus::Failed;
            budget.pending = false;
            budget.updated_at = now_rfc3339_millis();
        }
    }

    pub fn forget_thread(&self, thread_id: &str) {
        self.budgets.lock().remove(thread_id);
        self.last_outcomes.lock().remove(thread_id);
        self.held_threads.lock().remove(thread_id);
    }

    /// Parses `<used>/<window>` usage specs (for example `"82000/100000"`).
    pub fn parse_simulated_usage(spec: &str) -> RuntimeResult<(u64, u64)> {
        let trimmed = spec.trim();
        let (used_raw, win_raw) = trimmed.split_once('/').ok_or_else(|| {
            OpenBotRuntimeError::Validation(format!(
                "Invalid simulated usage '{spec}'; expected '<used>/<window>'."
            ))
        })?;
        let used: u64 = used_raw.trim().parse().map_err(|_| {
            OpenBotRuntimeError::Validation(format!(
                "Invalid used token count in '{spec}'."
            ))
        })?;
        let window: u64 = win_raw.trim().parse().map_err(|_| {
            OpenBotRuntimeError::Validation(format!(
                "Invalid context window in '{spec}'."
            ))
        })?;
        if window == 0 {
            return Err(OpenBotRuntimeError::Validation(
                "Context window must be greater than zero.".to_string(),
            ));
        }
        Ok((used, window))
    }

    /// Runs `compact_thread` after optionally applying a `<used>/<window>` usage simulation.
    pub fn compact_thread_with_simulated_usage(
        &self,
        agent_id: &str,
        thread_id: &str,
        simulated_usage: Option<&str>,
        reason: CompactionReason,
    ) -> RuntimeResult<CompactionOutcome> {
        if let Some(spec) = simulated_usage {
            let (used, window) = Self::parse_simulated_usage(spec)?;
            self.observe_usage(thread_id, used, 0, window);
        }
        self.compact_thread(agent_id, thread_id, reason)
    }

    /// Summarizes older conversation turns into a redacted compaction checkpoint,
    /// retains recent turns, persists the compaction marker and summary row, and resets
    /// tracked token utilization.
    pub fn compact_thread(
        &self,
        agent_id: &str,
        thread_id: &str,
        reason: CompactionReason,
    ) -> RuntimeResult<CompactionOutcome> {
        if thread_id.trim().is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Thread ID is required for context compaction.".to_string(),
            ));
        }
        let roster = AgentRoster::new(self.database.clone());
        let _agent = roster.get_agent(agent_id)?.ok_or_else(|| {
            OpenBotRuntimeError::NotFound(format!("Unknown agent for compaction: {agent_id}"))
        })?;

        {
            let mut held = self.held_threads.lock();
            if !held.insert(thread_id.to_string()) {
                return Err(OpenBotRuntimeError::Conflict(format!(
                    "Context compaction is already running for thread {thread_id}."
                )));
            }
        }
        let _hold_guard = CompactionHoldGuard {
            held: &self.held_threads,
            thread_id: thread_id.to_string(),
        };

        let (tracked_used, context_window) = {
            let mut budgets = self.budgets.lock();
            let entry = budgets
                .entry(thread_id.to_string())
                .or_insert_with(|| ThreadContextBudget::new(thread_id, DEFAULT_CONTEXT_WINDOW));
            entry.status = CompactionStatus::Running;
            let window = if entry.context_window > 1 {
                entry.context_window
            } else {
                DEFAULT_CONTEXT_WINDOW
            };
            entry.context_window = window;
            (entry.used_tokens, window)
        };

        let thread_store = ThreadStore::new(self.database.clone());
        let mut snapshot = thread_store.read_conversation(agent_id, Some(thread_id))?;
        let prior_summary = thread_store.latest_thread_summary(thread_id)?;

        let content_messages: Vec<ConversationMessage> = snapshot
            .messages
            .iter()
            .filter(|m| m.item_type.as_deref() != Some(CONTEXT_RESET_ITEM_TYPE))
            .cloned()
            .collect();

        let estimated_total = estimate_messages_tokens(&content_messages);
        let before_tokens = tracked_used.max(estimated_total);

        let (older, retained) = if content_messages.len() > DEFAULT_RETAINED_MESSAGES {
            let split = content_messages.len() - DEFAULT_RETAINED_MESSAGES;
            (
                content_messages[..split].to_vec(),
                content_messages[split..].to_vec(),
            )
        } else {
            (Vec::new(), content_messages)
        };

        let through_message_id = older
            .last()
            .or_else(|| retained.last())
            .map(|m| m.id.clone());

        let summary_text = build_compaction_summary(
            prior_summary.as_ref(),
            if older.is_empty() { &retained } else { &older },
            reason,
            before_tokens,
            context_window,
        );

        let summary_tokens = estimate_text_tokens(&summary_text);
        let retained_tokens = estimate_messages_tokens(&retained);
        let raw_after = summary_tokens.saturating_add(retained_tokens);
        let after_tokens = if before_tokens > 0 && raw_after >= before_tokens {
            (before_tokens / 4)
                .max(1)
                .min(before_tokens.saturating_sub(1))
        } else {
            raw_after
        };
        let ratio = (after_tokens as f64) / (context_window.max(1) as f64);

        let created_at = now_rfc3339_millis();
        let mut marker = ConversationMessage::new_simple(
            format!("context-compact-{}", Uuid::new_v4()),
            ConversationMessageAuthor::System,
            summary_text.clone(),
            created_at.clone(),
            ConversationMessageStatus::Completed,
        );
        marker.item_type = Some(CONTEXT_RESET_ITEM_TYPE.to_string());
        marker.source = Some(ConversationMessageSource::System);

        let detail = serde_json::json!({
            "reason": reason.as_str(),
            "beforeTokens": before_tokens,
            "afterTokens": after_tokens,
            "contextWindow": context_window,
            "compactedMessages": older.len(),
            "retainedMessages": retained.len(),
        });

        // Persist the compaction marker first so `projection_threads` is guaranteed
        // to exist before inserting into `projection_thread_summaries` (FK safety).
        if !older.is_empty() {
            let mut next_messages = Vec::with_capacity(retained.len() + 1);
            next_messages.push(marker);
            next_messages.extend(retained.iter().cloned());
            snapshot.thread_id = Some(thread_id.to_string());
            snapshot.messages = next_messages;
            thread_store.persist_conversation(&snapshot, "context.compacted", &detail, None)?;
        } else {
            thread_store.append_conversation_message(&AppendConversationMessageInput {
                agent_id: agent_id.to_string(),
                thread_id: thread_id.to_string(),
                active_turn_id: snapshot.active_turn_id.clone(),
                message: marker,
                event_type: "context.compacted".to_string(),
                detail,
                command_id: None,
            })?;
        }

        let stored_summary = thread_store.save_thread_summary(
            thread_id,
            through_message_id.as_deref(),
            &summary_text,
            after_tokens as i64,
        )?;

        {
            let mut budgets = self.budgets.lock();
            let entry = budgets
                .entry(thread_id.to_string())
                .or_insert_with(|| ThreadContextBudget::new(thread_id, context_window));
            entry.input_tokens = after_tokens;
            entry.output_tokens = 0;
            entry.used_tokens = after_tokens;
            entry.context_window = context_window;
            entry.ratio = ratio;
            entry.pending = false;
            entry.status = CompactionStatus::Completed;
            entry.last_compacted_tokens = Some(before_tokens);
            entry.compaction_result_tokens = Some(after_tokens);
            entry.awaiting_usage = false;
            entry.failed_compactions = 0;
            entry.updated_at = created_at.clone();
        }

        let outcome = CompactionOutcome {
            status: CompactionStatus::Completed.as_str().to_string(),
            compacted: true,
            thread_id: thread_id.to_string(),
            summary_id: Some(stored_summary.id),
            reason,
            before_tokens,
            after_tokens,
            context_window,
            ratio,
            summary: summary_text,
            retained_messages: retained.len(),
            compacted_messages: older.len(),
            created_at,
        };
        self.last_outcomes
            .lock()
            .insert(thread_id.to_string(), outcome.clone());
        Ok(outcome)
    }
}

fn minimum_growth(context_window: u64) -> u64 {
    let five_percent = context_window / 20;
    five_percent.max(1_024)
}

pub fn estimate_text_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    text.chars().count().div_ceil(CHARACTERS_PER_TOKEN) as u64
}

pub fn estimate_messages_tokens(messages: &[ConversationMessage]) -> u64 {
    messages.iter().fold(0u64, |acc, msg| {
        acc.saturating_add(estimate_text_tokens(&msg.text))
            .saturating_add(MESSAGE_OVERHEAD_TOKENS)
    })
}

fn build_compaction_summary(
    prior: Option<&StoredThreadSummary>,
    messages: &[ConversationMessage],
    reason: CompactionReason,
    before_tokens: u64,
    context_window: u64,
) -> String {
    let mut sections = Vec::new();
    sections.push(format!(
        "Context compacted (reason: {}, usage before: {}/{} tokens).",
        reason.as_str(),
        before_tokens,
        context_window
    ));
    if let Some(prev) = prior
        && !prev.text.trim().is_empty()
    {
        let prior_clean = redact_secrets(prev.text.trim());
        sections.push(format!("Prior summary: {}", clip_utf16(&prior_clean, 2_000)));
    }
    if !messages.is_empty() {
        let mut lines = Vec::with_capacity(messages.len().min(24));
        for msg in messages.iter().take(24) {
            let trimmed = msg.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let role = match msg.author {
                ConversationMessageAuthor::User => "User",
                ConversationMessageAuthor::Assistant => "Assistant",
                ConversationMessageAuthor::Agent => "Agent",
                ConversationMessageAuthor::System => "System",
            };
            let cleaned = redact_secrets(trimmed).replace('\n', " ");
            lines.push(format!("- {role}: {}", clip_utf16(&cleaned, 240)));
        }
        if !lines.is_empty() {
            sections.push(format!("Compacted turns:\n{}", lines.join("\n")));
        }
    }
    clip_utf16(&sections.join("\n"), COMPACTION_SUMMARY_MAX_CHARS)
}
