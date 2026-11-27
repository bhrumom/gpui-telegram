//! Proactive read-only Look Around engine & persistent Up Next suggestion store.
//! Enforces read-only agents (`claude`/`codex`), 4 daytime schedules with `weekdays_only`,
//! `since`, and sleep `due_now` catch-up, credential-URL stripping (`link_carries_credential`),
//! `<= 5` suggestions/run, and `suggestion_key` deduplication across live & dismissed entries.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use orca_core::bot_look_around::{
    BotUpNext, LookAroundEvaluation, LookAroundPatch, LookAroundState, SUGGESTION_LIMITS,
    Suggestion, UpNextState, dedup_and_cap_suggestions, ensure_look_around_agent,
    evaluate_look_around, link_carries_credential, patch_look_around, sanitize_suggestion_link,
    suggestion_key,
};
use orca_core::bot_types::{BotAgentId, OrcaBotRecord};
use orca_core::{BotError, BotErrorCode};
use serde::{Deserialize, Serialize};

use crate::home_store::atomic_write_bytes;
use crate::{OrcaBotsError, Result};

const UP_NEXT_FILE: &str = "up_next.json";

/// Persisted per-bot Look Around & Up Next state stored under `<state_dir>/<bot_id>/up_next.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedUpNextState {
    #[serde(default)]
    pub state: UpNextState,
    #[serde(default)]
    pub schedule: LookAroundState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<Suggestion>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dismissed_keys: BTreeMap<String, i64>,
}

/// Returned when the owner clicks Start on an Up Next suggestion, pre-filling the composer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartedSuggestion {
    pub suggestion: Suggestion,
    pub composer_ask: String,
}

/// Returned when a single Up Next suggestion is submitted directly via REST/CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedSuggestionResult {
    pub suggestion: Suggestion,
    pub deduped: bool,
    pub credential_link_stripped: bool,
}

