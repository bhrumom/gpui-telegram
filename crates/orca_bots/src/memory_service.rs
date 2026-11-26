//! Bot memory governance runtime: authority-ordered preamble, SHA-256 turn delta sync,
//! turn-end provenance tagging & 14-day Re-Add Guard, forget/undo history, deterministic
//! monthly closed-work journal (3-attempt CAS), and overnight tool-less 3AM-8AM Tidy-Up.

use std::collections::HashSet;
use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Datelike, Days, FixedOffset, NaiveDate, TimeDelta, Timelike, Utc};
use orca_core::bot_memory::{
    BOT_MEMORY_ARCHIVE_TOPIC, BotMemoryAuthor, BotMemoryChange, BotMemoryHistoryRow,
    BotMemoryLine, BotTidyUpSummary, ForgetMemoryResult, JournalAppendResult, JournalEntryInput,
    MemoryUndoResult, READD_GUARD_MS, ReaddInterceptedLine, RecentlyRemovedTidyUpLine,
    TIDY_UP_HOURS_FROM, TIDY_UP_HOURS_UNTIL, TidyUpDraft, TidyUpError, TidyUpErrorCode,
    TidyUpPlan, TidyUpValidationContext, append_journal_lines_idempotent, apply_bot_memory_change,
    apply_memory_undo, bot_memory_change, evaluate_readd_guard, forget_memory_fact,
    format_monthly_journal_lines, format_readd_archive_update, format_tidy_up_topic_file,
    is_valid_iso_date, parse_tidy_up_reply, recently_removed_from_history, tag_memory_changes,
    validate_tidy_up_against_live, validate_tidy_up_draft_with_options,
};
use orca_core::bot_preamble::{
    BOT_MEMORY_TOPICS_KEPT_BYTES, BOT_MEMORY_TOPICS_MAX_FILES, BotMemoryDeltaTracker,
    BotMemoryTopicSnapshot, BotPreambleInput, BotTurnDelta, bot_first_run_rules,
    bot_rules_update_key, build_bot_preamble, build_rules_update_notice,
};
use orca_core::bot_types::{BotAgentId, BotFolderSlug, OrcaBotRecord};
use orca_core::{BOT_MEMORY_INDEX_LINE_CAP, BotErrorCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::home_store::{BotHomeStore, atomic_write_bytes};
use crate::{OrcaBotsError, Result};

pub const TIDY_UP_MAX_CHATS: usize = 20;
pub const TIDY_UP_MAX_CHARS_PER_CHAT: usize = 4_000;
pub const TIDY_UP_MAX_TOTAL_CHARS: usize = 40_000;
pub const TIDY_UP_MAX_TOPICS_CHARS: usize = 12_000;
pub const TIDY_UP_MAX_ARCHIVE_CHARS: usize = 6_000;
pub const TIDY_UP_AUTO_OFF_FAILURE_THRESHOLD: u32 = 3;
const JOURNAL_CAS_MAX_ATTEMPTS: usize = 3;

const MEMORY_HISTORY_FILE: &str = "memory_history.json";
const TIDY_UP_STATE_FILE: &str = "tidy_up_state.json";

/// Frequency preset for overnight tool-less Tidy-Up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TidyUpSchedule {
    #[default]
    Night,
    Week,
}

/// Persisted per-bot Tidy-Up schedule, failure streak, auto-off flag, and 14-day Re-Add ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TidyUpRuntimeState {
    #[serde(default)]
    pub schedule: TidyUpSchedule,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub turned_off: bool,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_summary: Option<BotTidyUpSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readd_ledger: Vec<RecentlyRemovedTidyUpLine>,
}

impl Default for TidyUpRuntimeState {
    fn default() -> Self {
        Self {
            schedule: TidyUpSchedule::Night,
            enabled: false,
            turned_off: false,
            consecutive_failures: 0,
            since_ms: None,
            last_run_at_ms: None,
            last_error: None,
            last_summary: None,
            readd_ledger: Vec::new(),
        }
    }
}

/// A single turn in a bot chat transcript supplied to Tidy-Up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TidyUpChatTurn {
    pub chat_id: String,
    pub is_owner_authored: bool,
    pub is_external_or_automation: bool,
    pub text: String,
    pub timestamp_ms: i64,
}

/// A chat transcript considered by overnight Tidy-Up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TidyUpChatTranscript {
    pub chat_id: String,
    pub turns: Vec<TidyUpChatTurn>,
}

/// Bounded owner-only transcript context extracted for tool-less Tidy-Up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedOwnerChats {
    pub chats_reviewed: usize,
    pub text: String,
}

/// Strictly tool-less text prompt bundle passed to a `TidyUpDraftProvider`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TidyUpPromptInput {
    pub bot_id: String,
    pub bot_name: String,
    pub agent: BotAgentId,
    pub memory_md: String,
    pub topics_context: String,
    pub archive_context: String,
    pub owner_chats_context: String,
    pub chats_reviewed: usize,
    pub today: String,
}

/// Either a raw fenced markdown reply (parsed via `parse_tidy_up_reply`) or a structured draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TidyUpDraftResponse {
    RawReply(String),
    Draft(TidyUpDraft),
    StructuredDraft(TidyUpDraft),
}

/// Output returned by a `TidyUpDraftProvider`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TidyUpProviderOutput {
    pub response: TidyUpDraftResponse,
    pub elapsed_ms: u64,
}

/// Async tool-less draft provider invoked by `BotMemoryService::run_tidy_up`.
pub trait TidyUpDraftProvider: Send + Sync {
    fn draft_tidy_up<'a>(
        &'a self,
        input: TidyUpPromptInput,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<TidyUpProviderOutput, TidyUpError>> + Send + 'a>>;
}

/// Request parameters for executing overnight Tidy-Up.
#[derive(Debug, Clone)]
pub struct TidyUpRunRequest {
    pub chats: Vec<TidyUpChatTranscript>,
    pub now_ms: i64,
    pub today: String,
    pub utc_offset_seconds: i32,
    pub allow_sleep_catchup: bool,
    pub force: bool,
}

/// Outcome of `BotMemoryService::run_tidy_up`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TidyUpRunOutcome {
    SkippedNotDue,
    Applied {
        plan: Box<TidyUpPlan>,
        home_chat_note: String,
    },
}

/// Result of mid-session delta sync across `SOUL.md`, `MEMORY.md`, `memory/*.md`, and rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnMemorySyncResult {
    pub delta: BotTurnDelta,
    pub rules_update: Option<String>,
    pub first_run_rules: Option<String>,
}

impl TurnMemorySyncResult {
    pub fn is_empty(&self) -> bool {
        self.delta.is_empty() && self.rules_update.is_none() && self.first_run_rules.is_none()
    }
}

/// Stateful per-chat turn sync tracker wrapping `orca_core::bot_preamble::BotMemoryDeltaTracker`.
#[derive(Debug, Clone)]
pub struct BotTurnMemorySync {
    tracker: BotMemoryDeltaTracker,
    last_rules_key: String,
}

/// Result of turn-end memory provenance tagging, deletion restoration, and 14-day Re-Add Guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEndMemoryGovernanceResult {
    pub final_markdown: String,
    pub tagged_lines: usize,
    pub restored_lines: Vec<BotMemoryLine>,
    pub intercepted_for_archive: Vec<ReaddInterceptedLine>,
    pub change: BotMemoryChange,
    pub history_row: Option<BotMemoryHistoryRow>,
}

