//! Pure-Rust multi-agent orchestration DAG, generation-fenced worker dispatches,
//! non-guessing worker liveness, 9-type typed mailbox & routing guards,
//! review/merge gates with wait-deadline enforcement, and multi-host federation.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{LOCAL_EXECUTION_HOST_ID, OrcaIdeError, OrcaIdeErrorCode};

// ---------------------------------------------------------------------------
// Orchestration Task DAG
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationTaskStatus {
    Pending,
    Ready,
    Running,
    WaitingGate,
    Succeeded,
    Failed,
    Canceled,
}

impl OrchestrationTaskStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationTask {
    pub task_id: String,
    pub run_id: String,
    pub title: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub status: OrchestrationTaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_host_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_dispatch_id: Option<String>,
    pub nesting_depth: u32,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Deterministic orchestration Task DAG with Kahn's topological cycle detection,
/// readiness computation, and guarded state transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationDag {
    pub run_id: String,
    pub tasks: BTreeMap<String, OrchestrationTask>,
}

impl OrchestrationDag {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            tasks: BTreeMap::new(),
        }
    }

    pub fn add_task(&mut self, task: OrchestrationTask) -> Result<(), OrcaIdeError> {
        if task.task_id.trim().is_empty() {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::UnknownTask,
                "task_id cannot be empty",
            ));
        }
        if self.tasks.contains_key(&task.task_id) {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::UnknownTask,
                format!("duplicate task_id '{}'", task.task_id),
            ));
        }
        self.tasks.insert(task.task_id.clone(), task);
        Ok(())
    }

    /// Validates that every dependency ID exists and the graph is strictly acyclic
    /// using Kahn's topological sort, returning deterministic topological order.
    pub fn validate_acyclic(&self) -> Result<Vec<String>, OrcaIdeError> {
        let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for id in self.tasks.keys() {
            in_degree.insert(id.clone(), 0);
            dependents.insert(id.clone(), Vec::new());
        }

        for (id, task) in &self.tasks {
            let mut seen_deps = BTreeSet::new();
            for dep in &task.depends_on {
                if dep == id {
                    return Err(OrcaIdeError::new(
                        OrcaIdeErrorCode::UnknownTask,
                        format!("self-dependency cycle detected on task '{id}'"),
                    ));
                }
                if !self.tasks.contains_key(dep) {
                    return Err(OrcaIdeError::new(
                        OrcaIdeErrorCode::UnknownTask,
                        format!("task '{id}' depends on unknown task '{dep}'"),
                    ));
                }
                if seen_deps.insert(dep.as_str()) {
                    if let Some(deg) = in_degree.get_mut(id) {
                        *deg += 1;
                    }
                    if let Some(children) = dependents.get_mut(dep) {
                        children.push(id.clone());
                    }
                }
            }
        }

        // Deterministic lexicographical Kahn's queue via BTreeSet
        let mut ready_queue: BTreeSet<String> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(id, _)| id.clone())
            .collect();
        let mut order = Vec::with_capacity(self.tasks.len());

        while let Some(curr) = ready_queue.pop_first() {
            order.push(curr.clone());
            if let Some(children) = dependents.get(&curr) {
                for child in children {
                    if let Some(deg) = in_degree.get_mut(child) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            ready_queue.insert(child.clone());
                        }
                    }
                }
            }
        }

        if order.len() != self.tasks.len() {
            let cyclic: Vec<&str> = in_degree
                .iter()
                .filter_map(|(id, &deg)| (deg > 0).then_some(id.as_str()))
                .collect();
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::UnknownTask,
                format!("cycle detected in orchestration DAG involving: {}", cyclic.join(", ")),
            ));
        }
        Ok(order)
    }

    /// Returns all tasks currently eligible to run (`Pending` or `Ready` with all
    /// dependencies in `Succeeded`), ordered deterministically by `task_id`.
    pub fn ready_tasks(&self) -> Vec<&OrchestrationTask> {
        self.tasks
            .values()
            .filter(|task| {
                matches!(
                    task.status,
                    OrchestrationTaskStatus::Pending | OrchestrationTaskStatus::Ready
                ) && self.dependencies_succeeded(task)
            })
            .collect()
    }

    /// Promotes all `Pending` tasks whose dependencies have `Succeeded` to `Ready`.
    pub fn refresh_readiness(&mut self, now_ms: i64) -> Vec<String> {
        let ready_ids: Vec<String> = self
            .tasks
            .values()
            .filter(|t| {
                t.status == OrchestrationTaskStatus::Pending && self.dependencies_succeeded(t)
            })
            .map(|t| t.task_id.clone())
            .collect();
        for id in &ready_ids {
            if let Some(task) = self.tasks.get_mut(id) {
                task.status = OrchestrationTaskStatus::Ready;
                task.updated_at_ms = now_ms;
            }
        }
        ready_ids
    }

    /// Transitions `task_id` to `next`, validating existence, legal state progression,
    /// and dependency satisfaction, then refreshes downstream task readiness.
    pub fn transition_task(
        &mut self,
        task_id: &str,
        next: OrchestrationTaskStatus,
        now_ms: i64,
    ) -> Result<(), OrcaIdeError> {
        let current_status = self
            .tasks
            .get(task_id)
            .ok_or_else(|| {
                OrcaIdeError::new(
                    OrcaIdeErrorCode::UnknownTask,
                    format!("unknown task '{task_id}'"),
                )
            })?
            .status;

        if current_status.is_terminal() && current_status != next {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::UnknownTask,
                format!(
                    "cannot transition terminal task '{task_id}' from {current_status:?} to {next:?}"
                ),
            ));
        }

        if matches!(
            next,
            OrchestrationTaskStatus::Ready | OrchestrationTaskStatus::Running
        ) {
            let deps_ok = self
                .tasks
                .get(task_id)
                .is_some_and(|t| self.dependencies_succeeded(t));
            if !deps_ok {
                return Err(OrcaIdeError::new(
                    OrcaIdeErrorCode::UnknownTask,
                    format!("task '{task_id}' cannot become {next:?} while dependencies are incomplete"),
                ));
            }
        }

        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = next;
            task.updated_at_ms = now_ms;
        }
        if next == OrchestrationTaskStatus::Succeeded {
            self.refresh_readiness(now_ms);
        }
        Ok(())
    }

    fn dependencies_succeeded(&self, task: &OrchestrationTask) -> bool {
        task.depends_on.iter().all(|dep| {
            self.tasks
                .get(dep)
                .is_some_and(|t| t.status == OrchestrationTaskStatus::Succeeded)
        })
    }
}

