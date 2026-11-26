//! 12-node visual DAG `WorkflowEngine`, Kahn topological `graph_order`, `run_scope`,
//! `next_actions`, `{{field}}` template interpolation, `parse_workflow_check` /
//! `workflow_check_passes`, approval holds/resumes, output deliveries, and GitHub chain guards (`OB-F11a`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, LazyLock};

use chrono::{Datelike, TimeZone, Utc};
use openbot_contracts::channels::{
    ChannelMessage, ChannelMessageAuthor, ChannelMessageAuthorKind,
};
use openbot_contracts::conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus,
};
use openbot_contracts::limits::{INPUT_LIMITS, utf16_len};
use openbot_contracts::queue::QueueSender;
use openbot_contracts::routines::{next_routine_occurrence, validate_routine_schedule};
use openbot_contracts::workflows::{
    WorkflowBranch, WorkflowDetail, WorkflowEdge, WorkflowFieldValue, WorkflowGraph,
    WorkflowGraphProblem, WorkflowNode, WorkflowNodeKind, WorkflowRunSource, WorkflowRunStatus,
    WorkflowStepStatus, WorkflowSummary, workflow_graph_problem_at, workflow_loop_edges,
};
use openbot_storage::{
    ChannelStore, CreateWorkflowInput, EnqueueInput, MailboxStore, NewWorkflowRun, NewWorkflowStep,
    OpenBotDatabase, StorageError, StoredWorkflowRun, StoredWorkflowStep, WebhookRouteStore,
    WorkflowGitHubItem, WorkflowGitHubSkipReason, WorkflowGitHubStore, WorkflowOutputStatus,
    WorkflowRunStore, WorkflowStepEnd, WorkflowStepSource, WorkflowStore, WorkflowValuesMode,
    node_timezone, runtime_fields_of_node,
};
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{OpenBotRuntimeError, RuntimeResult};

pub const GITHUB_CHAIN_RUNS: u64 = 3;
pub const GITHUB_CHAIN_WINDOW_MS: i64 = 600_000;
pub const GITHUB_SELF_WINDOW_MS: i64 = 3_600_000;

