//! Interactive Pan/Zoom GPUI `WorkflowCanvas` (`OB-F18`).
//!
//! Supports all 12 `WorkflowNodeKind`s (9 step kinds + 3 annotation kinds), typed ports
//! (`Text`, `Image`, `File`, `Yes`/`No`), cubic Bezier edge painting with SCC loop-edge
//! highlighting, freehand ink strokes (`<= 500`), topological Kahn tidy layout (`Shift+T`),
//! fit-to-view (`Shift+1`), undo/redo stacks, multiplayer `PeerCursors`, `GroupFrame`s,
//! `WorkflowVersionBar`, live run HUD, `WorkflowMinimap`, and `.openbot-workflow` export/import.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::AvatarMood;
use openbot_contracts::workflows::{
    WORKFLOW_FILE_FORMAT, WORKFLOW_FILE_VERSION, WORKFLOW_FORMAT, WorkflowBranch, WorkflowEdge,
    WorkflowFieldValue, WorkflowGraph, WorkflowNode, WorkflowNodeKind, WorkflowPoint,
    WorkflowPortType, WorkflowRunStatus, WorkflowStepStatus, WorkflowStroke, WorkflowVersionKind,
    workflow_edge_problem, workflow_graph_has_unknown_key, workflow_loop_edges,
};
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{AgentAvatarProps, render_agent_avatar};
use crate::theme::openbot_palette::{OpenBotColorScheme, OpenBotPalette};
use crate::theme::{IconName, tg_icon_rgba};

/// Maximum nodes permitted in a single workflow canvas (`INPUT_LIMITS.workflow_nodes`).
pub const WORKFLOW_MAX_NODES: usize = 200;
/// Maximum directed edges permitted in a single workflow canvas (`INPUT_LIMITS.workflow_edges`).
pub const WORKFLOW_MAX_EDGES: usize = 400;
/// Maximum freehand ink strokes permitted (`INPUT_LIMITS.workflow_strokes`).
pub const WORKFLOW_MAX_STROKES: usize = 500;
/// Minimum zoom factor (`25%`).
pub const WORKFLOW_ZOOM_MIN: f32 = 0.25;
/// Maximum zoom factor (`200%`).
pub const WORKFLOW_ZOOM_MAX: f32 = 2.0;
/// Canonical zoom presets matching `WorkflowZoomMenu.tsx`.
pub const WORKFLOW_ZOOM_PRESETS: [f32; 6] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0];
/// Portable workflow export envelope format string.
pub const WORKFLOW_EXPORT_FORMAT: &str = WORKFLOW_FILE_FORMAT;
/// Portable workflow export envelope version (`1`).
pub const WORKFLOW_EXPORT_VERSION: u32 = WORKFLOW_FILE_VERSION;

/// All 9 executable step `WorkflowNodeKind`s.
pub const WORKFLOW_STEP_KINDS: [WorkflowNodeKind; 9] = [
    WorkflowNodeKind::Trigger,
    WorkflowNodeKind::Prompt,
    WorkflowNodeKind::Asset,
    WorkflowNodeKind::Llm,
    WorkflowNodeKind::Agent,
    WorkflowNodeKind::Image,
    WorkflowNodeKind::Condition,
    WorkflowNodeKind::Approval,
    WorkflowNodeKind::Output,
];

/// All 3 non-executable annotation `WorkflowNodeKind`s.
pub const WORKFLOW_ANNOTATION_KINDS: [WorkflowNodeKind; 3] = [
    WorkflowNodeKind::Text,
    WorkflowNodeKind::Note,
    WorkflowNodeKind::Shape,
];

/// Canonical string identifiers for all 12 `WorkflowNodeKind`s.
pub const WORKFLOW_ALL_NODE_KIND_STRS: [&str; 12] = [
    "trigger",
    "prompt",
    "asset",
    "llm",
    "agent",
    "image",
    "condition",
    "approval",
    "output",
    "text",
    "note",
    "shape",
];

/// Canonical string identifiers for the 3 annotation `WorkflowNodeKind`s.
pub const WORKFLOW_ANNOTATION_KIND_STRS: [&str; 3] = ["text", "note", "shape"];

pub const WORKFLOW_DEFAULT_NODE_WIDTH: f32 = 252.0;
pub const WORKFLOW_DEFAULT_NODE_HEIGHT: f32 = 124.0;
pub const WORKFLOW_TIDY_COL_GAP: f64 = 320.0;
pub const WORKFLOW_TIDY_ROW_GAP: f64 = 180.0;
pub const WORKFLOW_MAX_UNDO_DEPTH: usize = 100;

/// Active pointer/ink tool selected on the `WorkflowCanvas` toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowInkTool {
    #[default]
    Select,
    Pan,
    Pen,
    Highlighter,
    Eraser,
}

impl WorkflowInkTool {
    pub const ALL: [Self; 5] = [
        Self::Select,
        Self::Pan,
        Self::Pen,
        Self::Highlighter,
        Self::Eraser,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Pan => "Pan",
            Self::Pen => "Pen",
            Self::Highlighter => "Highlight",
            Self::Eraser => "Eraser",
        }
    }
}

/// Collaborative presence cursor for another workspace peer viewing or editing the canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPeerCursor {
    pub peer_id: String,
    pub display_name: String,
    pub color_hex: String,
    pub x: f64,
    pub y: f64,
    pub active: bool,
}

/// Visual grouping container framing a cluster of related workflow nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGroupFrame {
    pub id: String,
    pub title: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub color_hex: String,
    pub node_ids: Vec<String>,
}

/// Snapshot entry shown in `WorkflowVersionBar` (`auto | restore | run`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowVersionEntry {
    pub id: String,
    pub revision: u64,
    pub kind: WorkflowVersionKind,
    pub label: String,
    pub created_at: String,
}

/// Live execution status for an individual step in the workflow run HUD.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowHudStep {
    pub node_id: String,
    pub label: String,
    pub kind: WorkflowNodeKind,
    pub status: WorkflowStepStatus,
    pub branch: Option<WorkflowBranch>,
    pub elapsed_ms: u64,
    pub output_preview: Option<String>,
    pub error: Option<String>,
}

/// Live workflow execution HUD state overlaying the canvas during/after a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowHudRun {
    pub run_id: String,
    pub status: WorkflowRunStatus,
    pub started_at: String,
    pub steps: Vec<WorkflowHudStep>,
}

/// Complete interactive pan/zoom workflow DAG state controller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowCanvasState {
    pub workflow_id: String,
    pub title: String,
    pub revision: u64,
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
    pub graph: WorkflowGraph,
    pub selected_node_ids: Vec<String>,
    pub selected_edge_ids: Vec<String>,
    pub undo_stack: Vec<WorkflowGraph>,
    pub redo_stack: Vec<WorkflowGraph>,
    pub peer_cursors: Vec<WorkflowPeerCursor>,
    pub group_frames: Vec<WorkflowGroupFrame>,
    pub versions: Vec<WorkflowVersionEntry>,
    pub hud_run: Option<WorkflowHudRun>,
    pub minimap_visible: bool,
    pub hud_visible: bool,
    pub zoom_menu_open: bool,
    pub version_bar_open: bool,
    pub active_tool: WorkflowInkTool,
    pub tidy_applied: bool,
    pub reduced_motion: bool,
}

impl Default for WorkflowCanvasState {
    fn default() -> Self {
        Self::new("Untitled Workflow")
    }
}