// ---------------------------------------------------------------------------
// Generation-Fenced Dispatches & Non-Guessing Worker Liveness
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    Pending,
    Dispatched,
    Completed,
    Failed,
    CircuitBroken,
}

/// Non-guessing worker liveness state (SM1-10): silence, timeout, or `Unverifiable`
/// NEVER proves exit or authorizes autonomous retry/respawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorkerLivenessState {
    Alive,
    Exited { exit_code: Option<i32> },
    Unverifiable,
}

impl WorkerLivenessState {
    /// Returns `true` ONLY when exit is positively confirmed; `Alive` and
    /// `Unverifiable` strictly forbid settling a task or auto-retrying.
    pub const fn confirms_exit(self) -> bool {
        matches!(self, Self::Exited { .. })
    }

    pub const fn authorizes_autonomous_retry(self) -> bool {
        matches!(self, Self::Exited { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchRecord {
    pub dispatch_id: String,
    pub task_id: String,
    pub host_id: String,
    pub generation: u64,
    pub status: DispatchStatus,
    pub liveness: WorkerLivenessState,
    pub nesting_depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_of: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Pure in-memory dispatch registry enforcing strict monotonic generation fencing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchRegistry {
    pub records: BTreeMap<String, DispatchRecord>,
}

impl DispatchRegistry {
    pub fn register(&mut self, record: DispatchRecord) -> Result<(), OrcaIdeError> {
        if record.dispatch_id.trim().is_empty() {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::UnknownDispatch,
                "dispatch_id cannot be empty",
            ));
        }
        if let Some(existing) = self.records.get(&record.dispatch_id)
            && record.generation <= existing.generation
        {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::StaleDispatch,
                format!(
                    "dispatch '{}' generation {} is stale (current {})",
                    record.dispatch_id, record.generation, existing.generation
                ),
            ));
        }
        self.records.insert(record.dispatch_id.clone(), record);
        Ok(())
    }

    pub fn verify_generation(
        &self,
        dispatch_id: &str,
        incoming_generation: u64,
    ) -> Result<&DispatchRecord, OrcaIdeError> {
        let rec = self.records.get(dispatch_id).ok_or_else(|| {
            OrcaIdeError::new(
                OrcaIdeErrorCode::UnknownDispatch,
                format!("unknown dispatch '{dispatch_id}'"),
            )
        })?;
        if incoming_generation != rec.generation {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::StaleDispatch,
                format!(
                    "stale dispatch '{dispatch_id}': expected generation {}, got {incoming_generation}",
                    rec.generation
                ),
            ));
        }
        Ok(rec)
    }

    pub fn advance_status(
        &mut self,
        dispatch_id: &str,
        expected_generation: u64,
        next_status: DispatchStatus,
        liveness: WorkerLivenessState,
        now_ms: i64,
    ) -> Result<u64, OrcaIdeError> {
        let rec = self.records.get_mut(dispatch_id).ok_or_else(|| {
            OrcaIdeError::new(
                OrcaIdeErrorCode::UnknownDispatch,
                format!("unknown dispatch '{dispatch_id}'"),
            )
        })?;
        if expected_generation != rec.generation {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::StaleDispatch,
                format!(
                    "stale dispatch '{dispatch_id}': expected generation {}, got {expected_generation}",
                    rec.generation
                ),
            ));
        }
        rec.generation = rec.generation.saturating_add(1);
        rec.status = next_status;
        rec.liveness = liveness;
        rec.updated_at_ms = now_ms;
        Ok(rec.generation)
    }
}