impl BotTurnMemorySync {
    /// Captures the initial baseline fingerprints of `SOUL.md`, `MEMORY.md`, `memory/*.md`,
    /// and the rules update key for a newly opened bot chat session.
    pub fn capture(
        home_store: &BotHomeStore,
        bot: &OrcaBotRecord,
        cli_cmd: Option<&str>,
    ) -> Result<Self> {
        home_store.assert_bot_owns_home(&bot.folder, &bot.id)?;
        let dir = home_store.bot_dir(&bot.folder);
        let soul = home_store.read_text_optional(&dir.join("SOUL.md"))?;
        let memory = home_store.read_text_optional(&dir.join("MEMORY.md"))?;
        let topics = scan_memory_topic_snapshots(&dir.join("memory"))?;
        let tracker = BotMemoryDeltaTracker::new(
            soul.as_deref(),
            memory.as_deref(),
            &topics,
            BOT_MEMORY_INDEX_LINE_CAP,
        );
        let cli = cli_cmd.unwrap_or("orca");
        let last_rules_key = bot_rules_update_key(bot.name.as_str(), bot.agent, cli);
        Ok(Self {
            tracker,
            last_rules_key,
        })
    }

    /// Computes minimal fenced mid-session deltas for `SOUL.md`, `MEMORY.md` (with 200-line
    /// overflow guard), `memory/*.md`, `[Orca rules update]`, and `first_run` rules.
    pub fn sync_turn_memory_delta(
        &mut self,
        home_store: &BotHomeStore,
        bot: &OrcaBotRecord,
        cli_cmd: Option<&str>,
    ) -> Result<TurnMemorySyncResult> {
        home_store.assert_bot_owns_home(&bot.folder, &bot.id)?;
        let dir = home_store.bot_dir(&bot.folder);
        let soul = home_store.read_text_optional(&dir.join("SOUL.md"))?;
        let memory = home_store.read_text_optional(&dir.join("MEMORY.md"))?;
        let topics = scan_memory_topic_snapshots(&dir.join("memory"))?;
        let delta = self
            .tracker
            .compute_and_advance(soul.as_deref(), memory.as_deref(), &topics);

        let cli = cli_cmd.unwrap_or("orca");
        let next_rules_key = bot_rules_update_key(bot.name.as_str(), bot.agent, cli);
        let rules_update = if next_rules_key != self.last_rules_key {
            self.last_rules_key = next_rules_key;
            Some(build_rules_update_notice(bot.name.as_str(), bot.agent, cli))
        } else {
            None
        };
        let first_run_rules = if bot.first_run {
            Some(bot_first_run_rules(cli))
        } else {
            None
        };

        Ok(TurnMemorySyncResult {
            delta,
            rules_update,
            first_run_rules,
        })
    }
}

/// Host memory governance service integrating `BotHomeStore`, provenance tagging, 14-day Re-Add
/// Guard, forget/undo history, monthly closed-work journal, and overnight tool-less Tidy-Up.
#[derive(Debug, Clone)]
pub struct BotMemoryService {
    home_store: BotHomeStore,
    state_dir: PathBuf,
}

impl BotMemoryService {
    /// Creates a new memory service storing per-bot history & Tidy-Up state under `state_dir`.
    pub fn new(home_store: BotHomeStore, state_dir: impl Into<PathBuf>) -> Self {
        Self {
            home_store,
            state_dir: state_dir.into(),
        }
    }

    pub fn home_store(&self) -> &BotHomeStore {
        &self.home_store
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Assembles the authority-ordered system preamble reading live `SOUL.md` & `MEMORY.md`,
    /// appending `first_run` rules when `bot.first_run` is true and optional `[Orca rules update]`.
    pub fn build_session_preamble(
        &self,
        bot: &OrcaBotRecord,
        cli_cmd: Option<&str>,
        previous_rules_key: Option<&str>,
    ) -> Result<(String, String)> {
        self.home_store.assert_bot_owns_home(&bot.folder, &bot.id)?;
        let dir = self.home_store.bot_dir(&bot.folder);
        let soul = self.home_store.read_text_optional(&dir.join("SOUL.md"))?;
        let memory = self.home_store.read_text_optional(&dir.join("MEMORY.md"))?;
        let cli = cli_cmd.unwrap_or("orca");

        let mut preamble = build_bot_preamble(&BotPreambleInput {
            name: bot.name.as_str(),
            agent: bot.agent,
            soul: soul.as_deref(),
            memory: memory.as_deref(),
            cli_cmd: Some(cli),
        });

        let rules_key = bot_rules_update_key(bot.name.as_str(), bot.agent, cli);
        if let Some(prev) = previous_rules_key
            && prev != rules_key
        {
            preamble.push_str("\n\n");
            preamble.push_str(&build_rules_update_notice(bot.name.as_str(), bot.agent, cli));
        }

        if bot.first_run {
            preamble.push_str("\n\n");
            preamble.push_str(&bot_first_run_rules(cli));
        }

        Ok((preamble, rules_key))
    }

    /// Enforces turn-end memory governance:
    /// 1. Provenance tagging, ` · from:` separator defusing, and silent deletion restoration.
    /// 2. 14-day Re-Add Guard: writes intercepted lines to `memory/archive.md` suffixed
    ///    ` · kept out YYYY-MM-DD` BEFORE stripping them from `MEMORY.md`.
    /// 3. Atomic `MEMORY.md` update and persisted `BotMemoryHistoryRow`.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_turn_end_memory_governance(
        &self,
        bot: &OrcaBotRecord,
        before_memory_md: &str,
        turn_source: Option<&str>,
        owner_turn_text: &str,
        session_id: &str,
        allow_deletions: bool,
        now_ms: i64,
        today: &str,
    ) -> Result<TurnEndMemoryGovernanceResult> {
        self.home_store.assert_bot_owns_home(&bot.folder, &bot.id)?;
        let dir = self.home_store.bot_dir(&bot.folder);
        let memory_path = dir.join("MEMORY.md");
        let after_memory_md = self
            .home_store
            .read_text_optional(&memory_path)?
            .unwrap_or_default();

        let tagged = tag_memory_changes(
            before_memory_md,
            &after_memory_md,
            turn_source,
            allow_deletions,
        );

        let mut history = self.load_history(&bot.id)?;
        let tidy_state = self.load_tidy_up_state(&bot.id)?;
        let recently_removed = collect_recently_removed(&history, &tidy_state, now_ms);
        let newly_added: Vec<String> = tagged.change.added.iter().map(|l| l.line.clone()).collect();
        let readd = evaluate_readd_guard(
            &newly_added,
            &recently_removed,
            owner_turn_text,
            now_ms,
            today,
        );

        let mut final_markdown = tagged.markdown;
        if !readd.intercepted_for_archive.is_empty() {
            let archive_path = dir.join("memory").join("archive.md");
            let existing_archive = self.home_store.read_text_optional(&archive_path)?;
            if let Some(updated_archive) =
                format_readd_archive_update(existing_archive.as_deref(), &readd)
            {
                // CRITICAL INVARIANT: persist archive.md BEFORE removing lines from MEMORY.md.
                self.home_store
                    .replace_bytes(&archive_path, updated_archive.as_bytes())?;
            }
            let removal = BotMemoryChange {
                added: Vec::new(),
                removed: readd
                    .intercepted_for_archive
                    .iter()
                    .map(|item| BotMemoryLine {
                        line: item.original_line.clone(),
                        after: None,
                    })
                    .collect(),
            };
            final_markdown = apply_bot_memory_change(&final_markdown, &removal);
        }

        if final_markdown != after_memory_md {
            self.home_store
                .replace_bytes(&memory_path, final_markdown.as_bytes())?;
        }

        let final_change = bot_memory_change(before_memory_md, &final_markdown);
        let history_row = if final_change.is_empty() {
            None
        } else {
            let by = if session_id.trim().is_empty() {
                BotMemoryAuthor::Owner
            } else {
                BotMemoryAuthor::Chat {
                    session_id: session_id.to_string(),
                    source: turn_source.map(str::to_string),
                }
            };
            let row = BotMemoryHistoryRow {
                id: Uuid::new_v4().to_string(),
                at_ms: now_ms,
                by,
                change: final_change.clone(),
                undoable: true,
            };
            history.insert(0, row.clone());
            self.save_history(&bot.id, &history)?;
            Some(row)
        };

        Ok(TurnEndMemoryGovernanceResult {
            final_markdown,
            tagged_lines: tagged.tagged_lines,
            restored_lines: tagged.restored,
            intercepted_for_archive: readd.intercepted_for_archive,
            change: final_change,
            history_row,
        })
    }