impl WorkflowCanvasState {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            workflow_id: "wf-default".to_string(),
            title: title.into(),
            revision: 1,
            pan_x: 32.0,
            pan_y: 32.0,
            zoom: 1.0,
            graph: WorkflowGraph::default(),
            selected_node_ids: Vec::new(),
            selected_edge_ids: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            peer_cursors: Vec::new(),
            group_frames: Vec::new(),
            versions: vec![WorkflowVersionEntry {
                id: "ver-1".to_string(),
                revision: 1,
                kind: WorkflowVersionKind::Auto,
                label: "Initial snapshot".to_string(),
                created_at: "2026-03-29T09:00:00Z".to_string(),
            }],
            hud_run: None,
            minimap_visible: true,
            hud_visible: true,
            zoom_menu_open: false,
            version_bar_open: false,
            active_tool: WorkflowInkTool::Select,
            tidy_applied: false,
            reduced_motion: false,
        }
    }

    /// Creates a representative 5-node workflow graph with trigger, agent, condition, approval, and output.
    pub fn with_sample_pipeline() -> Self {
        let mut state = Self::new("Release Triage Pipeline");
        let make_node = |id: &str, kind: WorkflowNodeKind, x: f64, y: f64, fields: &[(&str, &str)]| {
            let mut map = BTreeMap::new();
            for &(k, v) in fields {
                map.insert(k.to_string(), WorkflowFieldValue::Text(v.to_string()));
            }
            WorkflowNode {
                id: id.to_string(),
                kind,
                at: WorkflowPoint { x, y },
                size: None,
                locked: None,
                fields: map,
                schedule: None,
            }
        };
        state.graph.nodes = vec![
            make_node("n-trig", WorkflowNodeKind::Trigger, 60.0, 120.0, &[("trigger", "github"), ("event", "pr.opened")]),
            make_node("n-agent", WorkflowNodeKind::Agent, 380.0, 120.0, &[("agentId", "triage-bot"), ("task", "Audit diff & tests")]),
            make_node("n-cond", WorkflowNodeKind::Condition, 700.0, 120.0, &[("rule", "risk == high")]),
            make_node("n-appr", WorkflowNodeKind::Approval, 1020.0, 40.0, &[("question", "Approve production deploy?")]),
            make_node("n-out", WorkflowNodeKind::Output, 1340.0, 120.0, &[("target", "telegram"), ("message", "Deploy status updated")]),
            make_node("n-note", WorkflowNodeKind::Note, 60.0, 320.0, &[("text", "Human approval required for high-risk PRs")]),
        ];
        state.graph.edges = vec![
            WorkflowEdge { id: "e1".into(), from: "n-trig".into(), to: "n-agent".into(), port_type: WorkflowPortType::Text, branch: None },
            WorkflowEdge { id: "e2".into(), from: "n-agent".into(), to: "n-cond".into(), port_type: WorkflowPortType::Text, branch: None },
            WorkflowEdge { id: "e3".into(), from: "n-cond".into(), to: "n-appr".into(), port_type: WorkflowPortType::Text, branch: Some(WorkflowBranch::Yes) },
            WorkflowEdge { id: "e4".into(), from: "n-cond".into(), to: "n-out".into(), port_type: WorkflowPortType::Text, branch: Some(WorkflowBranch::No) },
            WorkflowEdge { id: "e5".into(), from: "n-appr".into(), to: "n-out".into(), port_type: WorkflowPortType::Text, branch: None },
        ];
        state
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            self.zoom = zoom.clamp(WORKFLOW_ZOOM_MIN, WORKFLOW_ZOOM_MAX);
        }
    }

    pub fn zoom_in(&mut self) {
        let next = WORKFLOW_ZOOM_PRESETS
            .iter()
            .copied()
            .find(|&p| p > self.zoom + 0.01)
            .unwrap_or(WORKFLOW_ZOOM_MAX);
        self.set_zoom(next);
    }

    pub fn zoom_out(&mut self) {
        let prev = WORKFLOW_ZOOM_PRESETS
            .iter()
            .rev()
            .copied()
            .find(|&p| p < self.zoom - 0.01)
            .unwrap_or(WORKFLOW_ZOOM_MIN);
        self.set_zoom(prev);
    }

    pub fn reset_zoom(&mut self) {
        self.zoom = 1.0;
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        if dx.is_finite() {
            self.pan_x += dx;
        }
        if dy.is_finite() {
            self.pan_y += dy;
        }
    }

    pub fn push_undo_snapshot(&mut self) {
        if self.undo_stack.len() >= WORKFLOW_MAX_UNDO_DEPTH {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.graph.clone());
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(self.graph.clone());
        self.graph = prev;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(self.graph.clone());
        self.graph = next;
        true
    }

    pub fn add_node(&mut self, node: WorkflowNode) -> Result<(), String> {
        if self.graph.nodes.len() >= WORKFLOW_MAX_NODES {
            return Err(format!(
                "workflow node limit exceeded: maximum {WORKFLOW_MAX_NODES} nodes allowed"
            ));
        }
        if !node.is_valid() {
            return Err(format!("invalid workflow node '{}'", node.id));
        }
        if self.graph.nodes.iter().any(|n| n.id == node.id) {
            return Err(format!("duplicate workflow node id '{}'", node.id));
        }
        self.push_undo_snapshot();
        self.graph.nodes.push(node);
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn add_edge(&mut self, edge: WorkflowEdge) -> Result<(), String> {
        if self.graph.edges.len() >= WORKFLOW_MAX_EDGES {
            return Err(format!(
                "workflow edge limit exceeded: maximum {WORKFLOW_MAX_EDGES} edges allowed"
            ));
        }
        if edge.from == edge.to {
            return Err(format!("self-edge '{}' on node '{}' is not allowed", edge.id, edge.from));
        }
        if !edge.is_valid() {
            return Err(format!("invalid workflow edge '{}'", edge.id));
        }
        if self.graph.edges.iter().any(|e| e.id == edge.id) {
            return Err(format!("duplicate workflow edge id '{}'", edge.id));
        }
        let from = self
            .graph
            .nodes
            .iter()
            .find(|n| n.id == edge.from)
            .ok_or_else(|| format!("edge '{}' references unknown source node '{}'", edge.id, edge.from))?;
        let to = self
            .graph
            .nodes
            .iter()
            .find(|n| n.id == edge.to)
            .ok_or_else(|| format!("edge '{}' references unknown target node '{}'", edge.id, edge.to))?;
        if let Some(problem) = workflow_edge_problem(from, to, &edge) {
            return Err(format!("workflow edge '{}' problem: {problem:?}", edge.id));
        }
        self.push_undo_snapshot();
        self.graph.edges.push(edge);
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn add_stroke(&mut self, stroke: WorkflowStroke) -> Result<(), String> {
        if self.graph.strokes.len() >= WORKFLOW_MAX_STROKES {
            return Err(format!(
                "workflow stroke limit exceeded: maximum {WORKFLOW_MAX_STROKES} strokes allowed"
            ));
        }
        if !stroke.is_valid() {
            return Err(format!("invalid workflow ink stroke '{}'", stroke.id));
        }
        self.push_undo_snapshot();
        self.graph.strokes.push(stroke);
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn select_node(&mut self, node_id: &str, additive: bool) {
        if !additive {
            self.selected_node_ids.clear();
        }
        if !self.selected_node_ids.iter().any(|id| id == node_id) {
            self.selected_node_ids.push(node_id.to_string());
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected_node_ids.clear();
        self.selected_edge_ids.clear();
    }

    pub fn remove_selected(&mut self) {
        if self.selected_node_ids.is_empty() && self.selected_edge_ids.is_empty() {
            return;
        }
        self.push_undo_snapshot();
        let removed_nodes: HashSet<&str> = self
            .selected_node_ids
            .iter()
            .map(String::as_str)
            .collect();
        let removed_edges: HashSet<&str> = self
            .selected_edge_ids
            .iter()
            .map(String::as_str)
            .collect();
        self.graph
            .edges
            .retain(|e| {
                !removed_edges.contains(e.id.as_str())
                    && !removed_nodes.contains(e.from.as_str())
                    && !removed_nodes.contains(e.to.as_str())
            });
        self.graph
            .nodes
            .retain(|n| !removed_nodes.contains(n.id.as_str()));
        self.clear_selection();
        self.revision = self.revision.saturating_add(1);
    }

    /// Detects SCC back-edges and self-edges using `openbot_contracts::workflows::workflow_loop_edges`.
    pub fn detect_loop_edges(&self) -> Vec<String> {
        workflow_loop_edges(&self.graph.nodes, &self.graph.edges)
            .into_iter()
            .collect()
    }

    /// Parses a JSON workflow graph payload (tolerating omitted `strokes`/`fields`) and returns SCC loop edge IDs.
    pub fn detect_loop_edges_from_json(graph_json: &serde_json::Value) -> Result<Vec<String>, String> {
        let parsed = parse_workflow_graph_json(graph_json)?;
        Ok(workflow_loop_edges(&parsed.nodes, &parsed.edges)
            .into_iter()
            .collect())
    }

    /// Replaces the current canvas graph from a JSON workflow graph object.
    pub fn load_graph_json(&mut self, graph_json: &serde_json::Value) -> Result<(), String> {
        let parsed = parse_workflow_graph_json(graph_json)?;
        if parsed.nodes.len() > WORKFLOW_MAX_NODES {
            return Err(format!("too many workflow nodes (limit {WORKFLOW_MAX_NODES})"));
        }
        if parsed.edges.len() > WORKFLOW_MAX_EDGES {
            return Err(format!("too many workflow edges (limit {WORKFLOW_MAX_EDGES})"));
        }
        if parsed.strokes.len() > WORKFLOW_MAX_STROKES {
            return Err(format!("too many workflow strokes (limit {WORKFLOW_MAX_STROKES})"));
        }
        self.push_undo_snapshot();
        self.graph = parsed;
        self.tidy_applied = false;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    /// Genuine topological Kahn layering + barycenter row ordering (`Shift+T` Tidy auto-layout).
    /// Breaks SCC back-edges via `workflow_loop_edges` so cyclic graphs lay out deterministically too.
    pub fn apply_tidy_layout(&mut self) {
        if self.graph.nodes.is_empty() {
            self.tidy_applied = true;
            return;
        }
        self.push_undo_snapshot();
        let loop_set: BTreeSet<String> = workflow_loop_edges(&self.graph.nodes, &self.graph.edges);
        let mut step_indices: Vec<usize> = Vec::new();
        let mut ann_indices: Vec<usize> = Vec::new();
        let mut id_to_idx: HashMap<String, usize> = HashMap::new();
        for (idx, node) in self.graph.nodes.iter().enumerate() {
            id_to_idx.insert(node.id.clone(), idx);
            if node.kind.is_step() {
                step_indices.push(idx);
            } else {
                ann_indices.push(idx);
            }
        }

        let step_set: HashSet<usize> = step_indices.iter().copied().collect();
        let mut indegree: HashMap<usize, usize> = step_indices.iter().map(|&i| (i, 0)).collect();
        let mut outgoing: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut incoming: HashMap<usize, Vec<usize>> = HashMap::new();
        for edge in &self.graph.edges {
            if loop_set.contains(&edge.id) || edge.from == edge.to {
                continue;
            }
            if let (Some(&u), Some(&v)) = (id_to_idx.get(&edge.from), id_to_idx.get(&edge.to)) {
                if step_set.contains(&u) && step_set.contains(&v) {
                    outgoing.entry(u).or_default().push(v);
                    incoming.entry(v).or_default().push(u);
                    *indegree.entry(v).or_insert(0) += 1;
                }
            }
        }

        let mut initial_roots: Vec<usize> = step_indices
            .iter()
            .copied()
            .filter(|i| indegree.get(i).copied().unwrap_or(0) == 0)
            .collect();
        initial_roots.sort_by(|&a, &b| {
            let na = &self.graph.nodes[a];
            let nb = &self.graph.nodes[b];
            na.at
                .x
                .total_cmp(&nb.at.x)
                .then_with(|| na.at.y.total_cmp(&nb.at.y))
                .then_with(|| na.id.cmp(&nb.id))
        });

        let mut layer: HashMap<usize, usize> = HashMap::new();
        let mut queue: VecDeque<usize> = initial_roots.into_iter().collect();
        let mut visited: HashSet<usize> = HashSet::new();
        while let Some(u) = queue.pop_front() {
            if !visited.insert(u) {
                continue;
            }
            let cur_layer = layer.get(&u).copied().unwrap_or(0);
            layer.insert(u, cur_layer);
            if let Some(children) = outgoing.get(&u) {
                for &v in children {
                    let next_layer = cur_layer + 1;
                    let entry = layer.entry(v).or_insert(0);
                    if next_layer > *entry {
                        *entry = next_layer;
                    }
                    if let Some(deg) = indegree.get_mut(&v) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            queue.push_back(v);
                        }
                    }
                }
            }
        }

        let max_assigned = layer.values().copied().max().unwrap_or(0);
        for &idx in &step_indices {
            if !visited.contains(&idx) {
                layer.insert(idx, max_assigned + 1);
            }
        }

        let mut columns: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for &idx in &step_indices {
            let col = layer.get(&idx).copied().unwrap_or(0);
            columns.entry(col).or_default().push(idx);
        }

        let max_col_len = columns.values().map(Vec::len).max().unwrap_or(1);
        let total_height = ((max_col_len.saturating_sub(1)) as f64) * WORKFLOW_TIDY_ROW_GAP;
        let mut placed_y: HashMap<usize, f64> = HashMap::new();
        let mut max_bottom_y = 80.0_f64;

        for (col, mut rows) in columns {
            rows.sort_by(|&a, &b| {
                let bary = |idx: usize| -> f64 {
                    match incoming.get(&idx) {
                        Some(ps) if !ps.is_empty() => {
                            let sum: f64 = ps
                                .iter()
                                .filter_map(|p| placed_y.get(p).copied())
                                .sum();
                            let cnt = ps.iter().filter(|p| placed_y.contains_key(p)).count();
                            if cnt > 0 { sum / (cnt as f64) } else { self.graph.nodes[idx].at.y }
                        }
                        _ => self.graph.nodes[idx].at.y,
                    }
                };
                bary(a)
                    .total_cmp(&bary(b))
                    .then_with(|| self.graph.nodes[a].at.y.total_cmp(&self.graph.nodes[b].at.y))
                    .then_with(|| self.graph.nodes[a].id.cmp(&self.graph.nodes[b].id))
            });
            let col_height = ((rows.len().saturating_sub(1)) as f64) * WORKFLOW_TIDY_ROW_GAP;
            let y_offset = 80.0 + ((total_height - col_height) * 0.5).max(0.0);
            let x_pos = 80.0 + (col as f64) * WORKFLOW_TIDY_COL_GAP;
            for (row_idx, node_idx) in rows.into_iter().enumerate() {
                let y_pos = y_offset + (row_idx as f64) * WORKFLOW_TIDY_ROW_GAP;
                self.graph.nodes[node_idx].at = WorkflowPoint { x: x_pos, y: y_pos };
                placed_y.insert(node_idx, y_pos);
                max_bottom_y = max_bottom_y.max(y_pos + f64::from(WORKFLOW_DEFAULT_NODE_HEIGHT));
            }
        }

        ann_indices.sort_by(|&a, &b| self.graph.nodes[a].id.cmp(&self.graph.nodes[b].id));
        let ann_y = max_bottom_y + 96.0;
        for (i, node_idx) in ann_indices.into_iter().enumerate() {
            self.graph.nodes[node_idx].at = WorkflowPoint {
                x: 80.0 + (i as f64) * 280.0,
                y: ann_y,
            };
        }
        self.tidy_applied = true;
        self.revision = self.revision.saturating_add(1);
    }

    /// Fits all canvas nodes, strokes, and group frames inside `(viewport_w, viewport_h)` (`Shift+1`).
    pub fn fit_to_view(&mut self, viewport_w: f32, viewport_h: f32) {
        let vw = viewport_w.max(120.0);
        let vh = viewport_h.max(120.0);
        let Some((min_x, min_y, max_x, max_y)) = self.content_bounds() else {
            self.pan_x = 0.0;
            self.pan_y = 0.0;
            self.zoom = 1.0;
            return;
        };
        let pad = 64.0_f32;
        let content_w = (max_x - min_x).max(WORKFLOW_DEFAULT_NODE_WIDTH);
        let content_h = (max_y - min_y).max(WORKFLOW_DEFAULT_NODE_HEIGHT);
        let scale_x = (vw - pad * 2.0).max(64.0) / content_w;
        let scale_y = (vh - pad * 2.0).max(64.0) / content_h;
        self.set_zoom(scale_x.min(scale_y));
        let cx = (min_x + max_x) * 0.5;
        let cy = (min_y + max_y) * 0.5;
        self.pan_x = vw * 0.5 - cx * self.zoom;
        self.pan_y = vh * 0.5 - cy * self.zoom;
    }

    pub fn content_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for node in &self.graph.nodes {
            let (nw, nh) = node_dimensions(node);
            let x = node.at.x as f32;
            let y = node.at.y as f32;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + nw);
            max_y = max_y.max(y + nh);
        }
        for stroke in &self.graph.strokes {
            for pt in &stroke.points {
                min_x = min_x.min(pt.x as f32);
                min_y = min_y.min(pt.y as f32);
                max_x = max_x.max(pt.x as f32);
                max_y = max_y.max(pt.y as f32);
            }
        }
        for frame in &self.group_frames {
            min_x = min_x.min(frame.x as f32);
            min_y = min_y.min(frame.y as f32);
            max_x = max_x.max((frame.x + frame.width) as f32);
            max_y = max_y.max((frame.y + frame.height) as f32);
        }
        if min_x.is_finite() && min_y.is_finite() {
            Some((min_x, min_y, max_x, max_y))
        } else {
            None
        }
    }

    /// Exports a portable `.openbot-workflow.json` envelope (`format: "openbot.workflow"`, `version: 1`).
    pub fn export_portable_json(&self) -> serde_json::Value {
        let mut references = serde_json::Map::new();
        for node in &self.graph.nodes {
            if let Some(agent_id) = node.agent_id() {
                references.insert(
                    agent_id.to_string(),
                    serde_json::json!({
                        "kind": "agent",
                        "name": agent_id,
                    }),
                );
            }
            if let Some(WorkflowFieldValue::Text(channel_id)) = node.fields.get("channelId") {
                if !channel_id.is_empty() {
                    references.insert(
                        channel_id.clone(),
                        serde_json::json!({
                            "kind": "channel",
                            "name": channel_id,
                        }),
                    );
                }
            }
        }
        serde_json::json!({
            "format": WORKFLOW_EXPORT_FORMAT,
            "version": WORKFLOW_EXPORT_VERSION,
            "graphFormat": WORKFLOW_FORMAT,
            "minAppVersion": "0.78.0",
            "title": self.title,
            "graph": self.graph,
            "references": references,
            "links": [],
            "assets": {},
        })
    }

    /// Validates and imports a portable `.openbot-workflow.json` envelope into this canvas.
    pub fn import_portable_json(&mut self, raw: &serde_json::Value) -> Result<(), String> {
        let obj = raw
            .as_object()
            .ok_or_else(|| "portable workflow must be a JSON object".to_string())?;
        let fmt = obj.get("format").and_then(serde_json::Value::as_str).unwrap_or("");
        if fmt != WORKFLOW_EXPORT_FORMAT {
            return Err(format!(
                "unsupported workflow export format '{fmt}': expected '{WORKFLOW_EXPORT_FORMAT}'"
            ));
        }
        let ver = obj.get("version").and_then(serde_json::Value::as_u64).unwrap_or(0);
        if ver != u64::from(WORKFLOW_EXPORT_VERSION) {
            return Err(format!(
                "unsupported workflow export version {ver}: expected {WORKFLOW_EXPORT_VERSION}"
            ));
        }
        let graph_val = obj
            .get("graph")
            .ok_or_else(|| "portable workflow is missing 'graph' payload".to_string())?;
        if workflow_graph_has_unknown_key(graph_val) {
            return Err("portable workflow graph contains unknown keys from a newer app version".to_string());
        }
        let parsed = parse_workflow_graph_json(graph_val)?;
        parsed.validate().map_err(|e| e.to_string())?;
        self.push_undo_snapshot();
        if let Some(title) = obj.get("title").and_then(serde_json::Value::as_str) {
            let trimmed = title.trim();
            if !trimmed.is_empty() {
                self.title = trimmed.to_string();
            }
        }
        self.graph = parsed;
        self.tidy_applied = false;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    /// Produces the deterministic `workflow_canvas` JSON snapshot for headless `ui-probe`.
    pub fn probe_summary(&self, export_workflow: bool) -> serde_json::Value {
        let exported = if export_workflow {
            Some(self.export_portable_json())
        } else {
            None
        };
        serde_json::json!({
            "node_kinds": WORKFLOW_ALL_NODE_KIND_STRS,
            "step_kinds_count": WORKFLOW_STEP_KINDS.len(),
            "annotation_kinds": WORKFLOW_ANNOTATION_KIND_STRS,
            "loop_edges": self.detect_loop_edges(),
            "minimap": self.minimap_visible,
            "hud": self.hud_visible,
            "tidy_applied": self.tidy_applied,
            "exported_format": WORKFLOW_EXPORT_FORMAT,
            "exported_version": WORKFLOW_EXPORT_VERSION,
            "exported_workflow": exported,
            "zoom": self.zoom,
            "zoom_presets": WORKFLOW_ZOOM_PRESETS,
            "pan": [self.pan_x, self.pan_y],
            "node_count": self.graph.nodes.len(),
            "edge_count": self.graph.edges.len(),
            "stroke_count": self.graph.strokes.len(),
            "max_nodes": WORKFLOW_MAX_NODES,
            "max_edges": WORKFLOW_MAX_EDGES,
            "max_strokes": WORKFLOW_MAX_STROKES,
        })
    }
}

/// Parses a `WorkflowGraph` from JSON, filling in omitted `strokes`/`fields` defaults cleanly.
pub fn parse_workflow_graph_json(graph_json: &serde_json::Value) -> Result<WorkflowGraph, String> {
    let mut normalized = graph_json.clone();
    if let Some(obj) = normalized.as_object_mut() {
        obj.entry("nodes").or_insert_with(|| serde_json::json!([]));
        obj.entry("edges").or_insert_with(|| serde_json::json!([]));
        obj.entry("strokes").or_insert_with(|| serde_json::json!([]));
        if let Some(nodes) = obj.get_mut("nodes").and_then(serde_json::Value::as_array_mut) {
            for node in nodes {
                if let Some(nobj) = node.as_object_mut() {
                    nobj.entry("fields").or_insert_with(|| serde_json::json!({}));
                }
            }
        }
    }
    serde_json::from_value(normalized).map_err(|e| format!("invalid workflow graph JSON: {e}"))
}

fn node_dimensions(node: &WorkflowNode) -> (f32, f32) {
    let default_h = match node.kind {
        WorkflowNodeKind::Note | WorkflowNodeKind::Text => 96.0,
        WorkflowNodeKind::Shape => 112.0,
        WorkflowNodeKind::Condition | WorkflowNodeKind::Image => 136.0,
        _ => WORKFLOW_DEFAULT_NODE_HEIGHT,
    };
    let w = node
        .size
        .and_then(|s| s.width)
        .map(|v| (v as f32).clamp(120.0, 640.0))
        .unwrap_or(WORKFLOW_DEFAULT_NODE_WIDTH);
    let h = node
        .size
        .and_then(|s| s.height)
        .map(|v| (v as f32).clamp(72.0, 520.0))
        .unwrap_or(default_h);
    (w, h)
}

fn pal_rgba(palette: &OpenBotPalette, key: &str, fallback_hex: u32) -> Rgba {
    palette.rgba(key).unwrap_or_else(|| rgb(fallback_hex))
}

fn parse_hex_rgb(hex: &str, fallback: u32) -> Rgba {
    let trimmed = hex.trim().trim_start_matches('#');
    if trimmed.len() == 6 {
        if let Ok(val) = u32::from_str_radix(trimmed, 16) {
            return rgb(val);
        }
    }
    rgb(fallback)
}

fn port_color(port: WorkflowPortType, palette: &OpenBotPalette) -> Rgba {
    match port {
        WorkflowPortType::Text => pal_rgba(palette, "--openbot-accent", 0x007cf7),
        WorkflowPortType::Image => rgb(0xb474f4),
        WorkflowPortType::File => rgb(0x6bc7d9),
    }
}

fn node_kind_label(kind: WorkflowNodeKind) -> &'static str {
    match kind {
        WorkflowNodeKind::Trigger => "Trigger",
        WorkflowNodeKind::Prompt => "Prompt",
        WorkflowNodeKind::Asset => "Asset",
        WorkflowNodeKind::Llm => "LLM",
        WorkflowNodeKind::Agent => "Agent",
        WorkflowNodeKind::Image => "Image / Video",
        WorkflowNodeKind::Condition => "Condition",
        WorkflowNodeKind::Approval => "Approval",
        WorkflowNodeKind::Output => "Output",
        WorkflowNodeKind::Text => "Text",
        WorkflowNodeKind::Note => "Sticky Note",
        WorkflowNodeKind::Shape => "Shape",
    }
}

