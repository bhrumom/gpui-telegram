//! 12-node visual DAG `WorkflowGraph` & `WorkflowDefinition` (`WorkflowDetail`),
//! `WORKFLOW_PORTS`, `WORKFLOW_NODE_REGISTRY`, `200/400/500/2000/1MiB` validators,
//! SCC top-left entry `workflow_loop_edges` detector, 16 media models, and `.openbot-workflow.json` files.
//! Mirrors `packages/contracts/src/ipc-workflows.ts` and `ipc-workflow-{nodes,format,file,folders,github,discord,slack,telegram}.ts`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize};

use crate::limits::{
    ATTACHMENT_LIMITS, ContractError, INPUT_LIMITS, is_bounded_string, is_identifier,
    non_null_option, utf16_len,
};
use crate::routines::RoutineSchedule;

pub const WORKFLOW_FORMAT: u32 = 3;
pub const WORKFLOW_FILE_FORMAT: &str = "openbot.workflow";
pub const WORKFLOW_FILE_VERSION: u32 = 1;
pub const WORKFLOW_FILE_EXTENSION: &str = "openbot-workflow.json";
pub const WORKFLOW_MAX_COORDINATE: f64 = 1_000_000.0;
pub const WORKFLOW_MAX_SIZE: f64 = 100_000.0;
pub const WORKFLOW_AUTO_VERSION_INTERVAL_MS: u64 = 300_000;
pub const WORKFLOW_MAX_VERSIONS: usize = 50;
pub const WORKFLOW_MEDIA_SIZE_MIN: u32 = 64;
pub const WORKFLOW_MEDIA_SIZE_MAX: u32 = 4_096;
pub const WORKFLOW_MEDIA_CUSTOM_ASPECT: &str = "custom";
pub const WORKFLOW_STEP_NAME_FIELD: &str = "stepName";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkflowFolderLimits {
    pub folders: usize,
    pub name: usize,
    pub placements: usize,
    pub workflows: usize,
}

pub const WORKFLOW_FOLDER_LIMITS: WorkflowFolderLimits = WorkflowFolderLimits {
    folders: 200,
    name: 80,
    placements: 10_000,
    workflows: 1_000,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowNodeKind {
    Text,
    Prompt,
    Asset,
    Llm,
    Agent,
    Image,
    Trigger,
    Condition,
    Approval,
    Output,
    Note,
    Shape,
}

impl WorkflowNodeKind {
    pub const ALL: [Self; 12] = [
        Self::Text,
        Self::Prompt,
        Self::Asset,
        Self::Llm,
        Self::Agent,
        Self::Image,
        Self::Trigger,
        Self::Condition,
        Self::Approval,
        Self::Output,
        Self::Note,
        Self::Shape,
    ];

    pub const ANNOTATIONS: [Self; 3] = [Self::Text, Self::Note, Self::Shape];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Prompt => "prompt",
            Self::Asset => "asset",
            Self::Llm => "llm",
            Self::Agent => "agent",
            Self::Image => "image",
            Self::Trigger => "trigger",
            Self::Condition => "condition",
            Self::Approval => "approval",
            Self::Output => "output",
            Self::Note => "note",
            Self::Shape => "shape",
        }
    }

    pub const fn is_step(self) -> bool {
        !matches!(self, Self::Text | Self::Note | Self::Shape)
    }

    pub const fn ports(self) -> WorkflowNodePorts {
        workflow_ports(self)
    }
}