/// Persistent per-bot store for Look Around schedule and Up Next suggestions.
#[derive(Debug, Clone)]
pub struct UpNextStore {
    state_dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl UpNextStore {
    /// Opens an `UpNextStore` rooted at `state_dir`.
    pub fn open(state_dir: impl Into<PathBuf>) -> Result<Self> {
        let state_dir = state_dir.into();
        fs::create_dir_all(&state_dir)?;
        Ok(Self {
            state_dir,
            lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Loads persisted Look Around / Up Next state for `bot_id`.
    pub fn get_state(&self, bot_id: &str) -> Result<PersistedUpNextState> {
        let _guard = self.lock_store()?;
        self.load_unlocked(bot_id)
    }

    /// Applies a validated `LookAroundPatch`, enforcing the read-only agent guard when enabled
    /// and stamping `since_ms = Some(now_ms)` on `false -> true`.
    pub fn patch_schedule(
        &self,
        bot_id: &str,
        agent: BotAgentId,
        patch: LookAroundPatch,
        now_ms: i64,
    ) -> Result<LookAroundState> {
        let _guard = self.lock_store()?;
        let mut persisted = self.load_unlocked(bot_id)?;
        let turning_on = patch.on.unwrap_or(persisted.schedule.on);
        if turning_on {
            ensure_look_around_agent(agent)?;
        }
        let next_schedule = patch_look_around(&persisted.schedule, patch, now_ms)?;
        persisted.schedule = next_schedule.clone();
        self.save_unlocked(bot_id, &persisted)?;
        Ok(next_schedule)
    }

    /// Returns the current `BotUpNext` view (`state`, live `suggestions`, `shown`, `overflow`).
    pub fn list_suggestions(&self, bot_id: &str) -> Result<BotUpNext> {
        let _guard = self.lock_store()?;
        let persisted = self.load_unlocked(bot_id)?;
        Ok(BotUpNext::from_suggestions(
            persisted.state,
            persisted.suggestions,
        ))
    }

    /// Starts a suggestion by `suggestion_id`, removing it from active Up Next and returning
    /// its `ask` string for composer pre-fill.
    pub fn start_suggestion(
        &self,
        bot_id: &str,
        suggestion_id: &str,
    ) -> Result<StartedSuggestion> {
        let _guard = self.lock_store()?;
        let mut persisted = self.load_unlocked(bot_id)?;
        let pos = persisted
            .suggestions
            .iter()
            .position(|s| s.id == suggestion_id)
            .ok_or_else(|| {
                OrcaBotsError::bot(
                    BotErrorCode::NotFound,
                    format!("Suggestion '{suggestion_id}' not found."),
                )
            })?;
        let suggestion = persisted.suggestions.remove(pos);
        let composer_ask = suggestion.ask.clone();
        self.save_unlocked(bot_id, &persisted)?;
        Ok(StartedSuggestion {
            suggestion,
            composer_ask,
        })
    }

    /// Dismisses a suggestion by `suggestion_id` and records its canonical `suggestion_key`
    /// so future Look Around runs never resurface it.
    pub fn dismiss_suggestion(
        &self,
        bot_id: &str,
        suggestion_id: &str,
        now_ms: i64,
    ) -> Result<Suggestion> {
        let _guard = self.lock_store()?;
        let mut persisted = self.load_unlocked(bot_id)?;
        let pos = persisted
            .suggestions
            .iter()
            .position(|s| s.id == suggestion_id)
            .ok_or_else(|| {
                OrcaBotsError::bot(
                    BotErrorCode::NotFound,
                    format!("Suggestion '{suggestion_id}' not found."),
                )
            })?;
        let suggestion = persisted.suggestions.remove(pos);
        let key = suggestion_key(&suggestion.title, suggestion.link.as_deref());
        persisted.dismissed_keys.insert(key, now_ms);
        self.save_unlocked(bot_id, &persisted)?;
        Ok(suggestion)
    }

    /// Submits a single suggestion directly into a bot's Up Next queue while enforcing:
    /// - Read-only Look Around agent guard (`claude`/`codex`).
    /// - Field length bounds (`title` 1..=80, `why` 1..=400, `ask` 1..=2000).
    /// - Credential-bearing link stripping (`link_carries_credential` / `sanitize_suggestion_link`).
    /// - Canonical `suggestion_key` deduplication (GitHub PR/Issue, Linear issue, normalized title)
    ///   across both live and dismissed suggestions.
    /// - Max 5 live suggestions cap (`SUGGESTION_LIMITS.max_per_run`).
    #[allow(clippy::too_many_arguments)]
    pub fn submit_suggestion(
        &self,
        bot_id: &str,
        agent: BotAgentId,
        title: String,
        why: String,
        ask: String,
        link: Option<String>,
        now_ms: i64,
    ) -> Result<SubmittedSuggestionResult> {
        ensure_look_around_agent(agent)?;
        let title_trimmed = title.trim();
        let why_trimmed = why.trim();
        let ask_trimmed = ask.trim();
        if title_trimmed.is_empty()
            || title_trimmed.chars().count() > SUGGESTION_LIMITS.title_max
            || why_trimmed.is_empty()
            || why_trimmed.chars().count() > SUGGESTION_LIMITS.why_max
            || ask_trimmed.is_empty()
            || ask_trimmed.chars().count() > SUGGESTION_LIMITS.ask_max
        {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Suggestion title/why/ask must be non-empty and within length limits (80/400/2000).",
            ));
        }
        let had_cred_link = link.as_deref().is_some_and(link_carries_credential);
        let sanitized_link = sanitize_suggestion_link(link.as_deref());
        let key = suggestion_key(title_trimmed, sanitized_link.as_deref());

        let _guard = self.lock_store()?;
        let mut persisted = self.load_unlocked(bot_id)?;
        if let Some(existing) = persisted
            .suggestions
            .iter()
            .find(|s| suggestion_key(&s.title, s.link.as_deref()) == key)
            .cloned()
        {
            return Ok(SubmittedSuggestionResult {
                suggestion: existing,
                deduped: true,
                credential_link_stripped: had_cred_link,
            });
        }
        if persisted.dismissed_keys.contains_key(&key) {
            return Ok(SubmittedSuggestionResult {
                suggestion: Suggestion {
                    id: format!("sug-{}", uuid::Uuid::new_v4()),
                    title: title_trimmed.to_string(),
                    why: why_trimmed.to_string(),
                    ask: ask_trimmed.to_string(),
                    link: sanitized_link,
                    created_at_ms: Some(now_ms),
                },
                deduped: true,
                credential_link_stripped: had_cred_link,
            });
        }

        let created = Suggestion {
            id: format!("sug-{}", uuid::Uuid::new_v4()),
            title: title_trimmed.to_string(),
            why: why_trimmed.to_string(),
            ask: ask_trimmed.to_string(),
            link: sanitized_link,
            created_at_ms: Some(now_ms),
        };
        created.validate()?;
        if persisted.suggestions.len() < SUGGESTION_LIMITS.max_per_run {
            persisted.suggestions.push(created.clone());
        }
        persisted.state = UpNextState::Ready;
        self.save_unlocked(bot_id, &persisted)?;
        Ok(SubmittedSuggestionResult {
            suggestion: created,
            deduped: false,
            credential_link_stripped: had_cred_link,
        })
    }

    fn lock_store(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.lock.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "UpNextStore mutex poisoned.")
        })
    }

    fn file_path(&self, bot_id: &str) -> Result<PathBuf> {
        if bot_id.is_empty()
            || bot_id.contains('/')
            || bot_id.contains('\\')
            || bot_id.contains("..")
            || bot_id.contains('\0')
        {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Invalid bot_id for UpNextStore.",
            ));
        }
        Ok(self.state_dir.join(bot_id).join(UP_NEXT_FILE))
    }

    fn load_unlocked(&self, bot_id: &str) -> Result<PersistedUpNextState> {
        let path = self.file_path(bot_id)?;
        match fs::read_to_string(&path) {
            Ok(raw) if raw.trim().is_empty() => Ok(PersistedUpNextState::default()),
            Ok(raw) => Ok(serde_json::from_str(&raw)?),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(PersistedUpNextState::default()),
            Err(err) => Err(OrcaBotsError::Io(err)),
        }
    }

    fn save_unlocked(&self, bot_id: &str, state: &PersistedUpNextState) -> Result<()> {
        let path = self.file_path(bot_id)?;
        let mut json = serde_json::to_string_pretty(state)?;
        json.push('\n');
        atomic_write_bytes(&path, json.as_bytes())
    }
}