fn node_kind_accent(kind: WorkflowNodeKind, palette: &OpenBotPalette) -> Rgba {
    match kind {
        WorkflowNodeKind::Trigger => pal_rgba(palette, "--openbot-success", 0x31cf76),
        WorkflowNodeKind::Prompt => rgb(0x74b9ff),
        WorkflowNodeKind::Asset => rgb(0x6bc7d9),
        WorkflowNodeKind::Llm => rgb(0xd6adf2),
        WorkflowNodeKind::Agent => pal_rgba(palette, "--openbot-accent", 0x007cf7),
        WorkflowNodeKind::Image => rgb(0xd98ac9),
        WorkflowNodeKind::Condition => pal_rgba(palette, "--openbot-warning", 0xff9412),
        WorkflowNodeKind::Approval => pal_rgba(palette, "--openbot-danger", 0xfd2f3b),
        WorkflowNodeKind::Output => rgb(0x31cf76),
        WorkflowNodeKind::Text => pal_rgba(palette, "--openbot-text-secondary", 0xdcdcdc),
        WorkflowNodeKind::Note => rgb(0xe5a50a),
        WorkflowNodeKind::Shape => rgb(0x8688ff),
    }
}

fn node_kind_icon(kind: WorkflowNodeKind) -> IconName {
    match kind {
        WorkflowNodeKind::Trigger => IconName::Play,
        WorkflowNodeKind::Prompt => IconName::ComposePen,
        WorkflowNodeKind::Asset => IconName::Paperclip,
        WorkflowNodeKind::Llm => IconName::PremiumStar,
        WorkflowNodeKind::Agent => IconName::BotRobot,
        WorkflowNodeKind::Image => IconName::Camera,
        WorkflowNodeKind::Condition => IconName::Poll,
        WorkflowNodeKind::Approval => IconName::Lock,
        WorkflowNodeKind::Output => IconName::SendPlane,
        WorkflowNodeKind::Text => IconName::Edit,
        WorkflowNodeKind::Note => IconName::Pin,
        WorkflowNodeKind::Shape => IconName::FolderWork,
    }
}