const FIRST_TIME_ASSOCIATIONS: [&str; 2] = ["FIRST_TIMER", "FIRST_TIME_CONTRIBUTOR"];
const PULL_REQUEST_GITHUB_EVENTS: [&str; 6] = [
    "pr.opened",
    "pr.updated",
    "pr.reviewRequested",
    "pr.reviewSubmitted",
    "pr.checksFailed",
    "pr.merged",
];
const MAX_LOOP_ITERATIONS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSaveRequest {
    pub workflow_id: Option<String>,
    pub title: String,
    pub graph: WorkflowGraph,
    pub raw_json_bytes: Option<usize>,
    pub allow_loops: bool,
    pub base_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSaveOutcome {
    pub workflow_id: String,
    pub title: String,
    pub node_count: usize,
    pub step_count: usize,
    pub edge_count: usize,
    pub revision: u64,
    pub loop_edges: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunRequest {
    pub workflow_id: String,
    pub run_id: Option<String>,
    pub payload: Option<String>,
    pub approve_node: Option<String>,
    pub reject_node: Option<String>,
    pub condition_mocks: BTreeMap<String, String>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStepReport {
    pub step_id: String,
    pub node_id: String,
    pub kind: WorkflowNodeKind,
    pub status: String,
    pub branch: Option<WorkflowBranch>,
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunOutcome {
    pub run_id: String,
    pub workflow_id: String,
    pub status: String,
    pub steps: Vec<WorkflowStepReport>,
    pub outputs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkAction {
    Run {
        node_id: String,
        inputs: Vec<String>,
    },
    Skip {
        node_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowCheckOperator {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Contains,
    Set,
    Unset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowCheckTest {
    pub name: String,
    pub operator: WorkflowCheckOperator,
    pub value: String,
}

pub type WorkflowCheck = Vec<Vec<WorkflowCheckTest>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubEventFacts {
    pub repository: String,
    pub events: Vec<String>,
    pub item: WorkflowGitHubItem,
    pub actor: String,
    pub actor_is_bot: bool,
    pub author_is_bot: bool,
    pub caused_by_bot: bool,
    pub association: Option<String>,
    pub labels: Vec<String>,
    pub label: Option<String>,
    pub base_branch: Option<String>,
    pub object_key: Option<String>,
}

pub struct WorkflowEngine {
    database: OpenBotDatabase,
    store: WorkflowStore,
    runs: WorkflowRunStore,
    github: WorkflowGitHubStore,
    webhooks: WebhookRouteStore,
    channels: ChannelStore,
    mailbox_cache: Mutex<Option<MailboxStore>>,
}

impl WorkflowEngine {
    pub fn new(database: OpenBotDatabase) -> Arc<Self> {
        Arc::new(Self {
            store: database.workflow_store(),
            runs: database.workflow_run_store(),
            github: database.workflow_github_store(),
            webhooks: database.webhook_route_store(),
            channels: database.channel_store(),
            mailbox_cache: Mutex::new(None),
            database,
        })
    }

    pub fn github_store(&self) -> &WorkflowGitHubStore {
        &self.github
    }

    pub fn webhook_routes(&self) -> &WebhookRouteStore {
        &self.webhooks
    }

    pub fn channels(&self) -> &ChannelStore {
        &self.channels
    }

    fn mailbox(&self) -> RuntimeResult<MailboxStore> {
        let mut guard = self.mailbox_cache.lock();
        if let Some(store) = guard.as_ref() {
            return Ok(store.clone());
        }
        let opened = self.database.mailbox_store().map_err(map_storage_error)?;
        *guard = Some(opened.clone());
        Ok(opened)
    }

    pub fn save_workflow(&self, req: WorkflowSaveRequest) -> RuntimeResult<WorkflowSaveOutcome> {
        let trimmed_title = req.title.trim();
        if trimmed_title.is_empty() || utf16_len(trimmed_title) > INPUT_LIMITS.workflow_title {
            return Err(OpenBotRuntimeError::Validation(format!(
                "workflow title must be 1..={} characters",
                INPUT_LIMITS.workflow_title
            )));
        }

        if req.graph.nodes.len() > INPUT_LIMITS.workflow_nodes {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Too many workflow nodes: {} exceeds maximum limit of {}",
                req.graph.nodes.len(),
                INPUT_LIMITS.workflow_nodes
            )));
        }
        if req.graph.edges.len() > INPUT_LIMITS.workflow_edges {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Too many workflow edges: {} exceeds maximum limit of {}",
                req.graph.edges.len(),
                INPUT_LIMITS.workflow_edges
            )));
        }
        if req.graph.strokes.len() > INPUT_LIMITS.workflow_strokes {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Too many workflow strokes: {} exceeds maximum limit of {}",
                req.graph.strokes.len(),
                INPUT_LIMITS.workflow_strokes
            )));
        }

        if let Some(raw_bytes) = req.raw_json_bytes
            && raw_bytes > INPUT_LIMITS.workflow_graph_bytes
        {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Workflow graph JSON size {raw_bytes} bytes exceeds 1 MiB ({}) byte limit",
                INPUT_LIMITS.workflow_graph_bytes
            )));
        }
        let encoded_len = serde_json::to_vec(&req.graph)?.len();
        if encoded_len > INPUT_LIMITS.workflow_graph_bytes {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Workflow graph JSON size {encoded_len} bytes exceeds 1 MiB ({}) byte limit",
                INPUT_LIMITS.workflow_graph_bytes
            )));
        }

        for edge in &req.graph.edges {
            if edge.from == edge.to {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Workflow edge {} is a self-loop / self-edge on node {}",
                    edge.id, edge.from
                )));
            }
        }

        if let Some((problem, edge)) = workflow_graph_problem_at(&req.graph) {
            return Err(map_graph_problem(problem, &edge));
        }

        req.graph
            .validate()
            .map_err(|err| OpenBotRuntimeError::Validation(err.to_string()))?;

        let loop_edges: Vec<String> = workflow_loop_edges(&req.graph.nodes, &req.graph.edges)
            .into_iter()
            .collect();
        if !loop_edges.is_empty() && !req.allow_loops {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Workflow graph contains a cycle / loop edges ({}); DAG must be acyclic unless allowLoops is true",
                loop_edges.join(", ")
            )));
        }

        let now_ms = Utc::now().timestamp_millis();
        for node in &req.graph.nodes {
            if node.kind == WorkflowNodeKind::Trigger
                && let Some(schedule) = &node.schedule
            {
                let tz = node_timezone(node).unwrap_or("UTC");
                validate_routine_schedule(schedule, tz).map_err(|err| {
                    OpenBotRuntimeError::Validation(format!(
                        "Workflow trigger {} has invalid schedule: {err}",
                        node.id
                    ))
                })?;
                next_routine_occurrence(schedule, tz, now_ms).map_err(|err| {
                    OpenBotRuntimeError::Validation(format!(
                        "Workflow trigger {} schedule has no upcoming occurrence: {err}",
                        node.id
                    ))
                })?;
            }
        }

        let (workflow_id, revision) = if let Some(wf_id) = req.workflow_id {
            let existing = self
                .store
                .detail(&wf_id)
                .map_err(map_storage_error)?
                .ok_or_else(|| {
                    OpenBotRuntimeError::NotFound(format!("workflow {wf_id} not found"))
                })?;
            if existing.summary.title != trimmed_title {
                self.store
                    .rename(&wf_id, trimmed_title, Some(now_ms))
                    .map_err(map_storage_error)?;
            }
            let base_rev = req.base_revision.unwrap_or(existing.revision);
            let next_rev = self
                .store
                .save_graph_strict(&wf_id, base_rev, &req.graph, Some(now_ms))
                .map_err(map_storage_error)?;
            (wf_id, next_rev)
        } else {
            let created = self
                .store
                .create(
                    CreateWorkflowInput {
                        title: trimmed_title.to_string(),
                        graph: Some(WorkflowGraph::default()),
                        paused: None,
                    },
                    Some(now_ms),
                    WorkflowValuesMode::New,
                )
                .map_err(map_storage_error)?;
            let wf_id = created.summary.id.clone();
            match self
                .store
                .save_graph_strict(&wf_id, created.revision, &req.graph, Some(now_ms))
            {
                Ok(rev) => (wf_id, rev),
                Err(err) => {
                    let _ = self.store.remove(&wf_id, Some(now_ms));
                    return Err(map_storage_error(err));
                }
            }
        };

        let node_count = req.graph.nodes.len();
        let step_count = req.graph.nodes.iter().filter(|n| n.kind.is_step()).count();
        let edge_count = req.graph.edges.len();

        Ok(WorkflowSaveOutcome {
            workflow_id,
            title: trimmed_title.to_string(),
            node_count,
            step_count,
            edge_count,
            revision,
            loop_edges,
        })
    }

    pub fn get_workflow(&self, workflow_id: &str) -> RuntimeResult<Option<WorkflowDetail>> {
        self.store.detail(workflow_id).map_err(map_storage_error)
    }

    pub fn list_workflows(&self) -> RuntimeResult<Vec<WorkflowSummary>> {
        self.store.list().map_err(map_storage_error)
    }

    pub fn delete_workflow(&self, workflow_id: &str) -> RuntimeResult<bool> {
        self.store
            .remove(workflow_id, Some(Utc::now().timestamp_millis()))
            .map_err(map_storage_error)
    }

    pub fn run_workflow(&self, req: WorkflowRunRequest) -> RuntimeResult<WorkflowRunOutcome> {
        if let Some(ref run_id) = req.run_id {
            if let Some(ref approve_node) = req.approve_node {
                return self.decide_approval(run_id, approve_node, true, &req.condition_mocks);
            }
            if let Some(ref reject_node) = req.reject_node {
                return self.decide_approval(run_id, reject_node, false, &req.condition_mocks);
            }
        }

        let detail = self
            .store
            .detail(&req.workflow_id)
            .map_err(map_storage_error)?
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!("workflow {} not found", req.workflow_id))
            })?;

        if detail.summary.paused == Some(true) {
            return Err(OpenBotRuntimeError::Conflict(format!(
                "workflow {} is paused",
                req.workflow_id
            )));
        }

        let now_ms = Utc::now().timestamp_millis();
        self.store
            .add_run_version(&detail.summary.id, detail.revision, &detail.graph, Some(now_ms))
            .map_err(map_storage_error)?;

        let trigger_node = detail
            .graph
            .nodes
            .iter()
            .find(|n| n.kind == WorkflowNodeKind::Trigger);
        let trigger_node_id = trigger_node.map(|n| n.id.clone());
        let source = trigger_node
            .map(trigger_run_source)
            .unwrap_or(WorkflowRunSource::Manual);
        let idempotency_key = req
            .idempotency_key
            .filter(|k| !k.trim().is_empty())
            .unwrap_or_else(|| format!("manual:{}:{}", detail.summary.id, Uuid::new_v4()));

        let (run, _created) = self
            .runs
            .start_run(
                NewWorkflowRun {
                    workflow_id: detail.summary.id.clone(),
                    trigger_node_id,
                    from_node_id: None,
                    source,
                    scheduled_for: None,
                    idempotency_key,
                    revision: detail.revision,
                    graph: detail.graph.clone(),
                    payload: req.payload,
                },
                Some(now_ms),
            )
            .map_err(map_storage_error)?;

        self.advance_run(&run.id, &detail.summary.title, &req.condition_mocks)
    }

    pub fn decide_approval(
        &self,
        run_id: &str,
        node_or_step_id: &str,
        approved: bool,
        condition_mocks: &BTreeMap<String, String>,
    ) -> RuntimeResult<WorkflowRunOutcome> {
        let run = self
            .runs
            .run(run_id)
            .map_err(map_storage_error)?
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!("workflow run {run_id} not found"))
            })?;
        let steps = self.runs.steps(run_id).map_err(map_storage_error)?;
        let target = steps
            .iter()
            .rev()
            .find(|s| {
                s.status == WorkflowStepStatus::Approval
                    && (s.id == node_or_step_id || s.node_id == node_or_step_id)
            })
            .cloned()
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!(
                    "no approval step matching '{node_or_step_id}' awaiting approval in run {run_id}"
                ))
            })?;

        let now_ms = Utc::now().timestamp_millis();
        if approved {
            self.runs
                .settle_step(
                    &target.id,
                    &WorkflowStepEnd {
                        status: WorkflowStepStatus::Done,
                        output: Some(target.input.clone()),
                        branch: None,
                        error: None,
                    },
                    Some(now_ms),
                )
                .map_err(map_storage_error)?;
            self.runs
                .set_run_status(run_id, WorkflowRunStatus::Running, None, Some(now_ms))
                .map_err(map_storage_error)?;
            let title = self
                .store
                .detail(&run.workflow_id)
                .map_err(map_storage_error)?
                .map(|d| d.summary.title)
                .unwrap_or_default();
            self.advance_run(run_id, &title, condition_mocks)
        } else {
            self.runs
                .settle_step(
                    &target.id,
                    &WorkflowStepEnd {
                        status: WorkflowStepStatus::Stopped,
                        output: None,
                        branch: None,
                        error: Some("approval rejected".to_string()),
                    },
                    Some(now_ms),
                )
                .map_err(map_storage_error)?;
            for open in steps
                .iter()
                .filter(|s| s.id != target.id && s.is_open())
            {
                self.runs
                    .settle_step(
                        &open.id,
                        &WorkflowStepEnd {
                            status: WorkflowStepStatus::Stopped,
                            output: None,
                            branch: None,
                            error: Some("approval rejected".to_string()),
                        },
                        Some(now_ms),
                    )
                    .map_err(map_storage_error)?;
            }
            self.runs
                .set_run_status(
                    run_id,
                    WorkflowRunStatus::Stopped,
                    Some("approval rejected"),
                    Some(now_ms),
                )
                .map_err(map_storage_error)?;
            self.build_run_outcome(run_id)
        }
    }

    pub fn evaluate_github_guard(
        &self,
        workflow_id: &str,
        node: &WorkflowNode,
        facts: &GitHubEventFacts,
        now_ms: i64,
    ) -> RuntimeResult<Option<WorkflowGitHubSkipReason>> {
        if github_filter_refuses(node, facts) {
            return Ok(Some(WorkflowGitHubSkipReason::Filter));
        }
        if self
            .github
            .self_caused(
                workflow_id,
                &facts.repository,
                facts.item.number,
                facts.object_key.as_deref(),
                now_ms - GITHUB_SELF_WINDOW_MS,
            )
            .map_err(map_storage_error)?
        {
            return Ok(Some(WorkflowGitHubSkipReason::SelfCaused));
        }
        if facts.caused_by_bot {
            let recent_bot_runs = self
                .github
                .bot_runs_since(
                    &facts.repository,
                    facts.item.number,
                    now_ms - GITHUB_CHAIN_WINDOW_MS,
                )
                .map_err(map_storage_error)?;
            if recent_bot_runs >= GITHUB_CHAIN_RUNS {
                return Ok(Some(WorkflowGitHubSkipReason::Chain));
            }
        }
        Ok(None)
    }

    fn advance_run(
        &self,
        run_id: &str,
        workflow_title: &str,
        condition_mocks: &BTreeMap<String, String>,
    ) -> RuntimeResult<WorkflowRunOutcome> {
        let run = self
            .runs
            .run(run_id)
            .map_err(map_storage_error)?
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!("workflow run {run_id} not found"))
            })?;
        let nodes: HashMap<&str, &WorkflowNode> = run
            .graph
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();

        let mut latest = self.latest_steps(run_id)?;
        let mut iterations = 0usize;
        while iterations < MAX_LOOP_ITERATIONS {
            iterations += 1;
            let answered: HashSet<String> = latest
                .values()
                .filter(|s| s.status == WorkflowStepStatus::Done)
                .map(|s| s.node_id.clone())
                .collect();
            let (scope, _reused) = run_scope(
                &run.graph,
                run.trigger_node_id.as_deref(),
                run.from_node_id.as_deref(),
                &answered,
            );
            let actions = next_actions(&run.graph, &scope, &latest);
            if actions.is_empty() {
                break;
            }
            for action in actions {
                match action {
                    WalkAction::Skip { node_id } => {
                        let Some(&node) = nodes.get(node_id.as_str()) else {
                            continue;
                        };
                        let attempt = latest.get(&node.id).map_or(0, |s| s.attempt) + 1;
                        let now_ms = Utc::now().timestamp_millis();
                        self.runs
                            .add_step(
                                NewWorkflowStep {
                                    workflow_id: run.workflow_id.clone(),
                                    run_id: Some(run.id.clone()),
                                    node_id: node.id.clone(),
                                    kind: node.kind,
                                    request_id: format!("{}:{}:{attempt}", run.id, node.id),
                                    agent_id: node.agent_id().map(str::to_string),
                                    status: WorkflowStepStatus::Skipped,
                                    input: String::new(),
                                    settings: None,
                                    sources: None,
                                    output: None,
                                    branch: None,
                                    error: None,
                                },
                                Some(now_ms),
                            )
                            .map_err(map_storage_error)?;
                    }
                    WalkAction::Run { node_id, inputs } => {
                        let Some(&node) = nodes.get(node_id.as_str()) else {
                            continue;
                        };
                        let attempt = latest.get(&node.id).map_or(0, |s| s.attempt) + 1;
                        let now_ms = Utc::now().timestamp_millis();
                        let template_values = collect_template_values(&run, &latest);
                        let step_input =
                            compose_step_input(node, workflow_title, &inputs, &template_values);
                        let sources = collect_step_sources(&run.graph, &node.id, &latest);
                        let step = self
                            .runs
                            .add_step(
                                NewWorkflowStep {
                                    workflow_id: run.workflow_id.clone(),
                                    run_id: Some(run.id.clone()),
                                    node_id: node.id.clone(),
                                    kind: node.kind,
                                    request_id: format!("{}:{}:{attempt}", run.id, node.id),
                                    agent_id: node.agent_id().map(str::to_string),
                                    status: WorkflowStepStatus::Running,
                                    input: step_input,
                                    settings: Some(runtime_fields_of_node(node)),
                                    sources: Some(sources),
                                    output: None,
                                    branch: None,
                                    error: None,
                                },
                                Some(now_ms),
                            )
                            .map_err(map_storage_error)?;

                        if step.status == WorkflowStepStatus::Running {
                            self.execute_step(
                                &run,
                                workflow_title,
                                &step,
                                node,
                                &template_values,
                                &latest,
                                condition_mocks,
                                now_ms,
                            )?;
                        }
                    }
                }
            }
            latest = self.latest_steps(run_id)?;
        }

        let ends: Vec<&StoredWorkflowStep> = latest.values().collect();
        let open: Vec<&StoredWorkflowStep> = ends.iter().copied().filter(|s| s.is_open()).collect();
        let terminal_status = if !open.is_empty() {
            if open.iter().all(|s| s.status == WorkflowStepStatus::Approval) {
                WorkflowRunStatus::Approval
            } else {
                WorkflowRunStatus::Running
            }
        } else if ends.iter().any(|s| s.status == WorkflowStepStatus::Failed) {
            WorkflowRunStatus::Failed
        } else if ends.iter().any(|s| s.status == WorkflowStepStatus::Stopped) {
            WorkflowRunStatus::Stopped
        } else {
            WorkflowRunStatus::Done
        };

        let now_ms = Utc::now().timestamp_millis();
        self.runs
            .set_run_status(run_id, terminal_status, None, Some(now_ms))
            .map_err(map_storage_error)?;

        self.build_run_outcome(run_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_step(
        &self,
        run: &StoredWorkflowRun,
        workflow_title: &str,
        step: &StoredWorkflowStep,
        node: &WorkflowNode,
        template_values: &BTreeMap<String, String>,
        latest: &HashMap<String, StoredWorkflowStep>,
        condition_mocks: &BTreeMap<String, String>,
        now_ms: i64,
    ) -> RuntimeResult<()> {
        match node.kind {
            WorkflowNodeKind::Trigger => {
                let output = trigger_step_output(node, run, now_ms);
                self.runs
                    .settle_step(
                        &step.id,
                        &WorkflowStepEnd {
                            status: WorkflowStepStatus::Done,
                            output: Some(output),
                            branch: None,
                            error: None,
                        },
                        Some(now_ms),
                    )
                    .map_err(map_storage_error)?;
            }
            WorkflowNodeKind::Prompt => {
                let text = fill_workflow_template(field_text(node, "text"), template_values);
                self.runs
                    .settle_step(
                        &step.id,
                        &WorkflowStepEnd {
                            status: WorkflowStepStatus::Done,
                            output: Some(text),
                            branch: None,
                            error: None,
                        },
                        Some(now_ms),
                    )
                    .map_err(map_storage_error)?;
            }
            WorkflowNodeKind::Asset => {
                let file_id = field_text(node, "fileId");
                if file_id.trim().is_empty() {
                    self.runs
                        .settle_step(
                            &step.id,
                            &WorkflowStepEnd {
                                status: WorkflowStepStatus::Failed,
                                output: None,
                                branch: None,
                                error: Some("asset step requires non-empty fileId".to_string()),
                            },
                            Some(now_ms),
                        )
                        .map_err(map_storage_error)?;
                } else {
                    self.runs
                        .settle_step(
                            &step.id,
                            &WorkflowStepEnd {
                                status: WorkflowStepStatus::Done,
                                output: Some(file_id.to_string()),
                                branch: None,
                                error: None,
                            },
                            Some(now_ms),
                        )
                        .map_err(map_storage_error)?;
                }
            }
            WorkflowNodeKind::Llm => {
                let system = fill_workflow_template(field_text(node, "system"), template_values);
                let prompt = fill_workflow_template(field_text(node, "prompt"), template_values);
                let composed = model_step_prompt(&system, &prompt, &step.input);
                let output = if composed.is_empty() {
                    step.input.clone()
                } else {
                    composed
                };
                self.runs
                    .settle_step(
                        &step.id,
                        &WorkflowStepEnd {
                            status: WorkflowStepStatus::Done,
                            output: Some(output),
                            branch: None,
                            error: None,
                        },
                        Some(now_ms),
                    )
                    .map_err(map_storage_error)?;
            }
            WorkflowNodeKind::Agent => {
                let Some(agent_id) = node.agent_id() else {
                    self.runs
                        .settle_step(
                            &step.id,
                            &WorkflowStepEnd {
                                status: WorkflowStepStatus::Failed,
                                output: None,
                                branch: None,
                                error: Some("agent step missing agentId".to_string()),
                            },
                            Some(now_ms),
                        )
                        .map_err(map_storage_error)?;
                    return Ok(());
                };
                let mailbox = self.mailbox()?;
                let receipt = mailbox
                    .enqueue(EnqueueInput {
                        sender: Some(QueueSender::Routine {
                            routine_id: run.workflow_id.clone(),
                            run_id: run.id.clone(),
                            routine_name: workflow_title.to_string(),
                            scheduled_for: run
                                .scheduled_for
                                .clone()
                                .unwrap_or_else(|| run.started_at.clone()),
                        }),
                        recipient_agent_ids: vec![agent_id.to_string()],
                        text: step.input.clone(),
                        idempotency_key: Some(format!("workflow-step:{}", step.id)),
                        ..Default::default()
                    })
                    .map_err(map_storage_error)?;
                if let Some(deliv) = receipt.deliveries.first() {
                    self.runs
                        .attach_delivery(&step.id, &deliv.id, Some(now_ms))
                        .map_err(map_storage_error)?;
                }
                self.runs
                    .settle_step(
                        &step.id,
                        &WorkflowStepEnd {
                            status: WorkflowStepStatus::Done,
                            output: Some(step.input.clone()),
                            branch: None,
                            error: None,
                        },
                        Some(now_ms),
                    )
                    .map_err(map_storage_error)?;
            }
            WorkflowNodeKind::Image => {
                let media = field_text(node, "media");
                let media_kind = if media.is_empty() { "image" } else { media };
                let model = field_text(node, "model");
                let model_id = if model.is_empty() { "gpt-image" } else { model };
                let prompt = field_text(node, "prompt").trim();
                if prompt.is_empty() && step.input.trim().is_empty() {
                    self.runs
                        .settle_step(
                            &step.id,
                            &WorkflowStepEnd {
                                status: WorkflowStepStatus::Failed,
                                output: None,
                                branch: None,
                                error: Some("media step requires prompt or input".to_string()),
                            },
                            Some(now_ms),
                        )
                        .map_err(map_storage_error)?;
                } else {
                    let artifact_ref = format!("media:{media_kind}:{model_id}:{}", step.id);
                    self.runs
                        .settle_step(
                            &step.id,
                            &WorkflowStepEnd {
                                status: WorkflowStepStatus::Done,
                                output: Some(artifact_ref),
                                branch: None,
                                error: None,
                            },
                            Some(now_ms),
                        )
                        .map_err(map_storage_error)?;
                }
            }
            WorkflowNodeKind::Condition => {
                let branch = evaluate_condition_branch(node, step, run, latest, condition_mocks);
                self.runs
                    .settle_step(
                        &step.id,
                        &WorkflowStepEnd {
                            status: WorkflowStepStatus::Done,
                            output: Some(step.input.clone()),
                            branch: Some(branch),
                            error: None,
                        },
                        Some(now_ms),
                    )
                    .map_err(map_storage_error)?;
            }
            WorkflowNodeKind::Approval => {
                self.runs
                    .await_approval(&step.id, Some(now_ms))
                    .map_err(map_storage_error)?;
            }
            WorkflowNodeKind::Output => {
                self.execute_output_step(run, workflow_title, step, node, template_values, now_ms)?;
            }
            WorkflowNodeKind::Text | WorkflowNodeKind::Note | WorkflowNodeKind::Shape => {
                self.runs
                    .settle_step(
                        &step.id,
                        &WorkflowStepEnd {
                            status: WorkflowStepStatus::Skipped,
                            output: None,
                            branch: None,
                            error: None,
                        },
                        Some(now_ms),
                    )
                    .map_err(map_storage_error)?;
            }
        }
        Ok(())
    }

    fn execute_output_step(
        &self,
        run: &StoredWorkflowRun,
        workflow_title: &str,
        step: &StoredWorkflowStep,
        node: &WorkflowNode,
        template_values: &BTreeMap<String, String>,
        now_ms: i64,
    ) -> RuntimeResult<()> {
        let target_raw = field_text(node, "target");
        let target = if target_raw.is_empty() {
            "notify"
        } else {
            target_raw
        };
        if target == "file" {
            self.runs
                .settle_step(
                    &step.id,
                    &WorkflowStepEnd {
                        status: WorkflowStepStatus::Failed,
                        output: None,
                        branch: None,
                        error: Some("file output target is unsupported".to_string()),
                    },
                    Some(now_ms),
                )
                .map_err(map_storage_error)?;
            return Ok(());
        }

        let raw_msg = field_text(node, "message");
        let output_text = if !raw_msg.trim().is_empty() {
            fill_workflow_template(raw_msg, template_values)
        } else {
            step.input.clone()
        };

        let channel_id = field_text(node, "channelId");
        if matches!(target, "channel" | "chat-channel")
            && !channel_id.is_empty()
            && self.channels.exists(channel_id).unwrap_or(false)
            && let Ok(mut channel) = self.channels.get(channel_id)
            && !channel.archived
        {
            let next_seq = self
                .channels
                .messages(channel_id, None, None)
                .unwrap_or_default()
                .iter()
                .map(|m| m.sequence)
                .max()
                .unwrap_or(0)
                + 1;
            channel.revision += 1;
            let msg_id = Uuid::new_v4().to_string();
            let now_iso = ms_to_iso(now_ms);
            let ch_msg = ChannelMessage {
                id: msg_id.clone(),
                channel_id: channel.id.clone(),
                sequence: next_seq,
                author: ChannelMessageAuthor {
                    kind: ChannelMessageAuthorKind::Coordinator,
                    id: format!("workflow:{}", run.workflow_id),
                    name: workflow_title.to_string(),
                },
                task_id: None,
                superseded: false,
                message: ConversationMessage::new_simple(
                    msg_id,
                    ConversationMessageAuthor::System,
                    output_text.clone(),
                    now_iso,
                    ConversationMessageStatus::Completed,
                ),
                deleted: None,
                sender_task_id: None,
            };
            let _ = self.channels.capture_message(channel, ch_msg);
        }

        let place_id = [
            field_text(node, "placeId"),
            channel_id,
            field_text(node, "repository"),
            node.id.as_str(),
        ]
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .unwrap_or(node.id.as_str())
        .to_string();

        let conn_field = field_text(node, "connectionId");
        let connection_id = if !conn_field.trim().is_empty() {
            conn_field.trim().to_string()
        } else {
            format!("workflow-output:{target}:{place_id}")
        };

        self.ensure_output_connection(&connection_id, target, now_ms)?;
        self.runs
            .add_output(&step.id, &connection_id, &place_id, &output_text, Some(now_ms))
            .map_err(map_storage_error)?;
        let _ = self.runs.claim_output(&step.id, Some(now_ms));
        self.runs
            .end_output(&step.id, WorkflowOutputStatus::Sent, None, Some(now_ms))
            .map_err(map_storage_error)?;

        self.runs
            .settle_step(
                &step.id,
                &WorkflowStepEnd {
                    status: WorkflowStepStatus::Done,
                    output: Some(output_text),
                    branch: None,
                    error: None,
                },
                Some(now_ms),
            )
            .map_err(map_storage_error)?;
        Ok(())
    }

    fn ensure_output_connection(
        &self,
        connection_id: &str,
        platform: &str,
        now_ms: i64,
    ) -> RuntimeResult<()> {
        let now_iso = ms_to_iso(now_ms);
        self.database
            .with_conn(|conn| {
                conn.execute(
                    "INSERT OR IGNORE INTO projection_messaging_connections \
                       (connection_id, platform, workspace_id, workspace_name, enabled, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
                    params![connection_id, platform, connection_id, connection_id, now_iso, now_iso],
                )?;
                Ok(())
            })
            .map_err(map_storage_error)
    }

    fn latest_steps(&self, run_id: &str) -> RuntimeResult<HashMap<String, StoredWorkflowStep>> {
        let all = self.runs.steps(run_id).map_err(map_storage_error)?;
        let mut latest: HashMap<String, StoredWorkflowStep> = HashMap::new();
        for step in all {
            let replace = latest
                .get(&step.node_id)
                .is_none_or(|cur| step.attempt >= cur.attempt);
            if replace {
                latest.insert(step.node_id.clone(), step);
            }
        }
        Ok(latest)
    }

    fn build_run_outcome(&self, run_id: &str) -> RuntimeResult<WorkflowRunOutcome> {
        let run = self
            .runs
            .run(run_id)
            .map_err(map_storage_error)?
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!("workflow run {run_id} not found"))
            })?;
        let steps_raw = self.runs.steps(run_id).map_err(map_storage_error)?;
        let mut latest_outputs: BTreeMap<String, String> = BTreeMap::new();
        let mut reports = Vec::with_capacity(steps_raw.len());
        for s in steps_raw {
            if s.kind == WorkflowNodeKind::Output && s.status == WorkflowStepStatus::Done {
                latest_outputs.insert(s.node_id.clone(), s.output.clone().unwrap_or_default());
            }
            reports.push(WorkflowStepReport {
                step_id: s.id,
                node_id: s.node_id,
                kind: s.kind,
                status: wire_step_status(s.status).to_string(),
                branch: s.branch,
                output: s.output,
            });
        }
        Ok(WorkflowRunOutcome {
            run_id: run.id,
            workflow_id: run.workflow_id,
            status: wire_run_status(run.status).to_string(),
            steps: reports,
            outputs: latest_outputs,
        })
    }
}