pub const fn is_workflow_step_kind(kind: WorkflowNodeKind) -> bool {
    kind.is_step()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowPortType {
    Text,
    Image,
    File,
}

impl WorkflowPortType {
    pub const ALL: [Self; 3] = [Self::Text, Self::Image, Self::File];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::File => "file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBranch {
    Yes,
    No,
}

impl WorkflowBranch {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkflowNodePorts {
    pub inputs: &'static [WorkflowPortType],
    pub outputs: &'static [WorkflowPortType],
}

const ALL_PORTS: [WorkflowPortType; 3] = WorkflowPortType::ALL;
const TEXT_PORTS: [WorkflowPortType; 1] = [WorkflowPortType::Text];
const IMAGE_OUT_PORTS: [WorkflowPortType; 2] = [WorkflowPortType::Image, WorkflowPortType::File];
const EMPTY_PORTS: [WorkflowPortType; 0] = [];

pub const fn workflow_ports(kind: WorkflowNodeKind) -> WorkflowNodePorts {
    match kind {
        WorkflowNodeKind::Trigger | WorkflowNodeKind::Prompt => WorkflowNodePorts {
            inputs: &EMPTY_PORTS,
            outputs: &TEXT_PORTS,
        },
        WorkflowNodeKind::Asset => WorkflowNodePorts {
            inputs: &EMPTY_PORTS,
            outputs: &ALL_PORTS,
        },
        WorkflowNodeKind::Llm | WorkflowNodeKind::Agent => WorkflowNodePorts {
            inputs: &ALL_PORTS,
            outputs: &TEXT_PORTS,
        },
        WorkflowNodeKind::Image => WorkflowNodePorts {
            inputs: &ALL_PORTS,
            outputs: &IMAGE_OUT_PORTS,
        },
        WorkflowNodeKind::Condition | WorkflowNodeKind::Approval => WorkflowNodePorts {
            inputs: &TEXT_PORTS,
            outputs: &TEXT_PORTS,
        },
        WorkflowNodeKind::Output => WorkflowNodePorts {
            inputs: &ALL_PORTS,
            outputs: &EMPTY_PORTS,
        },
        WorkflowNodeKind::Text | WorkflowNodeKind::Note | WorkflowNodeKind::Shape => {
            WorkflowNodePorts {
                inputs: &EMPTY_PORTS,
                outputs: &EMPTY_PORTS,
            }
        }
    }
}

pub const WORKFLOW_PORTS: [(WorkflowNodeKind, WorkflowNodePorts); 12] = [
    (WorkflowNodeKind::Trigger, workflow_ports(WorkflowNodeKind::Trigger)),
    (WorkflowNodeKind::Prompt, workflow_ports(WorkflowNodeKind::Prompt)),
    (WorkflowNodeKind::Asset, workflow_ports(WorkflowNodeKind::Asset)),
    (WorkflowNodeKind::Llm, workflow_ports(WorkflowNodeKind::Llm)),
    (WorkflowNodeKind::Agent, workflow_ports(WorkflowNodeKind::Agent)),
    (WorkflowNodeKind::Image, workflow_ports(WorkflowNodeKind::Image)),
    (WorkflowNodeKind::Condition, workflow_ports(WorkflowNodeKind::Condition)),
    (WorkflowNodeKind::Approval, workflow_ports(WorkflowNodeKind::Approval)),
    (WorkflowNodeKind::Output, workflow_ports(WorkflowNodeKind::Output)),
    (WorkflowNodeKind::Text, workflow_ports(WorkflowNodeKind::Text)),
    (WorkflowNodeKind::Note, workflow_ports(WorkflowNodeKind::Note)),
    (WorkflowNodeKind::Shape, workflow_ports(WorkflowNodeKind::Shape)),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowTriggerType {
    Manual,
    Schedule,
    Webhook,
    Message,
    #[serde(rename = "github")]
    GitHub,
    Discord,
    Telegram,
    Slack,
}

pub const WORKFLOW_TRIGGER_TYPES: [&str; 8] = [
    "manual", "schedule", "webhook", "message", "github", "discord", "telegram", "slack",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowOutputTarget {
    Notify,
    Channel,
    File,
    #[serde(rename = "github")]
    GitHub,
    Discord,
    Telegram,
    Slack,
    ChatChannel,
}

pub const WORKFLOW_OUTPUT_TARGETS: [&str; 8] = [
    "notify",
    "channel",
    "file",
    "github",
    "discord",
    "telegram",
    "slack",
    "chat-channel",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkflowFieldValue {
    Bool(bool),
    Number(f64),
    Text(String),
}

pub type WorkflowFields = BTreeMap<String, WorkflowFieldValue>;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorkflowPoint {
    pub x: f64,
    pub y: f64,
}

impl WorkflowPoint {
    pub fn is_valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.x.abs() <= WORKFLOW_MAX_COORDINATE
            && self.y.abs() <= WORKFLOW_MAX_COORDINATE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorkflowNodeSize {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub font: Option<f64>,
}

impl WorkflowNodeSize {
    pub fn is_valid(self) -> bool {
        let dim_ok = |v: Option<f64>| v.is_none_or(|n| n.is_finite() && n > 0.0 && n <= WORKFLOW_MAX_SIZE);
        dim_ok(self.width) && dim_ok(self.height) && dim_ok(self.font)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowNode {
    pub id: String,
    pub kind: WorkflowNodeKind,
    pub at: WorkflowPoint,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub size: Option<WorkflowNodeSize>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub locked: Option<bool>,
    pub fields: WorkflowFields,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub schedule: Option<RoutineSchedule>,
}

impl WorkflowNode {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.id)
            && self.at.is_valid()
            && self.size.is_none_or(WorkflowNodeSize::is_valid)
            && self.fields.len() <= INPUT_LIMITS.workflow_node_fields
            && self.fields.iter().all(|(k, v)| {
                !k.is_empty()
                    && is_bounded_string(k, 64)
                    && match v {
                        WorkflowFieldValue::Bool(_) => true,
                        WorkflowFieldValue::Number(n) => n.is_finite(),
                        WorkflowFieldValue::Text(s) => {
                            is_bounded_string(s, INPUT_LIMITS.workflow_node_text)
                        }
                    }
            })
            && self
                .schedule
                .as_ref()
                .is_none_or(|s| self.kind == WorkflowNodeKind::Trigger && s.is_valid())
    }

    pub fn agent_id(&self) -> Option<&str> {
        if self.kind != WorkflowNodeKind::Agent {
            return None;
        }
        match self.fields.get("agentId")? {
            WorkflowFieldValue::Text(s) if !s.is_empty() => Some(s.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub port_type: WorkflowPortType,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub branch: Option<WorkflowBranch>,
}

impl WorkflowEdge {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.id)
            && is_identifier(&self.from)
            && is_identifier(&self.to)
            && self.from != self.to
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowStroke {
    pub id: String,
    pub brush: String,
    pub color: String,
    pub width: f64,
    pub points: Vec<WorkflowPoint>,
}

impl WorkflowStroke {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.id)
            && is_identifier(&self.brush)
            && is_identifier(&self.color)
            && self.width.is_finite()
            && self.width > 0.0
            && self.width <= WORKFLOW_MAX_SIZE
            && self.points.len() <= INPUT_LIMITS.workflow_stroke_points
            && self.points.iter().copied().all(WorkflowPoint::is_valid)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WorkflowGraph {
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    pub strokes: Vec<WorkflowStroke>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowGraphProblem {
    Annotation,
    Port,
    Branch,
    ConditionBranch,
    Duplicate,
}

pub fn workflow_edge_problem(
    from: &WorkflowNode,
    to: &WorkflowNode,
    edge: &WorkflowEdge,
) -> Option<WorkflowGraphProblem> {
    if !from.kind.is_step() || !to.kind.is_step() {
        return Some(WorkflowGraphProblem::Annotation);
    }
    if !from.kind.ports().outputs.contains(&edge.port_type)
        || !to.kind.ports().inputs.contains(&edge.port_type)
    {
        return Some(WorkflowGraphProblem::Port);
    }
    if from.kind == WorkflowNodeKind::Condition {
        if edge.branch.is_none() {
            return Some(WorkflowGraphProblem::ConditionBranch);
        }
        return None;
    }
    edge.branch.map(|_| WorkflowGraphProblem::Branch)
}

pub fn workflow_graph_problem_at(graph: &WorkflowGraph) -> Option<(WorkflowGraphProblem, WorkflowEdge)> {
    let nodes: HashMap<&str, &WorkflowNode> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut seen = HashSet::new();
    for edge in &graph.edges {
        let (Some(from), Some(to)) = (nodes.get(edge.from.as_str()), nodes.get(edge.to.as_str())) else {
            return Some((WorkflowGraphProblem::Port, edge.clone()));
        };
        if let Some(problem) = workflow_edge_problem(from, to, edge) {
            return Some((problem, edge.clone()));
        }
        let branch_key = edge.branch.map_or("", WorkflowBranch::as_str);
        let key = format!("{}\0{}\0{}\0{branch_key}", edge.from, edge.to, edge.port_type.as_str());
        if !seen.insert(key) {
            return Some((WorkflowGraphProblem::Duplicate, edge.clone()));
        }
    }
    None
}

pub fn workflow_graph_problem(graph: &WorkflowGraph) -> Option<WorkflowGraphProblem> {
    workflow_graph_problem_at(graph).map(|(p, _)| p)
}

impl WorkflowGraph {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.nodes.len() > INPUT_LIMITS.workflow_nodes {
            return Err(ContractError::InvalidValue("Too many workflow nodes (>200).".to_string()));
        }
        if self.edges.len() > INPUT_LIMITS.workflow_edges {
            return Err(ContractError::InvalidValue("Too many workflow edges (>400).".to_string()));
        }
        if self.strokes.len() > INPUT_LIMITS.workflow_strokes {
            return Err(ContractError::InvalidValue("Too many workflow strokes (>500).".to_string()));
        }
        let encoded = serde_json::to_string(self).map_err(|e| ContractError::Json(e.to_string()))?;
        if encoded.len() > INPUT_LIMITS.workflow_graph_bytes {
            return Err(ContractError::InvalidValue("Workflow graph exceeds 1 MiB JSON limit.".to_string()));
        }
        let mut node_ids = HashSet::new();
        for n in &self.nodes {
            if !n.is_valid() || !node_ids.insert(n.id.as_str()) {
                return Err(ContractError::InvalidValue(format!("Invalid or duplicate node: {}", n.id)));
            }
        }
        let mut edge_ids = HashSet::new();
        for e in &self.edges {
            if !e.is_valid() || !edge_ids.insert(e.id.as_str()) {
                return Err(ContractError::InvalidValue(format!("Invalid or duplicate edge: {}", e.id)));
            }
        }
        let mut stroke_ids = HashSet::new();
        for s in &self.strokes {
            if !s.is_valid() || !stroke_ids.insert(s.id.as_str()) {
                return Err(ContractError::InvalidValue(format!("Invalid or duplicate stroke: {}", s.id)));
            }
        }
        if let Some((prob, edge)) = workflow_graph_problem_at(self) {
            return Err(ContractError::InvalidValue(format!(
                "Workflow edge {} failed validation: {prob:?}",
                edge.id
            )));
        }
        Ok(())
    }

    pub fn graph_agent_ids(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter_map(|n| n.agent_id().map(str::to_string))
            .collect()
    }
}

fn reached_by<'a>(
    origin: &'a str,
    adj: &HashMap<&'a str, Vec<&'a WorkflowEdge>>,
    forward: bool,
) -> HashSet<&'a str> {
    let mut seen = HashSet::from([origin]);
    let mut stack = vec![origin];
    while let Some(id) = stack.pop() {
        if let Some(list) = adj.get(id) {
            for edge in list {
                let target = if forward { edge.to.as_str() } else { edge.from.as_str() };
                if seen.insert(target) {
                    stack.push(target);
                }
            }
        }
    }
    seen
}

/// Exact SCC + top-left entry back-edge loop detector (`ipc-workflows.ts:workflowLoopEdges`, `L464-L565`).
pub fn workflow_loop_edges<'a>(
    nodes: &'a [WorkflowNode],
    edges: &'a [WorkflowEdge],
) -> BTreeSet<String> {
    let mut loops: BTreeSet<String> = edges
        .iter()
        .filter(|e| e.from == e.to)
        .map(|e| e.id.clone())
        .collect();

    let mut out_adj: HashMap<&str, Vec<&WorkflowEdge>> = HashMap::new();
    let mut in_adj: HashMap<&str, Vec<&WorkflowEdge>> = HashMap::new();
    let mut entered: HashSet<&str> = HashSet::new();
    for e in edges {
        out_adj.entry(e.from.as_str()).or_default().push(e);
        in_adj.entry(e.to.as_str()).or_default().push(e);
        entered.insert(e.to.as_str());
    }

    let starts: Vec<&str> = nodes
        .iter()
        .filter(|n| !entered.contains(n.id.as_str()))
        .chain(nodes.iter().filter(|n| entered.contains(n.id.as_str())))
        .map(|n| n.id.as_str())
        .collect();

    let mut order: HashMap<&str, usize> = HashMap::new();
    let mut open: HashSet<&str> = HashSet::new();
    let mut cyclic = false;
    for start in starts {
        if order.contains_key(start) {
            continue;
        }
        order.insert(start, order.len());
        open.insert(start);
        let mut stack: Vec<(&str, usize)> = vec![(start, 0)];
        while let Some((top_id, idx)) = stack.last_mut() {
            let cur_id = *top_id;
            let next_edge = out_adj.get(cur_id).and_then(|v| v.get(*idx)).copied();
            let Some(edge) = next_edge else {
                stack.pop();
                open.remove(cur_id);
                continue;
            };
            *idx += 1;
            let to = edge.to.as_str();
            if open.contains(to) {
                cyclic = true;
            } else if !order.contains_key(to) {
                let next_ord = order.len();
                order.insert(to, next_ord);
                open.insert(to);
                stack.push((to, 0));
            }
        }
    }
    if !cyclic {
        return loops;
    }

    let place: HashMap<&'a str, WorkflowPoint> =
        nodes.iter().map(|n| (n.id.as_str(), n.at)).collect();
    let first_reached = |ids: &[&'a str]| -> &'a str {
        ids.iter()
            .copied()
            .min_by_key(|id| order.get(id).copied().unwrap_or(usize::MAX))
            .expect("non-empty loop group")
    };
    let top_left = |ids: &[&'a str]| -> Vec<&'a str> {
        let min_x = ids
            .iter()
            .filter_map(|id| place.get(id).map(|p| p.x))
            .fold(f64::INFINITY, f64::min);
        let left: Vec<&'a str> = ids
            .iter()
            .copied()
            .filter(|id| place.get(id).is_some_and(|p| (p.x - min_x).abs() == 0.0))
            .collect();
        let min_y = left
            .iter()
            .filter_map(|id| place.get(id).map(|p| p.y))
            .fold(f64::INFINITY, f64::min);
        left.into_iter()
            .filter(|id| place.get(id).is_some_and(|p| (p.y - min_y).abs() == 0.0))
            .collect()
    };

    let initial_group: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let mut pending: Vec<Vec<&str>> = vec![initial_group];
    while let Some(group) = pending.pop() {
        let group_set: HashSet<&str> = group.iter().copied().collect();
        let inside: Vec<&WorkflowEdge> = edges
            .iter()
            .filter(|e| {
                group_set.contains(e.from.as_str())
                    && group_set.contains(e.to.as_str())
                    && !loops.contains(e.id.as_str())
            })
            .collect();
        let mut inside_out: HashMap<&str, Vec<&WorkflowEdge>> = HashMap::new();
        let mut inside_in: HashMap<&str, Vec<&WorkflowEdge>> = HashMap::new();
        for &e in &inside {
            inside_out.entry(e.from.as_str()).or_default().push(e);
            inside_in.entry(e.to.as_str()).or_default().push(e);
        }
        let mut placed: HashSet<&str> = HashSet::new();
        for &id in &group {
            if placed.contains(id) {
                continue;
            }
            let behind = reached_by(id, &inside_in, false);
            let forward = reached_by(id, &inside_out, true);
            let scc: Vec<&str> = group
                .iter()
                .copied()
                .filter(|each| forward.contains(each) && behind.contains(each))
                .collect();
            for &each in &scc {
                placed.insert(each);
            }
            if scc.len() > 1 {
                let scc_set: HashSet<&str> = scc.iter().copied().collect();
                let entries: Vec<&str> = scc
                    .iter()
                    .copied()
                    .filter(|nid| {
                        in_adj
                            .get(nid)
                            .is_some_and(|list| list.iter().any(|e| !scc_set.contains(e.from.as_str())))
                    })
                    .collect();
                let start = if !entries.is_empty() {
                    let tl = top_left(&entries);
                    first_reached(if tl.is_empty() { &entries } else { &tl })
                } else {
                    first_reached(&scc)
                };
                for &e in &inside {
                    if e.to == start && scc_set.contains(e.from.as_str()) {
                        loops.insert(e.id.clone());
                    }
                }
                pending.push(scc);
            }
        }
    }
    loops
}

pub const WORKFLOW_GITHUB_EVENTS: [&str; 12] = [
    "issue.opened",
    "issue.closed",
    "issue.labeled",
    "pr.opened",
    "pr.updated",
    "pr.reviewRequested",
    "pr.reviewSubmitted",
    "pr.checksFailed",
    "pr.merged",
    "comment",
    "mention",
    "assigned",
];

pub const WORKFLOW_GITHUB_FILTERS: [&str; 4] = ["notBot", "firstTime", "title", "branch"];

pub const WORKFLOW_GITHUB_ACTIONS: [&str; 10] = [
    "comment",
    "label.add",
    "label.remove",
    "assign",
    "review.request",
    "review.approve",
    "review.changes",
    "close",
    "reopen",
    "issue.create",
];

pub const WORKFLOW_GITHUB_CLOSE_REASONS: [&str; 2] = ["completed", "notPlanned"];
pub const WORKFLOW_DISCORD_EVENTS: [&str; 1] = ["mention"];
pub const WORKFLOW_DISCORD_ACTIONS: [&str; 3] = ["reply", "post", "react"];
pub const WORKFLOW_DISCORD_EMOJI: [&str; 4] = ["✅", "👀", "🎉", "❌"];
pub const WORKFLOW_SLACK_EVENTS: [&str; 1] = ["mention"];
pub const WORKFLOW_SLACK_ACTIONS: [&str; 3] = ["reply", "post", "react"];
pub const WORKFLOW_SLACK_EMOJI: [&str; 4] = ["✅", "👀", "🎉", "❌"];
pub const WORKFLOW_TELEGRAM_EVENTS: [&str; 1] = ["mention"];
pub const WORKFLOW_TELEGRAM_ACTIONS: [&str; 3] = ["reply", "post", "react"];
pub const WORKFLOW_TELEGRAM_EMOJI: [&str; 4] = ["👍", "👀", "🎉", "👌"];
pub const WORKFLOW_VIDEO_VOICES: [&str; 4] = ["ara", "eve", "leo", "rex"];
pub const WORKFLOW_MEDIA_KINDS: [&str; 2] = ["image", "video"];
pub const WORKFLOW_MEDIA_ASPECTS: [&str; 8] =
    ["1:1", "4:3", "16:9", "9:16", "4:5", "5:4", "21:9", "3:4"];
pub const WORKFLOW_IMAGE_ASPECT_VALUES: [&str; 9] =
    ["1:1", "4:3", "16:9", "9:16", "4:5", "5:4", "21:9", "3:4", "custom"];
pub const WORKFLOW_REASONING_EFFORTS: [&str; 8] =
    ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];
pub const WORKFLOW_ALL_ACTIONS: [&str; 13] = [
    "comment",
    "label.add",
    "label.remove",
    "assign",
    "review.request",
    "review.approve",
    "review.changes",
    "close",
    "reopen",
    "issue.create",
    "reply",
    "post",
    "react",
];
pub const WORKFLOW_ALL_EMOJI: [&str; 6] = ["✅", "👀", "🎉", "❌", "👍", "👌"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowIdKind {
    Agent,
    Channel,
    Connection,
    Place,
    File,
    Account,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkflowFieldSpec {
    pub name: &'static str,
    pub runtime: bool,
    pub template: bool,
    pub list: bool,
    pub step: bool,
    pub id_kind: Option<WorkflowIdKind>,
    pub values: &'static [&'static str],
    pub admin_values: &'static [&'static str],
}

const fn f_runtime(name: &'static str) -> WorkflowFieldSpec {
    WorkflowFieldSpec { name, runtime: true, template: false, list: false, step: false, id_kind: None, values: &[], admin_values: &[] }
}
const fn f_values(name: &'static str, values: &'static [&'static str]) -> WorkflowFieldSpec {
    WorkflowFieldSpec { name, runtime: true, template: false, list: false, step: false, id_kind: None, values, admin_values: &[] }
}
const fn f_template(name: &'static str) -> WorkflowFieldSpec {
    WorkflowFieldSpec { name, runtime: true, template: true, list: false, step: false, id_kind: None, values: &[], admin_values: &[] }
}
const fn f_id(name: &'static str, kind: WorkflowIdKind) -> WorkflowFieldSpec {
    WorkflowFieldSpec { name, runtime: true, template: false, list: false, step: false, id_kind: Some(kind), values: &[], admin_values: &[] }
}
const fn f_step(name: &'static str) -> WorkflowFieldSpec {
    WorkflowFieldSpec { name, runtime: true, template: false, list: false, step: true, id_kind: None, values: &[], admin_values: &[] }
}

const TRIGGER_SPECS: [WorkflowFieldSpec; 13] = [
    WorkflowFieldSpec {
        name: "trigger",
        runtime: true,
        template: false,
        list: false,
        step: false,
        id_kind: None,
        values: &WORKFLOW_TRIGGER_TYPES,
        admin_values: &["github", "discord", "telegram", "slack"],
    },
    f_runtime("timezone"),
    f_runtime("repositories"),
    f_values("event", &WORKFLOW_GITHUB_EVENTS),
    WorkflowFieldSpec { name: "filters", runtime: true, template: false, list: true, step: false, id_kind: None, values: &WORKFLOW_GITHUB_FILTERS, admin_values: &[] },
    f_runtime("labels"),
    f_runtime("titleText"),
    f_runtime("branch"),
    f_id("connectionId", WorkflowIdKind::Connection),
    f_id("placeId", WorkflowIdKind::Place),
    f_runtime("roles"),
    f_runtime("text"),
    f_runtime("answer"),
];

const AGENT_SPECS: [WorkflowFieldSpec; 3] = [
    f_id("agentId", WorkflowIdKind::Agent),
    f_template("task"),
    f_runtime("answer"),
];

const LLM_SPECS: [WorkflowFieldSpec; 6] = [
    f_runtime("provider"),
    f_runtime("model"),
    f_values("effort", &WORKFLOW_REASONING_EFFORTS),
    f_template("prompt"),
    f_template("system"),
    f_runtime("answer"),
];

const CONDITION_SPECS: [WorkflowFieldSpec; 5] = [
    f_runtime("rule"),
    f_runtime("provider"),
    f_runtime("model"),
    f_values("effort", &WORKFLOW_REASONING_EFFORTS),
    f_runtime("check"),
];

const APPROVAL_SPECS: [WorkflowFieldSpec; 1] = [f_template("question")];

const OUTPUT_SPECS: [WorkflowFieldSpec; 14] = [
    WorkflowFieldSpec {
        name: "target",
        runtime: true,
        template: false,
        list: false,
        step: false,
        id_kind: None,
        values: &WORKFLOW_OUTPUT_TARGETS,
        admin_values: &["github", "discord", "telegram", "slack", "chat-channel"],
    },
    f_id("connectionId", WorkflowIdKind::Connection),
    f_id("placeId", WorkflowIdKind::Place),
    f_id("channelId", WorkflowIdKind::Channel),
    f_runtime("repository"),
    f_values("item", &["event", "input"]),
    f_values("action", &WORKFLOW_ALL_ACTIONS),
    f_template("labels"),
    f_values("labelsFrom", &["agent", "list"]),
    f_template("person"),
    f_values("reason", &WORKFLOW_GITHUB_CLOSE_REASONS),
    f_template("title"),
    f_template("message"),
    f_values("emoji", &WORKFLOW_ALL_EMOJI),
];

const TEXT_SPECS: [WorkflowFieldSpec; 1] = [f_runtime("text")];
const ASSET_SPECS: [WorkflowFieldSpec; 1] = [f_id("fileId", WorkflowIdKind::File)];

const IMAGE_SPECS: [WorkflowFieldSpec; 14] = [
    f_values("media", &WORKFLOW_MEDIA_KINDS),
    f_runtime("model"),
    f_id("account", WorkflowIdKind::Account),
    f_runtime("prompt"),
    f_values("aspect", &WORKFLOW_IMAGE_ASPECT_VALUES),
    f_runtime("width"),
    f_runtime("height"),
    f_values("background", &["solid", "transparent"]),
    f_runtime("duration"),
    f_runtime("resolution"),
    f_step("firstFrame"),
    f_step("lastFrame"),
    f_values("voice", &WORKFLOW_VIDEO_VOICES),
    f_runtime("options"),
];

pub const fn workflow_node_registry(kind: WorkflowNodeKind) -> &'static [WorkflowFieldSpec] {
    match kind {
        WorkflowNodeKind::Trigger => &TRIGGER_SPECS,
        WorkflowNodeKind::Agent => &AGENT_SPECS,
        WorkflowNodeKind::Llm => &LLM_SPECS,
        WorkflowNodeKind::Condition => &CONDITION_SPECS,
        WorkflowNodeKind::Approval => &APPROVAL_SPECS,
        WorkflowNodeKind::Output => &OUTPUT_SPECS,
        WorkflowNodeKind::Prompt | WorkflowNodeKind::Text | WorkflowNodeKind::Note | WorkflowNodeKind::Shape => &TEXT_SPECS,
        WorkflowNodeKind::Asset => &ASSET_SPECS,
        WorkflowNodeKind::Image => &IMAGE_SPECS,
    }
}

pub const WORKFLOW_NODE_REGISTRY: [(WorkflowNodeKind, &'static [WorkflowFieldSpec]); 12] = [
    (WorkflowNodeKind::Text, workflow_node_registry(WorkflowNodeKind::Text)),
    (WorkflowNodeKind::Prompt, workflow_node_registry(WorkflowNodeKind::Prompt)),
    (WorkflowNodeKind::Asset, workflow_node_registry(WorkflowNodeKind::Asset)),
    (WorkflowNodeKind::Llm, workflow_node_registry(WorkflowNodeKind::Llm)),
    (WorkflowNodeKind::Agent, workflow_node_registry(WorkflowNodeKind::Agent)),
    (WorkflowNodeKind::Image, workflow_node_registry(WorkflowNodeKind::Image)),
    (WorkflowNodeKind::Trigger, workflow_node_registry(WorkflowNodeKind::Trigger)),
    (WorkflowNodeKind::Condition, workflow_node_registry(WorkflowNodeKind::Condition)),
    (WorkflowNodeKind::Approval, workflow_node_registry(WorkflowNodeKind::Approval)),
    (WorkflowNodeKind::Output, workflow_node_registry(WorkflowNodeKind::Output)),
    (WorkflowNodeKind::Note, workflow_node_registry(WorkflowNodeKind::Note)),
    (WorkflowNodeKind::Shape, workflow_node_registry(WorkflowNodeKind::Shape)),
];

pub fn workflow_id_fields(kind: WorkflowNodeKind) -> Vec<(&'static str, WorkflowIdKind)> {
    workflow_node_registry(kind)
        .iter()
        .filter_map(|spec| spec.id_kind.map(|k| (spec.name, k)))
        .collect()
}

pub fn workflow_fields_without_ids(kind: WorkflowNodeKind, fields: &WorkflowFields) -> WorkflowFields {
    let mut kept = fields.clone();
    for (name, id_kind) in workflow_id_fields(kind) {
        let follows_trigger = matches!(
            id_kind,
            WorkflowIdKind::Connection | WorkflowIdKind::Place
        ) && matches!(kept.get(name), Some(WorkflowFieldValue::Text(s)) if s == "trigger");
        if !follows_trigger
            && matches!(kept.get(name), Some(WorkflowFieldValue::Text(s)) if !s.is_empty())
        {
            kept.insert(name.to_string(), WorkflowFieldValue::Text(String::new()));
        }
    }
    kept
}

pub fn workflow_step_fields(kind: WorkflowNodeKind) -> Vec<&'static str> {
    workflow_node_registry(kind)
        .iter()
        .filter(|s| s.step)
        .map(|s| s.name)
        .collect()
}

pub fn known_workflow_field_value(spec: &WorkflowFieldSpec, value: &str) -> bool {
    if spec.values.is_empty() || value.is_empty() {
        return true;
    }
    if spec.list {
        value
            .split('\n')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .all(|item| spec.values.contains(&item))
    } else {
        spec.values.contains(&value)
    }
}

pub fn workflow_node_has_unknown_value(node: &WorkflowNode) -> bool {
    workflow_node_registry(node.kind).iter().any(|spec| {
        matches!(
            node.fields.get(spec.name),
            Some(WorkflowFieldValue::Text(s)) if !known_workflow_field_value(spec, s)
        )
    })
}

pub fn workflow_needs_admin(node: &WorkflowNode) -> bool {
    workflow_node_registry(node.kind).iter().any(|spec| {
        !spec.admin_values.is_empty()
            && matches!(
                node.fields.get(spec.name),
                Some(WorkflowFieldValue::Text(s))
                    if spec.admin_values.contains(&s.as_str()) || !known_workflow_field_value(spec, s)
            )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkflowMediaModelSpec {
    pub id: &'static str,
    pub provider: &'static str,
    pub name: &'static str,
    pub maker: &'static str,
    pub credits: bool,
    pub voices: bool,
    pub transparent: bool,
}

pub const WORKFLOW_MEDIA_MODELS: [WorkflowMediaModelSpec; 16] = [
    WorkflowMediaModelSpec { id: "gpt-image", provider: "codex", name: "GPT Image", maker: "OpenAI", credits: false, voices: false, transparent: true },
    WorkflowMediaModelSpec { id: "grok-imagine", provider: "grok", name: "Grok Imagine", maker: "xAI", credits: false, voices: true, transparent: true },
    WorkflowMediaModelSpec { id: "sunra-nano-banana-pro", provider: "sunra", name: "Nano Banana Pro", maker: "Google", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "sunra-gpt-image-2", provider: "sunra", name: "GPT Image 2", maker: "OpenAI", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "sunra-seedream-5-pro", provider: "sunra", name: "Seedream 5.0 Pro", maker: "ByteDance", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "sunra-flux-3-image", provider: "sunra", name: "FLUX 3", maker: "Black Forest Labs", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "sunra-seedance-2.0", provider: "sunra", name: "Seedance 2.0", maker: "ByteDance", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "sunra-kling-v4", provider: "sunra", name: "Kling 4.0", maker: "Kuaishou", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "sunra-veo-3.1", provider: "sunra", name: "Veo 3.1", maker: "Google", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "higgsfield-gpt_image_2_6", provider: "higgsfield", name: "GPT Image 2.6", maker: "OpenAI", credits: true, voices: false, transparent: true },
    WorkflowMediaModelSpec { id: "higgsfield-nano_banana_pro", provider: "higgsfield", name: "Nano Banana Pro", maker: "Google", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "higgsfield-seedream_v5_pro", provider: "higgsfield", name: "Seedream 5.0 Pro", maker: "ByteDance", credits: true, voices: false, transparent: true },
    WorkflowMediaModelSpec { id: "higgsfield-flux_3_image", provider: "higgsfield", name: "FLUX 3", maker: "Black Forest Labs", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "higgsfield-seedance_2_0", provider: "higgsfield", name: "Seedance 2.0", maker: "ByteDance", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "higgsfield-kling4_0", provider: "higgsfield", name: "Kling 4.0", maker: "Kuaishou", credits: true, voices: false, transparent: false },
    WorkflowMediaModelSpec { id: "higgsfield-wan2_7", provider: "higgsfield", name: "Wan 2.7", maker: "Alibaba", credits: true, voices: false, transparent: false },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowRunStatus {
    Running,
    Approval,
    Stopping,
    Done,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStepStatus {
    Running,
    Approval,
    Done,
    Failed,
    Stopped,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowRunSource {
    Manual,
    Schedule,
    Webhook,
    #[serde(rename = "github")]
    GitHub,
    Discord,
    Telegram,
    Slack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowVersionKind {
    Auto,
    Restore,
    Run,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowLastRun {
    pub status: WorkflowRunStatus,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSummary {
    pub id: String,
    pub title: String,
    pub steps: u32,
    pub updated_at: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub last_run: Option<WorkflowLastRun>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub paused: Option<bool>,
}

impl WorkflowSummary {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.id)
            && !self.title.trim().is_empty()
            && utf16_len(&self.title) <= INPUT_LIMITS.workflow_title
            && self.paused.is_none_or(|p| p)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowTriggerState {
    pub node_id: String,
    #[serde(rename = "type")]
    pub trigger_type: WorkflowTriggerType,
    #[serde(deserialize_with = "Option::deserialize")]
    pub next_due_at: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub webhook_url: Option<String>,
}

impl WorkflowTriggerState {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.node_id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDetail {
    pub summary: WorkflowSummary,
    pub graph: WorkflowGraph,
    pub revision: u64,
    pub triggers: Vec<WorkflowTriggerState>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub needs_newer_app: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub format: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub host_format: Option<u32>,
}

/// Alias matching `PROJECT.md § Interface Contracts § 1` (`WorkflowDefinition`).
pub type WorkflowDefinition = WorkflowDetail;

impl WorkflowDetail {
    pub fn validate(&self) -> Result<(), ContractError> {
        if !self.summary.is_valid() {
            return Err(ContractError::InvalidValue("Invalid workflow summary.".to_string()));
        }
        if self.revision > (1u64 << 53) - 1 {
            return Err(ContractError::InvalidValue("Invalid workflow revision.".to_string()));
        }
        if !self.triggers.iter().all(WorkflowTriggerState::is_valid) {
            return Err(ContractError::InvalidValue("Invalid workflow trigger state.".to_string()));
        }
        if self.needs_newer_app == Some(false) {
            return Err(ContractError::InvalidValue(
                "needsNewerApp must be true or omitted.".to_string(),
            ));
        }
        if self.format.is_some_and(|f| f < 1) || self.host_format.is_some_and(|f| f < 1) {
            return Err(ContractError::InvalidValue(
                "Workflow format version must be >= 1.".to_string(),
            ));
        }
        self.graph.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowFileReference {
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub title: Option<String>,
}

impl WorkflowFileReference {
    pub fn is_valid(&self) -> bool {
        matches!(self.kind.as_str(), "agent" | "channel")
            && !self.name.trim().is_empty()
            && is_bounded_string(&self.name, 255)
            && self.title.as_deref().is_none_or(|t| is_bounded_string(t, 255))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowFileLink {
    pub node: String,
    pub app: String,
}

impl WorkflowFileLink {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.node) && is_bounded_string(&self.app, 255)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowFileAsset {
    pub name: String,
    #[serde(rename = "type")]
    pub asset_type: String,
    pub data: String,
}

fn valid_standard_base64_len(data: &str) -> Option<usize> {
    if data.is_empty() || !data.len().is_multiple_of(4) {
        return None;
    }
    let bytes = data.as_bytes();
    let pad = if bytes.ends_with(b"==") {
        2
    } else if bytes.ends_with(b"=") {
        1
    } else {
        0
    };
    let body = &bytes[..bytes.len() - pad];
    if !body
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/')
    {
        return None;
    }
    Some((data.len() / 4) * 3 - pad)
}

impl WorkflowFileAsset {
    pub fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
            && is_bounded_string(&self.name, 255)
            && is_bounded_string(&self.asset_type, 255)
            && valid_standard_base64_len(&self.data)
                .is_some_and(|len| len <= ATTACHMENT_LIMITS.file_bytes)
    }
}

fn object_has_only(value: &serde_json::Value, allowed: &[&str]) -> bool {
    let Some(obj) = value.as_object() else {
        return true;
    };
    obj.keys().all(|k| allowed.contains(&k.as_str()))
}

fn list_has_only<F>(value: Option<&serde_json::Value>, allowed: &[&str], each: F) -> bool
where
    F: Fn(&serde_json::Value) -> bool,
{
    let Some(arr) = value.and_then(serde_json::Value::as_array) else {
        return true;
    };
    arr.iter()
        .all(|item| object_has_only(item, allowed) && each(item))
}

/// Exact port of `workflowGraphHasUnknownKey` (`packages/contracts/src/ipc-workflow-format.ts:L62-L98`).
pub fn workflow_graph_has_unknown_key(graph: &serde_json::Value) -> bool {
    let Some(obj) = graph.as_object() else {
        return false;
    };
    const GRAPH_KEYS: [&str; 3] = ["nodes", "edges", "strokes"];
    const NODE_KEYS: [&str; 7] = ["id", "kind", "at", "size", "locked", "fields", "schedule"];
    const SIZE_KEYS: [&str; 3] = ["width", "height", "font"];
    const EDGE_KEYS: [&str; 5] = ["id", "from", "to", "type", "branch"];
    const STROKE_KEYS: [&str; 5] = ["id", "brush", "color", "width", "points"];

    !(obj.keys().all(|k| GRAPH_KEYS.contains(&k.as_str()))
        && list_has_only(obj.get("nodes"), &NODE_KEYS, |node| {
            node.as_object().is_none_or(|n| {
                n.get("size")
                    .is_none_or(|size| object_has_only(size, &SIZE_KEYS))
            })
        })
        && list_has_only(obj.get("edges"), &EDGE_KEYS, |_| true)
        && list_has_only(obj.get("strokes"), &STROKE_KEYS, |_| true))
}

fn deserialize_strict_workflow_graph<'de, D>(deserializer: D) -> Result<WorkflowGraph, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    if workflow_graph_has_unknown_key(&raw) {
        return Err(serde::de::Error::custom(
            "Workflow portable file graph contains unknown keys from a newer app version",
        ));
    }
    serde_json::from_value(raw).map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowPortableFile {
    pub format: String,
    pub version: u32,
    pub graph_format: u32,
    pub min_app_version: String,
    pub title: String,
    #[serde(deserialize_with = "deserialize_strict_workflow_graph")]
    pub graph: WorkflowGraph,
    pub references: BTreeMap<String, WorkflowFileReference>,
    pub links: Vec<WorkflowFileLink>,
    pub assets: BTreeMap<String, WorkflowFileAsset>,
}

impl WorkflowPortableFile {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.format != WORKFLOW_FILE_FORMAT
            || !(1..=WORKFLOW_FILE_VERSION).contains(&self.version)
            || !(1..=WORKFLOW_FORMAT).contains(&self.graph_format)
            || !is_bounded_string(&self.min_app_version, 32)
            || self.title.trim().is_empty()
            || !is_bounded_string(&self.title, INPUT_LIMITS.workflow_title)
        {
            return Err(ContractError::InvalidValue("Invalid workflow file header.".to_string()));
        }
        self.graph.validate()?;
        if self.graph.nodes.iter().any(workflow_node_has_unknown_value) {
            return Err(ContractError::InvalidValue(
                "Workflow node contains unknown constrained field value.".to_string(),
            ));
        }
        if self.references.len() > INPUT_LIMITS.workflow_nodes
            || !self.references.iter().all(|(k, r)| is_identifier(k) && r.is_valid())
            || self.assets.len() > INPUT_LIMITS.workflow_nodes
            || !self.assets.iter().all(|(k, a)| is_identifier(k) && a.is_valid())
            || self.links.len() > INPUT_LIMITS.workflow_nodes
            || !self.links.iter().all(WorkflowFileLink::is_valid)
        {
            return Err(ContractError::InvalidValue(
                "Invalid workflow file references, assets, or links.".to_string(),
            ));
        }
        let total_asset_bytes: usize = self
            .assets
            .values()
            .filter_map(|a| valid_standard_base64_len(&a.data))
            .fold(0usize, usize::saturating_add);
        if total_asset_bytes > ATTACHMENT_LIMITS.total_bytes {
            return Err(ContractError::InvalidValue(
                "Workflow file assets exceed total byte limit.".to_string(),
            ));
        }
        let node_ids: HashSet<&str> = self.graph.nodes.iter().map(|n| n.id.as_str()).collect();
        if !self.links.iter().all(|l| node_ids.contains(l.node.as_str())) {
            return Err(ContractError::InvalidValue("Workflow file link references unknown node.".to_string()));
        }
        for n in &self.graph.nodes {
            for (field_name, id_kind) in workflow_id_fields(n.kind) {
                let Some(WorkflowFieldValue::Text(val)) = n.fields.get(field_name) else {
                    continue;
                };
                if val.is_empty()
                    || (val == "trigger"
                        && matches!(id_kind, WorkflowIdKind::Connection | WorkflowIdKind::Place))
                {
                    continue;
                }
                let ok = match id_kind {
                    WorkflowIdKind::File => self.assets.contains_key(val.as_str()),
                    WorkflowIdKind::Agent => {
                        self.references.get(val.as_str()).is_some_and(|r| r.kind == "agent")
                    }
                    WorkflowIdKind::Channel => {
                        self.references.get(val.as_str()).is_some_and(|r| r.kind == "channel")
                    }
                    _ => false,
                };
                if !ok {
                    return Err(ContractError::InvalidValue(format!(
                        "Unresolved workflow file reference: {val}"
                    )));
                }
            }
        }
        Ok(())
    }
}