fn step_status_color(status: WorkflowStepStatus, palette: &OpenBotPalette) -> Rgba {
    match status {
        WorkflowStepStatus::Running => pal_rgba(palette, "--openbot-accent", 0x007cf7),
        WorkflowStepStatus::Approval => pal_rgba(palette, "--openbot-warning", 0xff9412),
        WorkflowStepStatus::Done => pal_rgba(palette, "--openbot-success", 0x31cf76),
        WorkflowStepStatus::Failed => pal_rgba(palette, "--openbot-danger", 0xfd2f3b),
        WorkflowStepStatus::Stopped => pal_rgba(palette, "--openbot-text-muted", 0x979797),
        WorkflowStepStatus::Skipped => pal_rgba(palette, "--openbot-text-dim", 0x6a6a6a),
    }
}

fn paint_workflow_edges_and_strokes(
    bounds: Bounds<Pixels>,
    state: &WorkflowCanvasState,
    loop_set: &BTreeSet<String>,
    palette: &OpenBotPalette,
    window: &mut Window,
) {
    let ox = f32::from(bounds.origin.x);
    let oy = f32::from(bounds.origin.y);
    let bw = f32::from(bounds.size.width);
    let bh = f32::from(bounds.size.height);
    if !bw.is_finite() || !bh.is_finite() || bw <= 2.0 || bh <= 2.0 {
        return;
    }

    // 1. Pan/zoom dot-grid background
    let step = (28.0 * state.zoom).clamp(12.0, 64.0);
    let dot_color: Hsla = pal_rgba(palette, "--openbot-border-strong", 0x333335).into();
    let start_x = ((state.pan_x % step) + step) % step;
    let start_y = ((state.pan_y % step) + step) % step;
    let mut grid_builder = PathBuilder::fill();
    let mut gx = start_x;
    while gx < bw {
        let mut gy = start_y;
        while gy < bh {
            let x = ox + gx;
            let y = oy + gy;
            grid_builder.move_to(point(px(x), px(y)));
            grid_builder.line_to(point(px(x + 1.5), px(y)));
            grid_builder.line_to(point(px(x + 1.5), px(y + 1.5)));
            grid_builder.line_to(point(px(x), px(y + 1.5)));
            grid_builder.close();
            gy += step;
        }
        gx += step;
    }
    if let Ok(path) = grid_builder.build() {
        window.paint_path(path, dot_color.opacity(0.65));
    }

    // 2. Group frames
    for frame in &state.group_frames {
        let fx = ox + (frame.x as f32) * state.zoom + state.pan_x;
        let fy = oy + (frame.y as f32) * state.zoom + state.pan_y;
        let fw = (frame.width as f32) * state.zoom;
        let fh = (frame.height as f32) * state.zoom;
        let tint: Hsla = parse_hex_rgb(&frame.color_hex, 0x007cf7).into();
        let mut fb = PathBuilder::fill();
        fb.move_to(point(px(fx), px(fy)));
        fb.line_to(point(px(fx + fw), px(fy)));
        fb.line_to(point(px(fx + fw), px(fy + fh)));
        fb.line_to(point(px(fx), px(fy + fh)));
        fb.close();
        if let Ok(path) = fb.build() {
            window.paint_path(path, tint.opacity(0.08));
        }
    }

    // 3. Cubic Bezier typed-port edges (loop edges highlighted in warning/danger)
    let node_map: HashMap<&str, &WorkflowNode> =
        state.graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let warn_color: Hsla = pal_rgba(palette, "--openbot-warning", 0xff9412).into();
    let danger_color: Hsla = pal_rgba(palette, "--openbot-danger", 0xfd2f3b).into();
    let success_color: Hsla = pal_rgba(palette, "--openbot-success", 0x31cf76).into();

    for edge in &state.graph.edges {
        let (Some(src), Some(dst)) = (
            node_map.get(edge.from.as_str()),
            node_map.get(edge.to.as_str()),
        ) else {
            continue;
        };
        let (sw, sh) = node_dimensions(src);
        let (_, dh) = node_dimensions(dst);
        let src_y_frac = match edge.branch {
            Some(WorkflowBranch::Yes) => 0.36,
            Some(WorkflowBranch::No) => 0.72,
            None => 0.52,
        };
        let sx = ox + (src.at.x as f32 + sw) * state.zoom + state.pan_x;
        let sy = oy + (src.at.y as f32 + sh * src_y_frac) * state.zoom + state.pan_y;
        let tx = ox + (dst.at.x as f32) * state.zoom + state.pan_x;
        let ty = oy + (dst.at.y as f32 + dh * 0.52) * state.zoom + state.pan_y;
        let is_loop = loop_set.contains(&edge.id);
        let dx = ((tx - sx).abs() * 0.5).max(52.0 * state.zoom);
        let (c1, c2) = if is_loop {
            let arch = 88.0 * state.zoom;
            (
                point(px(sx + dx), px(sy - arch)),
                point(px(tx - dx), px(ty - arch)),
            )
        } else {
            (point(px(sx + dx), px(sy)), point(px(tx - dx), px(ty)))
        };
        let stroke_w = if is_loop { 3.0 } else { 2.0 } * state.zoom.clamp(0.6, 1.5);
        let edge_color: Hsla = if is_loop {
            warn_color
        } else {
            match edge.branch {
                Some(WorkflowBranch::Yes) => success_color,
                Some(WorkflowBranch::No) => danger_color,
                None => port_color(edge.port_type, palette).into(),
            }
        };
        let mut eb = PathBuilder::stroke(px(stroke_w));
        eb.move_to(point(px(sx), px(sy)));
        // GPUI 0.2.2 `cubic_bezier_to(to, ctrl1, ctrl2)` takes destination first.
        eb.cubic_bezier_to(point(px(tx), px(ty)), c1, c2);
        if let Ok(path) = eb.build() {
            window.paint_path(path, edge_color.opacity(if is_loop { 0.95 } else { 0.82 }));
        }
    }

    // 4. Freehand ink strokes
    for stroke in &state.graph.strokes {
        if stroke.points.len() < 2 {
            continue;
        }
        let width_px = ((stroke.width as f32) * state.zoom).clamp(1.0, 28.0);
        let ink_base: Hsla = parse_hex_rgb(&stroke.color, 0x42a0ff).into();
        let alpha = if stroke.brush.eq_ignore_ascii_case("highlighter") {
            0.35
        } else {
            0.9
        };
        let mut sb = PathBuilder::stroke(px(width_px));
        for (idx, pt) in stroke.points.iter().enumerate() {
            let p = point(
                px(ox + (pt.x as f32) * state.zoom + state.pan_x),
                px(oy + (pt.y as f32) * state.zoom + state.pan_y),
            );
            if idx == 0 {
                sb.move_to(p);
            } else {
                sb.line_to(p);
            }
        }
        if let Ok(path) = sb.build() {
            window.paint_path(path, ink_base.opacity(alpha));
        }
    }
}