/// Exact 1:1 port of `workflow-walk.ts:runScope` (excludes annotation nodes `Text|Note|Shape`).
pub fn run_scope(
    graph: &WorkflowGraph,
    trigger_node_id: Option<&str>,
    from_node_id: Option<&str>,
    answered: &HashSet<String>,
) -> (HashSet<String>, Vec<String>) {
    let steps: HashMap<&str, &WorkflowNode> = graph
        .nodes
        .iter()
        .filter(|n| n.kind.is_step())
        .map(|n| (n.id.as_str(), n))
        .collect();
    let origin = trigger_node_id.or(from_node_id);
    let Some(origin_id) = origin.filter(|id| steps.contains_key(*id)) else {
        return (
            steps.keys().map(|id| (*id).to_string()).collect(),
            Vec::new(),
        );
    };

    let loops = workflow_loop_edges(&graph.nodes, &graph.edges);
    let forward: Vec<&WorkflowEdge> = graph
        .edges
        .iter()
        .filter(|e| !loops.contains(e.id.as_str()))
        .collect();
    let out_forward = edges_by(&forward, true);
    let all_edges: Vec<&WorkflowEdge> = graph.edges.iter().collect();
    let out_all = edges_by(&all_edges, true);
    let in_all = edges_by(&all_edges, false);

    let ahead = reach(origin_id, &out_forward);
    let after = reach(origin_id, &out_all);
    let mut nodes: HashSet<String> = after.iter().map(|s| (*s).to_string()).collect();
    let mut reused: HashSet<String> = if from_node_id.is_some() {
        after
            .iter()
            .filter(|id| !ahead.contains(*id) && answered.contains(**id))
            .map(|id| (*id).to_string())
            .collect()
    } else {
        HashSet::new()
    };

    let mut pending: Vec<String> = after
        .iter()
        .filter(|id| !reused.contains(**id))
        .map(|id| (*id).to_string())
        .collect();
    while let Some(id) = pending.pop() {
        if let Some(incoming) = in_all.get(id.as_str()) {
            for edge in incoming {
                let Some(&source) = steps.get(edge.from.as_str()) else {
                    continue;
                };
                if nodes.contains(&source.id) {
                    continue;
                }
                if trigger_node_id.is_some() && source.kind == WorkflowNodeKind::Trigger {
                    continue;
                }
                nodes.insert(source.id.clone());
                if from_node_id.is_some() && answered.contains(&source.id) {
                    reused.insert(source.id.clone());
                    continue;
                }
                pending.push(source.id.clone());
            }
        }
    }

    let ordered_reused = graph_order(&graph.nodes, &forward, &reused);
    (nodes, ordered_reused)
}