    /// Forgets a memory fact matching `query`, persists `MEMORY.md` via CAS `replace_text`,
    /// and prepends a `BotMemoryHistoryRow` to persisted history.
    pub fn forget_fact(
        &self,
        bot: &OrcaBotRecord,
        query: &str,
        by: BotMemoryAuthor,
        now_ms: i64,
    ) -> Result<ForgetMemoryResult> {
        self.home_store.assert_bot_owns_home(&bot.folder, &bot.id)?;
        let memory_path = self.home_store.bot_dir(&bot.folder).join("MEMORY.md");
        let current_md = self
            .home_store
            .read_text_optional(&memory_path)?
            .unwrap_or_default();

        let res = forget_memory_fact(
            &current_md,
            query,
            Uuid::new_v4().to_string(),
            now_ms,
            by,
        )?;
        self.home_store
            .replace_text(&memory_path, Some(&current_md), &res.markdown)?;

        let mut history = self.load_history(&bot.id)?;
        history.insert(0, res.history_entry.clone());
        self.save_history(&bot.id, &history)?;
        Ok(res)
    }

    /// Undoes a previously recorded `BotMemoryHistoryRow`, updating `MEMORY.md` and history.
    pub fn undo_memory_change(
        &self,
        bot: &OrcaBotRecord,
        target_row_id: &str,
        now_ms: i64,
    ) -> Result<MemoryUndoResult> {
        self.home_store.assert_bot_owns_home(&bot.folder, &bot.id)?;
        let memory_path = self.home_store.bot_dir(&bot.folder).join("MEMORY.md");
        let current_md = self
            .home_store
            .read_text_optional(&memory_path)?
            .unwrap_or_default();
        let mut history = self.load_history(&bot.id)?;

        let res = apply_memory_undo(
            &current_md,
            &history,
            target_row_id,
            Uuid::new_v4().to_string(),
            now_ms,
        )?;
        self.home_store
            .replace_text(&memory_path, Some(&current_md), &res.markdown)?;

        for row in &mut history {
            if row.id == target_row_id {
                row.undoable = false;
            }
        }
        history.insert(0, res.history_entry.clone());
        self.save_history(&bot.id, &history)?;
        Ok(res)
    }

    /// Returns persisted memory history rows (newest-first) for `bot_id`.
    pub fn list_history(&self, bot_id: &str) -> Result<Vec<BotMemoryHistoryRow>> {
        self.load_history(bot_id)
    }