fn paint_workflow_minimap(
    bounds: Bounds<Pixels>,
    state: &WorkflowCanvasState,
    loop_set: &BTreeSet<String>,
    palette: &OpenBotPalette,
    window: &mut Window,
) {
    let ox = f32::from(bounds.origin.x);
    let oy = f32::from(bounds.origin.y);
    let bw = f32::from(bounds.size.width).max(1.0);
    let bh = f32::from(bounds.size.height).max(1.0);
    let (min_x, min_y, max_x, max_y) = state
        .content_bounds()
        .unwrap_or((0.0, 0.0, 1200.0, 720.0));
    let span_x = (max_x - min_x + 120.0).max(400.0);
    let span_y = (max_y - min_y + 120.0).max(300.0);
    let scale = ((bw - 16.0) / span_x).min((bh - 16.0) / span_y).max(0.01);
    let pad_x = ox + (bw - span_x * scale) * 0.5;
    let pad_y = oy + (bh - span_y * scale) * 0.5;

    let map_pt = |wx: f32, wy: f32| -> (f32, f32) {
        (
            pad_x + (wx - min_x + 60.0) * scale,
            pad_y + (wy - min_y + 60.0) * scale,
        )
    };

    let node_map: HashMap<&str, &WorkflowNode> =
        state.graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let warn_c: Hsla = pal_rgba(palette, "--openbot-warning", 0xff9412).into();
    let muted_c: Hsla = pal_rgba(palette, "--openbot-text-muted", 0x979797).into();

    for edge in &state.graph.edges {
        let (Some(src), Some(dst)) = (
            node_map.get(edge.from.as_str()),
            node_map.get(edge.to.as_str()),
        ) else {
            continue;
        };
        let (sw, sh) = node_dimensions(src);
        let (_, dh) = node_dimensions(dst);
        let (x1, y1) = map_pt(src.at.x as f32 + sw, src.at.y as f32 + sh * 0.5);
        let (x2, y2) = map_pt(dst.at.x as f32, dst.at.y as f32 + dh * 0.5);
        let mut eb = PathBuilder::stroke(px(if loop_set.contains(&edge.id) { 1.6 } else { 1.0 }));
        eb.move_to(point(px(x1), px(y1)));
        eb.line_to(point(px(x2), px(y2)));
        if let Ok(path) = eb.build() {
            window.paint_path(
                path,
                if loop_set.contains(&edge.id) {
                    warn_c.opacity(0.95)
                } else {
                    muted_c.opacity(0.55)
                },
            );
        }
    }

    for node in &state.graph.nodes {
        let (nw, nh) = node_dimensions(node);
        let (x0, y0) = map_pt(node.at.x as f32, node.at.y as f32);
        let rw = (nw * scale).max(6.0);
        let rh = (nh * scale).max(4.0);
        let color: Hsla = node_kind_accent(node.kind, palette).into();
        let mut nb = PathBuilder::fill();
        nb.move_to(point(px(x0), px(y0)));
        nb.line_to(point(px(x0 + rw), px(y0)));
        nb.line_to(point(px(x0 + rw), px(y0 + rh)));
        nb.line_to(point(px(x0), px(y0 + rh)));
        nb.close();
        if let Ok(path) = nb.build() {
            window.paint_path(path, color.opacity(0.85));
        }
    }

    // Viewport lens box
    let vp_wx = -state.pan_x / state.zoom.max(0.05);
    let vp_wy = -state.pan_y / state.zoom.max(0.05);
    let vp_ww = 960.0 / state.zoom.max(0.05);
    let vp_wh = 600.0 / state.zoom.max(0.05);
    let (vx0, vy0) = map_pt(vp_wx, vp_wy);
    let vw = (vp_ww * scale).clamp(16.0, bw);
    let vh = (vp_wh * scale).clamp(12.0, bh);
    let accent: Hsla = pal_rgba(palette, "--openbot-accent", 0x007cf7).into();
    let mut vb = PathBuilder::stroke(px(1.2));
    vb.move_to(point(px(vx0), px(vy0)));
    vb.line_to(point(px(vx0 + vw), px(vy0)));
    vb.line_to(point(px(vx0 + vw), px(vy0 + vh)));
    vb.line_to(point(px(vx0), px(vy0 + vh)));
    vb.close();
    if let Ok(path) = vb.build() {
        window.paint_path(path, accent.opacity(0.8));
    }
}