/// Read-only context passed to a `LookAroundProvider` when gathering proactive suggestions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookAroundRunInput {
    pub bot_id: String,
    pub bot_name: String,
    pub agent: BotAgentId,
    pub focus: Option<String>,
    pub existing_keys: Vec<String>,
    pub now_ms: i64,
}

/// Async read-only suggestion provider invoked by `LookAroundEngine::run_look_around`.
pub trait LookAroundProvider: Send + Sync {
    fn gather_suggestions<'a>(
        &'a self,
        input: LookAroundRunInput,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Vec<Suggestion>, BotError>> + Send + 'a>>;
}

/// Outcome of executing `LookAroundEngine::run_look_around`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookAroundRunOutcome {
    SkippedNotDue,
    Completed {
        up_next: BotUpNext,
        added_count: usize,
        filtered_duplicates: usize,
        stripped_credential_links: usize,
    },
}

/// Proactive read-only Look Around scheduler & suggestion runner.
#[derive(Debug, Clone)]
pub struct LookAroundEngine {
    store: UpNextStore,
}

impl LookAroundEngine {
    /// Creates a new engine backed by `store`.
    pub fn new(store: UpNextStore) -> Self {
        Self { store }
    }

    /// Returns a reference to the underlying `UpNextStore`.
    pub fn store(&self) -> &UpNextStore {
        &self.store
    }

    /// Evaluates whether a bot's Look Around schedule is due at `now_ms`, enforcing
    /// the read-only agent guard (`claude`/`codex` only), `weekdays_only`, `since`,
    /// and sleep `due_now` catch-up via `orca_core::bot_look_around`.
    pub fn evaluate_due(
        &self,
        bot_id: &str,
        agent: BotAgentId,
        now_ms: i64,
        utc_offset_seconds: i32,
    ) -> Result<LookAroundEvaluation> {
        ensure_look_around_agent(agent)?;
        let persisted = self.store.get_state(bot_id)?;
        if !persisted.schedule.on {
            return Ok(LookAroundEvaluation {
                due_now: false,
                latest_slot_ms: None,
                next_slot_ms: None,
            });
        }
        Ok(evaluate_look_around(
            &persisted.schedule.schedule,
            now_ms,
            persisted.last_run_at_ms,
            utc_offset_seconds,
        )?)
    }