    /// Deterministically appends a closed Bot Item entry to `memory/journal/YYYY-MM.md`
    /// with zero LLM calls, 3-attempt CAS retry via `BotHomeStore::replace_text`,
    /// and idempotent `YYYY-MM-DD | item #<n>` deduplication.
    pub fn append_closed_item_journal(
        &self,
        bot_folder: &BotFolderSlug,
        bot_id: &str,
        entry: &JournalEntryInput,
    ) -> Result<JournalAppendResult> {
        self.home_store.assert_bot_owns_home(bot_folder, bot_id)?;
        if !is_valid_iso_date(&entry.date) {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                format!("Invalid ISO date '{}' for monthly journal entry.", entry.date),
            ));
        }
        let month = &entry.date[..7];
        let lines = format_monthly_journal_lines(entry);
        let journal_path = self
            .home_store
            .resolve_relative_path(bot_folder, &format!("memory/journal/{month}.md"))?;

        for attempt in 0..JOURNAL_CAS_MAX_ATTEMPTS {
            let existing = self.home_store.read_text_optional(&journal_path)?;
            let appended = append_journal_lines_idempotent(existing.as_deref(), month, &lines);
            if !appended.changed {
                return Ok(appended);
            }
            match self.home_store.replace_text(
                &journal_path,
                existing.as_deref(),
                &appended.markdown,
            ) {
                Ok(()) => return Ok(appended),
                Err(OrcaBotsError::PreconditionFailed { .. })
                    if attempt + 1 < JOURNAL_CAS_MAX_ATTEMPTS =>
                {
                    continue;
                }
                Err(err) => return Err(err),
            }
        }

        Err(OrcaBotsError::PreconditionFailed {
            path: journal_path.display().to_string(),
        })
    }

    /// Loads the persisted Tidy-Up runtime state for `bot_id`.
    pub fn get_tidy_up_state(&self, bot_id: &str) -> Result<TidyUpRuntimeState> {
        self.load_tidy_up_state(bot_id)
    }

    /// Configures Tidy-Up enabled flag & schedule, stamping `since_ms` when enabled (`false -> true`)
    /// and resetting `turned_off` / `consecutive_failures` when explicitly re-enabled.
    pub fn configure_tidy_up(
        &self,
        bot_id: &str,
        enabled: bool,
        schedule: TidyUpSchedule,
        now_ms: i64,
    ) -> Result<TidyUpRuntimeState> {
        let mut state = self.load_tidy_up_state(bot_id)?;
        if !state.enabled && enabled {
            state.since_ms = Some(now_ms);
            state.turned_off = false;
            state.consecutive_failures = 0;
            state.last_error = None;
        }
        state.enabled = enabled;
        state.schedule = schedule;
        self.save_tidy_up_state(bot_id, &state)?;
        Ok(state)
    }

    /// Evaluates whether Tidy-Up is due under the 3AM-8AM local schedule window (+ sleep catch-up).
    pub fn is_tidy_up_due(
        &self,
        bot_id: &str,
        agent: BotAgentId,
        now_ms: i64,
        utc_offset_seconds: i32,
        allow_sleep_catchup: bool,
    ) -> Result<bool> {
        if !agent.supports_tidy_up() {
            return Ok(false);
        }
        let state = self.load_tidy_up_state(bot_id)?;
        evaluate_tidy_up_due(&state, now_ms, utc_offset_seconds, allow_sleep_catchup)
    }

    /// Executes the overnight tool-less Tidy-Up flow:
    /// - Enforces agent support (`claude`/`codex`) and 3AM-8AM (+ sleep catch-up) window.
    /// - Extracts strictly owner-authored turns (`<=20` chats, `<=4k`/chat, `<=40k` total).
    /// - Validates draft against all 16 `TidyUpErrorCode` variants + protected owner lines + live race guard.
    /// - Writes `memory/<topic>.md` and `memory/archive.md` BEFORE updating `MEMORY.md`.
    /// - Records 14-day Re-Add Guard ledger + 1-click Undo history row + Home Chat note `Tidied memory · N changes`.
    /// - Auto-disables (`turned_off = true`) after `>= 3` consecutive failures.
    pub async fn run_tidy_up<P: TidyUpDraftProvider>(
        &self,
        bot: &OrcaBotRecord,
        provider: &P,
        req: TidyUpRunRequest,
    ) -> Result<TidyUpRunOutcome> {
        self.home_store.assert_bot_owns_home(&bot.folder, &bot.id)?;
        if !bot.agent.supports_tidy_up() {
            return Err(OrcaBotsError::TidyUp(TidyUpError::new(
                TidyUpErrorCode::UnsupportedAgent,
                format!("agent '{}' does not support tidy-up", bot.agent.as_str()),
            )));
        }

        let mut state = self.load_tidy_up_state(&bot.id)?;
        if !req.force {
            let due = evaluate_tidy_up_due(
                &state,
                req.now_ms,
                req.utc_offset_seconds,
                req.allow_sleep_catchup,
            )?;
            if !due {
                return Ok(TidyUpRunOutcome::SkippedNotDue);
            }
        }

        let dir = self.home_store.bot_dir(&bot.folder);
        let memory_path = dir.join("MEMORY.md");
        let before_md = self
            .home_store
            .read_text_optional(&memory_path)?
            .unwrap_or_default();
        let before_sha256 = hex::encode(Sha256::digest(before_md.as_bytes()));
        let topics_context = collect_bounded_topics_context(&dir.join("memory"))?;
        let archive_context = self
            .home_store
            .read_text_optional(&dir.join("memory").join("archive.md"))?
            .map(|s| truncate_chars(&s, TIDY_UP_MAX_ARCHIVE_CHARS))
            .unwrap_or_default();
        let extracted = extract_owner_only_chat_context(&req.chats);

        let prompt_input = TidyUpPromptInput {
            bot_id: bot.id.clone(),
            bot_name: bot.name.as_str().to_string(),
            agent: bot.agent,
            memory_md: before_md.clone(),
            topics_context,
            archive_context,
            owner_chats_context: extracted.text,
            chats_reviewed: extracted.chats_reviewed,
            today: req.today.clone(),
        };

        let provider_out = match provider.draft_tidy_up(prompt_input).await {
            Ok(out) => out,
            Err(err) => {
                self.record_tidy_up_failure(&bot.id, &mut state, &err.to_string())?;
                return Err(OrcaBotsError::TidyUp(err));
            }
        };

        let (draft, allow_new_topic_lines) = match provider_out.response {
            TidyUpDraftResponse::RawReply(raw) => match parse_tidy_up_reply(&raw) {
                Ok(d) => (d, false),
                Err(err) => {
                    self.record_tidy_up_failure(&bot.id, &mut state, &err.to_string())?;
                    return Err(OrcaBotsError::TidyUp(err));
                }
            },
            TidyUpDraftResponse::Draft(d) => (d, false),
            TidyUpDraftResponse::StructuredDraft(d) => (d, true),
        };

        let ctx = TidyUpValidationContext {
            before_md: &before_md,
            expected_before_sha256: Some(before_sha256),
            agent: bot.agent,
            elapsed_ms: provider_out.elapsed_ms,
            today: &req.today,
            now_ms: req.now_ms,
            chats_reviewed: extracted.chats_reviewed,
            history_row_id: Uuid::new_v4().to_string(),
        };

        let plan = match validate_tidy_up_draft_with_options(&ctx, &draft, allow_new_topic_lines) {
            Ok(p) => p,
            Err(err) => {
                self.record_tidy_up_failure(&bot.id, &mut state, &err.to_string())?;
                return Err(OrcaBotsError::TidyUp(err));
            }
        };

        let live_md = self
            .home_store
            .read_text_optional(&memory_path)?
            .unwrap_or_default();
        let updated_memory_md = match validate_tidy_up_against_live(&plan, &live_md) {
            Ok(md) => md,
            Err(err) => {
                self.record_tidy_up_failure(&bot.id, &mut state, &err.to_string())?;
                return Err(OrcaBotsError::TidyUp(err));
            }
        };

        // Write topic files and archive BEFORE updating MEMORY.md so no moved/filed facts are lost.
        for (topic, lines) in &plan.topic_writes {
            let topic_path = self
                .home_store
                .resolve_relative_path(&bot.folder, &format!("memory/{topic}.md"))?;
            let existing = self.home_store.read_text_optional(&topic_path)?;
            if let Some(updated_topic) =
                format_tidy_up_topic_file(topic, existing.as_deref(), lines)
            {
                self.home_store
                    .replace_bytes(&topic_path, updated_topic.as_bytes())?;
            }
        }

        if !plan.archive_lines.is_empty() {
            let archive_path = dir.join("memory").join("archive.md");
            let existing = self.home_store.read_text_optional(&archive_path)?;
            if let Some(updated_archive) = format_tidy_up_topic_file(
                BOT_MEMORY_ARCHIVE_TOPIC,
                existing.as_deref(),
                &plan.archive_lines,
            ) {
                self.home_store
                    .replace_bytes(&archive_path, updated_archive.as_bytes())?;
            }
        }

        let baseline = if memory_path.exists() {
            Some(live_md.as_str())
        } else {
            None
        };
        if let Err(err) = self
            .home_store
            .replace_text(&memory_path, baseline, &updated_memory_md)
        {
            let mapped = match err {
                OrcaBotsError::PreconditionFailed { .. } => TidyUpError::new(
                    TidyUpErrorCode::Changed,
                    "MEMORY.md changed during final tidy-up commit.",
                ),
                other => return Err(other),
            };
            self.record_tidy_up_failure(&bot.id, &mut state, &mapped.to_string())?;
            return Err(OrcaBotsError::TidyUp(mapped));
        }

        // Persist 1-click Undo history row and 14-day Re-Add Guard removal ledger.
        let mut history = self.load_history(&bot.id)?;
        history.insert(0, plan.undo_snapshot.history_row.clone());
        self.save_history(&bot.id, &history)?;

        let cutoff = req.now_ms.saturating_sub(READD_GUARD_MS as i64);
        state.readd_ledger.retain(|e| e.removed_at_ms >= cutoff);
        for removed in &plan.change.removed {
            state.readd_ledger.push(RecentlyRemovedTidyUpLine {
                line: removed.line.clone(),
                removed_at_ms: req.now_ms,
            });
        }
        state.consecutive_failures = 0;
        state.last_error = None;
        state.last_run_at_ms = Some(req.now_ms);
        state.last_summary = Some(plan.summary.clone());
        self.save_tidy_up_state(&bot.id, &state)?;

        let home_chat_note = plan.note_text.clone();
        Ok(TidyUpRunOutcome::Applied {
            plan: Box::new(plan),
            home_chat_note,
        })
    }

    /// 1-click Undo for a Tidy-Up run (defaults to the most recent undoable `TidyUp` row),
    /// also removing its lines from the 14-day Re-Add Guard ledger.
    pub fn undo_tidy_up(
        &self,
        bot: &OrcaBotRecord,
        target_row_id: Option<&str>,
        now_ms: i64,
    ) -> Result<MemoryUndoResult> {
        let history = self.load_history(&bot.id)?;
        let target_row = match target_row_id {
            Some(id) => history.iter().find(|r| r.id == id).cloned(),
            None => history
                .iter()
                .find(|r| r.by == BotMemoryAuthor::TidyUp && r.undoable)
                .cloned(),
        }
        .ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                "No undoable tidy-up history entry found.",
            )
        })?;

        let result = self.undo_memory_change(bot, &target_row.id, now_ms)?;

        let mut state = self.load_tidy_up_state(&bot.id)?;
        let restored_lines: HashSet<&str> = target_row
            .change
            .removed
            .iter()
            .map(|l| l.line.as_str())
            .collect();
        state.readd_ledger.retain(|entry| {
            !(entry.removed_at_ms == target_row.at_ms
                && restored_lines.contains(entry.line.as_str()))
        });
        self.save_tidy_up_state(&bot.id, &state)?;
        Ok(result)
    }

    fn bot_state_dir(&self, bot_id: &str) -> Result<PathBuf> {
        if bot_id.is_empty()
            || bot_id.contains('/')
            || bot_id.contains('\\')
            || bot_id.contains("..")
            || bot_id.contains('\0')
        {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Invalid bot_id for memory state directory.",
            ));
        }
        Ok(self.state_dir.join(bot_id))
    }

    fn load_history(&self, bot_id: &str) -> Result<Vec<BotMemoryHistoryRow>> {
        let path = self.bot_state_dir(bot_id)?.join(MEMORY_HISTORY_FILE);
        match fs::read_to_string(&path) {
            Ok(raw) if raw.trim().is_empty() => Ok(Vec::new()),
            Ok(raw) => Ok(serde_json::from_str(&raw)?),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(OrcaBotsError::Io(err)),
        }
    }

    fn save_history(&self, bot_id: &str, rows: &[BotMemoryHistoryRow]) -> Result<()> {
        let path = self.bot_state_dir(bot_id)?.join(MEMORY_HISTORY_FILE);
        let mut json = serde_json::to_string_pretty(rows)?;
        json.push('\n');
        atomic_write_bytes(&path, json.as_bytes())
    }

    fn load_tidy_up_state(&self, bot_id: &str) -> Result<TidyUpRuntimeState> {
        let path = self.bot_state_dir(bot_id)?.join(TIDY_UP_STATE_FILE);
        match fs::read_to_string(&path) {
            Ok(raw) if raw.trim().is_empty() => Ok(TidyUpRuntimeState::default()),
            Ok(raw) => Ok(serde_json::from_str(&raw)?),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(TidyUpRuntimeState::default()),
            Err(err) => Err(OrcaBotsError::Io(err)),
        }
    }

    fn save_tidy_up_state(&self, bot_id: &str, state: &TidyUpRuntimeState) -> Result<()> {
        let path = self.bot_state_dir(bot_id)?.join(TIDY_UP_STATE_FILE);
        let mut json = serde_json::to_string_pretty(state)?;
        json.push('\n');
        atomic_write_bytes(&path, json.as_bytes())
    }

    fn record_tidy_up_failure(
        &self,
        bot_id: &str,
        state: &mut TidyUpRuntimeState,
        error_msg: &str,
    ) -> Result<()> {
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        state.last_error = Some(error_msg.to_string());
        if state.consecutive_failures >= TIDY_UP_AUTO_OFF_FAILURE_THRESHOLD {
            state.turned_off = true;
            state.enabled = false;
        }
        self.save_tidy_up_state(bot_id, state)
    }
}