fn node_summary_text(node: &WorkflowNode) -> String {
    let field_str = |key: &str| -> Option<&str> {
        match node.fields.get(key) {
            Some(WorkflowFieldValue::Text(s)) if !s.trim().is_empty() => Some(s.as_str()),
            _ => None,
        }
    };
    let raw = match node.kind {
        WorkflowNodeKind::Trigger => field_str("event")
            .or_else(|| field_str("trigger"))
            .unwrap_or("Manual trigger"),
        WorkflowNodeKind::Agent => field_str("task")
            .or_else(|| field_str("agentId"))
            .unwrap_or("Assign agent task"),
        WorkflowNodeKind::Llm => field_str("prompt")
            .or_else(|| field_str("model"))
            .unwrap_or("Model completion step"),
        WorkflowNodeKind::Condition => field_str("rule")
            .or_else(|| field_str("check"))
            .unwrap_or("Evaluate branch rule"),
        WorkflowNodeKind::Approval => field_str("question").unwrap_or("Require human sign-off"),
        WorkflowNodeKind::Output => field_str("message")
            .or_else(|| field_str("target"))
            .unwrap_or("Emit workflow output"),
        WorkflowNodeKind::Image => field_str("prompt")
            .or_else(|| field_str("model"))
            .unwrap_or("Generate image or video"),
        WorkflowNodeKind::Asset => field_str("fileId").unwrap_or("Attached media asset"),
        WorkflowNodeKind::Prompt
        | WorkflowNodeKind::Text
        | WorkflowNodeKind::Note
        | WorkflowNodeKind::Shape => field_str("text").unwrap_or("Canvas annotation"),
    };
    if raw.chars().count() > 64 {
        let mut clipped: String = raw.chars().take(61).collect();
        clipped.push_str("...");
        clipped
    } else {
        raw.to_string()
    }
}