    /// Runs a proactive read-only Look Around pass:
    /// - Refuses unsupported agents (`claude`/`codex` only).
    /// - Checks daytime schedule + sleep catch-up unless `force == true`.
    /// - Strips credential-bearing links via `link_carries_credential` / `sanitize_suggestion_link`.
    /// - Deduplicates via `suggestion_key` against both live and dismissed suggestions.
    /// - Enforces `<= 5` (`SUGGESTION_LIMITS.max_per_run`) new suggestions per run.
    pub async fn run_look_around<P: LookAroundProvider>(
        &self,
        bot: &OrcaBotRecord,
        provider: &P,
        now_ms: i64,
        utc_offset_seconds: i32,
        force: bool,
    ) -> Result<LookAroundRunOutcome> {
        ensure_look_around_agent(bot.agent)?;

        let (focus, existing_keys) = {
            let _guard = self.store.lock_store()?;
            let mut persisted = self.store.load_unlocked(&bot.id)?;
            if !force {
                if !persisted.schedule.on {
                    return Ok(LookAroundRunOutcome::SkippedNotDue);
                }
                let eval = evaluate_look_around(
                    &persisted.schedule.schedule,
                    now_ms,
                    persisted.last_run_at_ms,
                    utc_offset_seconds,
                )?;
                if !eval.due_now {
                    return Ok(LookAroundRunOutcome::SkippedNotDue);
                }
            }

            let mut keys: Vec<String> = persisted
                .suggestions
                .iter()
                .map(|s| suggestion_key(&s.title, s.link.as_deref()))
                .collect();
            keys.extend(persisted.dismissed_keys.keys().cloned());

            persisted.state = UpNextState::Running;
            persisted.last_error = None;
            self.store.save_unlocked(&bot.id, &persisted)?;
            (persisted.schedule.schedule.focus.clone(), keys)
        };

        let input = LookAroundRunInput {
            bot_id: bot.id.clone(),
            bot_name: bot.name.as_str().to_string(),
            agent: bot.agent,
            focus,
            existing_keys,
            now_ms,
        };

        let raw_suggestions = match provider.gather_suggestions(input).await {
            Ok(list) => list,
            Err(err) => {
                let _guard = self.store.lock_store()?;
                let mut persisted = self.store.load_unlocked(&bot.id)?;
                persisted.state = UpNextState::Failed;
                persisted.last_error = Some(err.to_string());
                self.store.save_unlocked(&bot.id, &persisted)?;
                return Err(OrcaBotsError::Bot(err));
            }
        };

        let _guard = self.store.lock_store()?;
        let mut persisted = self.store.load_unlocked(&bot.id)?;

        let mut known_keys: HashSet<String> = persisted
            .suggestions
            .iter()
            .map(|s| suggestion_key(&s.title, s.link.as_deref()))
            .collect();
        known_keys.extend(persisted.dismissed_keys.keys().cloned());

        let mut stripped_credential_links = 0usize;
        let mut filtered_duplicates = 0usize;
        let mut candidates: Vec<Suggestion> = Vec::new();

        for mut suggestion in raw_suggestions {
            if let Some(ref raw_link) = suggestion.link {
                if link_carries_credential(raw_link) {
                    stripped_credential_links = stripped_credential_links.saturating_add(1);
                    suggestion.link = None;
                } else {
                    suggestion.link = sanitize_suggestion_link(Some(raw_link));
                }
            }
            if suggestion.created_at_ms.is_none() {
                suggestion.created_at_ms = Some(now_ms);
            }
            suggestion.validate()?;

            let key = suggestion_key(&suggestion.title, suggestion.link.as_deref());
            if known_keys.contains(&key) {
                filtered_duplicates = filtered_duplicates.saturating_add(1);
                continue;
            }
            candidates.push(suggestion);
        }

        let pre_dedup_len = candidates.len();
        let accepted = dedup_and_cap_suggestions(candidates)?;
        let intra_batch_dropped = pre_dedup_len
            .min(SUGGESTION_LIMITS.max_per_run)
            .saturating_sub(accepted.len());
        filtered_duplicates = filtered_duplicates.saturating_add(intra_batch_dropped);

        let added_count = accepted.len();
        persisted.suggestions.extend(accepted);
        persisted.state = UpNextState::Ready;
        persisted.last_run_at_ms = Some(now_ms);
        persisted.last_error = None;
        self.store.save_unlocked(&bot.id, &persisted)?;

        let up_next = BotUpNext::from_suggestions(persisted.state, persisted.suggestions);
        Ok(LookAroundRunOutcome::Completed {
            up_next,
            added_count,
            filtered_duplicates,
            stripped_credential_links,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::bot_look_around::{LookAroundEvery, parse_rfc3339_ms};
    use orca_core::bot_types::{
        BotAvatarSpec, BotConnectorLinks, BotFolderSlug, BotMascotId, BotName, BotNotifications,
    };
    use orca_core::LOCAL_EXECUTION_HOST_ID;
    use tempfile::TempDir;

    fn make_bot(id: &str, agent: BotAgentId) -> OrcaBotRecord {
        OrcaBotRecord {
            id: id.to_string(),
            name: BotName::new("Scout").unwrap(),
            host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
            folder: BotFolderSlug::new("scout").unwrap(),
            folder_workspace_id: None,
            agent,
            avatar_spec: BotAvatarSpec::new_with_mascot(BotMascotId::D1),
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
            first_run: false,
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

    struct StubProvider {
        suggestions: Vec<Suggestion>,
    }

    impl LookAroundProvider for StubProvider {
        fn gather_suggestions<'a>(
            &'a self,
            _input: LookAroundRunInput,
        ) -> Pin<Box<dyn Future<Output = std::result::Result<Vec<Suggestion>, BotError>> + Send + 'a>>
        {
            let out = self.suggestions.clone();
            Box::pin(async move { Ok(out) })
        }
    }

    fn sug(id: &str, title: &str, link: Option<&str>) -> Suggestion {
        Suggestion {
            id: id.to_string(),
            title: title.to_string(),
            why: "Worth checking".to_string(),
            ask: format!("Please handle {title}"),
            link: link.map(str::to_string),
            created_at_ms: None,
        }
    }

    #[tokio::test]
    async fn read_only_agent_guard_and_daytime_sleep_catchup_schedule() {
        let tmp = TempDir::new().unwrap();
        let store = UpNextStore::open(tmp.path()).unwrap();
        let engine = LookAroundEngine::new(store.clone());

        // Unsupported agent refused
        let grok = make_bot("b-grok", BotAgentId::Grok);
        assert_eq!(
            store
                .patch_schedule(
                    &grok.id,
                    grok.agent,
                    LookAroundPatch {
                        on: Some(true),
                        ..Default::default()
                    },
                    1_000
                )
                .unwrap_err()
                .code(),
            BotErrorCode::InvalidArgument
        );
        assert_eq!(
            engine
                .run_look_around(
                    &grok,
                    &StubProvider {
                        suggestions: Vec::new()
                    },
                    1_000,
                    0,
                    true
                )
                .await
                .unwrap_err()
                .code(),
            BotErrorCode::InvalidArgument
        );

        // Enable on Wednesday 2026-11-25 at 09:00 UTC -> since=09:00 skips 08:00 slot
        let wed_0900 = parse_rfc3339_ms("2026-11-25T09:00:00Z").unwrap();
        let wed_1000 = parse_rfc3339_ms("2026-11-25T10:00:00Z").unwrap();
        let wed_1330 = parse_rfc3339_ms("2026-11-25T13:30:00Z").unwrap();
        let sat_1400 = parse_rfc3339_ms("2026-11-28T14:00:00Z").unwrap();

        let claude = make_bot("b-claude", BotAgentId::Claude);
        store
            .patch_schedule(
                &claude.id,
                claude.agent,
                LookAroundPatch {
                    on: Some(true),
                    every: Some(LookAroundEvery::Every4h),
                    weekdays_only: Some(true),
                    focus: Some(Some("PR reviews".to_string())),
                    since_ms: None,
                },
                wed_0900,
            )
            .unwrap();

        // At 10:00, latest slot 08:00 < since 09:00 -> NOT due
        assert!(!engine.evaluate_due(&claude.id, claude.agent, wed_1000, 0).unwrap().due_now);
        // After sleep, waking at 13:30 catches up missed 12:00 slot -> DUE
        assert!(engine.evaluate_due(&claude.id, claude.agent, wed_1330, 0).unwrap().due_now);
        // Saturday with weekdays_only=true -> NOT due
        assert!(!engine.evaluate_due(&claude.id, claude.agent, sat_1400, 0).unwrap().due_now);
    }

    #[tokio::test]
    async fn credential_stripping_cap_5_dedup_live_and_dismissed_and_start() {
        let tmp = TempDir::new().unwrap();
        let store = UpNextStore::open(tmp.path()).unwrap();
        let engine = LookAroundEngine::new(store.clone());
        let bot = make_bot("b-codex", BotAgentId::Codex);
        let t1 = parse_rfc3339_ms("2026-11-25T12:05:00Z").unwrap();

        let provider = StubProvider {
            suggestions: vec![
                sug(
                    "s1",
                    "Review PR #12",
                    Some("https://github.com/Acme/Repo/pull/12/files#diff-1"),
                ),
                sug(
                    "s2-dup",
                    "Different title same PR",
                    Some("https://www.github.com/acme/repo/pull/12"),
                ),
                sug(
                    "s3-secret",
                    "Check Sentry alert",
                    Some("https://sentry.example.com/issues/5?api_key=supersecret123"),
                ),
                sug(
                    "s4",
                    "Triage Linear bug",
                    Some("https://linear.app/acme/issue/ENG-42/crash-on-boot"),
                ),
                sug("s5", "Update lockfile", None),
                sug("s6", "Check flaky test", None),
                sug("s7-over-cap", "Sixth item over cap", None),
            ],
        };

        let outcome = engine
            .run_look_around(&bot, &provider, t1, 0, true)
            .await
            .unwrap();
        match outcome {
            LookAroundRunOutcome::Completed {
                up_next,
                added_count,
                stripped_credential_links,
                ..
            } => {
                assert_eq!(added_count, 5);
                assert_eq!(stripped_credential_links, 1);
                assert_eq!(up_next.suggestions.len(), 5);
                assert_eq!(up_next.shown, 3);
                assert_eq!(up_next.overflow, 2);
                let s3 = up_next.suggestions.iter().find(|s| s.id == "s3-secret").unwrap();
                assert!(s3.link.is_none());
            }
            LookAroundRunOutcome::SkippedNotDue => panic!("expected completed run"),
        }

        // Start s1 -> removes from Up Next and returns composer_ask
        let started = store.start_suggestion(&bot.id, "s1").unwrap();
        assert_eq!(started.composer_ask, "Please handle Review PR #12");
        assert_eq!(store.list_suggestions(&bot.id).unwrap().suggestions.len(), 4);

        // Dismiss s4 -> future run with identical Linear issue URL is deduplicated
        store.dismiss_suggestion(&bot.id, "s4", t1 + 1_000).unwrap();
        let provider2 = StubProvider {
            suggestions: vec![
                sug(
                    "s4-again",
                    "Renamed Linear bug",
                    Some("https://linear.app/acme/issue/ENG-42"),
                ),
                sug("s5-again", "  update   lockfile  ", None),
                sug("s8-fresh", "Brand new suggestion", None),
            ],
        };
        let outcome2 = engine
            .run_look_around(&bot, &provider2, t1 + 2_000, 0, true)
            .await
            .unwrap();
        match outcome2 {
            LookAroundRunOutcome::Completed {
                added_count,
                filtered_duplicates,
                ..
            } => {
                assert_eq!(added_count, 1);
                assert_eq!(filtered_duplicates, 2);
            }
            LookAroundRunOutcome::SkippedNotDue => panic!("expected completed run"),
        }

        // Reopen store from disk -> persistence intact
        let reopened = UpNextStore::open(tmp.path()).unwrap();
        let listed = reopened.list_suggestions(&bot.id).unwrap();
        assert!(listed.suggestions.iter().any(|s| s.id == "s8-fresh"));
        assert!(!listed.suggestions.iter().any(|s| s.id == "s4-again"));
    }
}
