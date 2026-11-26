//! Strict 8-Condition Background Agent Hibernation & Transparent `--resume` Rehydration (`F13`).
//!
//! Wraps `orca_core::orchestration_protocol::{HibernationBlocker, HibernationCheckInput,
//! HibernationDecision, collect_hibernation_blockers, evaluate_hibernation,
//! DEFAULT_HIBERNATION_IDLE_MS}` and manages `TabLeafState::Live -> Sleeping -> Resume -> Live`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use orca_core::orchestration_protocol::{
    DEFAULT_HIBERNATION_IDLE_MS, HibernationBlocker, HibernationCheckInput, HibernationDecision,
    collect_hibernation_blockers, evaluate_hibernation,
};
use orca_core::workspace_layout::{TabLeafState, WorkspaceTab};
use orca_core::OrcaIdeErrorCode;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::{OrcaIdeRuntimeError, Result};

/// Minimum configurable idle threshold (1 minute).
pub const MIN_HIBERNATION_IDLE_MS: u64 = 60_000;

/// Maximum configurable idle threshold (24 hours).
pub const MAX_HIBERNATION_IDLE_MS: u64 = 86_400_000;

/// Exact 10 agent CLIs supporting transparent `--resume <session_handle>` rehydration.
pub const RESUMABLE_AGENTS_10: [&str; 10] = [
    "claude",
    "codex",
    "opencode",
    "opencode2",
    "cursor",
    "grok",
    "hermes",
    "kimi",
    "kiro",
    "pi",
];

/// Returns `true` if `agent_id` is one of the 10 resumable agent CLIs.
pub fn is_resumable_agent(agent_id: &str) -> bool {
    let norm = agent_id.trim().to_ascii_lowercase();
    RESUMABLE_AGENTS_10.contains(&norm.as_str())
}

/// Complete runtime snapshot for evaluating whether a background agent tab may hibernate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HibernationCandidateState {
    pub tab_id: String,
    pub agent_state_is_done: bool,
    pub tab_visible: bool,
    pub keystrokes_since_done: u64,
    pub agent_id: String,
    pub idle_duration_ms: u64,
    pub mobile_driver_attached: bool,
    pub unsettled_orchestration_dispatch: bool,
    pub live_subagent_count: usize,
    pub git_operation_in_progress: bool,
    pub active_port_forward: bool,
    pub pending_bot_approval: bool,
    pub unsynced_local_snapshot: bool,
    pub pinned_no_hibernate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_handle: Option<String>,
    pub original_cwd: PathBuf,
    pub original_args: Vec<String>,
    pub original_env: BTreeMap<String, String>,
}

impl HibernationCandidateState {
    /// Maps all runtime signals strictly onto `orca_core`'s 8-condition [`HibernationCheckInput`].
    pub fn to_check_input(&self, min_idle_threshold_ms: u64) -> HibernationCheckInput {
        let resumable_with_handle = is_resumable_agent(&self.agent_id)
            && self
                .session_handle
                .as_deref()
                .is_some_and(|h| !h.trim().is_empty());

        HibernationCheckInput {
            active_pty_process: !self.agent_state_is_done,
            running_agent_turn: self.live_subagent_count > 0
                || self.tab_visible
                || self.keystrokes_since_done > 0,
            git_operation_in_progress: self.git_operation_in_progress,
            active_port_forward: self.active_port_forward || self.mobile_driver_attached,
            active_review_or_merge_gate: self.unsettled_orchestration_dispatch,
            pending_bot_approval: self.pending_bot_approval,
            unsynced_local_snapshot: self.unsynced_local_snapshot,
            pinned_no_hibernate: self.pinned_no_hibernate || !resumable_with_handle,
            idle_duration_ms: self.idle_duration_ms,
            min_idle_threshold_ms,
        }
    }

    /// Collects every active [`HibernationBlocker`] preventing hibernation for this candidate.
    pub fn collect_blockers(&self, min_idle_threshold_ms: u64) -> Vec<HibernationBlocker> {
        collect_hibernation_blockers(&self.to_check_input(min_idle_threshold_ms))
    }
}

/// Stored metadata for a currently sleeping tab, enabling transparent `--resume` rehydration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HibernatedTabRecord {
    pub tab_id: String,
    pub agent_id: String,
    pub session_handle: String,
    pub original_cwd: PathBuf,
    pub original_args: Vec<String>,
    pub original_env: BTreeMap<String, String>,
    pub hibernated_at_ms: i64,
}