fn render_workflow_node_card(
    node: &WorkflowNode,
    state: &WorkflowCanvasState,
    hud_step: Option<&WorkflowHudStep>,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let (nw, nh) = node_dimensions(node);
    let sx = (node.at.x as f32) * state.zoom + state.pan_x;
    let sy = (node.at.y as f32) * state.zoom + state.pan_y;
    let sw = (nw * state.zoom).max(120.0);
    let sh = (nh * state.zoom).max(68.0);
    let selected = state.selected_node_ids.iter().any(|id| id == &node.id);
    let accent = node_kind_accent(node.kind, palette);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let border_default = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let card_bg = match node.kind {
        WorkflowNodeKind::Note => rgb(0x2a220e),
        WorkflowNodeKind::Shape => pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424),
        WorkflowNodeKind::Text => pal_rgba(palette, "--openbot-bg-glass", 0x212121),
        _ => pal_rgba(palette, "--openbot-bg-surface", 0x212121),
    };
    let ports = node.kind.ports();
    let is_dark = palette.scheme == OpenBotColorScheme::Dark;

    let mut header = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .px(px(10.0))
        .py(px(6.0))
        .border_b_1()
        .border_color(border_default);

    let mut header_left = div().flex().items_center().gap(px(6.0));
    if node.kind == WorkflowNodeKind::Agent {
        let mut props = AgentAvatarProps::new(node.agent_id().unwrap_or(node.id.as_str())).with_size(20.0);
        props.dark_scheme = is_dark;
        props.reduced_motion = state.reduced_motion;
        if let Some(step) = hud_step {
            props.mood = match step.status {
                WorkflowStepStatus::Running => AvatarMood::Working,
                WorkflowStepStatus::Approval => AvatarMood::Waiting,
                WorkflowStepStatus::Failed => AvatarMood::Failed,
                WorkflowStepStatus::Done => AvatarMood::Responded,
                _ => AvatarMood::Idle,
            };
        }
        header_left = header_left.child(render_agent_avatar(&props, None, now_sec));
    } else {
        header_left = header_left.child(tg_icon_rgba(node_kind_icon(node.kind), 14.0, accent));
    }
    header_left = header_left.child(
        div()
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(text_primary)
            .child(node_kind_label(node.kind)),
    );
    header = header.child(header_left).child(
        div()
            .text_size(px(10.0))
            .text_color(text_muted)
            .child(node.id.clone()),
    );

    let mut footer = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .px(px(8.0))
        .py(px(5.0));

    let mut in_ports = div().flex().items_center().gap(px(4.0));
    for &p in ports.inputs {
        in_ports = in_ports.child(
            div()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(999.0))
                .border_1()
                .border_color(port_color(p, palette))
                .text_size(px(9.0))
                .text_color(port_color(p, palette))
                .child(p.as_str()),
        );
    }

    let mut out_ports = div().flex().items_center().gap(px(4.0));
    if node.kind == WorkflowNodeKind::Condition {
        out_ports = out_ports
            .child(
                div()
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(999.0))
                    .bg(pal_rgba(palette, "--openbot-success-soft", 0x1b3a27))
                    .text_size(px(9.0))
                    .text_color(pal_rgba(palette, "--openbot-success", 0x31cf76))
                    .child("Yes"),
            )
            .child(
                div()
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(999.0))
                    .bg(pal_rgba(palette, "--openbot-danger-soft", 0x3a1b1f))
                    .text_size(px(9.0))
                    .text_color(pal_rgba(palette, "--openbot-danger", 0xfd2f3b))
                    .child("No"),
            );
    } else {
        for &p in ports.outputs {
            out_ports = out_ports.child(
                div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded(px(999.0))
                    .border_1()
                    .border_color(port_color(p, palette))
                    .text_size(px(9.0))
                    .text_color(port_color(p, palette))
                    .child(p.as_str()),
            );
        }
    }
    footer = footer.child(in_ports).child(out_ports);

    div()
        .absolute()
        .left(px(sx))
        .top(px(sy))
        .w(px(sw))
        .min_h(px(sh))
        .rounded(px(14.0))
        .bg(card_bg)
        .border_1()
        .border_color(if selected { accent } else { border_default })
        .flex()
        .flex_col()
        .justify_between()
        .child(header)
        .child(
            div()
                .px(px(10.0))
                .py(px(6.0))
                .text_size(px(12.0))
                .text_color(pal_rgba(palette, "--openbot-text-secondary", 0xdcdcdc))
                .child(node_summary_text(node)),
        )
        .when_some(hud_step, |el, step| {
            let sc = step_status_color(step.status, palette);
            el.child(
                div()
                    .mx(px(8.0))
                    .mb(px(4.0))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(sc)
                    .text_size(px(10.0))
                    .text_color(sc)
                    .child(format!("{:?} · {}ms", step.status, step.elapsed_ms)),
            )
        })
        .child(footer)
}