/// Kahn topological order over forward edges restricted to `scope` (`workflow-walk.ts:graphOrder`).
pub fn graph_order(
    nodes: &[WorkflowNode],
    forward: &[&WorkflowEdge],
    scope: &HashSet<String>,
) -> Vec<String> {
    let edges: Vec<&WorkflowEdge> = forward
        .iter()
        .copied()
        .filter(|e| scope.contains(&e.from) && scope.contains(&e.to))
        .collect();
    let mut remaining: HashMap<&str, usize> = scope.iter().map(|id| (id.as_str(), 0)).collect();
    for e in &edges {
        if let Some(cnt) = remaining.get_mut(e.to.as_str()) {
            *cnt += 1;
        }
    }
    let next = edges_by(&edges, true);
    let mut ready: VecDeque<&str> = nodes
        .iter()
        .filter(|n| remaining.get(n.id.as_str()) == Some(&0))
        .map(|n| n.id.as_str())
        .collect();
    let mut order = Vec::with_capacity(scope.len());
    while let Some(id) = ready.pop_front() {
        order.push(id.to_string());
        if let Some(outs) = next.get(id) {
            for edge in outs {
                if let Some(left) = remaining.get_mut(edge.to.as_str()) {
                    *left = left.saturating_sub(1);
                    if *left == 0 {
                        ready.push_back(edge.to.as_str());
                    }
                }
            }
        }
    }
    order
}