/// Extracts strictly owner-authored chat turns (excluding external connector, mail, and
/// automation turns), enforcing `<= 20` chats, `<= 4,000` chars/chat, and `<= 40,000` chars total.
pub fn extract_owner_only_chat_context(chats: &[TidyUpChatTranscript]) -> ExtractedOwnerChats {
    let mut sections: Vec<String> = Vec::new();
    let mut total_chars = 0usize;

    for chat in chats {
        if sections.len() >= TIDY_UP_MAX_CHATS || total_chars >= TIDY_UP_MAX_TOTAL_CHARS {
            break;
        }
        let mut chat_lines: Vec<&str> = Vec::new();
        for turn in &chat.turns {
            if !turn.is_owner_authored || turn.is_external_or_automation {
                continue;
            }
            let trimmed = turn.text.trim();
            if !trimmed.is_empty() {
                chat_lines.push(trimmed);
            }
        }
        if chat_lines.is_empty() {
            continue;
        }

        let joined = chat_lines.join("\n");
        let per_chat = truncate_chars(&joined, TIDY_UP_MAX_CHARS_PER_CHAT);
        let header = format!("### Chat {}\n", chat.chat_id);
        let section = format!("{header}{per_chat}");
        let remaining = TIDY_UP_MAX_TOTAL_CHARS.saturating_sub(total_chars);
        if remaining == 0 {
            break;
        }
        let bounded = truncate_chars(&section, remaining);
        total_chars = total_chars.saturating_add(bounded.chars().count());
        sections.push(bounded);
    }

    ExtractedOwnerChats {
        chats_reviewed: sections.len(),
        text: sections.join("\n\n"),
    }
}

/// Evaluates whether Tidy-Up is due under the 3AM-8AM local window, `since_ms`, and sleep catch-up.
pub fn evaluate_tidy_up_due(
    state: &TidyUpRuntimeState,
    now_ms: i64,
    utc_offset_seconds: i32,
    allow_sleep_catchup: bool,
) -> Result<bool> {
    if !state.enabled || state.turned_off {
        return Ok(false);
    }
    if FixedOffset::east_opt(utc_offset_seconds).is_none() {
        return Err(OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            format!("Invalid UTC offset seconds: {utc_offset_seconds}"),
        ));
    }
    let offset = TimeDelta::seconds(i64::from(utc_offset_seconds));
    let now_utc = DateTime::<Utc>::from_timestamp_millis(now_ms).ok_or_else(|| {
        OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Invalid epoch timestamp.")
    })?;
    let local_naive = now_utc
        .naive_utc()
        .checked_add_signed(offset)
        .ok_or_else(|| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Local timestamp out of range.")
        })?;
    let local_date = local_naive.date();
    let local_hour = u8::try_from(local_naive.hour()).unwrap_or(0);
    let in_window = (TIDY_UP_HOURS_FROM..TIDY_UP_HOURS_UNTIL).contains(&local_hour);

    let slot_date = match state.schedule {
        TidyUpSchedule::Night => {
            if local_hour >= TIDY_UP_HOURS_FROM {
                local_date
            } else {
                local_date.checked_sub_days(Days::new(1)).ok_or_else(|| {
                    OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Date underflow.")
                })?
            }
        }
        TidyUpSchedule::Week => {
            let days_from_mon = u64::from(local_date.weekday().num_days_from_monday());
            let this_monday = local_date
                .checked_sub_days(Days::new(days_from_mon))
                .ok_or_else(|| {
                    OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Date underflow.")
                })?;
            if local_date == this_monday && local_hour < TIDY_UP_HOURS_FROM {
                this_monday.checked_sub_days(Days::new(7)).ok_or_else(|| {
                    OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Date underflow.")
                })?
            } else {
                this_monday
            }
        }
    };

    let slot_ms = slot_3am_ms(slot_date, offset)?;
    let after_since = state.since_ms.is_none_or(|since| slot_ms >= since);
    let not_ran = state.last_run_at_ms.is_none_or(|last| last < slot_ms);
    if !after_since || !not_ran {
        return Ok(false);
    }
    Ok(in_window || allow_sleep_catchup)
}