/// Renders the interactive pan/zoom GPUI `WorkflowCanvas` including Bezier edges,
/// loop-edge highlights, freehand ink strokes, all 12 node kinds, peer cursors,
/// `WorkflowZoomMenu`, `WorkflowVersionBar`, live run HUD, and `WorkflowMinimap`.
pub fn render_workflow_canvas(
    state: &WorkflowCanvasState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let bg_canvas = pal_rgba(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let bg_surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let bg_glass = pal_rgba(palette, "--openbot-bg-glass", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let warning = pal_rgba(palette, "--openbot-warning", 0xff9412);

    let loop_set: BTreeSet<String> = workflow_loop_edges(&state.graph.nodes, &state.graph.edges);
    let hud_step_map: HashMap<&str, &WorkflowHudStep> = state
        .hud_run
        .as_ref()
        .map(|run| run.steps.iter().map(|s| (s.node_id.as_str(), s)).collect())
        .unwrap_or_default();

    // Top toolbar
    let mut tool_strip = div().flex().items_center().gap(px(4.0));
    for tool in WorkflowInkTool::ALL {
        let active = state.active_tool == tool;
        tool_strip = tool_strip.child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .bg(if active { accent } else { bg_control })
                .text_size(px(11.0))
                .text_color(if active {
                    pal_rgba(palette, "--openbot-text-on-accent", 0xffffff)
                } else {
                    text_muted
                })
                .child(tool.label()),
        );
    }

    let mut presence_bar = div().flex().items_center().gap(px(4.0));
    for peer in state.peer_cursors.iter().filter(|p| p.active) {
        let peer_c = parse_hex_rgb(&peer.color_hex, 0x42a0ff);
        presence_bar = presence_bar.child(
            div()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(999.0))
                .border_1()
                .border_color(peer_c)
                .text_size(px(10.0))
                .text_color(peer_c)
                .child(peer.display_name.clone()),
        );
    }

    let toolbar = div()
        .h(px(44.0))
        .px(px(14.0))
        .bg(bg_surface)
        .border_b_1()
        .border_color(border)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(tg_icon_rgba(IconName::FolderWork, 16.0, accent))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .child(state.title.clone()),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .bg(bg_control)
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child(format!(
                            "r{} · {}/{} nodes · {}/{} edges",
                            state.revision,
                            state.graph.nodes.len(),
                            WORKFLOW_MAX_NODES,
                            state.graph.edges.len(),
                            WORKFLOW_MAX_EDGES
                        )),
                )
                .when(!loop_set.is_empty(), |el| {
                    el.child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(6.0))
                            .bg(pal_rgba(palette, "--openbot-warning-soft", 0x3a280a))
                            .text_size(px(11.0))
                            .text_color(warning)
                            .child(format!("{} loop edge(s)", loop_set.len())),
                    )
                }),
        )
        .child(tool_strip)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(presence_bar)
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(8.0))
                        .bg(bg_control)
                        .border_1()
                        .border_color(border)
                        .text_size(px(11.0))
                        .text_color(text_primary)
                        .child(if state.tidy_applied { "Tidy ✓ (Shift+T)" } else { "Tidy (Shift+T)" }),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(8.0))
                        .bg(bg_control)
                        .border_1()
                        .border_color(border)
                        .text_size(px(11.0))
                        .text_color(text_primary)
                        .child("Fit (Shift+1)"),
                ),
        );

    // Main canvas stage
    let paint_state = state.clone();
    let paint_loops = loop_set.clone();
    let paint_palette = palette.clone();
    let mut stage = div()
        .relative()
        .flex_1()
        .min_h(px(0.0))
        .overflow_hidden()
        .bg(bg_canvas)
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    paint_workflow_edges_and_strokes(
                        bounds,
                        &paint_state,
                        &paint_loops,
                        &paint_palette,
                        window,
                    );
                },
            )
            .size_full(),
        );

    for node in &state.graph.nodes {
        let step = hud_step_map.get(node.id.as_str()).copied();
        stage = stage.child(render_workflow_node_card(node, state, step, palette, now_sec));
    }

    for peer in state.peer_cursors.iter().filter(|p| p.active) {
        let px_x = (peer.x as f32) * state.zoom + state.pan_x;
        let px_y = (peer.y as f32) * state.zoom + state.pan_y;
        let peer_c = parse_hex_rgb(&peer.color_hex, 0x42a0ff);
        stage = stage.child(
            div()
                .absolute()
                .left(px(px_x))
                .top(px(px_y))
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(999.0))
                .bg(peer_c)
                .text_size(px(10.0))
                .text_color(rgb(0x141414))
                .child(peer.display_name.clone()),
        );
    }

    // Bottom-left WorkflowZoomMenu
    let mut zoom_menu = div()
        .absolute()
        .left(px(14.0))
        .bottom(px(14.0))
        .p(px(8.0))
        .rounded(px(12.0))
        .bg(bg_glass)
        .border_1()
        .border_color(border)
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_primary)
                .child(format!("{}%", (state.zoom * 100.0).round() as u32)),
        );
    if state.zoom_menu_open {
        for preset in WORKFLOW_ZOOM_PRESETS {
            zoom_menu = zoom_menu.child(
                div()
                    .px(px(5.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .bg(if (preset - state.zoom).abs() < 0.01 { accent } else { bg_control })
                    .text_size(px(10.0))
                    .text_color(text_primary)
                    .child(format!("{}%", (preset * 100.0).round() as u32)),
            );
        }
    }
    stage = stage.child(zoom_menu);

    // Top-right Live Run HUD
    if state.hud_visible {
        let mut hud_box = div()
            .absolute()
            .right(px(14.0))
            .top(px(14.0))
            .w(px(240.0))
            .p(px(10.0))
            .rounded(px(14.0))
            .bg(bg_glass)
            .border_1()
            .border_color(border)
            .flex()
            .flex_col()
            .gap(px(6.0));
        match &state.hud_run {
            Some(run) => {
                hud_box = hud_box.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .child(format!("Run {} · {:?}", run.run_id, run.status)),
                        ),
                );
                for step in &run.steps {
                    let sc = step_status_color(step.status, palette);
                    hud_box = hud_box.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(11.0))
                            .child(div().text_color(text_primary).child(step.label.clone()))
                            .child(div().text_color(sc).child(format!("{:?}", step.status))),
                    );
                }
            }
            None => {
                hud_box = hud_box.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child("Live Run HUD · Ready"),
                );
            }
        }
        stage = stage.child(hud_box);
    }

    // Bottom-right WorkflowMinimap canvas
    if state.minimap_visible {
        let mm_state = state.clone();
        let mm_loops = loop_set;
        let mm_palette = palette.clone();
        stage = stage.child(
            div()
                .absolute()
                .right(px(14.0))
                .bottom(px(14.0))
                .w(px(180.0))
                .h(px(112.0))
                .rounded(px(12.0))
                .bg(bg_glass)
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _cx| {
                            paint_workflow_minimap(bounds, &mm_state, &mm_loops, &mm_palette, window);
                        },
                    )
                    .size_full(),
                ),
        );
    }

    // Optional WorkflowVersionBar strip
    let version_bar = if state.version_bar_open {
        let mut bar = div()
            .h(px(34.0))
            .px(px(14.0))
            .bg(bg_surface)
            .border_t_1()
            .border_color(border)
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().text_size(px(11.0)).text_color(text_muted).child("Versions:"));
        for ver in &state.versions {
            bar = bar.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .bg(bg_control)
                    .text_size(px(10.0))
                    .text_color(text_primary)
                    .child(format!("r{} ({:?})", ver.revision, ver.kind)),
            );
        }
        Some(bar)
    } else {
        None
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(bg_canvas)
        .child(toolbar)
        .child(stage)
        .when_some(version_bar, |el, bar| el.child(bar))
}