/// Exact 1:1 port of `workflow-walk.ts:nextActions`.
pub fn next_actions(
    graph: &WorkflowGraph,
    scope: &HashSet<String>,
    latest: &HashMap<String, StoredWorkflowStep>,
) -> Vec<WalkAction> {
    let loops = workflow_loop_edges(&graph.nodes, &graph.edges);
    let forward: Vec<&WorkflowEdge> = graph
        .edges
        .iter()
        .filter(|e| !loops.contains(e.id.as_str()))
        .collect();
    let all_edges: Vec<&WorkflowEdge> = graph.edges.iter().collect();
    let before = edges_by(&all_edges, false);
    let kinds: HashMap<&str, WorkflowNodeKind> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n.kind)).collect();

    let mut due: HashSet<String> = HashSet::new();
    let mut this_round: HashSet<String> = HashSet::new();
    let mut actions = Vec::new();

    for node_id in graph_order(&graph.nodes, &forward, scope) {
        let current = latest.get(&node_id);
        if current.is_some_and(StoredWorkflowStep::is_open) {
            continue;
        }
        let all_in = before
            .get(node_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let edges: Vec<&WorkflowEdge> = all_in
            .iter()
            .copied()
            .filter(|e| scope.contains(&e.from))
            .collect();
        let ahead: Vec<&WorkflowEdge> = edges
            .iter()
            .copied()
            .filter(|e| !loops.contains(e.id.as_str()))
            .collect();
        let back: Vec<&WorkflowEdge> = edges
            .iter()
            .copied()
            .filter(|e| loops.contains(e.id.as_str()))
            .collect();

        let is_newer = |edge: &WorkflowEdge| -> bool {
            latest
                .get(&edge.from)
                .is_some_and(|src| current.is_none_or(|cur| src.sequence > cur.sequence))
        };
        let next_pass = back.iter().any(|edge| {
            latest
                .get(&edge.from)
                .is_some_and(|src| edge_active(edge, src) && is_newer(edge))
        });
        let input_changes = ahead
            .iter()
            .any(|edge| is_newer(edge) || this_round.contains(&edge.from));
        if current.is_some() && !next_pass && !input_changes {
            continue;
        }
        due.insert(node_id.clone());
        if current.is_none() || input_changes {
            this_round.insert(node_id.clone());
        }
        if ahead
            .iter()
            .any(|edge| due.contains(&edge.from) || is_waiting(latest.get(&edge.from)))
        {
            continue;
        }
        let outside = all_in.len() > edges.len();
        let answered = edges.iter().any(|edge| {
            latest
                .get(&edge.from)
                .is_some_and(|src| edge_active(edge, src))
        });
        if !answered && (!ahead.is_empty() || outside) {
            actions.push(WalkAction::Skip { node_id });
            continue;
        }
        let text_in: Vec<&WorkflowEdge> = edges
            .iter()
            .copied()
            .filter(|e| {
                !matches!(
                    kinds.get(e.from.as_str()),
                    Some(WorkflowNodeKind::Image | WorkflowNodeKind::Asset)
                )
            })
            .collect();
        let inputs: Vec<String> = text_in
            .iter()
            .filter_map(|edge| {
                let src = latest.get(&edge.from)?;
                edge_active(edge, src).then(|| src.output.clone().unwrap_or_default())
            })
            .collect();
        actions.push(WalkAction::Run { node_id, inputs });
    }
    actions
}

static TEMPLATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]{0,39})\s*\}\}").expect("valid template regex")
});
static CHECK_CMP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z_][A-Za-z0-9_.]{0,63})\s*(!=|>=|<=|==|=|>|<)\s*([^\s=<>!].*)$")
        .expect("valid comparison regex")
});
static CHECK_WORDED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^([A-Za-z_][A-Za-z0-9_.]{0,63})\s+(contains|is not|is)\s+(.+)$")
        .expect("valid worded check regex")
});
static CHECK_BARE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(not\s+)?([A-Za-z_][A-Za-z0-9_.]{0,63})$").expect("valid bare check regex")
});
static CONDITION_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[^A-Za-z]*([A-Za-z]+)").expect("valid condition word regex")
});

/// Replaces `{{field}}` placeholders using `values` (`ipc-workflow-answers.ts:fillWorkflowTemplate`).
pub fn fill_workflow_template(text: &str, values: &BTreeMap<String, String>) -> String {
    if !text.contains("{{") {
        return text.to_string();
    }
    TEMPLATE_RE
        .replace_all(text, |caps: &regex::Captures<'_>| {
            let name = caps.get(1).map_or("", |m| m.as_str());
            values
                .get(name)
                .cloned()
                .unwrap_or_else(|| caps.get(0).map_or("", |m| m.as_str()).to_string())
        })
        .into_owned()
}

/// Parses a workflow condition check expression (`ipc-workflow-answers.ts:parseWorkflowCheck`).
pub fn parse_workflow_check(text: &str) -> Option<WorkflowCheck> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let or_groups = split_check_words(trimmed, "or");
    if or_groups.is_empty() {
        return None;
    }
    let mut check = Vec::with_capacity(or_groups.len());
    for group in or_groups {
        let and_parts = split_check_words(&group, "and");
        if and_parts.is_empty() {
            return None;
        }
        let mut tests = Vec::with_capacity(and_parts.len());
        for part in and_parts {
            tests.push(parse_check_test(&part)?);
        }
        check.push(tests);
    }
    Some(check)
}

/// Evaluates a parsed `WorkflowCheck` against a JSON object/answer map (`workflowCheckPasses`).
pub fn workflow_check_passes(
    check: &WorkflowCheck,
    answer: &BTreeMap<String, serde_json::Value>,
) -> bool {
    check.iter().any(|group| {
        group.iter().all(|test| {
            let value = lookup_answer_path(answer, &test.name);
            check_test_passes(test, value)
        })
    })
}

/// Parses a condition answer string (`yes`/`true` -> `Yes`, `no`/`false` -> `No`).
pub fn condition_answer(text: &str) -> Option<WorkflowBranch> {
    let word = CONDITION_WORD_RE
        .captures(text.trim())?
        .get(1)?
        .as_str()
        .to_ascii_lowercase();
    match word.as_str() {
        "yes" | "true" => Some(WorkflowBranch::Yes),
        "no" | "false" => Some(WorkflowBranch::No),
        _ => None,
    }
}

/// Exact 1:1 port of `github-matching.ts:githubFilterRefuses`.
pub fn github_filter_refuses(node: &WorkflowNode, facts: &GitHubEventFacts) -> bool {
    let event = field_text(node, "event");
    let filters: BTreeSet<String> = list_field(node, "filters").into_iter().collect();
    let labels: Vec<String> = list_field(node, "labels")
        .into_iter()
        .map(|l| l.to_ascii_lowercase())
        .collect();

    if filters.contains("notBot") && (facts.actor_is_bot || facts.author_is_bot) {
        return true;
    }
    if filters.contains("firstTime")
        && !facts
            .association
            .as_deref()
            .is_some_and(|a| FIRST_TIME_ASSOCIATIONS.contains(&a))
    {
        return true;
    }
    if filters.contains("title") {
        let wanted = field_text(node, "titleText").trim().to_ascii_lowercase();
        if !wanted.is_empty() && !facts.item.title.to_ascii_lowercase().contains(&wanted) {
            return true;
        }
    }
    if filters.contains("branch") && PULL_REQUEST_GITHUB_EVENTS.contains(&event) {
        let wanted = field_text(node, "branch").trim();
        if !wanted.is_empty() && facts.base_branch.as_deref() != Some(wanted) {
            return true;
        }
    }
    if !labels.is_empty() {
        if event == "issue.labeled" {
            return !facts
                .label
                .as_deref()
                .is_some_and(|l| labels.contains(&l.to_ascii_lowercase()));
        }
        if !facts
            .labels
            .iter()
            .any(|l| labels.contains(&l.to_ascii_lowercase()))
        {
            return true;
        }
    }
    false
}

fn parse_check_test(text: &str) -> Option<WorkflowCheckTest> {
    if let Some(caps) = CHECK_BARE_RE.captures(text) {
        let name = caps.get(2)?.as_str().to_string();
        let operator = if caps.get(1).is_some() {
            WorkflowCheckOperator::Unset
        } else {
            WorkflowCheckOperator::Set
        };
        return Some(WorkflowCheckTest {
            name,
            operator,
            value: String::new(),
        });
    }
    if let Some(caps) = CHECK_CMP_RE.captures(text) {
        let name = caps.get(1)?.as_str().to_string();
        let op_raw = caps.get(2)?.as_str();
        let operator = match op_raw {
            "=" | "==" => WorkflowCheckOperator::Eq,
            "!=" => WorkflowCheckOperator::Ne,
            ">" => WorkflowCheckOperator::Gt,
            ">=" => WorkflowCheckOperator::Ge,
            "<" => WorkflowCheckOperator::Lt,
            "<=" => WorkflowCheckOperator::Le,
            _ => return None,
        };
        let value = unquote_check_value(caps.get(3)?.as_str());
        return Some(WorkflowCheckTest {
            name,
            operator,
            value,
        });
    }
    if let Some(caps) = CHECK_WORDED_RE.captures(text) {
        let name = caps.get(1)?.as_str().to_string();
        let word = caps.get(2)?.as_str().to_ascii_lowercase();
        let operator = match word.as_str() {
            "contains" => WorkflowCheckOperator::Contains,
            "is" => WorkflowCheckOperator::Eq,
            "is not" => WorkflowCheckOperator::Ne,
            _ => return None,
        };
        let value = unquote_check_value(caps.get(3)?.as_str());
        return Some(WorkflowCheckTest {
            name,
            operator,
            value,
        });
    }
    None
}