fn slot_3am_ms(date: NaiveDate, offset: TimeDelta) -> Result<i64> {
    let dt = date
        .and_hms_opt(u32::from(TIDY_UP_HOURS_FROM), 0, 0)
        .ok_or_else(|| OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Invalid tidy-up hour."))?;
    let utc = dt.checked_sub_signed(offset).ok_or_else(|| {
        OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Tidy-up slot out of range.")
    })?;
    Ok(utc.and_utc().timestamp_millis())
}

fn scan_memory_topic_snapshots(memory_dir: &Path) -> Result<Vec<BotMemoryTopicSnapshot>> {
    let entries = match fs::read_dir(memory_dir) {
        Ok(rd) => rd,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(OrcaBotsError::Io(err)),
    };

    let mut md_files: Vec<(String, PathBuf)> = Vec::new();
    for entry_res in entries {
        let entry = entry_res?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }
        md_files.push((name.to_string(), path));
    }
    md_files.sort_by(|a, b| a.0.cmp(&b.0));
    md_files.truncate(BOT_MEMORY_TOPICS_MAX_FILES);

    let mut snapshots = Vec::with_capacity(md_files.len());
    for (name, path) in md_files {
        let meta = fs::metadata(&path)?;
        let size_bytes = meta.len();
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0);
        let content = if size_bytes <= BOT_MEMORY_TOPICS_KEPT_BYTES {
            fs::read_to_string(&path).ok()
        } else {
            None
        };
        snapshots.push(BotMemoryTopicSnapshot {
            name,
            size_bytes,
            mtime_ms,
            content,
        });
    }
    Ok(snapshots)
}

fn collect_bounded_topics_context(memory_dir: &Path) -> Result<String> {
    let snapshots = scan_memory_topic_snapshots(memory_dir)?;
    let mut parts: Vec<String> = Vec::new();
    let mut used = 0usize;
    for snap in snapshots {
        if snap.name == format!("{BOT_MEMORY_ARCHIVE_TOPIC}.md") {
            continue;
        }
        let Some(content) = snap.content else {
            continue;
        };
        let remaining = TIDY_UP_MAX_TOPICS_CHARS.saturating_sub(used);
        if remaining == 0 {
            break;
        }
        let chunk = truncate_chars(&format!("### memory/{}\n{content}", snap.name), remaining);
        used = used.saturating_add(chunk.chars().count());
        parts.push(chunk);
    }
    Ok(parts.join("\n\n"))
}