/// Result of calling [`HibernationController::hibernate_tab`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum HibernationOutcome {
    Hibernated { record: HibernatedTabRecord },
    Deferred { remaining_ms: u64 },
}

/// Transparent `--resume <session_handle>` launch plan constructed on wake/rehydration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RehydrationPlan {
    pub tab_id: String,
    pub agent_id: String,
    pub binary: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub session_handle: String,
    pub intermediate_state: TabLeafState,
    pub final_state: TabLeafState,
}

/// Pure-Rust background agent hibernation controller (`Send + Sync`).
pub struct HibernationController {
    idle_threshold_ms: RwLock<u64>,
    hibernated_tabs: RwLock<BTreeMap<String, HibernatedTabRecord>>,
}

impl Default for HibernationController {
    fn default() -> Self {
        Self::default_controller()
    }
}

impl HibernationController {
    /// Creates a controller with an explicit idle threshold validated in `60_000..=86_400_000` ms.
    pub fn new(idle_threshold_ms: u64) -> Result<Self> {
        validate_idle_threshold(idle_threshold_ms)?;
        Ok(Self {
            idle_threshold_ms: RwLock::new(idle_threshold_ms),
            hibernated_tabs: RwLock::new(BTreeMap::new()),
        })
    }

    /// Creates a controller initialized with `DEFAULT_HIBERNATION_IDLE_MS` (30 minutes).
    pub fn default_controller() -> Self {
        Self {
            idle_threshold_ms: RwLock::new(DEFAULT_HIBERNATION_IDLE_MS),
            hibernated_tabs: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn idle_threshold_ms(&self) -> u64 {
        *self.idle_threshold_ms.read()
    }

    pub fn set_idle_threshold_ms(&self, idle_threshold_ms: u64) -> Result<()> {
        validate_idle_threshold(idle_threshold_ms)?;
        *self.idle_threshold_ms.write() = idle_threshold_ms;
        Ok(())
    }

    /// Evaluates all 8 hibernation guards via `orca_core::orchestration_protocol::evaluate_hibernation`.
    /// Returns `Err(OrcaIdeRuntimeError::Ide(HibernationBlocked))` if ANY condition fails.
    pub fn evaluate(&self, candidate: &HibernationCandidateState) -> Result<HibernationDecision> {
        let threshold = self.idle_threshold_ms();
        let input = candidate.to_check_input(threshold);
        Ok(evaluate_hibernation(&input)?)
    }

    /// Evaluates `candidate` and, if clear & sufficiently idle, transitions
    /// `tab.leaf_state` from `TabLeafState::Live` to `TabLeafState::Sleeping`.
    pub fn hibernate_tab(
        &self,
        tab: &mut WorkspaceTab,
        candidate: &HibernationCandidateState,
        now_ms: i64,
    ) -> Result<HibernationOutcome> {
        if tab.tab_id != candidate.tab_id {
            return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "tab_id mismatch: tab '{}' vs candidate '{}'",
                tab.tab_id, candidate.tab_id
            )));
        }
        if tab.leaf_state != TabLeafState::Live {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "tab '{}' is not in Live state and cannot be hibernated",
                tab.tab_id
            )));
        }

        match self.evaluate(candidate)? {
            HibernationDecision::DeferNotIdleEnough { remaining_ms } => {
                Ok(HibernationOutcome::Deferred { remaining_ms })
            }
            HibernationDecision::HibernateNow => {
                let handle = candidate
                    .session_handle
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        OrcaIdeRuntimeError::ide(
                            OrcaIdeErrorCode::HibernationBlocked,
                            "cannot hibernate without a resumable session_handle",
                        )
                    })?
                    .to_string();

                let agent_id = candidate.agent_id.trim().to_ascii_lowercase();
                let record = HibernatedTabRecord {
                    tab_id: tab.tab_id.clone(),
                    agent_id: agent_id.clone(),
                    session_handle: handle,
                    original_cwd: candidate.original_cwd.clone(),
                    original_args: candidate.original_args.clone(),
                    original_env: candidate.original_env.clone(),
                    hibernated_at_ms: now_ms,
                };

                tab.leaf_state = TabLeafState::Sleeping {
                    hibernated_at_ms: now_ms,
                    reason: format!(
                        "background idle >= {}ms for resumable agent '{agent_id}'",
                        self.idle_threshold_ms()
                    ),
                };
                self.hibernated_tabs
                    .write()
                    .insert(tab.tab_id.clone(), record.clone());
                Ok(HibernationOutcome::Hibernated { record })
            }
        }
    }

    /// Builds an exact transparent `--resume <session_handle>` command preserving original
    /// `args`/`env`/`cwd` and transitions `tab.leaf_state`: `Sleeping -> Resume -> Live`.
    pub fn build_rehydration_plan(
        &self,
        tab: &mut WorkspaceTab,
        now_ms: i64,
    ) -> Result<RehydrationPlan> {
        if !matches!(tab.leaf_state, TabLeafState::Sleeping { .. }) {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "tab '{}' is not currently Sleeping",
                tab.tab_id
            )));
        }
        let record = self
            .hibernated_tabs
            .write()
            .remove(&tab.tab_id)
            .ok_or_else(|| {
                OrcaIdeRuntimeError::not_found(format!(
                    "no hibernated record found for tab '{}'",
                    tab.tab_id
                ))
            })?;

        // Step 1: Transition Sleeping -> Resume { session_handle }
        let intermediate_state = TabLeafState::Resume {
            session_handle: record.session_handle.clone(),
        };
        tab.leaf_state = intermediate_state.clone();

        // Preserve original args while stripping any prior `--resume <val>` pair before appending.
        let mut args = strip_existing_resume_flag(&record.original_args);
        args.push("--resume".to_string());
        args.push(record.session_handle.clone());

        // Step 2: Transition Resume -> Live
        let final_state = TabLeafState::Live;
        tab.leaf_state = final_state.clone();
        tab.last_active_at_ms = now_ms;

        Ok(RehydrationPlan {
            tab_id: record.tab_id,
            binary: record.agent_id.clone(),
            agent_id: record.agent_id,
            args,
            cwd: record.original_cwd,
            env: record.original_env,
            session_handle: record.session_handle,
            intermediate_state,
            final_state,
        })
    }

    pub fn get_hibernated(&self, tab_id: &str) -> Option<HibernatedTabRecord> {
        self.hibernated_tabs.read().get(tab_id).cloned()
    }

    pub fn list_hibernated(&self) -> Vec<HibernatedTabRecord> {
        self.hibernated_tabs.read().values().cloned().collect()
    }
}