/// Idempotency receipt for durable orchestration mutations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationReceipt {
    pub caller_fingerprint: String,
    pub request_id: String,
    pub payload_hash: String,
    pub created_at_ms: i64,
}

// ---------------------------------------------------------------------------
// 9-Type Typed Mailbox & Routing Guards
// ---------------------------------------------------------------------------

/// Exhaustive 9 orchestration mailbox message types matching `ORCHESTRATION_MESSAGE_TYPES_9`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailboxMessageType {
    Status,
    Dispatch,
    WorkerDone,
    MergeReady,
    Escalation,
    Handoff,
    DecisionGate,
    Question,
    Heartbeat,
}

impl MailboxMessageType {
    pub const ALL: [Self; 9] = [
        Self::Status,
        Self::Dispatch,
        Self::WorkerDone,
        Self::MergeReady,
        Self::Escalation,
        Self::Handoff,
        Self::DecisionGate,
        Self::Question,
        Self::Heartbeat,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Dispatch => "dispatch",
            Self::WorkerDone => "worker_done",
            Self::MergeReady => "merge_ready",
            Self::Escalation => "escalation",
            Self::Handoff => "handoff",
            Self::DecisionGate => "decision_gate",
            Self::Question => "question",
            Self::Heartbeat => "heartbeat",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailboxPriority {
    #[default]
    Normal,
    High,
    Urgent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MailboxTarget {
    Direct { recipient_id: String },
    GroupAll,
    GroupIdle,
    GroupAgent { agent: String },
    GroupWorktree { worktree_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MailboxRoute {
    LocalOnly,
    ServerRelay { server_host_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailboxDeliveryState {
    Pending,
    Outstanding,
    Acknowledged,
    Fenced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailboxMessage {
    pub message_id: String,
    pub run_id: String,
    pub from_id: String,
    pub target: MailboxTarget,
    pub message_type: MailboxMessageType,
    pub priority: MailboxPriority,
    pub route: MailboxRoute,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_generation: Option<u64>,
    pub delivery_state: MailboxDeliveryState,
    pub created_at_ms: i64,
}

/// Validates local vs server mailbox routing rules:
/// - Rejects attempting server mail when the mailbox is strictly local (`MailboxIsLocal`).
/// - Rejects server relay when the target server does not support peer mail (`ServerMailUnsupported`).
pub fn validate_mailbox_route(
    route: &MailboxRoute,
    mailbox_is_local_only: bool,
    server_supports_mail: bool,
) -> Result<(), OrcaIdeError> {
    match route {
        MailboxRoute::LocalOnly => Ok(()),
        MailboxRoute::ServerRelay { server_host_id } => {
            if mailbox_is_local_only || server_host_id == LOCAL_EXECUTION_HOST_ID {
                return Err(OrcaIdeError::new(
                    OrcaIdeErrorCode::MailboxIsLocal,
                    format!(
                        "cannot route mailbox message through server relay '{server_host_id}': mailbox is local-only"
                    ),
                ));
            }
            if !server_supports_mail {
                return Err(OrcaIdeError::new(
                    OrcaIdeErrorCode::ServerMailUnsupported,
                    format!(
                        "remote server '{server_host_id}' does not support orchestration peer mail"
                    ),
                ));
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Review & Merge Gates + Wait Deadline Evaluation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pending,
    Approved,
    Rejected,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewGate {
    pub gate_id: String,
    pub task_id: String,
    pub required_approvals: u32,
    #[serde(default)]
    pub approvals: Vec<String>,
    #[serde(default)]
    pub rejections: Vec<String>,
    pub status: GateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<i64>,
}

impl ReviewGate {
    /// Evaluates current review gate state against `now_ms`, returning
    /// `OrcaIdeErrorCode::WaitTimeout` if an unresolved gate has exceeded its deadline.
    pub fn evaluate(&self, now_ms: i64) -> Result<GateStatus, OrcaIdeError> {
        if !self.rejections.is_empty() || self.status == GateStatus::Rejected {
            return Ok(GateStatus::Rejected);
        }
        let unique_approvals: BTreeSet<&str> =
            self.approvals.iter().map(String::as_str).collect();
        if unique_approvals.len() >= (self.required_approvals as usize)
            && self.required_approvals > 0
        {
            return Ok(GateStatus::Approved);
        }
        if self.status == GateStatus::Approved {
            return Ok(GateStatus::Approved);
        }
        evaluate_wait_deadline(self.deadline_ms, now_ms)?;
        Ok(GateStatus::Pending)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeGate {
    pub gate_id: String,
    pub task_id: String,
    pub ci_passed: bool,
    pub conflicts_resolved: bool,
    #[serde(default)]
    pub review_gates: Vec<ReviewGate>,
    pub require_human_approval: bool,
    pub human_approved: bool,
    pub status: GateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<i64>,
}

/// Checks whether a wait/gate deadline has expired, returning `OrcaIdeErrorCode::WaitTimeout`.
pub fn evaluate_wait_deadline(deadline_ms: Option<i64>, now_ms: i64) -> Result<(), OrcaIdeError> {
    if let Some(dl) = deadline_ms
        && now_ms >= dl
    {
        return Err(OrcaIdeError::new(
            OrcaIdeErrorCode::WaitTimeout,
            format!("wait deadline {dl} exceeded at {now_ms}"),
        ));
    }
    Ok(())
}

/// Evaluates a composite `MergeGate` (CI status, merge conflicts, child `ReviewGate`s,
/// optional human approval, and wait deadline).
pub fn evaluate_merge_gate(gate: &MergeGate, now_ms: i64) -> Result<GateStatus, OrcaIdeError> {
    if gate.status == GateStatus::Rejected {
        return Ok(GateStatus::Rejected);
    }

    let mut all_reviews_approved = true;
    for review in &gate.review_gates {
        match review.evaluate(now_ms)? {
            GateStatus::Rejected => return Ok(GateStatus::Rejected),
            GateStatus::Approved => {}
            GateStatus::Pending | GateStatus::TimedOut => {
                all_reviews_approved = false;
            }
        }
    }

    let human_ok = !gate.require_human_approval || gate.human_approved;
    if gate.ci_passed && gate.conflicts_resolved && all_reviews_approved && human_ok {
        return Ok(GateStatus::Approved);
    }

    evaluate_wait_deadline(gate.deadline_ms, now_ms)?;
    Ok(GateStatus::Pending)
}

// ---------------------------------------------------------------------------
// Multi-Host Federation & Execution Host Selection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionHostRecord {
    pub host_id: String,
    pub display_name: String,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
    pub online: bool,
    pub protocol_version: u32,
    pub active_workers: u32,
    pub max_workers: u32,
    pub last_heartbeat_ms: i64,
}

impl ExecutionHostRecord {
    pub fn is_local(&self) -> bool {
        self.host_id == LOCAL_EXECUTION_HOST_ID
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSelectionCriteria {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_host_id: Option<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    pub min_protocol_version: u32,
    pub max_heartbeat_age_ms: i64,
}

/// Selects a compatible online execution host for federated worker dispatch.
///
/// If `preferred_host_id` is explicitly provided (e.g., `--on <server>`), this
/// fails closed (`None`) rather than silently rerouting to another host if the
/// target host is offline, stale, incompatible, or at worker capacity.
pub fn select_execution_host<'a>(
    hosts: &'a [ExecutionHostRecord],
    criteria: &HostSelectionCriteria,
    now_ms: i64,
) -> Option<&'a ExecutionHostRecord> {
    let is_eligible = |h: &&ExecutionHostRecord| -> bool {
        if !h.online || h.protocol_version < criteria.min_protocol_version {
            return false;
        }
        if h.active_workers >= h.max_workers {
            return false;
        }
        let age = now_ms.saturating_sub(h.last_heartbeat_ms);
        if age < 0 || age > criteria.max_heartbeat_age_ms {
            return false;
        }
        criteria
            .required_capabilities
            .iter()
            .all(|cap| h.capabilities.contains(cap))
    };

    if let Some(preferred) = criteria.preferred_host_id.as_deref() {
        return hosts
            .iter()
            .find(|h| h.host_id == preferred && is_eligible(h));
    }

    hosts.iter().filter(is_eligible).min_by(|a, b| {
        a.active_workers
            .cmp(&b.active_workers)
            .then_with(|| b.is_local().cmp(&a.is_local()))
            .then_with(|| a.host_id.cmp(&b.host_id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_task(id: &str, deps: &[&str]) -> OrchestrationTask {
        OrchestrationTask {
            task_id: id.to_string(),
            run_id: "run_1".to_string(),
            title: format!("Task {id}"),
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            status: OrchestrationTaskStatus::Pending,
            assigned_host_id: None,
            assigned_dispatch_id: None,
            nesting_depth: 1,
            created_at_ms: 100,
            updated_at_ms: 100,
        }
    }

    #[test]
    fn dag_topological_sort_ready_tasks_transitions_and_cycle_rejection() {
        let mut dag = OrchestrationDag::new("run_1");
        dag.add_task(mk_task("t1", &[])).expect("add t1");
        dag.add_task(mk_task("t2", &["t1"])).expect("add t2");
        dag.add_task(mk_task("t3", &["t1"])).expect("add t3");
        dag.add_task(mk_task("t4", &["t2", "t3"])).expect("add t4");

        let order = dag.validate_acyclic().expect("acyclic");
        assert_eq!(order, vec!["t1", "t2", "t3", "t4"]);

        let ready: Vec<&str> = dag.ready_tasks().iter().map(|t| t.task_id.as_str()).collect();
        assert_eq!(ready, vec!["t1"]);

        // Cannot start t2 before t1 succeeds
        assert_eq!(
            dag.transition_task("t2", OrchestrationTaskStatus::Running, 200)
                .expect_err("dep not succeeded")
                .code,
            OrcaIdeErrorCode::UnknownTask
        );

        dag.transition_task("t1", OrchestrationTaskStatus::Running, 200)
            .expect("t1 running");
        dag.transition_task("t1", OrchestrationTaskStatus::Succeeded, 300)
            .expect("t1 succeeded");
        let ready2: Vec<&str> = dag.ready_tasks().iter().map(|t| t.task_id.as_str()).collect();
        assert_eq!(ready2, vec!["t2", "t3"]);

        // Unknown dependency rejection
        let mut bad_dep = OrchestrationDag::new("run_2");
        bad_dep.add_task(mk_task("a", &["missing"])).expect("add a");
        assert_eq!(
            bad_dep.validate_acyclic().expect_err("unknown dep").code,
            OrcaIdeErrorCode::UnknownTask
        );

        // Cycle rejection
        let mut cyclic = OrchestrationDag::new("run_3");
        cyclic.add_task(mk_task("a", &["b"])).expect("add a");
        cyclic.add_task(mk_task("b", &["a"])).expect("add b");
        assert_eq!(
            cyclic.validate_acyclic().expect_err("cycle").code,
            OrcaIdeErrorCode::UnknownTask
        );
    }

    #[test]
    fn dispatch_generation_fencing_liveness_and_mailbox_routes() {
        let mut reg = DispatchRegistry::default();
        reg.register(DispatchRecord {
            dispatch_id: "d1".to_string(),
            task_id: "t1".to_string(),
            host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
            generation: 1,
            status: DispatchStatus::Dispatched,
            liveness: WorkerLivenessState::Unverifiable,
            nesting_depth: 1,
            follow_up_of: None,
            created_at_ms: 100,
            updated_at_ms: 100,
        })
        .expect("register d1");

        // Unverifiable liveness NEVER authorizes retry or exit confirmation
        assert!(!WorkerLivenessState::Unverifiable.confirms_exit());
        assert!(!WorkerLivenessState::Unverifiable.authorizes_autonomous_retry());
        assert!(WorkerLivenessState::Exited { exit_code: Some(0) }.confirms_exit());

        // Unknown dispatch -> UnknownDispatch
        assert_eq!(
            reg.verify_generation("nope", 1).expect_err("unknown").code,
            OrcaIdeErrorCode::UnknownDispatch
        );

        // Advance generation 1 -> 2, then stale gen 1 fails with StaleDispatch
        let new_gen = reg
            .advance_status(
                "d1",
                1,
                DispatchStatus::Completed,
                WorkerLivenessState::Exited { exit_code: Some(0) },
                500,
            )
            .expect("advance");
        assert_eq!(new_gen, 2);
        assert_eq!(
            reg.verify_generation("d1", 1).expect_err("stale").code,
            OrcaIdeErrorCode::StaleDispatch
        );

        // Exact 9 mailbox message types
        let expected_types = [
            "status",
            "dispatch",
            "worker_done",
            "merge_ready",
            "escalation",
            "handoff",
            "decision_gate",
            "question",
            "heartbeat",
        ];
        assert_eq!(MailboxMessageType::ALL.len(), 9);
        for (ty, wire) in MailboxMessageType::ALL.into_iter().zip(expected_types) {
            assert_eq!(ty.as_str(), wire);
            assert_eq!(MailboxMessageType::parse(wire), Some(ty));
        }

        // Mailbox route guards
        let relay = MailboxRoute::ServerRelay {
            server_host_id: "remote-1".to_string(),
        };
        assert_eq!(
            validate_mailbox_route(&relay, true, true).expect_err("local mailbox").code,
            OrcaIdeErrorCode::MailboxIsLocal
        );
        assert_eq!(
            validate_mailbox_route(&relay, false, false).expect_err("unsupported").code,
            OrcaIdeErrorCode::ServerMailUnsupported
        );
        assert!(validate_mailbox_route(&relay, false, true).is_ok());
    }

    #[test]
    fn gates_wait_timeout_and_multi_host_federation_selection() {
        let mut merge = MergeGate {
            gate_id: "mg_1".to_string(),
            task_id: "t4".to_string(),
            ci_passed: true,
            conflicts_resolved: true,
            review_gates: vec![ReviewGate {
                gate_id: "rg_1".to_string(),
                task_id: "t4".to_string(),
                required_approvals: 2,
                approvals: vec!["alice".to_string()],
                rejections: Vec::new(),
                status: GateStatus::Pending,
                deadline_ms: Some(1_000),
            }],
            require_human_approval: true,
            human_approved: false,
            status: GateStatus::Pending,
            deadline_ms: Some(2_000),
        };
        assert_eq!(evaluate_merge_gate(&merge, 500).expect("pending"), GateStatus::Pending);
        assert_eq!(
            evaluate_merge_gate(&merge, 1_500).expect_err("review timeout").code,
            OrcaIdeErrorCode::WaitTimeout
        );
        merge.review_gates[0].approvals.push("bob".to_string());
        merge.human_approved = true;
        assert_eq!(evaluate_merge_gate(&merge, 1_500).expect("approved"), GateStatus::Approved);

        let hosts = vec![
            ExecutionHostRecord {
                host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
                display_name: "Local".to_string(),
                capabilities: BTreeSet::from(["pty".to_string(), "git".to_string()]),
                online: true,
                protocol_version: 3,
                active_workers: 2,
                max_workers: 4,
                last_heartbeat_ms: 9_900,
            },
            ExecutionHostRecord {
                host_id: "srv-west".to_string(),
                display_name: "West Build".to_string(),
                capabilities: BTreeSet::from(["pty".to_string(), "git".to_string(), "gpu".to_string()]),
                online: true,
                protocol_version: 3,
                active_workers: 1,
                max_workers: 8,
                last_heartbeat_ms: 9_950,
            },
        ];
        let criteria = HostSelectionCriteria {
            preferred_host_id: None,
            required_capabilities: vec!["pty".to_string(), "git".to_string()],
            min_protocol_version: 2,
            max_heartbeat_age_ms: 1_000,
        };
        assert_eq!(
            select_execution_host(&hosts, &criteria, 10_000).map(|h| h.host_id.as_str()),
            Some("srv-west")
        );

        // Explicit preferred host fails closed if offline/missing capability
        let strict_criteria = HostSelectionCriteria {
            preferred_host_id: Some(LOCAL_EXECUTION_HOST_ID.to_string()),
            required_capabilities: vec!["gpu".to_string()],
            min_protocol_version: 2,
            max_heartbeat_age_ms: 1_000,
        };
        assert!(select_execution_host(&hosts, &strict_criteria, 10_000).is_none());
    }
}