fn collect_recently_removed(
    history: &[BotMemoryHistoryRow],
    tidy_state: &TidyUpRuntimeState,
    now_ms: i64,
) -> Vec<RecentlyRemovedTidyUpLine> {
    let cutoff = now_ms.saturating_sub(READD_GUARD_MS as i64);
    let mut out = recently_removed_from_history(history, now_ms);
    let mut seen: HashSet<(String, i64)> = out
        .iter()
        .map(|e| (e.line.clone(), e.removed_at_ms))
        .collect();
    for entry in &tidy_state.readd_ledger {
        if entry.removed_at_ms >= cutoff
            && entry.removed_at_ms <= now_ms
            && seen.insert((entry.line.clone(), entry.removed_at_ms))
        {
            out.push(entry.clone());
        }
    }
    out
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use orca_core::bot_memory::{
        JournalAsker, JournalCloseAttribution, JournalCloseStatus,
    };
    use orca_core::bot_preamble::BOT_MEMORY_SEED;
    use orca_core::bot_types::{
        BotAvatarSpec, BotConnectorLinks, BotMascotId, BotName, BotNotifications,
    };
    use orca_core::LOCAL_EXECUTION_HOST_ID;
    use tempfile::TempDir;

    fn make_bot(id: &str, name: &str, folder: &str, agent: BotAgentId) -> OrcaBotRecord {
        OrcaBotRecord {
            id: id.to_string(),
            name: BotName::new(name).unwrap(),
            host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
            folder: BotFolderSlug::new(folder).unwrap(),
            folder_workspace_id: None,
            agent,
            avatar_spec: BotAvatarSpec::new_with_mascot(BotMascotId::A1),
            paused: false,
            permission_mode: None,
            launch: None,
            notifications: BotNotifications::default(),
            members: Vec::new(),
            connectors: BotConnectorLinks::default(),
            skill_projects: None,
            sidebar_order: None,
            chat_session_id: None,
            look_around: None,
            tidy_up: None,
            items_stale: None,
            items_seen: None,
            first_run: true,
            accepted_offers: None,
            offer_answers: None,
            handed_paired_workspaces: None,
            cancelled_waits: None,
            imported_from: None,
            shared_from: None,
            template_id: None,
            created_at: 1_000,
            updated_at: 1_000,
        }
    }

    struct StaticTidyProvider {
        reply: std::result::Result<TidyUpProviderOutput, TidyUpError>,
        calls: Arc<AtomicUsize>,
    }

    impl TidyUpDraftProvider for StaticTidyProvider {
        fn draft_tidy_up<'a>(
            &'a self,
            _input: TidyUpPromptInput,
        ) -> Pin<
            Box<
                dyn Future<Output = std::result::Result<TidyUpProviderOutput, TidyUpError>>
                    + Send
                    + 'a,
            >,
        > {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let res = self.reply.clone();
            Box::pin(async move { res })
        }
    }

    #[test]
    fn preamble_authority_order_first_run_and_turn_delta_overflow_guard() {
        let tmp = TempDir::new().unwrap();
        let home = BotHomeStore::new(tmp.path().join("bots"));
        let svc = BotMemoryService::new(home.clone(), tmp.path().join("state"));
        let mut bot = make_bot("b1", "Sentinel", "sentinel", BotAgentId::Claude);
        let dir = home.provision_bot_home(&bot, Some("# Sentinel\nReviews PRs.")).unwrap();

        let (preamble, rules_key) = svc.build_session_preamble(&bot, Some("orca"), None).unwrap();
        let tools_pos = preamble.find("## Tools").unwrap();
        let soul_pos = preamble.find("## SOUL.md").unwrap();
        let mem_pos = preamble.find("## MEMORY.md").unwrap();
        let rules_pos = preamble.find("## Rules").unwrap();
        assert!(tools_pos < soul_pos && soul_pos < mem_pos && mem_pos < rules_pos);
        assert!(preamble.contains("## First run"));

        // Populate 205 lines in MEMORY.md to verify 200-line overflow guard
        let mut lines = vec![BOT_MEMORY_SEED.trim_end().to_string()];
        for i in 1..=205 {
            lines.push(format!("- 2026-11-25 Fact: memory item number {i}"));
        }
        let initial_mem = format!("{}\n", lines.join("\n"));
        fs::write(dir.join("MEMORY.md"), &initial_mem).unwrap();
        // Write one small topic file (<=20KB) and one large topic file (>20KB)
        fs::write(dir.join("memory").join("ci.md"), "# ci\n- old note\n").unwrap();
        fs::write(dir.join("memory").join("big.md"), "x".repeat(25_000)).unwrap();

        let mut sync = BotTurnMemorySync::capture(&home, &bot, Some("orca")).unwrap();

        // Prepend a new line at top, pushing item 200 past the 200-line index window
        let updated_mem = format!(
            "{}\n- 2026-11-25 Fact: brand new top fact\n{}",
            BOT_MEMORY_SEED.trim_end(),
            lines[1..].join("\n")
        );
        fs::write(dir.join("MEMORY.md"), &updated_mem).unwrap();
        fs::write(
            dir.join("memory").join("ci.md"),
            "# ci\n- old note\n- new ci tip\n",
        )
        .unwrap();
        bot.name = BotName::new("Sentinel Prime").unwrap();
        bot.first_run = false;

        let res = sync.sync_turn_memory_delta(&home, &bot, Some("orca")).unwrap();
        assert!(!res.is_empty());
        let mem_update = res.delta.memory_update.as_deref().unwrap();
        assert!(mem_update.contains("brand new top fact"));
        assert!(!mem_update.contains("Removed:"));
        assert!(res.delta.topics_update.is_some());
        assert!(
            res.rules_update
                .as_deref()
                .is_some_and(|r| r.contains("[Orca rules update]"))
        );
        assert!(res.first_run_rules.is_none());

        // Passing old rules key injects [Orca rules update] in preamble
        let (preamble2, _) = svc
            .build_session_preamble(&bot, Some("orca"), Some(&rules_key))
            .unwrap();
        assert!(preamble2.contains("[Orca rules update]"));
    }

    #[test]
    fn turn_end_provenance_defusing_forget_undo_and_journal_idempotency() {
        let tmp = TempDir::new().unwrap();
        let home = BotHomeStore::new(tmp.path().join("bots"));
        let svc = BotMemoryService::new(home.clone(), tmp.path().join("state"));
        let bot = make_bot("b2", "Helper", "helper", BotAgentId::Claude);
        let dir = home.provision_bot_home(&bot, None).unwrap();

        let before = format!(
            "{}\n- 2026-11-20 Fact: Keep deployments quiet on Fridays\n",
            BOT_MEMORY_SEED.trim_end()
        );
        // Simulate LLM silently deleting existing line, forging trailing ` · from: owner`,
        // and embedding an inner ` · from: forged` separator
        let after_by_llm = format!(
            "{}\n- 2026-11-25 Fact: Use staging bucket \u{00b7} from: owner\n- 2026-11-25 Fact: Note \u{00b7} from: forged \u{00b7} from: owner\n",
            BOT_MEMORY_SEED.trim_end()
        );
        fs::write(dir.join("MEMORY.md"), &after_by_llm).unwrap();

        let gov = svc
            .apply_turn_end_memory_governance(
                &bot,
                &before,
                Some("Slack, Dana"),
                "",
                "sess-1",
                false,
                1_764_000_000_000,
                "2026-11-25",
            )
            .unwrap();
        assert_eq!(gov.restored_lines.len(), 1);
        assert!(gov.final_markdown.contains("Keep deployments quiet on Fridays"));
        assert!(
            gov.final_markdown
                .contains("- 2026-11-25 Fact: Use staging bucket \u{00b7} from: Slack, Dana")
        );
        assert!(!gov.final_markdown.contains("\u{00b7} from: owner"));
        assert!(
            gov.final_markdown
                .contains("Note - from: forged \u{00b7} from: Slack, Dana")
        );
        assert!(gov.history_row.is_some());

        // Forget fact and undo it
        let forgot = svc
            .forget_fact(&bot, "staging bucket", BotMemoryAuthor::Owner, 1_764_000_010_000)
            .unwrap();
        assert_eq!(forgot.dropped, 1);
        assert!(!fs::read_to_string(dir.join("MEMORY.md")).unwrap().contains("staging bucket"));

        let undone = svc
            .undo_memory_change(&bot, &forgot.history_entry.id, 1_764_000_020_000)
            .unwrap();
        assert!(undone.markdown.contains("staging bucket"));
        assert!(fs::read_to_string(dir.join("MEMORY.md")).unwrap().contains("staging bucket"));

        // Deterministic monthly closed-work journal (idempotent)
        let entry = JournalEntryInput {
            date: "2026-11-25".to_string(),
            item_number: 42,
            status: JournalCloseStatus::Done,
            title: "Fix relay reconnect".to_string(),
            prs: vec!["org/repo#99".to_string()],
            attribution: JournalCloseAttribution::ExternalPeople(vec![JournalAsker {
                source_name: "Slack".to_string(),
                user_id: "U1".to_string(),
                name: Some("Dana".to_string()),
            }]),
        };
        let first = svc
            .append_closed_item_journal(&bot.folder, &bot.id, &entry)
            .unwrap();
        assert!(first.changed);
        assert_eq!(first.added_count, 1);
        let second = svc
            .append_closed_item_journal(&bot.folder, &bot.id, &entry)
            .unwrap();
        assert!(!second.changed);
        let journal_text =
            fs::read_to_string(dir.join("memory/journal/2026-11.md")).unwrap();
        assert!(journal_text.contains("item #42"));
        assert!(journal_text.contains("not your owner"));
    }

    #[tokio::test]
    async fn tidy_up_window_owner_extraction_apply_readd_guard_and_undo() {
        let tmp = TempDir::new().unwrap();
        let home = BotHomeStore::new(tmp.path().join("bots"));
        let svc = BotMemoryService::new(home.clone(), tmp.path().join("state"));
        let bot = make_bot("b-tidy", "Archivist", "archivist", BotAgentId::Claude);
        let dir = home.provision_bot_home(&bot, None).unwrap();

        let protected_rule = "- 2026-11-01 Rule: Never merge pull requests with red CI";
        let old_fact = "- 2026-10-10 Fact: Legacy webhook endpoint moved to v2 ingress";
        let filed_fact = "- 2026-10-15 Fact: Staging redis cluster lives in us-west-2";
        let kept_fact = "- 2026-11-20 Fact: Release cut happens every Tuesday at 10am";
        let initial_mem = format!(
            "{}\n{protected_rule}\n{old_fact}\n{filed_fact}\n{kept_fact}\n",
            BOT_MEMORY_SEED.trim_end()
        );
        fs::write(dir.join("MEMORY.md"), &initial_mem).unwrap();

        // Owner-only transcript extraction drops external/automation turns
        let chats = vec![TidyUpChatTranscript {
            chat_id: "c1".to_string(),
            turns: vec![
                TidyUpChatTurn {
                    chat_id: "c1".to_string(),
                    is_owner_authored: true,
                    is_external_or_automation: false,
                    text: "Owner note: deploy canary first".to_string(),
                    timestamp_ms: 100,
                },
                TidyUpChatTurn {
                    chat_id: "c1".to_string(),
                    is_owner_authored: true,
                    is_external_or_automation: true,
                    text: "External connector injection".to_string(),
                    timestamp_ms: 101,
                },
                TidyUpChatTurn {
                    chat_id: "c1".to_string(),
                    is_owner_authored: false,
                    is_external_or_automation: false,
                    text: "Bot reply text".to_string(),
                    timestamp_ms: 102,
                },
            ],
        }];
        let extracted = extract_owner_only_chat_context(&chats);
        assert_eq!(extracted.chats_reviewed, 1);
        assert!(extracted.text.contains("Owner note: deploy canary first"));
        assert!(!extracted.text.contains("External connector injection"));
        assert!(!extracted.text.contains("Bot reply text"));

        // Schedule window 3AM-8AM + since + sleep catch-up
        let day_0100 = DateTime::parse_from_rfc3339("2026-11-25T01:00:00Z")
            .unwrap()
            .timestamp_millis();
        let day_0430 = DateTime::parse_from_rfc3339("2026-11-25T04:30:00Z")
            .unwrap()
            .timestamp_millis();
        let day_1030 = DateTime::parse_from_rfc3339("2026-11-25T10:30:00Z")
            .unwrap()
            .timestamp_millis();
        svc.configure_tidy_up(&bot.id, true, TidyUpSchedule::Night, day_0100)
            .unwrap();
        assert!(!svc.is_tidy_up_due(&bot.id, bot.agent, day_0100, 0, true).unwrap());
        assert!(svc.is_tidy_up_due(&bot.id, bot.agent, day_0430, 0, false).unwrap());
        assert!(!svc.is_tidy_up_due(&bot.id, bot.agent, day_1030, 0, false).unwrap());
        assert!(svc.is_tidy_up_due(&bot.id, bot.agent, day_1030, 0, true).unwrap());

        // Successful Tidy-Up run filing `filed_fact` -> `memory/infra.md`, removing `old_fact` -> archive
        let mut topics = BTreeMap::new();
        topics.insert("infra".to_string(), vec![filed_fact.to_string()]);
        let draft = TidyUpDraft {
            index: format!(
                "{}\n{protected_rule}\n{kept_fact}\n- 2026-11-25 Fact: Deploy canary before full rollout\n",
                BOT_MEMORY_SEED.trim_end()
            ),
            topics,
            replaced: vec![old_fact.to_string()],
            merged: Vec::new(),
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = StaticTidyProvider {
            reply: Ok(TidyUpProviderOutput {
                response: TidyUpDraftResponse::Draft(draft),
                elapsed_ms: 1_200,
            }),
            calls: calls.clone(),
        };

        let outcome = svc
            .run_tidy_up(
                &bot,
                &provider,
                TidyUpRunRequest {
                    chats,
                    now_ms: day_0430,
                    today: "2026-11-25".to_string(),
                    utc_offset_seconds: 0,
                    allow_sleep_catchup: true,
                    force: false,
                },
            )
            .await
            .unwrap();

        let (plan, note) = match outcome {
            TidyUpRunOutcome::Applied {
                plan,
                home_chat_note,
            } => (plan, home_chat_note),
            TidyUpRunOutcome::SkippedNotDue => panic!("expected tidy-up to run"),
        };
        assert!(note.starts_with("Tidied memory \u{00b7} "));
        assert!(fs::read_to_string(dir.join("memory/infra.md")).unwrap().contains("Staging redis"));
        assert!(fs::read_to_string(dir.join("memory/archive.md")).unwrap().contains("Legacy webhook"));
        let mem_after_tidy = fs::read_to_string(dir.join("MEMORY.md")).unwrap();
        assert!(mem_after_tidy.contains(protected_rule));
        assert!(!mem_after_tidy.contains("Legacy webhook"));

        // 14-day Re-Add Guard: stale chat re-inserts removed line -> archived with `· kept out`, kept out of MEMORY.md
        let readded_by_stale_chat = format!(
            "{mem_after_tidy}- 2026-11-26 Fact: Legacy webhook endpoint moved to v2 ingress\n"
        );
        fs::write(dir.join("MEMORY.md"), &readded_by_stale_chat).unwrap();
        let gov = svc
            .apply_turn_end_memory_governance(
                &bot,
                &mem_after_tidy,
                None,
                "Check build status please",
                "stale-chat",
                false,
                day_0430 + 86_400_000,
                "2026-11-26",
            )
            .unwrap();
        assert_eq!(gov.intercepted_for_archive.len(), 1);
        assert!(!gov.final_markdown.contains("Legacy webhook"));
        let archive_now = fs::read_to_string(dir.join("memory/archive.md")).unwrap();
        assert!(archive_now.contains("\u{00b7} kept out 2026-11-26"));

        // 1-click Undo restores MEMORY.md and clears Re-Add Guard ledger
        let undo_res = svc
            .undo_tidy_up(&bot, Some(&plan.undo_snapshot.history_row.id), day_0430 + 90_000_000)
            .unwrap();
        assert!(undo_res.markdown.contains("Legacy webhook"));
        assert!(svc.get_tidy_up_state(&bot.id).unwrap().readd_ledger.is_empty());
    }

    #[tokio::test]
    async fn tidy_up_protects_owner_rules_and_auto_disables_after_three_failures() {
        let tmp = TempDir::new().unwrap();
        let home = BotHomeStore::new(tmp.path().join("bots"));
        let svc = BotMemoryService::new(home.clone(), tmp.path().join("state"));
        let bot = make_bot("b-fail", "StrictBot", "strict-bot", BotAgentId::Codex);
        let dir = home.provision_bot_home(&bot, None).unwrap();

        let original = format!(
            "{}\n- 2026-11-01 Rule: Never push directly to main\n",
            BOT_MEMORY_SEED.trim_end()
        );
        fs::write(dir.join("MEMORY.md"), &original).unwrap();
        svc.configure_tidy_up(&bot.id, true, TidyUpSchedule::Night, 1_000)
            .unwrap();

        // Draft that deletes the protected owner rule -> fails validation 3 times -> turned_off=true
        let bad_draft = TidyUpDraft {
            index: format!("{}\n", BOT_MEMORY_SEED.trim_end()),
            topics: BTreeMap::new(),
            replaced: Vec::new(),
            merged: Vec::new(),
        };
        let provider = StaticTidyProvider {
            reply: Ok(TidyUpProviderOutput {
                response: TidyUpDraftResponse::Draft(bad_draft),
                elapsed_ms: 500,
            }),
            calls: Arc::new(AtomicUsize::new(0)),
        };

        for attempt in 1..=3u32 {
            let err = svc
                .run_tidy_up(
                    &bot,
                    &provider,
                    TidyUpRunRequest {
                        chats: Vec::new(),
                        now_ms: 10_000 + i64::from(attempt),
                        today: "2026-11-25".to_string(),
                        utc_offset_seconds: 0,
                        allow_sleep_catchup: true,
                        force: true,
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(err.tidy_up_code(), Some(TidyUpErrorCode::ProtectedLineRemoved));
            assert_eq!(fs::read_to_string(dir.join("MEMORY.md")).unwrap(), original);
        }

        let st = svc.get_tidy_up_state(&bot.id).unwrap();
        assert_eq!(st.consecutive_failures, 3);
        assert!(st.turned_off);
        assert!(!st.enabled);

        // Unsupported agent rejected cleanly
        let grok = make_bot("b-grok", "GrokBot", "grok-bot", BotAgentId::Grok);
        home.provision_bot_home(&grok, None).unwrap();
        let unsup = svc
            .run_tidy_up(
                &grok,
                &provider,
                TidyUpRunRequest {
                    chats: Vec::new(),
                    now_ms: 20_000,
                    today: "2026-11-25".to_string(),
                    utc_offset_seconds: 0,
                    allow_sleep_catchup: true,
                    force: true,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(unsup.tidy_up_code(), Some(TidyUpErrorCode::UnsupportedAgent));
    }
}