fn unquote_check_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn split_check_words(text: &str, keyword: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let kw_len = keyword.len();
    let mut parts = Vec::new();
    let mut quote: Option<u8> = None;
    let mut start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if (b == b'"' || b == b'\'')
            && (i == 0 || matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b'=' | b'<' | b'>' | b'!'))
        {
            quote = Some(b);
            i += 1;
            continue;
        }
        if b.is_ascii_whitespace() {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j + kw_len < bytes.len()
                && text[j..j + kw_len].eq_ignore_ascii_case(keyword)
                && bytes[j + kw_len].is_ascii_whitespace()
            {
                let mut k = j + kw_len;
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                let piece = text[start..i].trim();
                if piece.is_empty() {
                    return Vec::new();
                }
                parts.push(piece.to_string());
                start = k;
                i = k;
                continue;
            }
        }
        i += 1;
    }

    let tail = text[start..].trim();
    if tail.is_empty() {
        return Vec::new();
    }
    parts.push(tail.to_string());
    parts
}

fn lookup_answer_path<'a>(
    answer: &'a BTreeMap<String, serde_json::Value>,
    path: &str,
) -> Option<&'a serde_json::Value> {
    if let Some(direct) = answer.get(path) {
        return Some(direct);
    }
    let mut segments = path.split('.');
    let first = segments.next()?;
    let mut current = answer.get(first)?;
    for seg in segments {
        current = current.as_object()?.get(seg)?;
    }
    Some(current)
}

fn check_test_passes(test: &WorkflowCheckTest, value: Option<&serde_json::Value>) -> bool {
    if test.operator == WorkflowCheckOperator::Set {
        return is_json_value_set(value);
    }
    if test.operator == WorkflowCheckOperator::Unset {
        return !is_json_value_set(value);
    }
    let Some(val) = value else {
        return false;
    };
    let wanted = test.value.trim();
    match val {
        serde_json::Value::Array(items) => {
            let lowered: Vec<String> = items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_ascii_lowercase))
                .collect();
            let w = wanted.to_ascii_lowercase();
            match test.operator {
                WorkflowCheckOperator::Contains => lowered.iter().any(|s| s.contains(&w)),
                WorkflowCheckOperator::Eq => lowered.iter().any(|s| s == &w),
                WorkflowCheckOperator::Ne => !lowered.iter().any(|s| s == &w),
                op => wanted
                    .parse::<f64>()
                    .ok()
                    .is_some_and(|n| compare_numbers(op, items.len() as f64, n)),
            }
        }
        serde_json::Value::Number(num) => {
            let Some(left) = num.as_f64() else {
                return false;
            };
            if test.operator == WorkflowCheckOperator::Contains {
                return num.to_string().contains(wanted);
            }
            wanted
                .parse::<f64>()
                .ok()
                .is_some_and(|right| compare_numbers(test.operator, left, right))
        }
        serde_json::Value::Bool(flag) => {
            let w = wanted.to_ascii_lowercase();
            let target = matches!(w.as_str(), "true" | "yes");
            match test.operator {
                WorkflowCheckOperator::Eq => *flag == target,
                WorkflowCheckOperator::Ne => *flag != target,
                _ => false,
            }
        }
        serde_json::Value::String(s) => {
            let left = s.trim().to_ascii_lowercase();
            let w = wanted.to_ascii_lowercase();
            match test.operator {
                WorkflowCheckOperator::Contains => left.contains(&w),
                WorkflowCheckOperator::Eq => left == w,
                WorkflowCheckOperator::Ne => left != w,
                op => match (s.trim().parse::<f64>(), wanted.parse::<f64>()) {
                    (Ok(l), Ok(r)) => compare_numbers(op, l, r),
                    _ => false,
                },
            }
        }
        serde_json::Value::Null | serde_json::Value::Object(_) => false,
    }
}

fn is_json_value_set(value: Option<&serde_json::Value>) -> bool {
    match value {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Number(n)) => n.as_f64().is_some_and(|v| v != 0.0),
        Some(serde_json::Value::Array(arr)) => !arr.is_empty(),
        Some(serde_json::Value::Object(map)) => !map.is_empty(),
        Some(serde_json::Value::String(s)) => !s.trim().is_empty(),
    }
}

fn compare_numbers(op: WorkflowCheckOperator, left: f64, right: f64) -> bool {
    if !left.is_finite() || !right.is_finite() {
        return false;
    }
    match op {
        WorkflowCheckOperator::Eq => (left - right).abs() == 0.0,
        WorkflowCheckOperator::Ne => (left - right).abs() != 0.0,
        WorkflowCheckOperator::Gt => left > right,
        WorkflowCheckOperator::Ge => left >= right,
        WorkflowCheckOperator::Lt => left < right,
        WorkflowCheckOperator::Le => left <= right,
        _ => false,
    }
}

fn evaluate_condition_branch(
    node: &WorkflowNode,
    step: &StoredWorkflowStep,
    run: &StoredWorkflowRun,
    latest: &HashMap<String, StoredWorkflowStep>,
    condition_mocks: &BTreeMap<String, String>,
) -> WorkflowBranch {
    if let Some(mock) = condition_mocks.get(&node.id)
        && let Some(branch) = condition_answer(mock)
    {
        return branch;
    }
    let answer_map = collect_answer_json_map(run, latest);
    let check_expr = field_text(node, "check").trim();
    if !check_expr.is_empty()
        && let Some(parsed) = parse_workflow_check(check_expr)
    {
        return if workflow_check_passes(&parsed, &answer_map) {
            WorkflowBranch::Yes
        } else {
            WorkflowBranch::No
        };
    }
    let rule_expr = field_text(node, "rule").trim();
    if !rule_expr.is_empty() {
        if let Some(parsed) = parse_workflow_check(rule_expr) {
            return if workflow_check_passes(&parsed, &answer_map) {
                WorkflowBranch::Yes
            } else {
                WorkflowBranch::No
            };
        }
        if let Some(direct) = condition_answer(rule_expr) {
            return direct;
        }
    }
    if step.input.trim().is_empty() {
        WorkflowBranch::No
    } else {
        WorkflowBranch::Yes
    }
}

fn collect_answer_json_map(
    run: &StoredWorkflowRun,
    latest: &HashMap<String, StoredWorkflowStep>,
) -> BTreeMap<String, serde_json::Value> {
    let mut map = BTreeMap::new();
    if let Some(ref payload) = run.payload
        && let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(payload)
    {
        for (k, v) in obj {
            map.insert(k, v);
        }
    }
    let mut ordered: Vec<&StoredWorkflowStep> = latest
        .values()
        .filter(|s| s.status == WorkflowStepStatus::Done)
        .collect();
    ordered.sort_by_key(|s| s.sequence);
    for step in ordered {
        if let Some(ref out) = step.output
            && let Ok(serde_json::Value::Object(obj)) =
                serde_json::from_str::<serde_json::Value>(out)
        {
            for (k, v) in obj {
                map.insert(k, v);
            }
        }
    }
    map
}

fn collect_template_values(
    run: &StoredWorkflowRun,
    latest: &HashMap<String, StoredWorkflowStep>,
) -> BTreeMap<String, String> {
    let json_map = collect_answer_json_map(run, latest);
    let mut values = BTreeMap::new();
    for (k, v) in json_map {
        let rendered = match v {
            serde_json::Value::Null => String::new(),
            serde_json::Value::Bool(b) => if b { "yes" } else { "no" }.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s,
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            serde_json::Value::Object(_) => v.to_string(),
        };
        values.insert(k, rendered);
    }
    values
}