fn validate_idle_threshold(ms: u64) -> Result<()> {
    if !(MIN_HIBERNATION_IDLE_MS..=MAX_HIBERNATION_IDLE_MS).contains(&ms) {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "hibernation idle threshold {ms}ms must be within {MIN_HIBERNATION_IDLE_MS}..={MAX_HIBERNATION_IDLE_MS}"
        )));
    }
    Ok(())
}

fn strip_existing_resume_flag(original: &[String]) -> Vec<String> {
    let mut cleaned = Vec::with_capacity(original.len());
    let mut skip_next = false;
    for arg in original {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--resume" {
            skip_next = true;
            continue;
        }
        if arg.starts_with("--resume=") {
            continue;
        }
        cleaned.push(arg.clone());
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::workspace_layout::{TabContentType, TabViewMode};

    fn assert_send_sync<T: Send + Sync>() {}

    fn all_clear_candidate() -> HibernationCandidateState {
        let mut env = BTreeMap::new();
        env.insert("ORCA_PROFILE".into(), "work".into());
        HibernationCandidateState {
            tab_id: "tab-agent-1".into(),
            agent_state_is_done: true,
            tab_visible: false,
            keystrokes_since_done: 0,
            agent_id: "claude".into(),
            idle_duration_ms: DEFAULT_HIBERNATION_IDLE_MS,
            mobile_driver_attached: false,
            unsettled_orchestration_dispatch: false,
            live_subagent_count: 0,
            git_operation_in_progress: false,
            active_port_forward: false,
            pending_bot_approval: false,
            unsynced_local_snapshot: false,
            pinned_no_hibernate: false,
            session_handle: Some("sess_9911".into()),
            original_cwd: PathBuf::from("/workspace/repo"),
            original_args: vec![
                "--model".into(),
                "opus-4".into(),
                "--resume".into(),
                "old_handle".into(),
            ],
            original_env: env,
        }
    }

    fn live_tab(tab_id: &str) -> WorkspaceTab {
        WorkspaceTab {
            tab_id: tab_id.into(),
            content_type: TabContentType::AgentSession,
            title: "Claude".into(),
            pinned: false,
            preview: false,
            view_mode: TabViewMode::Terminal,
            leaf_state: TabLeafState::Live,
            pty_incarnation_id: Some("inc-1".into()),
            dirty: false,
            created_at_ms: 1_000,
            last_active_at_ms: 1_000,
        }
    }

    #[test]
    fn all_eight_blockers_and_runtime_guards_fail_closed_with_hibernation_blocked() {
        assert_send_sync::<HibernationController>();
        assert_eq!(RESUMABLE_AGENTS_10.len(), 10);
        for agent in RESUMABLE_AGENTS_10 {
            assert!(is_resumable_agent(agent));
        }
        assert!(!is_resumable_agent("aider"));

        // Idle threshold validation bounds (60_000..=86_400_000)
        assert!(HibernationController::new(59_999).is_err());
        assert!(HibernationController::new(86_400_001).is_err());
        let ctrl = HibernationController::new(120_000).expect("valid threshold");

        // Every single one of the 8 blockers + extended runtime signals must return HibernationBlocked
        type BlockerMutator = fn(&mut HibernationCandidateState);
        let cases: [(&str, BlockerMutator); 13] = [
            ("not_done", |c| c.agent_state_is_done = false),
            ("tab_visible", |c| c.tab_visible = true),
            ("keystrokes", |c| c.keystrokes_since_done = 1),
            ("live_subagent", |c| c.live_subagent_count = 1),
            ("git_op", |c| c.git_operation_in_progress = true),
            ("port_forward", |c| c.active_port_forward = true),
            ("mobile_driver", |c| c.mobile_driver_attached = true),
            ("unsettled_dispatch", |c| {
                c.unsettled_orchestration_dispatch = true
            }),
            ("pending_bot_approval", |c| c.pending_bot_approval = true),
            ("unsynced_snapshot", |c| c.unsynced_local_snapshot = true),
            ("pinned", |c| c.pinned_no_hibernate = true),
            ("non_resumable_agent", |c| c.agent_id = "aider".into()),
            ("missing_handle", |c| c.session_handle = None),
        ];

        for (label, mutate) in cases {
            let mut cand = all_clear_candidate();
            mutate(&mut cand);
            let err = ctrl
                .evaluate(&cand)
                .expect_err(&format!("case '{label}' must fail closed"));
            assert_eq!(
                err.ide_error_code(),
                Some(OrcaIdeErrorCode::HibernationBlocked),
                "case '{label}' wrong code"
            );
            assert!(!cand.collect_blockers(ctrl.idle_threshold_ms()).is_empty());
        }
    }

    #[test]
    fn all_clear_hibernates_and_builds_transparent_resume_rehydration_plan() {
        let ctrl = HibernationController::default_controller();
        let mut tab = live_tab("tab-agent-1");

        // Insufficient idle defers cleanly without changing tab state
        let mut too_fresh = all_clear_candidate();
        too_fresh.idle_duration_ms = 60_000;
        let deferred = ctrl
            .hibernate_tab(&mut tab, &too_fresh, 10_000)
            .expect("defer");
        assert_eq!(
            deferred,
            HibernationOutcome::Deferred {
                remaining_ms: DEFAULT_HIBERNATION_IDLE_MS - 60_000
            }
        );
        assert_eq!(tab.leaf_state, TabLeafState::Live);

        // Sufficient idle + all 8 checks clear -> Sleeping
        let ready = all_clear_candidate();
        let hibernated = ctrl
            .hibernate_tab(&mut tab, &ready, 20_000)
            .expect("hibernate");
        assert!(matches!(hibernated, HibernationOutcome::Hibernated { .. }));
        assert!(matches!(tab.leaf_state, TabLeafState::Sleeping { .. }));
        assert!(ctrl.get_hibernated("tab-agent-1").is_some());

        // Rehydration transitions Sleeping -> Resume -> Live and builds exact `--resume <handle>`
        let plan = ctrl
            .build_rehydration_plan(&mut tab, 30_000)
            .expect("rehydrate");
        assert_eq!(plan.agent_id, "claude");
        assert_eq!(plan.binary, "claude");
        assert_eq!(
            plan.args,
            vec!["--model", "opus-4", "--resume", "sess_9911"]
        );
        assert_eq!(plan.cwd, PathBuf::from("/workspace/repo"));
        assert_eq!(
            plan.env.get("ORCA_PROFILE").map(String::as_str),
            Some("work")
        );
        assert_eq!(
            plan.intermediate_state,
            TabLeafState::Resume {
                session_handle: "sess_9911".into()
            }
        );
        assert_eq!(plan.final_state, TabLeafState::Live);
        assert_eq!(tab.leaf_state, TabLeafState::Live);
        assert!(ctrl.get_hibernated("tab-agent-1").is_none());
    }
}