fn compose_step_input(
    node: &WorkflowNode,
    workflow_title: &str,
    inputs: &[String],
    template_values: &BTreeMap<String, String>,
) -> String {
    if node.kind == WorkflowNodeKind::Output {
        let message = field_text(node, "message");
        if !message.trim().is_empty() {
            return fill_workflow_template(message, template_values);
        }
    }
    let joined = join_input_text(inputs);
    if node.kind != WorkflowNodeKind::Agent {
        return joined;
    }
    let task = fill_workflow_template(field_text(node, "task"), template_values);
    let header = format!(
        "The workflow \"{workflow_title}\" gives you one step.\n\n{}",
        if task.trim().is_empty() {
            "Do this step of the workflow with the input below."
        } else {
            task.trim()
        }
    );
    if joined.is_empty() {
        header
    } else {
        format!("{header}\n\n--- input ---\n{joined}")
    }
}

fn join_input_text(inputs: &[String]) -> String {
    let parts: Vec<&str> = inputs
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let joined = if parts.len() <= 1 {
        parts.first().copied().unwrap_or("").to_string()
    } else {
        parts
            .iter()
            .enumerate()
            .map(|(idx, part)| format!("Input {}:\n\n{part}", idx + 1))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    if joined.len() > INPUT_LIMITS.workflow_step_input {
        joined.chars().take(INPUT_LIMITS.workflow_step_input).collect()
    } else {
        joined
    }
}

fn model_step_prompt(system: &str, prompt: &str, input: &str) -> String {
    let mut parts = Vec::new();
    if !system.trim().is_empty() {
        parts.push(system.trim().to_string());
    }
    if !prompt.trim().is_empty() {
        parts.push(prompt.trim().to_string());
    }
    if !input.trim().is_empty() {
        parts.push(format!("--- input ---\n{}", input.trim()));
    }
    parts.join("\n\n")
}

fn trigger_step_output(node: &WorkflowNode, run: &StoredWorkflowRun, now_ms: i64) -> String {
    if let Some(ref payload) = run.payload {
        return payload.clone();
    }
    if field_text(node, "trigger") == "schedule" {
        let dt = Utc
            .timestamp_millis_opt(now_ms)
            .single()
            .unwrap_or_else(Utc::now);
        return serde_json::json!({
            "run_time": dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "local_date": dt.format("%Y-%m-%d").to_string(),
            "local_time": dt.format("%H:%M").to_string(),
            "weekday": dt.weekday().to_string(),
        })
        .to_string();
    }
    String::new()
}

fn collect_step_sources(
    graph: &WorkflowGraph,
    node_id: &str,
    latest: &HashMap<String, StoredWorkflowStep>,
) -> Vec<WorkflowStepSource> {
    graph
        .edges
        .iter()
        .filter(|e| e.to == node_id)
        .filter_map(|e| {
            let src = latest.get(&e.from)?;
            edge_active(e, src).then(|| WorkflowStepSource {
                node_id: e.from.clone(),
                port_type: e.port_type,
                output: src.output.clone(),
            })
        })
        .collect()
}

fn trigger_run_source(node: &WorkflowNode) -> WorkflowRunSource {
    match field_text(node, "trigger") {
        "schedule" => WorkflowRunSource::Schedule,
        "webhook" => WorkflowRunSource::Webhook,
        "github" => WorkflowRunSource::GitHub,
        "discord" => WorkflowRunSource::Discord,
        "telegram" => WorkflowRunSource::Telegram,
        "slack" => WorkflowRunSource::Slack,
        _ => WorkflowRunSource::Manual,
    }
}

fn field_text<'a>(node: &'a WorkflowNode, key: &str) -> &'a str {
    match node.fields.get(key) {
        Some(WorkflowFieldValue::Text(s)) => s.as_str(),
        _ => "",
    }
}

fn list_field(node: &WorkflowNode, key: &str) -> Vec<String> {
    field_text(node, key)
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn edge_active(edge: &WorkflowEdge, source: &StoredWorkflowStep) -> bool {
    if source.status != WorkflowStepStatus::Done {
        return false;
    }
    source.kind != WorkflowNodeKind::Condition || edge.branch == source.branch
}

fn is_waiting(step: Option<&StoredWorkflowStep>) -> bool {
    step.is_none_or(StoredWorkflowStep::is_open)
}

fn edges_by<'a>(
    edges: &[&'a WorkflowEdge],
    outgoing: bool,
) -> HashMap<&'a str, Vec<&'a WorkflowEdge>> {
    let mut map: HashMap<&'a str, Vec<&'a WorkflowEdge>> = HashMap::new();
    for &edge in edges {
        let key = if outgoing {
            edge.from.as_str()
        } else {
            edge.to.as_str()
        };
        map.entry(key).or_default().push(edge);
    }
    map
}

fn reach<'a>(
    origin: &'a str,
    adj: &HashMap<&'a str, Vec<&'a WorkflowEdge>>,
) -> HashSet<&'a str> {
    let mut seen = HashSet::from([origin]);
    let mut stack = vec![origin];
    while let Some(id) = stack.pop() {
        if let Some(list) = adj.get(id) {
            for edge in list {
                if seen.insert(edge.to.as_str()) {
                    stack.push(edge.to.as_str());
                }
            }
        }
    }
    seen
}

fn wire_run_status(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Done => "completed",
        WorkflowRunStatus::Approval => "awaiting_approval",
        WorkflowRunStatus::Stopped | WorkflowRunStatus::Stopping => "stopped",
        WorkflowRunStatus::Failed => "failed",
        WorkflowRunStatus::Running => "running",
    }
}

fn wire_step_status(status: WorkflowStepStatus) -> &'static str {
    match status {
        WorkflowStepStatus::Running => "running",
        WorkflowStepStatus::Approval => "approval",
        WorkflowStepStatus::Done => "done",
        WorkflowStepStatus::Failed => "failed",
        WorkflowStepStatus::Stopped => "stopped",
        WorkflowStepStatus::Skipped => "skipped",
    }
}

fn map_graph_problem(problem: WorkflowGraphProblem, edge: &WorkflowEdge) -> OpenBotRuntimeError {
    match problem {
        WorkflowGraphProblem::Annotation => OpenBotRuntimeError::Validation(format!(
            "Workflow edge {} cannot connect to annotation node (text/note/shape)",
            edge.id
        )),
        WorkflowGraphProblem::Port => OpenBotRuntimeError::Validation(format!(
            "Workflow edge {} has incompatible port type '{}'",
            edge.id,
            edge.port_type.as_str()
        )),
        WorkflowGraphProblem::ConditionBranch => OpenBotRuntimeError::Validation(format!(
            "Workflow edge {} from condition node requires condition-branch ('yes' or 'no')",
            edge.id
        )),
        WorkflowGraphProblem::Branch => OpenBotRuntimeError::Validation(format!(
            "Workflow edge {} sets branch on non-condition source node",
            edge.id
        )),
        WorkflowGraphProblem::Duplicate => OpenBotRuntimeError::Validation(format!(
            "Workflow edge {} duplicates an existing connection",
            edge.id
        )),
    }
}

fn map_storage_error(err: StorageError) -> OpenBotRuntimeError {
    match err {
        StorageError::Validation(msg) => OpenBotRuntimeError::Validation(msg),
        StorageError::NotFound(msg) => OpenBotRuntimeError::NotFound(msg),
        StorageError::Conflict(msg) => OpenBotRuntimeError::Conflict(msg),
        StorageError::LimitExceeded(msg) => OpenBotRuntimeError::LimitExceeded(msg),
        StorageError::Forbidden(msg) => OpenBotRuntimeError::Forbidden(msg),
        other => OpenBotRuntimeError::Storage(other),
    }
}

fn ms_to_iso(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
