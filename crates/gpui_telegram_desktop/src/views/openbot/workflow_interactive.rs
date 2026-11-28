//! Full 1:1 OpenBot Interactive `WorkflowsPage` & `WorkflowCanvas` Studio Controller + GPUI Renderer.
//!
//! Mirrors `packages/ui/src/features/workflows/`:
//! - `WorkflowsEmpty.tsx` (ambient DAG backdrop + pitch + `+ 新建工作流` CTA when 0 workflows exist)
//! - `workflow-start.tsx` (5 OpenBot starter templates with mini DAG previews + `+ 空白画布`)
//! - `WorkflowCorner` & `WorkflowSwitcher.tsx` (folders, workflows list, new, import, versions, rename, move to folder, pause/resume, duplicate, export, delete)
//! - `WorkflowTabs.tsx` (multi-workflow tab bar with Waypoints icon, close `×`, middle-click close, `+`)
//! - `WorkflowCanvasHud.tsx` (`第 N 次运行`, `已暂停 · ▶`, live status pill, `历史 [N]`, `▶ 运行工作流`)
//! - `WorkflowCanvas.tsx` (dot-grid pan/zoom canvas, drag-to-move nodes, port-to-port Bezier edge wiring, freehand Draw/Marker/Eraser, floating bottom tool dock + sub-toolbars, `WorkflowAddMenu`, `WorkflowSearch`, `WorkflowZoomMenu`, clickable `WorkflowMinimap`, `WorkflowVersionBar`, `WorkflowDeleteUndoToast`, selected-node inspector, and live SQLite `openbot.db` persistence).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::Path;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::AvatarMood;
use openbot_contracts::routines::RoutineSchedule;
use openbot_contracts::workflows::{
    WORKFLOW_FILE_EXTENSION, WorkflowBranch, WorkflowDetail, WorkflowEdge, WorkflowFieldValue,
    WorkflowGraph, WorkflowNode, WorkflowNodeKind, WorkflowPoint, WorkflowPortType,
    WorkflowRunStatus, WorkflowStepStatus, WorkflowStroke, WorkflowSummary,
    workflow_loop_edges,
};
use openbot_runtime::OpenBotHostRuntime;
use openbot_runtime::platform::workflows::{WorkflowRunRequest, WorkflowSaveRequest};
use openbot_storage::{CreateWorkflowInput, WorkflowValuesMode};
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{AgentAvatarProps, render_agent_avatar};
use crate::i18n::tr_str;
use crate::theme::openbot_palette::{OpenBotColorScheme, OpenBotPalette};
use crate::theme::{IconName, tg_icon_rgba};

use super::shell_state::write_json_atomic;
use super::workflow_canvas::{
    DeletedWorkflowUndo, WORKFLOW_ZOOM_MAX, WORKFLOW_ZOOM_MIN, WORKFLOW_ZOOM_PRESETS,
    WorkflowCanvasState, WorkflowFolderItem, WorkflowHudRun, WorkflowHudStep, WorkflowInkTool,
    WorkflowPastRunSummary, WorkflowPointerDrag, WorkflowPortConnectStart, WorkflowSubToolPanel,
    WorkflowTextInputTarget, WorkflowVersionEntry, node_dimensions, node_kind_accent,
    node_kind_icon, node_summary_text, paint_workflow_edges_and_strokes, paint_workflow_minimap,
    pal_rgba, parse_hex_rgb, port_color, step_status_color,
};

pub const WORKFLOW_FOLDERS_FILE: &str = "openbot-workflow-folders-v1.json";

const INK_SWATCHES: [&str; 6] = [
    "#42a0ff", "#31cf76", "#ff9412", "#fd2f3b", "#b474f4", "#e5e5e5",
];
const NOTE_TONES: [&str; 5] = ["#e5a50a", "#31cf76", "#42a0ff", "#d98ac9", "#fd2f3b"];
const SHAPE_KINDS: [(&str, &str, &str); 3] = [
    ("rect", "矩形", "Rectangle"),
    ("pill", "圆角", "Rounded"),
    ("diamond", "菱形", "Diamond"),
];

/// Persisted folder structure matching OpenBot `WorkflowFoldersSnapshot`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowFoldersDisk {
    version: u32,
    folders: Vec<WorkflowFolderItem>,
    placement: BTreeMap<String, String>,
}

/// Interactive UI action emitted by the OpenBot `WorkflowCanvas` surface.
#[derive(Debug, Clone)]
pub enum WorkflowUiAction {
    // Library & Tabs
    ToggleSwitcher,
    SelectWorkflow(String),
    CreateNewWorkflow,
    ApplyTemplate(&'static str),
    StartBlankCanvas,
    CloseTab(String),
    StartRenameWorkflow,
    TogglePaused,
    DuplicateCurrent,
    ExportCurrent,
    OpenImportDialog,
    CloseImportDialog,
    ConfirmImportJson,
    DeleteCurrentWorkflow,
    UndoDeleteWorkflow,
    DismissDeleteUndo,
    DismissToast,
    // Folders
    ToggleFolderExpand(String),
    ToggleMoveSubmenu,
    ToggleVersionsSubmenu,
    StartNewFolder,
    StartRenameFolder(String),
    DeleteFolder(String),
    MoveWorkflowToFolder(Option<String>),
    // Versions
    PreviewVersion(String),
    CloseVersionPreview,
    RestorePreviewVersion,
    ToggleVersionBar,
    // Execution & History
    RunWorkflow {
        from_node_id: Option<String>,
    },
    DecideApproval {
        run_id: String,
        node_id: String,
        approve: bool,
    },
    ToggleRunsMenu,
    ViewRun(Option<u64>),
    FocusNode(String),
    // Canvas Tools & Nodes
    SetTool(WorkflowInkTool),
    ToggleSubToolPanel(WorkflowSubToolPanel),
    SetInkColor(String),
    SetNoteTone(String),
    SetShapeKind(String),
    SetShapeFill(String),
    ToggleAddMenu,
    AddNode {
        kind: WorkflowNodeKind,
        preset: Option<&'static str>,
    },
    SelectNode {
        node_id: String,
        additive: bool,
    },
    ClearSelection,
    DuplicateSelected,
    ToggleLockSelected,
    DeleteSelected,
    DisconnectNodeEdges(String),
    StartConnectFrom {
        node_id: String,
        port_type: WorkflowPortType,
        branch: Option<WorkflowBranch>,
    },
    CompleteConnectTo {
        target_node_id: String,
    },
    CancelConnect,
    StartEditNodeField {
        node_id: String,
        field_key: String,
    },
    SetNodeFieldValue {
        node_id: String,
        field_key: String,
        value: String,
    },
    CommitTextInput,
    CancelTextInput,
    // View & Layout
    ZoomIn,
    ZoomOut,
    ZoomTo(f32),
    ToggleZoomMenu,
    FitToView,
    TidyLayout,
    ToggleMinimap,
    ToggleHud,
    Undo,
    Redo,
    ToggleSearch,
    PanBy(f32, f32),
}

/// Static descriptor for the 5 authentic OpenBot starter workflow templates (`workflow-start.tsx`).
#[derive(Debug, Clone, Copy)]
pub struct WorkflowTemplateSpec {
    pub id: &'static str,
    pub title_zh: &'static str,
    pub title_en: &'static str,
    pub brief_zh: &'static str,
    pub brief_en: &'static str,
}

pub const OPENBOT_WORKFLOW_TEMPLATES: [WorkflowTemplateSpec; 5] = [
    WorkflowTemplateSpec {
        id: "morningBrief",
        title_zh: "晨间简报",
        title_en: "Morning brief",
        brief_zh: "每天早上 08:00，获取 5 行你关注主题的新闻。",
        brief_en: "Every morning at 08:00, get 5 lines of news on your topics.",
    },
    WorkflowTemplateSpec {
        id: "answerIssues",
        title_zh: "回复新 Issue",
        title_en: "Answer new issues",
        brief_zh: "有人在 GitHub 上新建 Issue 时，回复它并添加一个标签。",
        brief_en: "When someone opens a GitHub issue, reply to it and add a label.",
    },
    WorkflowTemplateSpec {
        id: "weeklyReport",
        title_zh: "周报",
        title_en: "Weekly report",
        brief_zh: "每周五 16:00 总结本周。发送前由你批准。",
        brief_en: "Every Friday at 16:00, sum up the week. You approve it before it is sent.",
    },
    WorkflowTemplateSpec {
        id: "priceWatch",
        title_zh: "价格监控",
        title_en: "Price watch",
        brief_zh: "每天早上 09:00 检查一个商品的价格。低于你的上限时收到通知。",
        brief_en: "Every morning at 09:00, check a product price and notify when lower.",
    },
    WorkflowTemplateSpec {
        id: "socialPost",
        title_zh: "带图片的帖子",
        title_en: "Social post",
        brief_zh: "写下一个想法，得到一条简短的帖子和一张方形图片。",
        brief_en: "Write down an idea and get a short post plus a square image.",
    },
];

fn localized_kind_label(kind: WorkflowNodeKind, lang: &str) -> &'static str {
    match kind {
        WorkflowNodeKind::Trigger => tr_str(lang, "触发器", "Trigger"),
        WorkflowNodeKind::Prompt => tr_str(lang, "提示词", "Prompt"),
        WorkflowNodeKind::Asset => tr_str(lang, "素材", "Asset"),
        WorkflowNodeKind::Llm => tr_str(lang, "大模型", "LLM"),
        WorkflowNodeKind::Agent => tr_str(lang, "智能体", "Agent"),
        WorkflowNodeKind::Image => tr_str(lang, "图片 / 视频", "Image / Video"),
        WorkflowNodeKind::Condition => tr_str(lang, "条件判断", "Condition"),
        WorkflowNodeKind::Approval => tr_str(lang, "人工审批", "Approval"),
        WorkflowNodeKind::Output => tr_str(lang, "输出交付", "Output"),
        WorkflowNodeKind::Text => tr_str(lang, "文本", "Text"),
        WorkflowNodeKind::Note => tr_str(lang, "便签", "Sticky Note"),
        WorkflowNodeKind::Shape => tr_str(lang, "形状", "Shape"),
    }
}

fn primary_editable_field(kind: WorkflowNodeKind) -> (&'static str, &'static str, &'static str) {
    match kind {
        WorkflowNodeKind::Trigger => ("text", "触发说明", "Trigger note"),
        WorkflowNodeKind::Prompt => ("text", "提示词内容", "Prompt text"),
        WorkflowNodeKind::Asset => ("fileId", "文件 ID", "File ID"),
        WorkflowNodeKind::Llm => ("prompt", "提示词", "Prompt"),
        WorkflowNodeKind::Agent => ("task", "任务指令", "Task instruction"),
        WorkflowNodeKind::Image => ("prompt", "生成描述", "Media prompt"),
        WorkflowNodeKind::Condition => ("rule", "判断规则", "Condition rule"),
        WorkflowNodeKind::Approval => ("question", "审批问题", "Approval question"),
        WorkflowNodeKind::Output => ("message", "输出内容", "Output message"),
        WorkflowNodeKind::Text => ("text", "文本内容", "Text content"),
        WorkflowNodeKind::Note => ("text", "便签内容", "Note content"),
        WorkflowNodeKind::Shape => ("text", "形状文字", "Shape label"),
    }
}

fn make_fields(pairs: &[(&str, &str)]) -> BTreeMap<String, WorkflowFieldValue> {
    let mut map = BTreeMap::new();
    for &(k, v) in pairs {
        map.insert(k.to_string(), WorkflowFieldValue::Text(v.to_string()));
    }
    map
}

fn edge(from: &str, to: &str, branch: Option<WorkflowBranch>) -> WorkflowEdge {
    let id = match branch {
        Some(b) => format!("{from}.{}>{to}:text", b.as_str()),
        None => format!("{from}>{to}:text"),
    };
    WorkflowEdge {
        id,
        from: from.to_string(),
        to: to.to_string(),
        port_type: WorkflowPortType::Text,
        branch,
    }
}

/// Builds the exact OpenBot starter template graph for `template_id` (`workflow-start.tsx`).
pub fn build_openbot_template_graph(
    template_id: &str,
    lang: &str,
    default_agent_id: Option<&str>,
) -> Option<(String, WorkflowGraph)> {
    let is_zh = lang.starts_with("zh");
    let agent_id = default_agent_id.unwrap_or("default-agent");
    match template_id {
        "morningBrief" => {
            let title = if is_zh { "晨间简报" } else { "Morning brief" }.to_string();
            let task = if is_zh {
                "查找过去一天关于 AI、初创公司的新闻。写出最重要的 5 条，每条一行并附链接。"
            } else {
                "Find news from the past day on AI and startups. Write the 5 most important lines with links."
            };
            let graph = WorkflowGraph {
                nodes: vec![
                    WorkflowNode {
                        id: "start".into(),
                        kind: WorkflowNodeKind::Trigger,
                        at: WorkflowPoint { x: 80.0, y: 150.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("trigger", "schedule")]),
                        schedule: Some(RoutineSchedule::Daily {
                            time: "08:00".to_string(),
                        }),
                    },
                    WorkflowNode {
                        id: "collect".into(),
                        kind: WorkflowNodeKind::Agent,
                        at: WorkflowPoint { x: 400.0, y: 140.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("agentId", agent_id), ("task", task)]),
                        schedule: None,
                    },
                    WorkflowNode {
                        id: "send".into(),
                        kind: WorkflowNodeKind::Output,
                        at: WorkflowPoint { x: 720.0, y: 150.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("target", "notify"), ("title", &title)]),
                        schedule: None,
                    },
                ],
                edges: vec![edge("start", "collect", None), edge("collect", "send", None)],
                strokes: Vec::new(),
            };
            Some((title, graph))
        }
        "answerIssues" => {
            let title = if is_zh {
                "回复新 Issue"
            } else {
                "Answer new issues"
            }
            .to_string();
            let task = if is_zh {
                "阅读这个 Issue。在代码中找出可能的原因。写一条简短、友好的回复，并选择一个标签。"
            } else {
                "Read this issue, identify likely causes in code, draft a concise friendly reply, and pick a label."
            };
            let graph = WorkflowGraph {
                nodes: vec![
                    WorkflowNode {
                        id: "issue".into(),
                        kind: WorkflowNodeKind::Trigger,
                        at: WorkflowPoint { x: 80.0, y: 170.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[
                            ("trigger", "github"),
                            ("event", "issue.opened"),
                            ("filters", "notBot"),
                        ]),
                        schedule: None,
                    },
                    WorkflowNode {
                        id: "triage".into(),
                        kind: WorkflowNodeKind::Agent,
                        at: WorkflowPoint { x: 400.0, y: 170.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("agentId", agent_id), ("task", task)]),
                        schedule: None,
                    },
                    WorkflowNode {
                        id: "reply".into(),
                        kind: WorkflowNodeKind::Output,
                        at: WorkflowPoint { x: 740.0, y: 80.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[
                            ("target", "github"),
                            ("action", "comment"),
                            ("item", "event"),
                            ("repository", "trigger"),
                        ]),
                        schedule: None,
                    },
                    WorkflowNode {
                        id: "label".into(),
                        kind: WorkflowNodeKind::Output,
                        at: WorkflowPoint { x: 740.0, y: 260.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[
                            ("target", "github"),
                            ("action", "label.add"),
                            ("item", "event"),
                            ("repository", "trigger"),
                            ("labelsFrom", "agent"),
                            ("labels", "bug\nquestion\nenhancement"),
                        ]),
                        schedule: None,
                    },
                ],
                edges: vec![
                    edge("issue", "triage", None),
                    edge("triage", "reply", None),
                    edge("triage", "label", None),
                ],
                strokes: Vec::new(),
            };
            Some((title, graph))
        }
        "weeklyReport" => {
            let title = if is_zh { "周报" } else { "Weekly report" }.to_string();
            let task = if is_zh {
                "总结团队本周的工作：已完成的工作、未解决的问题和下一步。使用简短的列表。"
            } else {
                "Summarize the team's week: completed work, open issues, and next steps as a short list."
            };
            let question = if is_zh {
                "发送这份周报吗？"
            } else {
                "Send this weekly report?"
            };
            let graph = WorkflowGraph {
                nodes: vec![
                    WorkflowNode {
                        id: "friday".into(),
                        kind: WorkflowNodeKind::Trigger,
                        at: WorkflowPoint { x: 60.0, y: 160.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("trigger", "schedule")]),
                        schedule: Some(RoutineSchedule::Weekly {
                            weekday: 5,
                            time: "16:00".to_string(),
                        }),
                    },
                    WorkflowNode {
                        id: "sum".into(),
                        kind: WorkflowNodeKind::Agent,
                        at: WorkflowPoint { x: 360.0, y: 160.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("agentId", agent_id), ("task", task)]),
                        schedule: None,
                    },
                    WorkflowNode {
                        id: "check".into(),
                        kind: WorkflowNodeKind::Approval,
                        at: WorkflowPoint { x: 660.0, y: 160.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("question", question)]),
                        schedule: None,
                    },
                    WorkflowNode {
                        id: "post".into(),
                        kind: WorkflowNodeKind::Output,
                        at: WorkflowPoint { x: 960.0, y: 160.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("target", "notify"), ("title", &title)]),
                        schedule: None,
                    },
                ],
                edges: vec![
                    edge("friday", "sum", None),
                    edge("sum", "check", None),
                    edge("check", "post", None),
                ],
                strokes: Vec::new(),
            };
            Some((title, graph))
        }
        "priceWatch" => {
            let title = if is_zh { "价格监控" } else { "Price watch" }.to_string();
            let task = if is_zh {
                "打开商品页面并读取最新价格：https://example.com/product。只输出价格和链接。"
            } else {
                "Open the product page and read its current price: https://example.com/product."
            };
            let rule = if is_zh {
                "价格低于 100。"
            } else {
                "Price is below 100."
            };
            let alert = if is_zh { "降价了" } else { "Price dropped" };
            let graph = WorkflowGraph {
                nodes: vec![
                    WorkflowNode {
                        id: "morning".into(),
                        kind: WorkflowNodeKind::Trigger,
                        at: WorkflowPoint { x: 60.0, y: 160.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("trigger", "schedule")]),
                        schedule: Some(RoutineSchedule::Daily {
                            time: "09:00".to_string(),
                        }),
                    },
                    WorkflowNode {
                        id: "read".into(),
                        kind: WorkflowNodeKind::Agent,
                        at: WorkflowPoint { x: 360.0, y: 160.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("agentId", agent_id), ("task", task)]),
                        schedule: None,
                    },
                    WorkflowNode {
                        id: "lower".into(),
                        kind: WorkflowNodeKind::Condition,
                        at: WorkflowPoint { x: 660.0, y: 160.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("rule", rule)]),
                        schedule: None,
                    },
                    WorkflowNode {
                        id: "alert".into(),
                        kind: WorkflowNodeKind::Output,
                        at: WorkflowPoint { x: 960.0, y: 160.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("target", "notify"), ("title", alert)]),
                        schedule: None,
                    },
                ],
                edges: vec![
                    edge("morning", "read", None),
                    edge("read", "lower", None),
                    edge("lower", "alert", Some(WorkflowBranch::Yes)),
                ],
                strokes: Vec::new(),
            };
            Some((title, graph))
        }
        "socialPost" => {
            let title = if is_zh {
                "带图片的帖子"
            } else {
                "Social post"
            }
            .to_string();
            let idea = if is_zh {
                "我们现在周日也营业，时间为 10:00 到 16:00。"
            } else {
                "We are now open on Sundays from 10:00 to 16:00."
            };
            let prompt = if is_zh {
                "就这条消息写一条不超过 280 个字符的社交媒体帖子。第一行要清楚，不要使用话题标签。"
            } else {
                "Write a social media post under 280 characters for this update. Clear first line, no hashtags."
            };
            let picture = if is_zh {
                "为这条帖子配一张简洁明亮的图片。"
            } else {
                "Create a clean, bright square image to pair with this post."
            };
            let graph = WorkflowGraph {
                nodes: vec![
                    WorkflowNode {
                        id: "idea".into(),
                        kind: WorkflowNodeKind::Prompt,
                        at: WorkflowPoint { x: 80.0, y: 180.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("text", idea)]),
                        schedule: None,
                    },
                    WorkflowNode {
                        id: "write".into(),
                        kind: WorkflowNodeKind::Llm,
                        at: WorkflowPoint { x: 420.0, y: 80.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[("prompt", prompt)]),
                        schedule: None,
                    },
                    WorkflowNode {
                        id: "picture".into(),
                        kind: WorkflowNodeKind::Image,
                        at: WorkflowPoint { x: 420.0, y: 280.0 },
                        size: None,
                        locked: None,
                        fields: make_fields(&[
                            ("media", "image"),
                            ("prompt", picture),
                            ("aspect", "1:1"),
                        ]),
                        schedule: None,
                    },
                ],
                edges: vec![
                    edge("idea", "write", None),
                    edge("idea", "picture", None),
                ],
                strokes: Vec::new(),
            };
            Some((title, graph))
        }
        _ => None,
    }
}

pub fn load_workflow_folders(data_dir: &Path) -> (Vec<WorkflowFolderItem>, BTreeMap<String, String>) {
    let path = data_dir.join(WORKFLOW_FOLDERS_FILE);
    let Ok(bytes) = fs::read(&path) else {
        return (Vec::new(), BTreeMap::new());
    };
    let Ok(disk) = serde_json::from_slice::<WorkflowFoldersDisk>(&bytes) else {
        return (Vec::new(), BTreeMap::new());
    };
    (disk.folders, disk.placement)
}

pub fn save_workflow_folders(
    data_dir: &Path,
    folders: &[WorkflowFolderItem],
    placement: &BTreeMap<String, String>,
) {
    let path = data_dir.join(WORKFLOW_FOLDERS_FILE);
    let payload = WorkflowFoldersDisk {
        version: 1,
        folders: folders.to_vec(),
        placement: placement.clone(),
    };
    let _ = write_json_atomic(&path, &payload);
}

fn hydrate_detail_into_canvas(
    state: &mut WorkflowCanvasState,
    detail: WorkflowDetail,
    rt: &OpenBotHostRuntime,
) {
    state.workflow_id = detail.summary.id.clone();
    state.title = detail.summary.title.clone();
    state.revision = detail.revision;
    state.paused = detail.summary.paused.unwrap_or(false);
    state.graph = detail.graph;
    state.has_active_workflow = true;
    state.selected_node_ids.clear();
    state.selected_edge_ids.clear();
    state.undo_stack.clear();
    state.redo_stack.clear();
    state.preview_version_id = None;
    state.preview_graph = None;
    state.connecting_from = None;
    state.viewing_run_number = None;
    state.pending_approval = None;
    if !state.open_tab_ids.iter().any(|id| id == &state.workflow_id) {
        state.open_tab_ids.push(state.workflow_id.clone());
    }
    refresh_versions_and_runs(state, rt);
}

fn refresh_versions_and_runs(state: &mut WorkflowCanvasState, rt: &OpenBotHostRuntime) {
    if !state.has_active_workflow || state.workflow_id.is_empty() {
        state.versions.clear();
        state.past_runs.clear();
        state.hud_run = None;
        return;
    }
    let wf_store = rt.database().workflow_store();
    if let Ok(vers) = wf_store.versions(&state.workflow_id) {
        state.versions = vers
            .into_iter()
            .map(|v| WorkflowVersionEntry {
                id: v.id,
                revision: v.revision,
                kind: v.kind,
                label: format!("r{}", v.revision),
                created_at: v.at,
            })
            .collect();
    }
    let run_store = rt.database().workflow_run_store();
    if let Ok(page) = run_store.page(&state.workflow_id, None, 20) {
        let node_kinds: HashMap<&str, WorkflowNodeKind> = state
            .graph
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.kind))
            .collect();
        let mut past = Vec::with_capacity(page.runs.len());
        for r in page.runs {
            let steps: Vec<WorkflowHudStep> = r
                .steps
                .into_iter()
                .map(|st| WorkflowHudStep {
                    label: st.node_id.clone(),
                    kind: node_kinds
                        .get(st.node_id.as_str())
                        .copied()
                        .unwrap_or(WorkflowNodeKind::Agent),
                    node_id: st.node_id,
                    status: st.status,
                    branch: st.branch,
                    elapsed_ms: 0,
                    output_preview: st.output,
                    error: st.error,
                })
                .collect();
            past.push(WorkflowPastRunSummary {
                run_id: r.run.id,
                number: r.run.number,
                status: r.run.status,
                started_at: r.run.started_at,
                ended_at: r.run.ended_at,
                steps,
            });
        }
        if let Some(latest) = past.first() {
            state.pending_approval = latest
                .steps
                .iter()
                .find(|s| s.status == WorkflowStepStatus::Approval)
                .map(|s| (latest.run_id.clone(), s.node_id.clone()));
            state.hud_run = Some(WorkflowHudRun {
                run_id: latest.run_id.clone(),
                status: latest.status,
                started_at: latest.started_at.clone(),
                steps: latest.steps.clone(),
            });
        } else {
            state.hud_run = None;
            state.pending_approval = None;
        }
        state.past_runs = past;
    }
}

/// Hydrates `WorkflowCanvasState` from live SQLite `openbot.db` + `workflow-folders.json`.
/// When 0 workflows exist in SQLite, puts the studio cleanly into OpenBot's `WorkflowsEmpty` screen.
pub fn hydrate_workflows_from_runtime(
    state: &mut WorkflowCanvasState,
    rt: &OpenBotHostRuntime,
    data_dir: &Path,
) {
    let (folders, placement) = load_workflow_folders(data_dir);
    state.folders = folders;
    state.folder_placement = placement;
    let summaries = rt.workflows().list_workflows().unwrap_or_default();
    state.library = summaries.clone();
    state.hydrated_from_store = true;
    // Prune closed/deleted workflow IDs from open tabs
    let valid_ids: BTreeSet<&str> = summaries.iter().map(|w| w.id.as_str()).collect();
    state
        .open_tab_ids
        .retain(|id| valid_ids.contains(id.as_str()));

    if summaries.is_empty() {
        state.has_active_workflow = false;
        state.workflow_id.clear();
        state.title.clear();
        state.graph = WorkflowGraph::default();
        state.versions.clear();
        state.past_runs.clear();
        state.hud_run = None;
        state.pending_approval = None;
        return;
    }

    let preferred_id = if state.has_active_workflow && valid_ids.contains(state.workflow_id.as_str()) {
        state.workflow_id.clone()
    } else if let Some(first_tab) = state.open_tab_ids.first() {
        first_tab.clone()
    } else {
        summaries[0].id.clone()
    };

    if let Ok(Some(detail)) = rt.workflows().get_workflow(&preferred_id) {
        hydrate_detail_into_canvas(state, detail, rt);
    }
}

fn persist_graph_to_runtime(
    state: &mut WorkflowCanvasState,
    rt: &OpenBotHostRuntime,
    lang: &str,
) {
    if !state.has_active_workflow || state.workflow_id.is_empty() {
        return;
    }
    let title = if state.title.trim().is_empty() {
        tr_str(lang, "未命名工作流", "Untitled workflow").to_string()
    } else {
        state.title.clone()
    };
    match rt.workflows().save_workflow(WorkflowSaveRequest {
        workflow_id: Some(state.workflow_id.clone()),
        title,
        graph: state.graph.clone(),
        raw_json_bytes: None,
        allow_loops: true,
        base_revision: Some(state.revision),
    }) {
        Ok(outcome) => {
            state.revision = outcome.revision;
            if let Ok(list) = rt.workflows().list_workflows() {
                state.library = list;
            }
            refresh_versions_and_runs(state, rt);
        }
        Err(_) => {
            // Retry without base_revision if revision drifted
            if let Ok(outcome) = rt.workflows().save_workflow(WorkflowSaveRequest {
                workflow_id: Some(state.workflow_id.clone()),
                title: state.title.clone(),
                graph: state.graph.clone(),
                raw_json_bytes: None,
                allow_loops: true,
                base_revision: None,
            }) {
                state.revision = outcome.revision;
                if let Ok(list) = rt.workflows().list_workflows() {
                    state.library = list;
                }
            }
        }
    }
}

fn next_node_id(graph: &WorkflowGraph, prefix: &str) -> String {
    let mut n = 1usize;
    loop {
        let cand = format!("{prefix}-{n}");
        if !graph.nodes.iter().any(|node| node.id == cand) {
            return cand;
        }
        n += 1;
    }
}

fn next_edge_id(graph: &WorkflowGraph, from: &str, to: &str, branch: Option<WorkflowBranch>) -> String {
    let base = match branch {
        Some(b) => format!("{from}.{}>{to}", b.as_str()),
        None => format!("{from}>{to}"),
    };
    if !graph.edges.iter().any(|e| e.id == base) {
        return base;
    }
    let mut i = 2usize;
    loop {
        let cand = format!("{base}-{i}");
        if !graph.edges.iter().any(|e| e.id == cand) {
            return cand;
        }
        i += 1;
    }
}

fn default_node_for_kind(
    graph: &WorkflowGraph,
    kind: WorkflowNodeKind,
    preset: Option<&str>,
    at: WorkflowPoint,
    lang: &str,
    default_agent_id: Option<&str>,
    note_tone: &str,
    shape_kind: &str,
    shape_fill: &str,
) -> WorkflowNode {
    let is_zh = lang.starts_with("zh");
    let prefix = match (kind, preset) {
        (WorkflowNodeKind::Image, Some("video")) => "video",
        (WorkflowNodeKind::Trigger, _) => "trigger",
        (WorkflowNodeKind::Prompt, _) => "prompt",
        (WorkflowNodeKind::Asset, _) => "asset",
        (WorkflowNodeKind::Llm, _) => "llm",
        (WorkflowNodeKind::Agent, _) => "agent",
        (WorkflowNodeKind::Image, _) => "image",
        (WorkflowNodeKind::Condition, _) => "cond",
        (WorkflowNodeKind::Approval, _) => "approve",
        (WorkflowNodeKind::Output, _) => "output",
        (WorkflowNodeKind::Text, _) => "text",
        (WorkflowNodeKind::Note, _) => "note",
        (WorkflowNodeKind::Shape, _) => "shape",
    };
    let fields = match (kind, preset) {
        (WorkflowNodeKind::Trigger, _) => make_fields(&[("trigger", "manual")]),
        (WorkflowNodeKind::Prompt, _) => make_fields(&[(
            "text",
            if is_zh { "在这里输入提示词…" } else { "Write prompt…" },
        )]),
        (WorkflowNodeKind::Asset, _) => make_fields(&[("fileId", "asset-1")]),
        (WorkflowNodeKind::Llm, _) => make_fields(&[(
            "prompt",
            if is_zh {
                "总结上一步输入并输出要点。"
            } else {
                "Summarize the input into key bullets."
            },
        )]),
        (WorkflowNodeKind::Agent, _) => make_fields(&[
            ("agentId", default_agent_id.unwrap_or("default-agent")),
            (
                "task",
                if is_zh {
                    "分析输入并完成执行任务。"
                } else {
                    "Analyze input and execute task."
                },
            ),
        ]),
        (WorkflowNodeKind::Image, Some("video")) => make_fields(&[
            ("media", "video"),
            (
                "prompt",
                if is_zh {
                    "生成一段简短演示视频"
                } else {
                    "Generate a short video clip"
                },
            ),
            ("aspect", "16:9"),
        ]),
        (WorkflowNodeKind::Image, _) => make_fields(&[
            ("media", "image"),
            (
                "prompt",
                if is_zh {
                    "生成一张简洁明亮的配图"
                } else {
                    "Generate a clean bright illustration"
                },
            ),
            ("aspect", "1:1"),
        ]),
        (WorkflowNodeKind::Condition, _) => make_fields(&[(
            "rule",
            if is_zh {
                "结果满足预期条件"
            } else {
                "Output meets condition"
            },
        )]),
        (WorkflowNodeKind::Approval, _) => make_fields(&[(
            "question",
            if is_zh {
                "确认继续执行后续步骤？"
            } else {
                "Approve continuing to next steps?"
            },
        )]),
        (WorkflowNodeKind::Output, _) => make_fields(&[
            ("target", "notify"),
            (
                "message",
                if is_zh { "工作流执行完成" } else { "Workflow completed" },
            ),
        ]),
        (WorkflowNodeKind::Text, _) => make_fields(&[(
            "text",
            if is_zh { "画布标题说明" } else { "Canvas heading" },
        )]),
        (WorkflowNodeKind::Note, _) => make_fields(&[
            (
                "text",
                if is_zh { "便签备注" } else { "Sticky note" },
            ),
            ("tone", note_tone),
        ]),
        (WorkflowNodeKind::Shape, _) => make_fields(&[
            (
                "text",
                if is_zh { "分组区块" } else { "Section" },
            ),
            ("shape", shape_kind),
            ("fill", shape_fill),
        ]),
    };
    WorkflowNode {
        id: next_node_id(graph, prefix),
        kind,
        at,
        size: None,
        locked: None,
        fields,
        schedule: None,
    }
}

fn parse_step_status(raw: &str) -> WorkflowStepStatus {
    match raw.to_ascii_lowercase().as_str() {
        "running" => WorkflowStepStatus::Running,
        "approval" | "awaiting_approval" => WorkflowStepStatus::Approval,
        "done" | "completed" => WorkflowStepStatus::Done,
        "failed" | "error" => WorkflowStepStatus::Failed,
        "stopped" | "cancelled" => WorkflowStepStatus::Stopped,
        "skipped" => WorkflowStepStatus::Skipped,
        _ => WorkflowStepStatus::Done,
    }
}

fn parse_run_status(raw: &str) -> WorkflowRunStatus {
    match raw.to_ascii_lowercase().as_str() {
        "running" => WorkflowRunStatus::Running,
        "awaiting_approval" | "approval" | "waiting" => WorkflowRunStatus::Approval,
        "stopping" => WorkflowRunStatus::Stopping,
        "failed" | "error" => WorkflowRunStatus::Failed,
        "stopped" | "cancelled" => WorkflowRunStatus::Stopped,
        _ => WorkflowRunStatus::Done,
    }
}

/// Executes any `WorkflowUiAction` against `WorkflowCanvasState` and real SQLite `OpenBotHostRuntime`.
pub fn apply_workflow_ui_action(
    state: &mut WorkflowCanvasState,
    action: WorkflowUiAction,
    runtime: Option<&OpenBotHostRuntime>,
    data_dir: Option<&Path>,
    lang: &str,
) {
    let is_zh = lang.starts_with("zh");
    let default_agent_id = state.available_agents.first().map(|(id, _)| id.clone());

    match action {
        WorkflowUiAction::ToggleSwitcher => {
            let next = !state.switcher_open;
            state.close_transient_menus();
            state.switcher_open = next;
        }
        WorkflowUiAction::SelectWorkflow(wf_id) => {
            state.close_transient_menus();
            state.active_text_input = WorkflowTextInputTarget::None;
            if let Some(rt) = runtime {
                if let Ok(Some(detail)) = rt.workflows().get_workflow(&wf_id) {
                    hydrate_detail_into_canvas(state, detail, rt);
                    state.fit_to_view(960.0, 560.0);
                }
            } else if state.workflow_id != wf_id {
                state.workflow_id = wf_id;
                state.has_active_workflow = true;
            }
        }
        WorkflowUiAction::CreateNewWorkflow => {
            state.close_transient_menus();
            state.active_text_input = WorkflowTextInputTarget::None;
            state.deleted_undo = None;
            let idx = state.library.len() + 1;
            let title = if is_zh {
                format!("工作流 #{idx}")
            } else {
                format!("Workflow #{idx}")
            };
            if let Some(rt) = runtime {
                let wf_store = rt.database().workflow_store();
                if let Ok(detail) = wf_store.create(
                    CreateWorkflowInput {
                        title,
                        graph: Some(WorkflowGraph::default()),
                        paused: None,
                    },
                    None,
                    WorkflowValuesMode::New,
                ) {
                    if let Ok(list) = rt.workflows().list_workflows() {
                        state.library = list;
                    }
                    state.dismissed_starter_for = None;
                    hydrate_detail_into_canvas(state, detail, rt);
                }
            } else {
                let new_id = format!("wf-{idx}");
                state.workflow_id = new_id.clone();
                state.title = title.clone();
                state.graph = WorkflowGraph::default();
                state.has_active_workflow = true;
                state.dismissed_starter_for = None;
                state.open_tab_ids.push(new_id.clone());
                state.library.push(WorkflowSummary {
                    id: new_id,
                    title,
                    steps: 0,
                    updated_at: chrono::Utc::now().to_rfc3339(),
                    last_run: None,
                    paused: None,
                });
            }
        }
        WorkflowUiAction::ApplyTemplate(template_id) => {
            if let Some((tmpl_title, tmpl_graph)) =
                build_openbot_template_graph(template_id, lang, default_agent_id.as_deref())
            {
                if !state.has_active_workflow {
                    apply_workflow_ui_action(
                        state,
                        WorkflowUiAction::CreateNewWorkflow,
                        runtime,
                        data_dir,
                        lang,
                    );
                }
                state.push_undo_snapshot();
                state.graph = tmpl_graph;
                state.dismissed_starter_for = Some(state.workflow_id.clone());
                let is_generic_name = state.title.starts_with("工作流 #")
                    || state.title.starts_with("Workflow #")
                    || state.title == "Untitled Workflow"
                    || state.title.trim().is_empty();
                if is_generic_name {
                    state.title = tmpl_title.clone();
                    if let Some(rt) = runtime {
                        let _ = rt
                            .database()
                            .workflow_store()
                            .rename(&state.workflow_id, &tmpl_title, None);
                    }
                }
                state.fit_to_view(960.0, 560.0);
                if let Some(rt) = runtime {
                    persist_graph_to_runtime(state, rt, lang);
                }
            }
        }
        WorkflowUiAction::StartBlankCanvas => {
            state.dismissed_starter_for = Some(state.workflow_id.clone());
        }
        WorkflowUiAction::CloseTab(wf_id) => {
            if let Some(pos) = state.open_tab_ids.iter().position(|id| id == &wf_id) {
                state.open_tab_ids.remove(pos);
                if state.workflow_id == wf_id {
                    if let Some(next_id) = state
                        .open_tab_ids
                        .get(pos.saturating_sub(1))
                        .or_else(|| state.open_tab_ids.first())
                        .cloned()
                    {
                        apply_workflow_ui_action(
                            state,
                            WorkflowUiAction::SelectWorkflow(next_id),
                            runtime,
                            data_dir,
                            lang,
                        );
                    } else if let Some(other) = state
                        .library
                        .iter()
                        .find(|w| w.id != wf_id)
                        .map(|w| w.id.clone())
                    {
                        apply_workflow_ui_action(
                            state,
                            WorkflowUiAction::SelectWorkflow(other),
                            runtime,
                            data_dir,
                            lang,
                        );
                    } else if let Some(only) = state.library.first().map(|w| w.id.clone()) {
                        state.open_tab_ids.push(only);
                    }
                }
            }
        }
        WorkflowUiAction::StartRenameWorkflow => {
            state.close_transient_menus();
            if state.has_active_workflow {
                state.input_buffer = state.title.clone();
                state.active_text_input = WorkflowTextInputTarget::RenameWorkflow;
            }
        }
        WorkflowUiAction::TogglePaused => {
            state.close_transient_menus();
            if state.has_active_workflow {
                let next_paused = !state.paused;
                state.paused = next_paused;
                if let Some(rt) = runtime {
                    let _ = rt.database().workflow_store().set_paused(
                        &state.workflow_id,
                        next_paused,
                        None,
                    );
                    if let Ok(list) = rt.workflows().list_workflows() {
                        state.library = list;
                    }
                }
                state.status_toast = Some(
                    if next_paused {
                        tr_str(lang, "工作流已暂停", "Workflow paused")
                    } else {
                        tr_str(lang, "工作流已恢复", "Workflow resumed")
                    }
                    .to_string(),
                );
            }
        }
        WorkflowUiAction::DuplicateCurrent => {
            state.close_transient_menus();
            if state.has_active_workflow {
                let copy_title = if is_zh {
                    format!("{} 副本", state.title)
                } else {
                    format!("{} Copy", state.title)
                };
                let copy_graph = state.graph.clone();
                if let Some(rt) = runtime {
                    if let Ok(detail) = rt.database().workflow_store().create(
                        CreateWorkflowInput {
                            title: copy_title.clone(),
                            graph: Some(copy_graph),
                            paused: Some(true),
                        },
                        None,
                        WorkflowValuesMode::New,
                    ) {
                        if let Ok(list) = rt.workflows().list_workflows() {
                            state.library = list;
                        }
                        hydrate_detail_into_canvas(state, detail, rt);
                        state.status_toast = Some(
                            tr_str(lang, "已创建工作流副本（暂停状态）", "Duplicated workflow (paused)")
                                .to_string(),
                        );
                    }
                }
            }
        }
        WorkflowUiAction::ExportCurrent => {
            state.close_transient_menus();
            if state.has_active_workflow {
                let payload = state.export_portable_json();
                let safe_name: String = state
                    .title
                    .chars()
                    .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
                    .collect();
                let filename = format!(
                    "{}.{}",
                    if safe_name.trim_matches('-').is_empty() {
                        "workflow"
                    } else {
                        safe_name.trim_matches('-')
                    },
                    WORKFLOW_FILE_EXTENSION
                );
                let out_dir = data_dir
                    .map(|d| d.join("exports"))
                    .unwrap_or_else(std::env::temp_dir);
                let _ = fs::create_dir_all(&out_dir);
                let out_path = out_dir.join(&filename);
                if let Ok(pretty) = serde_json::to_string_pretty(&payload) {
                    if fs::write(&out_path, pretty).is_ok() {
                        state.status_toast = Some(format!(
                            "{}: {}",
                            tr_str(lang, "已导出工作流", "Exported workflow"),
                            out_path.display()
                        ));
                    }
                }
            }
        }
        WorkflowUiAction::OpenImportDialog => {
            state.close_transient_menus();
            state.import_dialog_open = true;
            state.import_error = None;
            state.input_buffer.clear();
            state.active_text_input = WorkflowTextInputTarget::ImportJson;
        }
        WorkflowUiAction::CloseImportDialog => {
            state.import_dialog_open = false;
            state.import_error = None;
            if state.active_text_input == WorkflowTextInputTarget::ImportJson {
                state.active_text_input = WorkflowTextInputTarget::None;
                state.input_buffer.clear();
            }
        }
        WorkflowUiAction::ConfirmImportJson => {
            let raw_text = state.input_buffer.trim().to_string();
            match serde_json::from_str::<serde_json::Value>(&raw_text) {
                Ok(val) => {
                    let mut probe = WorkflowCanvasState::new(tr_str(lang, "导入的工作流", "Imported workflow"));
                    match probe.import_portable_json(&val) {
                        Ok(()) => {
                            if let Some(rt) = runtime {
                                if let Ok(detail) = rt.database().workflow_store().create(
                                    CreateWorkflowInput {
                                        title: probe.title.clone(),
                                        graph: Some(probe.graph.clone()),
                                        paused: Some(true),
                                    },
                                    None,
                                    WorkflowValuesMode::New,
                                ) {
                                    if let Ok(list) = rt.workflows().list_workflows() {
                                        state.library = list;
                                    }
                                    hydrate_detail_into_canvas(state, detail, rt);
                                    state.fit_to_view(960.0, 560.0);
                                }
                            } else {
                                state.title = probe.title;
                                state.graph = probe.graph;
                                state.has_active_workflow = true;
                            }
                            state.import_dialog_open = false;
                            state.import_error = None;
                            state.active_text_input = WorkflowTextInputTarget::None;
                            state.input_buffer.clear();
                            state.status_toast = Some(
                                tr_str(lang, "工作流导入成功", "Workflow imported").to_string(),
                            );
                        }
                        Err(e) => {
                            state.import_error = Some(e);
                        }
                    }
                }
                Err(e) => {
                    state.import_error = Some(format!(
                        "{}: {e}",
                        tr_str(lang, "JSON 解析失败", "Invalid JSON")
                    ));
                }
            }
        }
        WorkflowUiAction::DeleteCurrentWorkflow => {
            state.close_transient_menus();
            if state.has_active_workflow && !state.workflow_id.is_empty() {
                let del_id = state.workflow_id.clone();
                let folder_id = state.folder_placement.remove(&del_id);
                state.deleted_undo = Some(DeletedWorkflowUndo {
                    title: state.title.clone(),
                    graph: state.graph.clone(),
                    paused: state.paused,
                    folder_id,
                });
                state.open_tab_ids.retain(|id| id != &del_id);
                if let Some(rt) = runtime {
                    let _ = rt.workflows().delete_workflow(&del_id);
                    if let Some(dir) = data_dir {
                        save_workflow_folders(dir, &state.folders, &state.folder_placement);
                    }
                    let summaries = rt.workflows().list_workflows().unwrap_or_default();
                    state.library = summaries.clone();
                    if let Some(next_id) = state
                        .open_tab_ids
                        .first()
                        .cloned()
                        .or_else(|| summaries.first().map(|w| w.id.clone()))
                    {
                        if let Ok(Some(detail)) = rt.workflows().get_workflow(&next_id) {
                            hydrate_detail_into_canvas(state, detail, rt);
                        }
                    } else {
                        state.has_active_workflow = false;
                        state.workflow_id.clear();
                        state.title.clear();
                        state.graph = WorkflowGraph::default();
                        state.versions.clear();
                        state.past_runs.clear();
                        state.hud_run = None;
                    }
                } else {
                    state.library.retain(|w| w.id != del_id);
                    state.has_active_workflow = false;
                    state.workflow_id.clear();
                    state.graph = WorkflowGraph::default();
                }
            }
        }
        WorkflowUiAction::UndoDeleteWorkflow => {
            if let Some(undo) = state.deleted_undo.take() {
                if let Some(rt) = runtime {
                    if let Ok(detail) = rt.database().workflow_store().create(
                        CreateWorkflowInput {
                            title: undo.title.clone(),
                            graph: Some(undo.graph),
                            paused: undo.paused.then_some(true),
                        },
                        None,
                        WorkflowValuesMode::New,
                    ) {
                        if let Some(fid) = undo.folder_id {
                            if state.folders.iter().any(|f| f.id == fid) {
                                state
                                    .folder_placement
                                    .insert(detail.summary.id.clone(), fid);
                                if let Some(dir) = data_dir {
                                    save_workflow_folders(
                                        dir,
                                        &state.folders,
                                        &state.folder_placement,
                                    );
                                }
                            }
                        }
                        if let Ok(list) = rt.workflows().list_workflows() {
                            state.library = list;
                        }
                        hydrate_detail_into_canvas(state, detail, rt);
                    }
                } else {
                    state.title = undo.title;
                    state.graph = undo.graph;
                    state.has_active_workflow = true;
                }
            }
        }
        WorkflowUiAction::DismissDeleteUndo => {
            state.deleted_undo = None;
        }
        WorkflowUiAction::DismissToast => {
            state.status_toast = None;
        }
        WorkflowUiAction::ToggleFolderExpand(fid) => {
            if state.switcher_expanded_folder.as_deref() == Some(fid.as_str()) {
                state.switcher_expanded_folder = None;
            } else {
                state.switcher_expanded_folder = Some(fid);
            }
        }
        WorkflowUiAction::ToggleMoveSubmenu => {
            state.switcher_move_open = !state.switcher_move_open;
            if state.switcher_move_open {
                state.switcher_versions_open = false;
            }
        }
        WorkflowUiAction::ToggleVersionsSubmenu => {
            state.switcher_versions_open = !state.switcher_versions_open;
            if state.switcher_versions_open {
                state.switcher_move_open = false;
            }
        }
        WorkflowUiAction::StartNewFolder => {
            state.close_transient_menus();
            state.input_buffer.clear();
            state.active_text_input = WorkflowTextInputTarget::NewFolder;
        }
        WorkflowUiAction::StartRenameFolder(fid) => {
            state.close_transient_menus();
            if let Some(f) = state.folders.iter().find(|f| f.id == fid) {
                state.input_buffer = f.name.clone();
                state.active_text_input = WorkflowTextInputTarget::RenameFolder { folder_id: fid };
            }
        }
        WorkflowUiAction::DeleteFolder(fid) => {
            state.folders.retain(|f| f.id != fid);
            state.folder_placement.retain(|_, v| v != &fid);
            if state.switcher_expanded_folder.as_deref() == Some(fid.as_str()) {
                state.switcher_expanded_folder = None;
            }
            if let Some(dir) = data_dir {
                save_workflow_folders(dir, &state.folders, &state.folder_placement);
            }
        }
        WorkflowUiAction::MoveWorkflowToFolder(folder_opt) => {
            if state.has_active_workflow && !state.workflow_id.is_empty() {
                match folder_opt {
                    Some(fid) => {
                        state
                            .folder_placement
                            .insert(state.workflow_id.clone(), fid);
                    }
                    None => {
                        state.folder_placement.remove(&state.workflow_id);
                    }
                }
                if let Some(dir) = data_dir {
                    save_workflow_folders(dir, &state.folders, &state.folder_placement);
                }
            }
            state.switcher_move_open = false;
            state.switcher_open = false;
        }
        WorkflowUiAction::PreviewVersion(ver_id) => {
            state.close_transient_menus();
            if let Some(rt) = runtime {
                if let Ok(Some(g)) = rt
                    .database()
                    .workflow_store()
                    .version_graph(&state.workflow_id, &ver_id)
                {
                    state.preview_version_id = Some(ver_id);
                    state.preview_graph = Some(g);
                    state.version_bar_open = true;
                }
            }
        }
        WorkflowUiAction::CloseVersionPreview => {
            state.preview_version_id = None;
            state.preview_graph = None;
            state.version_bar_open = false;
        }
        WorkflowUiAction::RestorePreviewVersion => {
            if let Some(ver_id) = state.preview_version_id.clone() {
                if let Some(rt) = runtime {
                    if let Ok(detail) = rt.database().workflow_store().restore_version(
                        &state.workflow_id,
                        &ver_id,
                        None,
                    ) {
                        hydrate_detail_into_canvas(state, detail, rt);
                        if let Ok(list) = rt.workflows().list_workflows() {
                            state.library = list;
                        }
                        state.status_toast = Some(
                            tr_str(lang, "已恢复历史版本", "Restored workflow version").to_string(),
                        );
                    }
                }
            }
            state.preview_version_id = None;
            state.preview_graph = None;
            state.version_bar_open = false;
        }
        WorkflowUiAction::ToggleVersionBar => {
            state.version_bar_open = !state.version_bar_open;
        }
        WorkflowUiAction::RunWorkflow { from_node_id } => {
            state.close_transient_menus();
            if !state.has_active_workflow || state.workflow_id.is_empty() {
                return;
            }
            if let Some(rt) = runtime {
                // Save any pending edits first
                persist_graph_to_runtime(state, rt, lang);
                let payload = from_node_id
                    .as_ref()
                    .map(|n| format!("Run from step {n}"))
                    .or_else(|| Some("Manual run from Workflows canvas".to_string()));
                match rt.workflows().run_workflow(WorkflowRunRequest {
                    workflow_id: state.workflow_id.clone(),
                    run_id: None,
                    payload,
                    approve_node: None,
                    reject_node: None,
                    condition_mocks: BTreeMap::new(),
                    idempotency_key: None,
                }) {
                    Ok(outcome) => {
                        let steps: Vec<WorkflowHudStep> = outcome
                            .steps
                            .iter()
                            .map(|s| WorkflowHudStep {
                                node_id: s.node_id.clone(),
                                label: s.node_id.clone(),
                                kind: s.kind,
                                status: parse_step_status(&s.status),
                                branch: s.branch,
                                elapsed_ms: 12,
                                output_preview: s.output.clone(),
                                error: None,
                            })
                            .collect();
                        state.pending_approval = steps
                            .iter()
                            .find(|s| s.status == WorkflowStepStatus::Approval)
                            .map(|s| (outcome.run_id.clone(), s.node_id.clone()));
                        let status_msg = if state.pending_approval.is_some() {
                            tr_str(
                                lang,
                                "工作流暂停在人工审批节点，请点击节点上的批准或拒绝",
                                "Paused at approval step — click Approve or Reject on the node",
                            )
                        } else {
                            tr_str(lang, "工作流执行完成", "Workflow run completed")
                        };
                        state.hud_run = Some(WorkflowHudRun {
                            run_id: outcome.run_id,
                            status: parse_run_status(&outcome.status),
                            started_at: chrono::Utc::now().to_rfc3339(),
                            steps,
                        });
                        state.viewing_run_number = None;
                        state.status_toast = Some(status_msg.to_string());
                        if let Ok(list) = rt.workflows().list_workflows() {
                            state.library = list;
                        }
                        refresh_versions_and_runs(state, rt);
                    }
                    Err(e) => {
                        state.status_toast = Some(format!(
                            "{}: {e}",
                            tr_str(lang, "运行失败", "Run failed")
                        ));
                    }
                }
            }
        }
        WorkflowUiAction::DecideApproval {
            run_id,
            node_id,
            approve,
        } => {
            if let Some(rt) = runtime {
                match rt
                    .workflows()
                    .decide_approval(&run_id, &node_id, approve, &BTreeMap::new())
                {
                    Ok(outcome) => {
                        state.pending_approval = None;
                        state.status_toast = Some(
                            if approve {
                                tr_str(lang, "已批准审批节点，工作流继续执行完成", "Approved · workflow completed")
                            } else {
                                tr_str(lang, "已拒绝审批节点", "Rejected approval step")
                            }
                            .to_string(),
                        );
                        let _ = outcome;
                        refresh_versions_and_runs(state, rt);
                    }
                    Err(e) => {
                        state.status_toast = Some(format!(
                            "{}: {e}",
                            tr_str(lang, "审批操作失败", "Approval failed")
                        ));
                    }
                }
            }
        }
        WorkflowUiAction::ToggleRunsMenu => {
            let next = !state.runs_menu_open;
            state.close_transient_menus();
            state.runs_menu_open = next;
        }
        WorkflowUiAction::ViewRun(num_opt) => {
            state.runs_menu_open = false;
            state.viewing_run_number = num_opt;
            match num_opt {
                Some(num) => {
                    if let Some(found) = state.past_runs.iter().find(|r| r.number == num).cloned() {
                        state.hud_run = Some(WorkflowHudRun {
                            run_id: found.run_id,
                            status: found.status,
                            started_at: found.started_at,
                            steps: found.steps,
                        });
                    }
                }
                None => {
                    if let Some(latest) = state.past_runs.first().cloned() {
                        state.hud_run = Some(WorkflowHudRun {
                            run_id: latest.run_id,
                            status: latest.status,
                            started_at: latest.started_at,
                            steps: latest.steps,
                        });
                    }
                }
            }
        }
        WorkflowUiAction::FocusNode(node_id) => {
            if let Some(node) = state.display_graph().nodes.iter().find(|n| n.id == node_id) {
                let (nw, nh) = node_dimensions(node);
                let cx = node.at.x as f32 + nw * 0.5;
                let cy = node.at.y as f32 + nh * 0.5;
                state.pan_x = 480.0 - cx * state.zoom;
                state.pan_y = 280.0 - cy * state.zoom;
                state.select_node(&node_id, false);
            }
        }
        WorkflowUiAction::SetTool(tool) => {
            state.active_tool = tool;
            state.add_menu_open = false;
            state.sub_tool_panel = match tool {
                WorkflowInkTool::Pen | WorkflowInkTool::Highlighter | WorkflowInkTool::Eraser => {
                    WorkflowSubToolPanel::Draw
                }
                _ => WorkflowSubToolPanel::None,
            };
        }
        WorkflowUiAction::ToggleSubToolPanel(panel) => {
            if state.sub_tool_panel == panel {
                state.sub_tool_panel = WorkflowSubToolPanel::None;
                if panel == WorkflowSubToolPanel::Draw {
                    state.active_tool = WorkflowInkTool::Select;
                }
            } else {
                state.sub_tool_panel = panel;
                if panel == WorkflowSubToolPanel::Draw
                    && !matches!(
                        state.active_tool,
                        WorkflowInkTool::Pen
                            | WorkflowInkTool::Highlighter
                            | WorkflowInkTool::Eraser
                    )
                {
                    state.active_tool = WorkflowInkTool::Pen;
                }
            }
            state.add_menu_open = false;
        }
        WorkflowUiAction::SetInkColor(hex) => {
            state.ink_color_hex = hex;
        }
        WorkflowUiAction::SetNoteTone(hex) => {
            state.note_tone_hex = hex;
        }
        WorkflowUiAction::SetShapeKind(kind) => {
            state.shape_kind = kind;
        }
        WorkflowUiAction::SetShapeFill(hex) => {
            state.shape_fill_hex = hex;
        }
        WorkflowUiAction::ToggleAddMenu => {
            let next = !state.add_menu_open;
            state.close_transient_menus();
            state.add_menu_open = next;
        }
        WorkflowUiAction::AddNode { kind, preset } => {
            state.add_menu_open = false;
            if state.preview_version_id.is_some() {
                return;
            }
            if !state.has_active_workflow {
                apply_workflow_ui_action(
                    state,
                    WorkflowUiAction::CreateNewWorkflow,
                    runtime,
                    data_dir,
                    lang,
                );
            }
            state.dismissed_starter_for = Some(state.workflow_id.clone());
            // Place nicely to the right of the rightmost node, or near canvas center
            let (nx, ny) = if let Some(last) = state.graph.nodes.last() {
                (last.at.x + 310.0, last.at.y + if state.graph.nodes.len() % 2 == 0 { 0.0 } else { 36.0 })
            } else {
                let wx = ((360.0 - state.pan_x) / state.zoom.max(0.25)) as f64;
                let wy = ((180.0 - state.pan_y) / state.zoom.max(0.25)) as f64;
                (wx, wy)
            };
            let node = default_node_for_kind(
                &state.graph,
                kind,
                preset,
                WorkflowPoint { x: nx, y: ny },
                lang,
                default_agent_id.as_deref(),
                &state.note_tone_hex,
                &state.shape_kind,
                &state.shape_fill_hex,
            );
            let nid = node.id.clone();
            if let Err(err) = state.add_node(node) {
                state.status_toast = Some(err);
            } else {
                state.select_node(&nid, false);
                state.active_tool = WorkflowInkTool::Select;
                state.sub_tool_panel = WorkflowSubToolPanel::None;
                if let Some(rt) = runtime {
                    persist_graph_to_runtime(state, rt, lang);
                }
            }
        }
        WorkflowUiAction::SelectNode { node_id, additive } => {
            state.close_transient_menus();
            state.select_node(&node_id, additive);
        }
        WorkflowUiAction::ClearSelection => {
            state.close_transient_menus();
            state.connecting_from = None;
            state.clear_selection();
        }
        WorkflowUiAction::DuplicateSelected => {
            if state.preview_version_id.is_some() || state.selected_node_ids.is_empty() {
                return;
            }
            let copies: Vec<WorkflowNode> = state
                .graph
                .nodes
                .iter()
                .filter(|n| state.selected_node_ids.contains(&n.id))
                .cloned()
                .collect();
            let mut new_ids = Vec::new();
            for mut node in copies {
                node.id = next_node_id(&state.graph, "node");
                node.at.x += 48.0;
                node.at.y += 48.0;
                let nid = node.id.clone();
                if state.add_node(node).is_ok() {
                    new_ids.push(nid);
                }
            }
            if !new_ids.is_empty() {
                state.selected_node_ids = new_ids;
                if let Some(rt) = runtime {
                    persist_graph_to_runtime(state, rt, lang);
                }
            }
        }
        WorkflowUiAction::ToggleLockSelected => {
            if state.preview_version_id.is_some() || state.selected_node_ids.is_empty() {
                return;
            }
            let any_unlocked = state
                .graph
                .nodes
                .iter()
                .filter(|n| state.selected_node_ids.contains(&n.id))
                .any(|n| n.locked != Some(true));
            state.push_undo_snapshot();
            for node in &mut state.graph.nodes {
                if state.selected_node_ids.contains(&node.id) {
                    node.locked = if any_unlocked { Some(true) } else { None };
                }
            }
            if let Some(rt) = runtime {
                persist_graph_to_runtime(state, rt, lang);
            }
        }
        WorkflowUiAction::DeleteSelected => {
            if state.preview_version_id.is_some() {
                return;
            }
            // Skip locked nodes
            state.selected_node_ids.retain(|id| {
                state
                    .graph
                    .nodes
                    .iter()
                    .find(|n| &n.id == id)
                    .is_some_and(|n| n.locked != Some(true))
            });
            state.remove_selected();
            if let Some(rt) = runtime {
                persist_graph_to_runtime(state, rt, lang);
            }
        }
        WorkflowUiAction::DisconnectNodeEdges(node_id) => {
            if state.preview_version_id.is_some() {
                return;
            }
            let before = state.graph.edges.len();
            state.push_undo_snapshot();
            state
                .graph
                .edges
                .retain(|e| e.from != node_id && e.to != node_id);
            if state.graph.edges.len() != before {
                state.revision = state.revision.saturating_add(1);
                if let Some(rt) = runtime {
                    persist_graph_to_runtime(state, rt, lang);
                }
            }
        }
        WorkflowUiAction::StartConnectFrom {
            node_id,
            port_type,
            branch,
        } => {
            if state.preview_version_id.is_some() {
                return;
            }
            state.connecting_from = Some(WorkflowPortConnectStart {
                from_node_id: node_id,
                port_type,
                branch,
            });
        }
        WorkflowUiAction::CompleteConnectTo { target_node_id } => {
            if state.preview_version_id.is_some() {
                state.connecting_from = None;
                return;
            }
            let Some(start) = state.connecting_from.take() else {
                return;
            };
            if start.from_node_id == target_node_id {
                state.status_toast = Some(
                    tr_str(lang, "节点不能连接到自身", "Cannot connect a node to itself")
                        .to_string(),
                );
                return;
            }
            let eid = next_edge_id(&state.graph, &start.from_node_id, &target_node_id, start.branch);
            let new_edge = WorkflowEdge {
                id: eid,
                from: start.from_node_id,
                to: target_node_id,
                port_type: start.port_type,
                branch: start.branch,
            };
            match state.add_edge(new_edge) {
                Ok(()) => {
                    if let Some(rt) = runtime {
                        persist_graph_to_runtime(state, rt, lang);
                    }
                }
                Err(err) => {
                    state.status_toast = Some(err);
                }
            }
        }
        WorkflowUiAction::CancelConnect => {
            state.connecting_from = None;
        }
        WorkflowUiAction::StartEditNodeField { node_id, field_key } => {
            if state.preview_version_id.is_some() {
                return;
            }
            let current_val = state
                .graph
                .nodes
                .iter()
                .find(|n| n.id == node_id)
                .and_then(|n| match n.fields.get(&field_key) {
                    Some(WorkflowFieldValue::Text(s)) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            state.input_buffer = current_val;
            state.active_text_input = WorkflowTextInputTarget::EditNodeField { node_id, field_key };
        }
        WorkflowUiAction::SetNodeFieldValue {
            node_id,
            field_key,
            value,
        } => {
            if state.preview_version_id.is_some() {
                return;
            }
            state.push_undo_snapshot();
            if let Some(node) = state.graph.nodes.iter_mut().find(|n| n.id == node_id) {
                node.fields
                    .insert(field_key, WorkflowFieldValue::Text(value));
                state.revision = state.revision.saturating_add(1);
            }
            if let Some(rt) = runtime {
                persist_graph_to_runtime(state, rt, lang);
            }
        }
        WorkflowUiAction::CommitTextInput => {
            let target = std::mem::take(&mut state.active_text_input);
            let value = state.input_buffer.trim().to_string();
            state.input_buffer.clear();
            match target {
                WorkflowTextInputTarget::RenameWorkflow => {
                    if !value.is_empty() && state.has_active_workflow {
                        state.title = value.clone();
                        if let Some(rt) = runtime {
                            let _ = rt.database().workflow_store().rename(
                                &state.workflow_id,
                                &value,
                                None,
                            );
                            if let Ok(list) = rt.workflows().list_workflows() {
                                state.library = list;
                            }
                        } else if let Some(item) =
                            state.library.iter_mut().find(|w| w.id == state.workflow_id)
                        {
                            item.title = value;
                        }
                    }
                }
                WorkflowTextInputTarget::NewFolder => {
                    if !value.is_empty() {
                        let fid = format!("folder-{}", uuid::Uuid::new_v4().simple());
                        state.folders.push(WorkflowFolderItem {
                            id: fid.clone(),
                            name: value,
                        });
                        if state.has_active_workflow && !state.workflow_id.is_empty() {
                            state
                                .folder_placement
                                .insert(state.workflow_id.clone(), fid);
                        }
                        if let Some(dir) = data_dir {
                            save_workflow_folders(dir, &state.folders, &state.folder_placement);
                        }
                    }
                }
                WorkflowTextInputTarget::RenameFolder { folder_id } => {
                    if !value.is_empty() {
                        if let Some(f) = state.folders.iter_mut().find(|f| f.id == folder_id) {
                            f.name = value;
                        }
                        if let Some(dir) = data_dir {
                            save_workflow_folders(dir, &state.folders, &state.folder_placement);
                        }
                    }
                }
                WorkflowTextInputTarget::EditNodeField { node_id, field_key } => {
                    state.push_undo_snapshot();
                    if let Some(node) = state.graph.nodes.iter_mut().find(|n| n.id == node_id) {
                        node.fields
                            .insert(field_key, WorkflowFieldValue::Text(value));
                        state.revision = state.revision.saturating_add(1);
                    }
                    if let Some(rt) = runtime {
                        persist_graph_to_runtime(state, rt, lang);
                    }
                }
                WorkflowTextInputTarget::SearchNodes => {
                    // Focus first matching node on Enter
                    let q = state.search_query.trim().to_ascii_lowercase();
                    if !q.is_empty() {
                        if let Some(hit) = state.display_graph().nodes.iter().find(|n| {
                            n.id.to_ascii_lowercase().contains(&q)
                                || node_summary_text(n).to_ascii_lowercase().contains(&q)
                        }) {
                            let nid = hit.id.clone();
                            apply_workflow_ui_action(
                                state,
                                WorkflowUiAction::FocusNode(nid),
                                runtime,
                                data_dir,
                                lang,
                            );
                        }
                    }
                    state.active_text_input = WorkflowTextInputTarget::SearchNodes;
                }
                WorkflowTextInputTarget::ImportJson => {
                    state.input_buffer = value;
                    apply_workflow_ui_action(
                        state,
                        WorkflowUiAction::ConfirmImportJson,
                        runtime,
                        data_dir,
                        lang,
                    );
                }
                WorkflowTextInputTarget::None => {}
            }
        }
        WorkflowUiAction::CancelTextInput => {
            if state.active_text_input == WorkflowTextInputTarget::SearchNodes {
                state.search_open = false;
                state.search_query.clear();
            }
            if state.active_text_input == WorkflowTextInputTarget::ImportJson {
                state.import_dialog_open = false;
                state.import_error = None;
            }
            state.active_text_input = WorkflowTextInputTarget::None;
            state.input_buffer.clear();
        }
        WorkflowUiAction::ZoomIn => state.zoom_in(),
        WorkflowUiAction::ZoomOut => state.zoom_out(),
        WorkflowUiAction::ZoomTo(z) => {
            state.set_zoom(z);
            state.zoom_menu_open = false;
        }
        WorkflowUiAction::ToggleZoomMenu => {
            let next = !state.zoom_menu_open;
            state.close_transient_menus();
            state.zoom_menu_open = next;
        }
        WorkflowUiAction::FitToView => {
            state.zoom_menu_open = false;
            state.fit_to_view(960.0, 560.0);
        }
        WorkflowUiAction::TidyLayout => {
            if state.preview_version_id.is_none() {
                state.apply_tidy_layout();
                state.fit_to_view(960.0, 560.0);
                if let Some(rt) = runtime {
                    persist_graph_to_runtime(state, rt, lang);
                }
            }
        }
        WorkflowUiAction::ToggleMinimap => {
            state.minimap_visible = !state.minimap_visible;
        }
        WorkflowUiAction::ToggleHud => {
            state.hud_visible = !state.hud_visible;
        }
        WorkflowUiAction::Undo => {
            if state.preview_version_id.is_none() && state.undo() {
                if let Some(rt) = runtime {
                    persist_graph_to_runtime(state, rt, lang);
                }
            }
        }
        WorkflowUiAction::Redo => {
            if state.preview_version_id.is_none() && state.redo() {
                if let Some(rt) = runtime {
                    persist_graph_to_runtime(state, rt, lang);
                }
            }
        }
        WorkflowUiAction::ToggleSearch => {
            state.search_open = !state.search_open;
            if state.search_open {
                state.close_transient_menus();
                state.active_text_input = WorkflowTextInputTarget::SearchNodes;
            } else {
                state.search_query.clear();
                if state.active_text_input == WorkflowTextInputTarget::SearchNodes {
                    state.active_text_input = WorkflowTextInputTarget::None;
                }
            }
        }
        WorkflowUiAction::PanBy(dx, dy) => {
            state.pan_by(dx, dy);
        }
    }
}

/// Handles pointer down on the canvas viewport background (panning or freehand ink drawing/erasing).
pub fn handle_canvas_pointer_down(
    state: &mut WorkflowCanvasState,
    pos_x: f32,
    pos_y: f32,
) {
    state.close_transient_menus();
    if state.preview_version_id.is_none()
        && matches!(
            state.active_tool,
            WorkflowInkTool::Pen | WorkflowInkTool::Highlighter
        )
    {
        let wx = ((pos_x - state.pan_x) / state.zoom.max(0.1)) as f64;
        let wy = ((pos_y - state.pan_y) / state.zoom.max(0.1)) as f64;
        let sid = format!("stroke-{}", uuid::Uuid::new_v4().simple());
        let brush = if state.active_tool == WorkflowInkTool::Highlighter {
            "highlighter"
        } else {
            "pen"
        };
        let width = if state.active_tool == WorkflowInkTool::Highlighter {
            14.0
        } else {
            3.0
        };
        let stroke = WorkflowStroke {
            id: sid.clone(),
            brush: brush.to_string(),
            color: state.ink_color_hex.trim_start_matches('#').to_string(),
            width,
            points: vec![WorkflowPoint { x: wx, y: wy }],
        };
        if state.add_stroke(stroke).is_ok() {
            state.pointer_drag = Some(WorkflowPointerDrag::DrawStroke { stroke_id: sid });
        }
        return;
    }
    if state.preview_version_id.is_none() && state.active_tool == WorkflowInkTool::Eraser {
        let wx = ((pos_x - state.pan_x) / state.zoom.max(0.1)) as f64;
        let wy = ((pos_y - state.pan_y) / state.zoom.max(0.1)) as f64;
        let before = state.graph.strokes.len();
        state.graph.strokes.retain(|s| {
            !s.points
                .iter()
                .any(|p| (p.x - wx).hypot(p.y - wy) < 28.0)
        });
        if state.graph.strokes.len() != before {
            state.revision = state.revision.saturating_add(1);
        }
        return;
    }
    state.clear_selection();
    state.connecting_from = None;
    state.pointer_drag = Some(WorkflowPointerDrag::PanCanvas {
        last_x: pos_x,
        last_y: pos_y,
    });
}

/// Handles pointer down on an individual workflow node card (select + initiate drag-to-move).
pub fn handle_node_pointer_down(
    state: &mut WorkflowCanvasState,
    node_id: &str,
    additive: bool,
    pos_x: f32,
    pos_y: f32,
) {
    state.close_transient_menus();
    state.select_node(node_id, additive);
    if state.preview_version_id.is_some() || state.active_tool == WorkflowInkTool::Pan {
        state.pointer_drag = Some(WorkflowPointerDrag::PanCanvas {
            last_x: pos_x,
            last_y: pos_y,
        });
        return;
    }
    let locked = state
        .graph
        .nodes
        .iter()
        .find(|n| n.id == node_id)
        .is_some_and(|n| n.locked == Some(true));
    if !locked {
        state.pointer_drag = Some(WorkflowPointerDrag::MoveNode {
            node_id: node_id.to_string(),
            last_x: pos_x,
            last_y: pos_y,
            moved: false,
        });
    }
}

/// Handles pointer movement during active canvas pan, node drag, or ink stroke drawing.
/// Returns `true` if visual state changed.
pub fn handle_canvas_pointer_move(
    state: &mut WorkflowCanvasState,
    pos_x: f32,
    pos_y: f32,
) -> bool {
    let Some(drag) = state.pointer_drag.clone() else {
        return false;
    };
    match drag {
        WorkflowPointerDrag::PanCanvas { last_x, last_y } => {
            let dx = pos_x - last_x;
            let dy = pos_y - last_y;
            state.pan_by(dx, dy);
            state.pointer_drag = Some(WorkflowPointerDrag::PanCanvas {
                last_x: pos_x,
                last_y: pos_y,
            });
            true
        }
        WorkflowPointerDrag::MoveNode {
            node_id,
            last_x,
            last_y,
            moved,
        } => {
            let dx = (pos_x - last_x) / state.zoom.max(0.1);
            let dy = (pos_y - last_y) / state.zoom.max(0.1);
            if dx.abs() < 0.01 && dy.abs() < 0.01 {
                return false;
            }
            if !moved {
                state.push_undo_snapshot();
            }
            if let Some(node) = state.graph.nodes.iter_mut().find(|n| n.id == node_id) {
                node.at.x += f64::from(dx);
                node.at.y += f64::from(dy);
            }
            state.pointer_drag = Some(WorkflowPointerDrag::MoveNode {
                node_id,
                last_x: pos_x,
                last_y: pos_y,
                moved: true,
            });
            true
        }
        WorkflowPointerDrag::DrawStroke { stroke_id } => {
            let wx = ((pos_x - state.pan_x) / state.zoom.max(0.1)) as f64;
            let wy = ((pos_y - state.pan_y) / state.zoom.max(0.1)) as f64;
            if let Some(stroke) = state.graph.strokes.iter_mut().find(|s| s.id == stroke_id) {
                if stroke.points.len() < 4000 {
                    stroke.points.push(WorkflowPoint { x: wx, y: wy });
                }
            }
            true
        }
    }
}

/// Finishes any active pointer drag and persists node moves / ink strokes to SQLite.
pub fn handle_canvas_pointer_up(
    state: &mut WorkflowCanvasState,
    runtime: Option<&OpenBotHostRuntime>,
    lang: &str,
) -> bool {
    let Some(drag) = state.pointer_drag.take() else {
        return false;
    };
    let should_save = matches!(
        drag,
        WorkflowPointerDrag::MoveNode { moved: true, .. } | WorkflowPointerDrag::DrawStroke { .. }
    );
    if should_save {
        state.revision = state.revision.saturating_add(1);
        if let Some(rt) = runtime {
            persist_graph_to_runtime(state, rt, lang);
        }
    }
    true
}

/// Handles wheel zoom / pan on the workflow canvas.
pub fn handle_canvas_scroll_wheel(
    state: &mut WorkflowCanvasState,
    delta_x: f32,
    delta_y: f32,
    is_zoom_gesture: bool,
) {
    if is_zoom_gesture {
        let factor = if delta_y < 0.0 { 1.08 } else { 1.0 / 1.08 };
        state.set_zoom(state.zoom * factor);
    } else {
        state.pan_by(delta_x, delta_y);
    }
}

/// Renders the complete 1:1 OpenBot interactive `WorkflowsPage` + `WorkflowCanvas` surface.
#[allow(clippy::too_many_arguments)]
pub fn render_workflow_canvas_interactive<V: 'static>(
    state: &WorkflowCanvasState,
    palette: &OpenBotPalette,
    lang: &str,
    now_sec: f64,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, WorkflowUiAction, &mut Context<V>) + 'static,
    on_canvas_down: impl Fn(&mut V, f32, f32, &mut Context<V>) + 'static,
    on_node_down: impl Fn(&mut V, String, bool, f32, f32, &mut Context<V>) + 'static,
    on_pointer_move: impl Fn(&mut V, f32, f32, &mut Context<V>) + 'static,
    on_pointer_up: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_scroll: impl Fn(&mut V, f32, f32, bool, &mut Context<V>) + 'static,
) -> Div {
    let on_action: Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)> = Rc::new(on_action);
    let on_canvas_down = Rc::new(on_canvas_down);
    let on_node_down: Rc<dyn Fn(&mut V, String, bool, f32, f32, &mut Context<V>)> =
        Rc::new(on_node_down);
    let on_pointer_move = Rc::new(on_pointer_move);
    let on_pointer_up = Rc::new(on_pointer_up);
    let on_scroll = Rc::new(on_scroll);

    let is_dark = palette.scheme == OpenBotColorScheme::Dark;
    let bg_frame = pal_rgba(palette, "--openbot-bg-frame", if is_dark { 0x0d0d0d } else { 0xf5f5f5 });
    let bg_surface = pal_rgba(palette, "--openbot-bg-surface", if is_dark { 0x1e1e1f } else { 0xffffff });
    let bg_raised = pal_rgba(
        palette,
        "--openbot-bg-surface-raised",
        if is_dark { 0x262628 } else { 0xf2f2f3 },
    );
    let bg_hover = pal_rgba(
        palette,
        "--openbot-bg-control-hover",
        if is_dark { 0x2f2f32 } else { 0xececee },
    );
    let bg_active = pal_rgba(
        palette,
        "--openbot-bg-control-active",
        if is_dark { 0x38383c } else { 0xe2e2e5 },
    );
    let border_hair = pal_rgba(
        palette,
        "--openbot-border",
        if is_dark { 0x2c2c2f } else { 0xe4e4e7 },
    );
    let border_strong = pal_rgba(
        palette,
        "--openbot-border-strong",
        if is_dark { 0x3b3b40 } else { 0xd4d4d8 },
    );
    let text_pri = pal_rgba(palette, "--openbot-text-primary", if is_dark { 0xffffff } else { 0x141414 });
    let text_sec = pal_rgba(palette, "--openbot-text-secondary", if is_dark { 0xd4d4d8 } else { 0x3f3f46 });
    let text_mut = pal_rgba(palette, "--openbot-text-muted", if is_dark { 0x979797 } else { 0x71717a });
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let on_accent = pal_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let success = pal_rgba(palette, "--openbot-success", 0x31cf76);
    let warning = pal_rgba(palette, "--openbot-warning", 0xff9412);
    let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);

    // 1. Empty workspace welcome canvas (`WorkflowsEmpty.tsx` 1:1)
    if !state.has_active_workflow {
        return render_workflows_empty_surface(
            state,
            palette,
            lang,
            bg_frame,
            bg_surface,
            bg_raised,
            border_strong,
            text_pri,
            text_mut,
            accent,
            on_accent,
            cx,
            on_action,
        );
    }

    let graph = state.display_graph();
    let loop_set: BTreeSet<String> = workflow_loop_edges(&graph.nodes, &graph.edges);
    let hud_step_map: HashMap<&str, &WorkflowHudStep> = state
        .hud_run
        .as_ref()
        .map(|run| run.steps.iter().map(|s| (s.node_id.as_str(), s)).collect())
        .unwrap_or_default();
    let showing_start = state.showing_start_templates();
    let previewing = state.preview_version_id.is_some();

    // 2. Interactive dot-grid canvas stage
    let paint_state = state.clone();
    let paint_loops = loop_set.clone();
    let paint_palette = palette.clone();
    let cd = on_canvas_down.clone();
    let pm = on_pointer_move.clone();
    let pu = on_pointer_up.clone();
    let sc = on_scroll.clone();

    let mut stage = div()
        .id("openbot-workflow-canvas-stage")
        .relative()
        .size_full()
        .overflow_hidden()
        .bg(bg_frame)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cd(this, f32::from(ev.position.x), f32::from(ev.position.y), cx);
            }),
        )
        .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
            pm(this, f32::from(ev.position.x), f32::from(ev.position.y), cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                pu(this, cx);
            }),
        )
        .on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, _, cx| {
            let delta = ev.delta.pixel_delta(px(20.0));
            let zoom_gesture = ev.modifiers.control || ev.modifiers.platform;
            sc(this, f32::from(delta.x), f32::from(delta.y), zoom_gesture, cx);
        }))
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

    // 3. OpenBot Node Cards on the canvas
    for node in &graph.nodes {
        let step = hud_step_map.get(node.id.as_str()).copied();
        stage = stage.child(render_interactive_node_card(
            node,
            state,
            step,
            palette,
            lang,
            now_sec,
            cx,
            &on_action,
            &on_node_down,
        ));
    }

    // 4. OpenBot `WorkflowStart` template picker when active workflow has 0 nodes
    if showing_start {
        stage = stage.child(render_workflow_start_overlay(
            palette,
            lang,
            bg_surface,
            bg_raised,
            bg_hover,
            border_strong,
            text_pri,
            text_mut,
            accent,
            cx,
            &on_action,
        ));
    }

    // 5. Connecting-port banner hint
    if let Some(ref conn) = state.connecting_from {
        let a_cancel = on_action.clone();
        stage = stage.child(
            div()
                .id("wfn-connect-banner")
                .absolute()
                .top(px(64.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded(px(999.0))
                        .bg(bg_surface)
                        .border_1()
                        .border_color(accent)
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(12.0))
                        .text_color(text_pri)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(format!(
                            "{} `{}` → {}",
                            tr_str(lang, "正在连接", "Connecting"),
                            conn.from_node_id,
                            tr_str(lang, "点击目标节点的输入端口完成连接", "Click any target node's input port")
                        ))
                        .child(
                            div()
                                .id("wfn-connect-cancel")
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(2.0))
                                .rounded(px(6.0))
                                .bg(bg_hover)
                                .text_size(px(11.0))
                                .text_color(text_mut)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        a_cancel(this, WorkflowUiAction::CancelConnect, cx);
                                    }),
                                )
                                .child(tr_str(lang, "取消", "Cancel")),
                        ),
                ),
        );
    }

    // 6. Top Floating Header Row: Left (`WorkflowCorner` + `WorkflowTabs`) & Right (`WorkflowCanvasHeader`)
    stage = stage.child(render_top_hud_bar(
        state,
        palette,
        lang,
        previewing,
        &loop_set,
        bg_surface,
        bg_raised,
        bg_hover,
        bg_active,
        border_hair,
        border_strong,
        text_pri,
        text_sec,
        text_mut,
        accent,
        on_accent,
        success,
        warning,
        danger,
        cx,
        &on_action,
    ));

    // 7. Selected-Node Floating Inspector (right side, when a single node is selected and not starting)
    if !showing_start && state.selected_node_ids.len() == 1 {
        if let Some(sel_node) = graph
            .nodes
            .iter()
            .find(|n| n.id == state.selected_node_ids[0])
        {
            stage = stage.child(render_selected_node_inspector(
                sel_node,
                state,
                palette,
                lang,
                previewing,
                bg_surface,
                bg_raised,
                bg_hover,
                border_hair,
                border_strong,
                text_pri,
                text_sec,
                text_mut,
                accent,
                danger,
                cx,
                &on_action,
            ));
        }
    }

    // 8. Floating `WorkflowSearch` panel (`⌘F`)
    if state.search_open {
        stage = stage.child(render_workflow_search_popover(
            state,
            lang,
            bg_surface,
            bg_raised,
            bg_hover,
            border_strong,
            text_pri,
            text_mut,
            accent,
            cx,
            &on_action,
        ));
    }

    // 9. Bottom-Center Floating Tool Dock (`wfn-hud-tools`) + Sub-Toolbars (`wfn-hud-sub`) + `WorkflowAddMenu`
    if !showing_start && !previewing && state.hud_visible {
        stage = stage.child(render_bottom_tool_dock(
            state,
            palette,
            lang,
            bg_surface,
            bg_raised,
            bg_hover,
            bg_active,
            border_hair,
            border_strong,
            text_pri,
            text_mut,
            accent,
            on_accent,
            cx,
            &on_action,
        ));
    }

    // 10. Floating `WorkflowVersionBar` when previewing a historical revision
    if previewing {
        let a_back = on_action.clone();
        let a_restore = on_action.clone();
        let ver_label = state
            .preview_version_id
            .as_deref()
            .and_then(|vid| state.versions.iter().find(|v| v.id == vid))
            .map(|v| format!("{} · {}", v.label, v.created_at))
            .unwrap_or_else(|| tr_str(lang, "历史版本", "Version").to_string());
        stage = stage.child(
            div()
                .id("wfn-version-bar")
                .absolute()
                .bottom(px(16.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .h(px(40.0))
                        .px(px(12.0))
                        .rounded(px(12.0))
                        .bg(bg_surface)
                        .border_1()
                        .border_color(border_strong)
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(tg_icon_rgba(IconName::Lock, 13.0, text_mut))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_pri)
                                .child(format!(
                                    "{} · {ver_label}",
                                    tr_str(lang, "版本预览（只读）", "Version preview (read-only)")
                                )),
                        )
                        .child(
                            div()
                                .id("wfn-ver-back")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.0))
                                .rounded(px(7.0))
                                .bg(bg_hover)
                                .text_size(px(12.0))
                                .text_color(text_pri)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        a_back(this, WorkflowUiAction::CloseVersionPreview, cx);
                                    }),
                                )
                                .child(tr_str(lang, "返回", "Back")),
                        )
                        .child(
                            div()
                                .id("wfn-ver-restore")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(7.0))
                                .bg(accent)
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(on_accent)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        a_restore(this, WorkflowUiAction::RestorePreviewVersion, cx);
                                    }),
                                )
                                .child(tr_str(lang, "设为当前版本", "Make current")),
                        ),
                ),
        );
    }

    // 11. Bottom-Left Floating `WorkflowZoomMenu` (`−`, `100%`, `+`, Fit, Tidy, Minimap)
    if !showing_start && state.hud_visible {
        stage = stage.child(render_zoom_menu_hud(
            state,
            lang,
            bg_surface,
            bg_hover,
            bg_active,
            border_hair,
            border_strong,
            text_pri,
            text_mut,
            accent,
            on_accent,
            cx,
            &on_action,
        ));
    }

    // 12. Bottom-Right Clickable `WorkflowMinimap`
    if !showing_start && state.minimap_visible && state.hud_visible {
        let mm_state = state.clone();
        let mm_loops = loop_set;
        let mm_palette = palette.clone();
        let a_fit = on_action.clone();
        stage = stage.child(
            div()
                .id("wfn-minimap")
                .absolute()
                .right(px(16.0))
                .bottom(px(16.0))
                .w(px(184.0))
                .h(px(114.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .overflow_hidden()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_fit(this, WorkflowUiAction::FitToView, cx);
                    }),
                )
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _cx| {
                            paint_workflow_minimap(
                                bounds,
                                &mm_state,
                                &mm_loops,
                                &mm_palette,
                                window,
                            );
                        },
                    )
                    .size_full(),
                ),
        );
    }

    // 13. Inline Folder Name / Import JSON Dialogs
    if matches!(
        state.active_text_input,
        WorkflowTextInputTarget::NewFolder | WorkflowTextInputTarget::RenameFolder { .. }
    ) {
        stage = stage.child(render_folder_name_dialog(
            state,
            lang,
            bg_surface,
            bg_raised,
            bg_hover,
            border_strong,
            text_pri,
            text_mut,
            accent,
            on_accent,
            cx,
            &on_action,
        ));
    }
    if state.import_dialog_open {
        stage = stage.child(render_import_workflow_dialog(
            state,
            lang,
            bg_surface,
            bg_raised,
            bg_hover,
            border_strong,
            text_pri,
            text_mut,
            accent,
            on_accent,
            danger,
            cx,
            &on_action,
        ));
    }

    // 14. Floating `WorkflowDeleteUndoToast` or Status Toast
    if let Some(ref undo) = state.deleted_undo {
        let a_undo = on_action.clone();
        let a_dismiss = on_action.clone();
        stage = stage.child(
            div()
                .id("wfn-delete-undo-toast")
                .absolute()
                .bottom(px(72.0))
                .right(px(16.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(10.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .items_center()
                .gap(px(10.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(text_pri)
                        .child(format!(
                            "{} {}",
                            undo.title,
                            tr_str(lang, "已删除", "deleted")
                        )),
                )
                .child(
                    div()
                        .id("wfn-delete-undo-btn")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(accent)
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(on_accent)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_undo(this, WorkflowUiAction::UndoDeleteWorkflow, cx);
                            }),
                        )
                        .child(tr_str(lang, "撤销", "Undo")),
                )
                .child(
                    div()
                        .id("wfn-delete-undo-close")
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(text_mut)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_dismiss(this, WorkflowUiAction::DismissDeleteUndo, cx);
                            }),
                        )
                        .child("×"),
                ),
        );
    } else if let Some(ref toast_msg) = state.status_toast {
        let a_dismiss = on_action.clone();
        stage = stage.child(
            div()
                .id("wfn-status-toast")
                .absolute()
                .bottom(px(72.0))
                .left(px(16.0))
                .max_w(px(420.0))
                .px(px(12.0))
                .py(px(7.0))
                .rounded(px(10.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .items_center()
                .gap(px(8.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(12.0))
                        .text_color(text_pri)
                        .child(toast_msg.clone()),
                )
                .child(
                    div()
                        .id("wfn-status-toast-close")
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(text_mut)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_dismiss(this, WorkflowUiAction::DismissToast, cx);
                            }),
                        )
                        .child("×"),
                ),
        );
    }

    div().size_full().flex().flex_col().bg(bg_frame).child(stage)
}

#[allow(clippy::too_many_arguments)]
fn render_workflows_empty_surface<V: 'static>(
    state: &WorkflowCanvasState,
    palette: &OpenBotPalette,
    lang: &str,
    bg_frame: Rgba,
    bg_surface: Rgba,
    bg_raised: Rgba,
    border_strong: Rgba,
    text_pri: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    on_accent: Rgba,
    cx: &mut Context<V>,
    on_action: Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Div {
    let a_new = on_action.clone();
    let a_import = on_action.clone();
    let a_undo = on_action.clone();
    let paint_state = WorkflowCanvasState::empty_workspace();
    let paint_palette = palette.clone();
    let empty_loops = BTreeSet::new();

    // Subtle ambient silhouette node chain matching OpenBot `WorkflowsEmptyBackdrop`
    let sample_card = |label: &str, icon: IconName, tint: Rgba| {
        div()
            .w(px(164.0))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .opacity(0.42)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(tg_icon_rgba(icon, 12.0, tint))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_mut)
                            .child(label.to_string()),
                    ),
            )
            .child(
                div()
                    .p(px(10.0))
                    .rounded(px(14.0))
                    .bg(bg_surface)
                    .border_1()
                    .border_color(border_strong)
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(div().h(px(7.0)).w(px(104.0)).rounded_full().bg(bg_raised))
                    .child(div().h(px(7.0)).w(px(126.0)).rounded_full().bg(bg_raised))
                    .child(div().h(px(7.0)).w(px(78.0)).rounded_full().bg(bg_raised)),
            )
    };

    let mut root = div()
        .id("openbot-workflows-empty")
        .relative()
        .size_full()
        .overflow_hidden()
        .bg(bg_frame)
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    paint_workflow_edges_and_strokes(
                        bounds,
                        &paint_state,
                        &empty_loops,
                        &paint_palette,
                        window,
                    );
                },
            )
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(28.0))
                .px(px(24.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(20.0))
                        .child(sample_card(
                            tr_str(lang, "触发器", "Trigger"),
                            IconName::Play,
                            pal_rgba(palette, "--openbot-success", 0x31cf76),
                        ))
                        .child(div().w(px(28.0)).h(px(2.0)).bg(border_strong))
                        .child(sample_card(
                            tr_str(lang, "智能体", "Agent"),
                            IconName::BotRobot,
                            accent,
                        ))
                        .child(div().w(px(28.0)).h(px(2.0)).bg(border_strong))
                        .child(sample_card(
                            tr_str(lang, "输出交付", "Output"),
                            IconName::SendPlane,
                            pal_rgba(palette, "--openbot-success", 0x31cf76),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(22.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(text_mut)
                                        .child(tr_str(lang, "步骤只需设置一次。", "Set up the steps once.")),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .text_size(px(24.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(text_pri)
                                        .child(tr_str(lang, "然后让它", "Then let it"))
                                        .child(
                                            div()
                                                .px(px(8.0))
                                                .py(px(1.0))
                                                .rounded(px(8.0))
                                                .bg(Rgba { a: 0.14, ..accent })
                                                .text_color(accent)
                                                .child(tr_str(lang, "运行", "run")),
                                        )
                                        .child(tr_str(lang, "。", ".")),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(13.5))
                                .text_color(text_mut)
                                .child(tr_str(
                                    lang,
                                    "触发器启动工作流。每一步把结果交给下一步。",
                                    "Triggers start a workflow. Each step hands its result to the next.",
                                )),
                        )
                        .child(
                            div()
                                .mt(px(6.0))
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .id("wfn-empty-create-btn")
                                        .cursor_pointer()
                                        .h(px(36.0))
                                        .px(px(16.0))
                                        .rounded(px(10.0))
                                        .bg(accent)
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(on_accent)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                a_new(this, WorkflowUiAction::CreateNewWorkflow, cx);
                                            }),
                                        )
                                        .child("+")
                                        .child(tr_str(lang, "新建工作流", "New workflow")),
                                )
                                .child(
                                    div()
                                        .id("wfn-empty-import-btn")
                                        .cursor_pointer()
                                        .h(px(36.0))
                                        .px(px(14.0))
                                        .rounded(px(10.0))
                                        .bg(bg_surface)
                                        .border_1()
                                        .border_color(border_strong)
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .text_size(px(13.0))
                                        .text_color(text_pri)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                a_import(this, WorkflowUiAction::OpenImportDialog, cx);
                                            }),
                                        )
                                        .child(tr_str(lang, "导入工作流", "Import workflow")),
                                ),
                        ),
                ),
        );

    if let Some(ref undo) = state.deleted_undo {
        root = root.child(
            div()
                .id("wfn-empty-undo-toast")
                .absolute()
                .bottom(px(20.0))
                .right(px(20.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(10.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(text_pri)
                        .child(format!(
                            "{} {}",
                            undo.title,
                            tr_str(lang, "已删除", "deleted")
                        )),
                )
                .child(
                    div()
                        .id("wfn-empty-undo-btn")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(accent)
                        .text_size(px(12.0))
                        .text_color(on_accent)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_undo(this, WorkflowUiAction::UndoDeleteWorkflow, cx);
                            }),
                        )
                        .child(tr_str(lang, "撤销", "Undo")),
                ),
        );
    }

    if state.import_dialog_open {
        root = root.child(render_import_workflow_dialog(
            state,
            lang,
            bg_surface,
            bg_raised,
            bg_raised,
            border_strong,
            text_pri,
            text_mut,
            accent,
            on_accent,
            pal_rgba(palette, "--openbot-danger", 0xfd2f3b),
            cx,
            &on_action,
        ));
    }

    div().size_full().flex().flex_col().bg(bg_frame).child(root)
}

#[allow(clippy::too_many_arguments)]
fn render_workflow_start_overlay<V: 'static>(
    palette: &OpenBotPalette,
    lang: &str,
    bg_surface: Rgba,
    bg_raised: Rgba,
    bg_hover: Rgba,
    border_strong: Rgba,
    text_pri: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Stateful<Div> {
    let is_zh = lang.starts_with("zh");
    let success = pal_rgba(palette, "--openbot-success", 0x31cf76);
    let warning = pal_rgba(palette, "--openbot-warning", 0xff9412);
    let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);

    let mut grid = div()
        .w_full()
        .max_w(px(780.0))
        .flex()
        .flex_wrap()
        .justify_center()
        .gap(px(12.0));

    for tmpl in OPENBOT_WORKFLOW_TEMPLATES {
        let a_pick = on_action.clone();
        let tid = tmpl.id;
        let title = if is_zh { tmpl.title_zh } else { tmpl.title_en };
        let brief = if is_zh { tmpl.brief_zh } else { tmpl.brief_en };
        let mini_nodes: Vec<Rgba> = match tid {
            "morningBrief" => vec![success, accent, success],
            "answerIssues" => vec![success, accent, success, success],
            "weeklyReport" => vec![success, accent, danger, success],
            "priceWatch" => vec![success, accent, warning, success],
            _ => vec![rgb(0x74b9ff), rgb(0xd6adf2), rgb(0xd98ac9)],
        };
        let mut thumb = div()
            .w_full()
            .h(px(64.0))
            .rounded(px(10.0))
            .bg(bg_raised)
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0));
        for (i, col) in mini_nodes.into_iter().enumerate() {
            if i > 0 {
                thumb = thumb.child(div().w(px(12.0)).h(px(2.0)).bg(border_strong));
            }
            thumb = thumb.child(
                div()
                    .w(px(34.0))
                    .h(px(22.0))
                    .rounded(px(5.0))
                    .bg(bg_surface)
                    .border_1()
                    .border_color(col),
            );
        }

        grid = grid.child(
            div()
                .id(SharedString::from(format!("wfn-template-{tid}")))
                .cursor_pointer()
                .w(px(244.0))
                .p(px(10.0))
                .rounded(px(14.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .hover(move |s| s.bg(bg_hover))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_pick(this, WorkflowUiAction::ApplyTemplate(tid), cx);
                    }),
                )
                .child(thumb)
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(text_mut)
                        .child(brief),
                ),
        );
    }

    let a_blank = on_action.clone();
    div()
        .id("wfn-start-overlay")
        .absolute()
        .inset_0()
        .pt(px(56.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(16.0))
        .px(px(24.0))
        .child(
            div()
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_pri)
                .child(tr_str(lang, "从模板开始", "Start from a template")),
        )
        .child(grid)
        .child(
            div()
                .id("wfn-start-blank-btn")
                .cursor_pointer()
                .mt(px(4.0))
                .h(px(34.0))
                .px(px(14.0))
                .rounded(px(10.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .hover(move |s| s.bg(bg_hover))
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(text_pri)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_blank(this, WorkflowUiAction::StartBlankCanvas, cx);
                    }),
                )
                .child("+")
                .child(tr_str(lang, "空白画布", "Blank canvas")),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_top_hud_bar<V: 'static>(
    state: &WorkflowCanvasState,
    palette: &OpenBotPalette,
    lang: &str,
    previewing: bool,
    loop_set: &BTreeSet<String>,
    bg_surface: Rgba,
    bg_raised: Rgba,
    bg_hover: Rgba,
    bg_active: Rgba,
    border_hair: Rgba,
    border_strong: Rgba,
    text_pri: Rgba,
    text_sec: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    on_accent: Rgba,
    success: Rgba,
    warning: Rgba,
    danger: Rgba,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Stateful<Div> {
    let is_zh = lang.starts_with("zh");
    let a_toggle_sw = on_action.clone();
    let a_commit = on_action.clone();
    let a_cancel = on_action.clone();

    // Top-left Pill #1: WorkflowCorner (`所有工作流 [N] ▾` or inline Rename input)
    let corner_pill = if state.active_text_input == WorkflowTextInputTarget::RenameWorkflow {
        div()
            .id("wfn-corner-rename")
            .h(px(36.0))
            .px(px(8.0))
            .rounded(px(12.0))
            .bg(bg_surface)
            .border_1()
            .border_color(accent)
            .flex()
            .items_center()
            .gap(px(6.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .min_w(px(140.0))
                    .max_w(px(220.0))
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(text_pri)
                    .child(format!("{}|", state.input_buffer)),
            )
            .child(
                div()
                    .id("wfn-corner-rename-ok")
                    .cursor_pointer()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .bg(accent)
                    .text_size(px(11.0))
                    .text_color(on_accent)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_commit(this, WorkflowUiAction::CommitTextInput, cx);
                        }),
                    )
                    .child(tr_str(lang, "确定", "Save")),
            )
            .child(
                div()
                    .id("wfn-corner-rename-cancel")
                    .cursor_pointer()
                    .px(px(5.0))
                    .text_size(px(11.5))
                    .text_color(text_mut)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_cancel(this, WorkflowUiAction::CancelTextInput, cx);
                        }),
                    )
                    .child("×"),
            )
    } else {
        div()
            .id("wfn-corner-trigger")
            .relative()
            .h(px(36.0))
            .px(px(10.0))
            .rounded(px(12.0))
            .bg(bg_surface)
            .border_1()
            .border_color(border_strong)
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .flex()
            .items_center()
            .gap(px(6.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_toggle_sw(this, WorkflowUiAction::ToggleSwitcher, cx);
                }),
            )
            .child(tg_icon_rgba(IconName::WorkflowWaypoints, 14.0, text_sec))
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_pri)
                    .child(tr_str(lang, "所有工作流", "All workflows")),
            )
            .child(
                div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded(px(5.0))
                    .bg(bg_raised)
                    .text_size(px(11.0))
                    .text_color(text_mut)
                    .child(format!("{}", state.library.len().max(1))),
            )
            .child(tg_icon_rgba(IconName::ChevronDown, 12.0, text_mut))
    };

    // Top-left Pill #2: WorkflowTabs strip + `+`
    let mut tabs_bar = div()
        .id("wfn-tabs-pill")
        .h(px(36.0))
        .px(px(4.0))
        .rounded(px(12.0))
        .bg(bg_surface)
        .border_1()
        .border_color(border_strong)
        .flex()
        .items_center()
        .gap(px(2.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

    let tab_ids: Vec<String> = if state.open_tab_ids.is_empty() && !state.workflow_id.is_empty() {
        vec![state.workflow_id.clone()]
    } else {
        state.open_tab_ids.clone()
    };
    let show_close = tab_ids.len() > 1;
    for tid in tab_ids {
        let is_cur = tid == state.workflow_id;
        let tab_title = if is_cur {
            state.title.clone()
        } else {
            state
                .library
                .iter()
                .find(|w| w.id == tid)
                .map(|w| w.title.clone())
                .unwrap_or_else(|| tid.clone())
        };
        let a_sel = on_action.clone();
        let a_mid = on_action.clone();
        let a_close = on_action.clone();
        let tid_sel = tid.clone();
        let tid_mid = tid.clone();
        let tid_close = tid.clone();
        tabs_bar = tabs_bar.child(
            div()
                .id(SharedString::from(format!("wfn-tab-{tid}")))
                .h(px(28.0))
                .max_w(px(176.0))
                .pl(px(8.0))
                .pr(px(if show_close { 4.0 } else { 9.0 }))
                .rounded(px(8.0))
                .cursor_pointer()
                .when(is_cur, |el| el.bg(bg_active))
                .when(!is_cur, |el| el.hover(move |s| s.bg(bg_hover)))
                .flex()
                .items_center()
                .gap(px(5.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_sel(this, WorkflowUiAction::SelectWorkflow(tid_sel.clone()), cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_mid(this, WorkflowUiAction::CloseTab(tid_mid.clone()), cx);
                    }),
                )
                .child(tg_icon_rgba(
                    IconName::WorkflowWaypoints,
                    13.0,
                    if is_cur { text_pri } else { text_mut },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(12.5))
                        .font_weight(if is_cur {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if is_cur { text_pri } else { text_mut })
                        .child(tab_title),
                )
                .when(show_close, |el| {
                    el.child(
                        div()
                            .id(SharedString::from(format!("wfn-tab-close-{tid}")))
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(move |s| s.bg(bg_hover))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.0))
                            .text_color(text_mut)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_close(this, WorkflowUiAction::CloseTab(tid_close.clone()), cx);
                                }),
                            )
                            .child("×"),
                    )
                }),
        );
    }

    let a_new_tab = on_action.clone();
    tabs_bar = tabs_bar.child(
        div()
            .id("wfn-tab-new-btn")
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(15.0))
            .text_color(text_mut)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_new_tab(this, WorkflowUiAction::CreateNewWorkflow, cx);
                }),
            )
            .child("+"),
    );

    let mut left_group = div()
        .relative()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(corner_pill)
        .child(tabs_bar);

    if state.switcher_open {
        left_group = left_group.child(render_workflow_switcher_menu(
            state,
            lang,
            bg_surface,
            bg_raised,
            bg_hover,
            bg_active,
            border_hair,
            border_strong,
            text_pri,
            text_mut,
            accent,
            danger,
            cx,
            on_action,
        ));
    }

    // Top-right HUD group (`WorkflowCanvasHeader`)
    let mut right_group = div().relative().flex().items_center().gap(px(8.0));

    if !loop_set.is_empty() {
        right_group = right_group.child(
            div()
                .h(px(36.0))
                .px(px(10.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(warning)
                .flex()
                .items_center()
                .gap(px(5.0))
                .text_size(px(11.5))
                .text_color(warning)
                .child(format!(
                    "{} {}",
                    loop_set.len(),
                    tr_str(lang, "条回环边", "loop edge(s)")
                )),
        );
    }

    if let Some(run_num) = state.viewing_run_number {
        let a_latest = on_action.clone();
        right_group = right_group.child(
            div()
                .h(px(36.0))
                .px(px(10.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .items_center()
                .gap(px(6.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_pri)
                        .child(if is_zh {
                            format!("第 {run_num} 次运行")
                        } else {
                            format!("Run #{run_num}")
                        }),
                )
                .child(
                    div()
                        .id("wfn-view-latest-btn")
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(text_mut)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_latest(this, WorkflowUiAction::ViewRun(None), cx);
                            }),
                        )
                        .child("×"),
                ),
        );
    }

    if state.paused {
        let a_resume = on_action.clone();
        right_group = right_group.child(
            div()
                .h(px(36.0))
                .px(px(10.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .items_center()
                .gap(px(6.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(warning)
                        .child(tr_str(lang, "已暂停", "Paused")),
                )
                .child(
                    div()
                        .id("wfn-hud-resume-btn")
                        .cursor_pointer()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .bg(bg_hover)
                        .text_size(px(11.5))
                        .text_color(text_pri)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_resume(this, WorkflowUiAction::TogglePaused, cx);
                            }),
                        )
                        .child("▶"),
                ),
        );
    }

    // Live approval / failed / done status pill
    if !previewing {
        if let Some((ref run_id, ref appr_nid)) = state.pending_approval {
            let a_focus = on_action.clone();
            let a_ok = on_action.clone();
            let a_no = on_action.clone();
            let nid_f = appr_nid.clone();
            let rid_ok = run_id.clone();
            let nid_ok = appr_nid.clone();
            let rid_no = run_id.clone();
            let nid_no = appr_nid.clone();
            right_group = right_group.child(
                div()
                    .h(px(36.0))
                    .px(px(10.0))
                    .rounded(px(12.0))
                    .bg(bg_surface)
                    .border_1()
                    .border_color(warning)
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .id("wfn-hud-approval-focus")
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(warning)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_focus(this, WorkflowUiAction::FocusNode(nid_f.clone()), cx);
                                }),
                            )
                            .child(format!(
                                "{} {}",
                                tr_str(lang, "等待审批", "Approval"),
                                appr_nid
                            )),
                    )
                    .child(
                        div()
                            .id("wfn-hud-approve-btn")
                            .cursor_pointer()
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(6.0))
                            .bg(success)
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0x0b160e))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_ok(
                                        this,
                                        WorkflowUiAction::DecideApproval {
                                            run_id: rid_ok.clone(),
                                            node_id: nid_ok.clone(),
                                            approve: true,
                                        },
                                        cx,
                                    );
                                }),
                            )
                            .child(tr_str(lang, "批准", "Approve")),
                    )
                    .child(
                        div()
                            .id("wfn-hud-reject-btn")
                            .cursor_pointer()
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(6.0))
                            .bg(bg_hover)
                            .text_size(px(11.0))
                            .text_color(danger)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_no(
                                        this,
                                        WorkflowUiAction::DecideApproval {
                                            run_id: rid_no.clone(),
                                            node_id: nid_no.clone(),
                                            approve: false,
                                        },
                                        cx,
                                    );
                                }),
                            )
                            .child(tr_str(lang, "拒绝", "Reject")),
                    ),
            );
        }

        // Combined History + Run Workflow pill
        let has_steps = state.graph.nodes.iter().any(|n| n.kind.is_step());
        let a_hist = on_action.clone();
        let a_run = on_action.clone();
        let mut run_pill = div()
            .h(px(36.0))
            .px(px(4.0))
            .rounded(px(12.0))
            .bg(bg_surface)
            .border_1()
            .border_color(border_strong)
            .flex()
            .items_center()
            .gap(px(4.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        if !state.past_runs.is_empty() {
            run_pill = run_pill
                .child(
                    div()
                        .id("wfn-hud-history-btn")
                        .h(px(28.0))
                        .px(px(8.0))
                        .rounded(px(8.0))
                        .cursor_pointer()
                        .hover(move |s| s.bg(bg_hover))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .text_size(px(12.0))
                        .text_color(text_pri)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_hist(this, WorkflowUiAction::ToggleRunsMenu, cx);
                            }),
                        )
                        .child(tg_icon_rgba(IconName::ClockSending, 13.0, text_mut))
                        .child(tr_str(lang, "历史", "History"))
                        .child(
                            div()
                                .px(px(5.0))
                                .rounded(px(4.0))
                                .bg(bg_raised)
                                .text_size(px(10.5))
                                .text_color(text_mut)
                                .child(format!("{}", state.past_runs.len())),
                        ),
                )
                .child(div().w(px(1.0)).h(px(16.0)).bg(border_hair));
        }

        run_pill = run_pill.child(
            div()
                .id("wfn-hud-run-btn")
                .h(px(28.0))
                .px(px(11.0))
                .rounded(px(8.0))
                .when(has_steps, |el| {
                    el.cursor_pointer()
                        .bg(accent)
                        .text_color(on_accent)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_run(
                                    this,
                                    WorkflowUiAction::RunWorkflow { from_node_id: None },
                                    cx,
                                );
                            }),
                        )
                })
                .when(!has_steps, |el| {
                    el.bg(bg_raised).text_color(text_mut)
                })
                .flex()
                .items_center()
                .gap(px(5.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .child(tg_icon_rgba(
                    IconName::Play,
                    12.0,
                    if has_steps { on_accent } else { text_mut },
                ))
                .child(tr_str(lang, "运行工作流", "Run workflow")),
        );

        right_group = right_group.child(run_pill);

        if state.runs_menu_open {
            let mut menu = div()
                .id("wfn-runs-history-menu")
                .absolute()
                .top(px(44.0))
                .right(px(0.0))
                .w(px(240.0))
                .max_h(px(300.0))
                .overflow_y_scroll()
                .p(px(5.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .flex_col()
                .gap(px(2.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            let a_lat = on_action.clone();
            menu = menu
                .child(
                    div()
                        .id("wfn-run-item-latest")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(7.0))
                        .hover(move |s| s.bg(bg_hover))
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(12.0))
                        .text_color(text_pri)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_lat(this, WorkflowUiAction::ViewRun(None), cx);
                            }),
                        )
                        .child(tr_str(lang, "最新结果", "Latest results"))
                        .when(state.viewing_run_number.is_none(), |el| {
                            el.child(tg_icon_rgba(IconName::SelectCheck, 12.0, accent))
                        }),
                )
                .child(div().h(px(1.0)).mx(px(4.0)).my(px(2.0)).bg(border_hair));

            for run in &state.past_runs {
                let num = run.number;
                let selected = state.viewing_run_number == Some(num);
                let a_sel = on_action.clone();
                let st_col = match run.status {
                    WorkflowRunStatus::Done => success,
                    WorkflowRunStatus::Failed => danger,
                    WorkflowRunStatus::Running => accent,
                    WorkflowRunStatus::Approval | WorkflowRunStatus::Stopping => warning,
                    WorkflowRunStatus::Stopped => text_mut,
                };
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("wfn-run-item-{num}")))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(7.0))
                        .hover(move |s| s.bg(bg_hover))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(6.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_sel(this, WorkflowUiAction::ViewRun(Some(num)), cx);
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_pri)
                                .child(if is_zh {
                                    format!("第 {num} 次运行")
                                } else {
                                    format!("Run #{num}")
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(st_col)
                                        .child(format!("{:?}", run.status)),
                                )
                                .when(selected, |el| {
                                    el.child(tg_icon_rgba(IconName::SelectCheck, 12.0, accent))
                                }),
                        ),
                );
            }
            right_group = right_group.child(menu);
        }
    }

    let _ = palette;
    div()
        .id("wfn-hud-top-bar")
        .absolute()
        .top(px(14.0))
        .left(px(14.0))
        .right(px(14.0))
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.0))
        .child(left_group)
        .child(right_group)
}

#[allow(clippy::too_many_arguments)]
fn render_workflow_switcher_menu<V: 'static>(
    state: &WorkflowCanvasState,
    lang: &str,
    bg_surface: Rgba,
    bg_raised: Rgba,
    bg_hover: Rgba,
    bg_active: Rgba,
    border_hair: Rgba,
    border_strong: Rgba,
    text_pri: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    danger: Rgba,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Stateful<Div> {
    let is_zh = lang.starts_with("zh");
    let valid_folder_ids: BTreeSet<&str> = state.folders.iter().map(|f| f.id.as_str()).collect();
    let folder_of = |wf_id: &str| -> Option<&str> {
        state
            .folder_placement
            .get(wf_id)
            .map(String::as_str)
            .filter(|fid| valid_folder_ids.contains(fid))
    };

    let mut menu = div()
        .id("wfn-switcher-dropdown")
        .absolute()
        .top(px(44.0))
        .left(px(0.0))
        .w(px(268.0))
        .max_h(px(460.0))
        .overflow_y_scroll()
        .p(px(6.0))
        .rounded(px(12.0))
        .bg(bg_surface)
        .border_1()
        .border_color(border_strong)
        .flex()
        .flex_col()
        .gap(px(2.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_mut)
                .child(tr_str(lang, "工作流", "Workflows")),
        );

    // Folders section
    for folder in &state.folders {
        let fid = folder.id.clone();
        let expanded = state.switcher_expanded_folder.as_deref() == Some(fid.as_str());
        let in_folder: Vec<&WorkflowSummary> = state
            .library
            .iter()
            .filter(|w| folder_of(&w.id) == Some(fid.as_str()))
            .collect();
        let a_exp = on_action.clone();
        let a_ren = on_action.clone();
        let a_del = on_action.clone();
        let fid_exp = fid.clone();
        let fid_ren = fid.clone();
        let fid_del = fid.clone();
        menu = menu.child(
            div()
                .id(SharedString::from(format!("wfn-folder-row-{fid}")))
                .cursor_pointer()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(bg_hover))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_exp(this, WorkflowUiAction::ToggleFolderExpand(fid_exp.clone()), cx);
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon_rgba(IconName::FolderAll, 14.0, text_mut))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.5))
                                .text_color(text_pri)
                                .child(folder.name.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_mut)
                                .child(format!("{}", in_folder.len())),
                        )
                        .child(tg_icon_rgba(
                            if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            },
                            11.0,
                            text_mut,
                        )),
                ),
        );
        if expanded {
            for wf in in_folder {
                menu = menu.child(render_switcher_workflow_row(
                    wf,
                    &state.workflow_id,
                    true,
                    is_zh,
                    bg_hover,
                    bg_active,
                    text_pri,
                    text_mut,
                    accent,
                    cx,
                    on_action,
                ));
            }
            menu = menu.child(
                div()
                    .pl(px(22.0))
                    .pr(px(8.0))
                    .py(px(3.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("wfn-folder-ren-{fid}")))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(text_mut)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_ren(
                                        this,
                                        WorkflowUiAction::StartRenameFolder(fid_ren.clone()),
                                        cx,
                                    );
                                }),
                            )
                            .child(tr_str(lang, "重命名", "Rename")),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("wfn-folder-del-{fid}")))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(danger)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_del(
                                        this,
                                        WorkflowUiAction::DeleteFolder(fid_del.clone()),
                                        cx,
                                    );
                                }),
                            )
                            .child(tr_str(lang, "删除文件夹", "Delete folder")),
                    ),
            );
        }
    }

    // Root workflows (unassigned to a folder)
    for wf in state.library.iter().filter(|w| folder_of(&w.id).is_none()) {
        menu = menu.child(render_switcher_workflow_row(
            wf,
            &state.workflow_id,
            false,
            is_zh,
            bg_hover,
            bg_active,
            text_pri,
            text_mut,
            accent,
            cx,
            on_action,
        ));
    }

    // Create + Import
    let a_new = on_action.clone();
    let a_imp = on_action.clone();
    menu = menu
        .child(
            div()
                .id("wfn-sw-new")
                .cursor_pointer()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(bg_hover))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(accent)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_new(this, WorkflowUiAction::CreateNewWorkflow, cx);
                    }),
                )
                .child("+")
                .child(tr_str(lang, "新建工作流", "New workflow")),
        )
        .child(
            div()
                .id("wfn-sw-import")
                .cursor_pointer()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(bg_hover))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(12.5))
                .text_color(text_pri)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_imp(this, WorkflowUiAction::OpenImportDialog, cx);
                    }),
                )
                .child(tg_icon_rgba(IconName::ArrowUp, 13.0, text_mut))
                .child(tr_str(lang, "导入工作流", "Import workflow")),
        )
        .child(div().h(px(1.0)).mx(px(4.0)).my(px(3.0)).bg(border_hair));

    // Version history collapsible section
    if !state.versions.is_empty() {
        let a_ver_tog = on_action.clone();
        menu = menu.child(
            div()
                .id("wfn-sw-versions-toggle")
                .cursor_pointer()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(bg_hover))
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_ver_tog(this, WorkflowUiAction::ToggleVersionsSubmenu, cx);
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(12.5))
                        .text_color(text_pri)
                        .child(tg_icon_rgba(IconName::ClockSending, 13.0, text_mut))
                        .child(tr_str(lang, "版本历史", "Version history")),
                )
                .child(tg_icon_rgba(
                    if state.switcher_versions_open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    },
                    11.0,
                    text_mut,
                )),
        );
        if state.switcher_versions_open {
            for ver in state.versions.iter().take(8) {
                let vid = ver.id.clone();
                let active_v = state.preview_version_id.as_deref() == Some(vid.as_str());
                let a_pv = on_action.clone();
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("wfn-sw-ver-{vid}")))
                        .cursor_pointer()
                        .pl(px(24.0))
                        .pr(px(8.0))
                        .py(px(5.0))
                        .rounded(px(6.0))
                        .when(active_v, |el| el.bg(bg_active))
                        .hover(move |s| s.bg(bg_hover))
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(11.5))
                        .text_color(text_pri)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_pv(this, WorkflowUiAction::PreviewVersion(vid.clone()), cx);
                            }),
                        )
                        .child(format!("r{} ({:?})", ver.revision, ver.kind))
                        .child(div().text_color(text_mut).child(ver.label.clone())),
                );
            }
        }
    }

    // Rename, Move to folder, Pause/Resume, Duplicate, Export, Delete
    let a_ren = on_action.clone();
    let a_mv_tog = on_action.clone();
    let a_pause = on_action.clone();
    let a_dup = on_action.clone();
    let a_exp = on_action.clone();
    let a_del = on_action.clone();

    menu = menu
        .child(
            div()
                .id("wfn-sw-rename")
                .cursor_pointer()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(bg_hover))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(12.5))
                .text_color(text_pri)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_ren(this, WorkflowUiAction::StartRenameWorkflow, cx);
                    }),
                )
                .child(tg_icon_rgba(IconName::Edit, 13.0, text_mut))
                .child(tr_str(lang, "重命名", "Rename")),
        )
        .child(
            div()
                .id("wfn-sw-move-toggle")
                .cursor_pointer()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(bg_hover))
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_mv_tog(this, WorkflowUiAction::ToggleMoveSubmenu, cx);
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(12.5))
                        .text_color(text_pri)
                        .child(tg_icon_rgba(IconName::FolderWork, 13.0, text_mut))
                        .child(tr_str(lang, "移动到文件夹", "Move to folder")),
                )
                .child(tg_icon_rgba(
                    if state.switcher_move_open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    },
                    11.0,
                    text_mut,
                )),
        );

    if state.switcher_move_open {
        let cur_folder = folder_of(&state.workflow_id);
        for folder in &state.folders {
            let fid = folder.id.clone();
            let checked = cur_folder == Some(fid.as_str());
            let a_place = on_action.clone();
            menu = menu.child(
                div()
                    .id(SharedString::from(format!("wfn-sw-mv-{fid}")))
                    .cursor_pointer()
                    .pl(px(24.0))
                    .pr(px(8.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .hover(move |s| s.bg(bg_hover))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.0))
                    .text_color(text_pri)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_place(
                                this,
                                WorkflowUiAction::MoveWorkflowToFolder(Some(fid.clone())),
                                cx,
                            );
                        }),
                    )
                    .child(folder.name.clone())
                    .when(checked, |el| {
                        el.child(tg_icon_rgba(IconName::SelectCheck, 12.0, accent))
                    }),
            );
        }
        let a_none = on_action.clone();
        let a_newf = on_action.clone();
        menu = menu
            .child(
                div()
                    .id("wfn-sw-mv-none")
                    .cursor_pointer()
                    .pl(px(24.0))
                    .pr(px(8.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .hover(move |s| s.bg(bg_hover))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.0))
                    .text_color(text_mut)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_none(this, WorkflowUiAction::MoveWorkflowToFolder(None), cx);
                        }),
                    )
                    .child(tr_str(lang, "无文件夹", "No folder"))
                    .when(cur_folder.is_none(), |el| {
                        el.child(tg_icon_rgba(IconName::SelectCheck, 12.0, accent))
                    }),
            )
            .child(
                div()
                    .id("wfn-sw-new-folder")
                    .cursor_pointer()
                    .pl(px(24.0))
                    .pr(px(8.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .hover(move |s| s.bg(bg_hover))
                    .text_size(px(12.0))
                    .text_color(accent)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_newf(this, WorkflowUiAction::StartNewFolder, cx);
                        }),
                    )
                    .child(tr_str(lang, "+ 新建文件夹", "+ New folder")),
            );
    }

    let _ = bg_raised;
    menu.child(
        div()
            .id("wfn-sw-pause")
            .cursor_pointer()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(7.0))
            .hover(move |s| s.bg(bg_hover))
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(12.5))
            .text_color(text_pri)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_pause(this, WorkflowUiAction::TogglePaused, cx);
                }),
            )
            .child(tg_icon_rgba(
                if state.paused { IconName::Play } else { IconName::Pause },
                13.0,
                text_mut,
            ))
            .child(if state.paused {
                tr_str(lang, "恢复", "Resume")
            } else {
                tr_str(lang, "暂停", "Pause")
            }),
    )
    .child(
        div()
            .id("wfn-sw-dup")
            .cursor_pointer()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(7.0))
            .hover(move |s| s.bg(bg_hover))
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(12.5))
            .text_color(text_pri)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_dup(this, WorkflowUiAction::DuplicateCurrent, cx);
                }),
            )
            .child(tg_icon_rgba(IconName::Copy, 13.0, text_mut))
            .child(tr_str(lang, "创建副本", "Duplicate")),
    )
    .child(
        div()
            .id("wfn-sw-export")
            .cursor_pointer()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(7.0))
            .hover(move |s| s.bg(bg_hover))
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(12.5))
            .text_color(text_pri)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_exp(this, WorkflowUiAction::ExportCurrent, cx);
                }),
            )
            .child(tg_icon_rgba(IconName::FileDoc, 13.0, text_mut))
            .child(tr_str(lang, "导出", "Export")),
    )
    .child(div().h(px(1.0)).mx(px(4.0)).my(px(3.0)).bg(border_hair))
    .child(
        div()
            .id("wfn-sw-delete")
            .cursor_pointer()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(7.0))
            .hover(move |s| s.bg(bg_hover))
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(danger)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_del(this, WorkflowUiAction::DeleteCurrentWorkflow, cx);
                }),
            )
            .child(tg_icon_rgba(IconName::DeleteTrash, 13.0, danger))
            .child(tr_str(lang, "删除工作流", "Delete workflow")),
    )
}

#[allow(clippy::too_many_arguments)]
fn render_switcher_workflow_row<V: 'static>(
    wf: &WorkflowSummary,
    current_id: &str,
    indented: bool,
    is_zh: bool,
    bg_hover: Rgba,
    bg_active: Rgba,
    text_pri: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Stateful<Div> {
    let is_cur = wf.id == current_id;
    let wid = wf.id.clone();
    let a_open = on_action.clone();
    let trailing = if wf.paused == Some(true) {
        if is_zh {
            "已暂停".to_string()
        } else {
            "Paused".to_string()
        }
    } else if is_zh {
        format!("{} 个步骤", wf.steps)
    } else {
        format!("{} steps", wf.steps)
    };
    div()
        .id(SharedString::from(format!("wfn-sw-item-{}", wf.id)))
        .cursor_pointer()
        .pl(px(if indented { 22.0 } else { 8.0 }))
        .pr(px(8.0))
        .py(px(6.0))
        .rounded(px(7.0))
        .when(is_cur, |el| el.bg(bg_active))
        .when(!is_cur, |el| el.hover(move |s| s.bg(bg_hover)))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                a_open(this, WorkflowUiAction::SelectWorkflow(wid.clone()), cx);
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .w(px(14.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(is_cur, |el| {
                            el.child(tg_icon_rgba(IconName::SelectCheck, 12.0, accent))
                        }),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(text_pri)
                        .child(wf.title.clone()),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(11.0))
                .text_color(text_mut)
                .child(trailing),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_interactive_node_card<V: 'static>(
    node: &WorkflowNode,
    state: &WorkflowCanvasState,
    hud_step: Option<&WorkflowHudStep>,
    palette: &OpenBotPalette,
    lang: &str,
    now_sec: f64,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
    on_node_down: &Rc<dyn Fn(&mut V, String, bool, f32, f32, &mut Context<V>)>,
) -> Stateful<Div> {
    let is_dark = palette.scheme == OpenBotColorScheme::Dark;
    let (nw, nh) = node_dimensions(node);
    let sx = (node.at.x as f32) * state.zoom + state.pan_x;
    let sy = (node.at.y as f32) * state.zoom + state.pan_y;
    let sw = (nw * state.zoom).max(160.0);
    let sh = (nh * state.zoom).max(88.0);
    let selected = state.selected_node_ids.iter().any(|id| id == &node.id);
    let is_locked = node.locked == Some(true);
    let is_connecting_target = state
        .connecting_from
        .as_ref()
        .is_some_and(|c| c.from_node_id != node.id && !node.kind.ports().inputs.is_empty());

    let accent = node_kind_accent(node.kind, palette);
    let text_pri = pal_rgba(palette, "--openbot-text-primary", if is_dark { 0xffffff } else { 0x141414 });
    let text_sec = pal_rgba(palette, "--openbot-text-secondary", if is_dark { 0xd4d4d8 } else { 0x3f3f46 });
    let text_mut = pal_rgba(palette, "--openbot-text-muted", if is_dark { 0x979797 } else { 0x71717a });
    let border_col = if selected {
        accent
    } else if is_connecting_target {
        pal_rgba(palette, "--openbot-accent", 0x007cf7)
    } else {
        pal_rgba(
            palette,
            "--openbot-border-strong",
            if is_dark { 0x343438 } else { 0xd8d8dc },
        )
    };
    let card_bg = match node.kind {
        WorkflowNodeKind::Note => {
            if is_dark {
                rgb(0x2a2310)
            } else {
                rgb(0xfef9e7)
            }
        }
        WorkflowNodeKind::Shape => pal_rgba(
            palette,
            "--openbot-bg-surface-raised",
            if is_dark { 0x242428 } else { 0xf2f2f5 },
        ),
        _ => pal_rgba(
            palette,
            "--openbot-bg-surface",
            if is_dark { 0x1e1e1f } else { 0xffffff },
        ),
    };
    let field_bg = pal_rgba(
        palette,
        "--openbot-bg-surface-raised",
        if is_dark { 0x262629 } else { 0xf4f4f6 },
    );
    let ports = node.kind.ports();

    // External OpenBot `.wfn-label` header directly above `.wfn-card`
    let mut external_label = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .px(px(4.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(if node.kind == WorkflowNodeKind::Agent {
                    let mut av_props =
                        AgentAvatarProps::new(node.agent_id().unwrap_or(node.id.as_str()))
                            .with_size(16.0);
                    av_props.dark_scheme = is_dark;
                    av_props.reduced_motion = state.reduced_motion;
                    if let Some(step) = hud_step {
                        av_props.mood = match step.status {
                            WorkflowStepStatus::Running => AvatarMood::Working,
                            WorkflowStepStatus::Approval => AvatarMood::Waiting,
                            WorkflowStepStatus::Failed => AvatarMood::Failed,
                            WorkflowStepStatus::Done => AvatarMood::Responded,
                            _ => AvatarMood::Idle,
                        };
                    }
                    render_agent_avatar(&av_props, None, now_sec).into_any_element()
                } else {
                    tg_icon_rgba(node_kind_icon(node.kind), 13.0, accent).into_any_element()
                })
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if selected { text_pri } else { text_mut })
                        .child(localized_kind_label(node.kind, lang)),
                )
                .when(is_locked, |el| {
                    el.child(tg_icon_rgba(IconName::Lock, 11.0, text_mut))
                }),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(text_mut)
                .child(node.id.clone()),
        );
    if let Some(step) = hud_step {
        let sc = step_status_color(step.status, palette);
        external_label = external_label.child(
            div()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(999.0))
                .bg(Rgba { a: 0.16, ..sc })
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(sc)
                .child(format!("{:?}", step.status)),
        );
    }

    // Ports row with live click-to-connect handlers
    let mut in_ports = div().flex().items_center().gap(px(4.0));
    for (pi, &p) in ports.inputs.iter().enumerate() {
        let a_conn = on_action.clone();
        let target_id = node.id.clone();
        let pc = port_color(p, palette);
        in_ports = in_ports.child(
            div()
                .id(SharedString::from(format!("wfn-in-{}-{pi}", node.id)))
                .cursor_pointer()
                .px(px(6.0))
                .py(px(1.5))
                .rounded(px(999.0))
                .bg(if is_connecting_target {
                    Rgba { a: 0.2, ..pc }
                } else {
                    field_bg
                })
                .border_1()
                .border_color(pc)
                .text_size(px(9.5))
                .text_color(pc)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_conn(
                            this,
                            WorkflowUiAction::CompleteConnectTo {
                                target_node_id: target_id.clone(),
                            },
                            cx,
                        );
                    }),
                )
                .child(format!("● {}", p.as_str())),
        );
    }

    let mut out_ports = div().flex().items_center().gap(px(4.0));
    if node.kind == WorkflowNodeKind::Condition {
        let success = pal_rgba(palette, "--openbot-success", 0x31cf76);
        let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);
        for (br, lbl, col) in [
            (WorkflowBranch::Yes, tr_str(lang, "是", "Yes"), success),
            (WorkflowBranch::No, tr_str(lang, "否", "No"), danger),
        ] {
            let a_start = on_action.clone();
            let nid = node.id.clone();
            out_ports = out_ports.child(
                div()
                    .id(SharedString::from(format!("wfn-out-{}-{}", node.id, br.as_str())))
                    .cursor_pointer()
                    .px(px(7.0))
                    .py(px(1.5))
                    .rounded(px(999.0))
                    .bg(Rgba { a: 0.16, ..col })
                    .border_1()
                    .border_color(col)
                    .text_size(px(9.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(col)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_start(
                                this,
                                WorkflowUiAction::StartConnectFrom {
                                    node_id: nid.clone(),
                                    port_type: WorkflowPortType::Text,
                                    branch: Some(br),
                                },
                                cx,
                            );
                        }),
                    )
                    .child(format!("{lbl} ●")),
            );
        }
    } else {
        for (pi, &p) in ports.outputs.iter().enumerate() {
            let a_start = on_action.clone();
            let nid = node.id.clone();
            let pc = port_color(p, palette);
            out_ports = out_ports.child(
                div()
                    .id(SharedString::from(format!("wfn-out-{}-{pi}", node.id)))
                    .cursor_pointer()
                    .px(px(6.0))
                    .py(px(1.5))
                    .rounded(px(999.0))
                    .bg(field_bg)
                    .border_1()
                    .border_color(pc)
                    .text_size(px(9.5))
                    .text_color(pc)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_start(
                                this,
                                WorkflowUiAction::StartConnectFrom {
                                    node_id: nid.clone(),
                                    port_type: p,
                                    branch: None,
                                },
                                cx,
                            );
                        }),
                    )
                    .child(format!("{} ●", p.as_str())),
            );
        }
    }

    // Inline approval buttons right on Approval node card when paused at this node
    let approval_pair = state
        .pending_approval
        .as_ref()
        .filter(|(_, nid)| nid == &node.id)
        .cloned();

    let mut card = div()
        .w_full()
        .min_h(px(sh - 22.0))
        .p(px(8.0))
        .rounded(px(16.0))
        .bg(card_bg)
        .border_1()
        .border_color(border_col)
        .flex()
        .flex_col()
        .justify_between()
        .gap(px(7.0))
        .child(
            div()
                .w_full()
                .p(px(9.0))
                .rounded(px(10.0))
                .bg(field_bg)
                .text_size(px(12.0))
                .text_color(text_sec)
                .child(node_summary_text(node)),
        );

    if let Some((run_id, appr_nid)) = approval_pair {
        let success = pal_rgba(palette, "--openbot-success", 0x31cf76);
        let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);
        let a_ok = on_action.clone();
        let a_no = on_action.clone();
        let r1 = run_id.clone();
        let n1 = appr_nid.clone();
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id(SharedString::from(format!("wfn-node-approve-{}", node.id)))
                        .cursor_pointer()
                        .flex_1()
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(success)
                        .flex()
                        .justify_center()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0x0b160e))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_ok(
                                    this,
                                    WorkflowUiAction::DecideApproval {
                                        run_id: r1.clone(),
                                        node_id: n1.clone(),
                                        approve: true,
                                    },
                                    cx,
                                );
                            }),
                        )
                        .child(tr_str(lang, "批准", "Approve")),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("wfn-node-reject-{}", node.id)))
                        .cursor_pointer()
                        .flex_1()
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(field_bg)
                        .border_1()
                        .border_color(danger)
                        .flex()
                        .justify_center()
                        .text_size(px(11.5))
                        .text_color(danger)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_no(
                                    this,
                                    WorkflowUiAction::DecideApproval {
                                        run_id: run_id.clone(),
                                        node_id: appr_nid.clone(),
                                        approve: false,
                                    },
                                    cx,
                                );
                            }),
                        )
                        .child(tr_str(lang, "拒绝", "Reject")),
                ),
        );
    } else if let Some(step) = hud_step {
        if let Some(ref preview) = step.output_preview {
            if !preview.trim().is_empty() {
                let clipped: String = if preview.chars().count() > 72 {
                    format!("{}…", preview.chars().take(71).collect::<String>())
                } else {
                    preview.clone()
                };
                card = card.child(
                    div()
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(8.0))
                        .bg(Rgba {
                            a: 0.1,
                            ..step_status_color(step.status, palette)
                        })
                        .text_size(px(10.5))
                        .text_color(text_sec)
                        .child(clipped),
                );
            }
        }
    }

    card = card.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(in_ports)
            .child(out_ports),
    );

    let nd = on_node_down.clone();
    let nid_down = node.id.clone();
    div()
        .id(SharedString::from(format!("wfn-node-{}", node.id)))
        .absolute()
        .left(px(sx))
        .top(px(sy))
        .w(px(sw))
        .cursor_grab()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                nd(
                    this,
                    nid_down.clone(),
                    ev.modifiers.shift,
                    f32::from(ev.position.x),
                    f32::from(ev.position.y),
                    cx,
                );
            }),
        )
        .child(external_label)
        .child(card)
}

#[allow(clippy::too_many_arguments)]
fn render_selected_node_inspector<V: 'static>(
    node: &WorkflowNode,
    state: &WorkflowCanvasState,
    palette: &OpenBotPalette,
    lang: &str,
    read_only: bool,
    bg_surface: Rgba,
    bg_raised: Rgba,
    bg_hover: Rgba,
    border_hair: Rgba,
    border_strong: Rgba,
    text_pri: Rgba,
    text_sec: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    danger: Rgba,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Stateful<Div> {
    let is_zh = lang.starts_with("zh");
    let (field_key, lbl_zh, lbl_en) = primary_editable_field(node.kind);
    let field_label = if is_zh { lbl_zh } else { lbl_en };
    let editing_this = matches!(
        &state.active_text_input,
        WorkflowTextInputTarget::EditNodeField { node_id, field_key: fk }
            if node_id == &node.id && fk == field_key
    );
    let current_text = if editing_this {
        format!("{}|", state.input_buffer)
    } else {
        match node.fields.get(field_key) {
            Some(WorkflowFieldValue::Text(s)) if !s.is_empty() => s.clone(),
            _ => tr_str(lang, "点击输入内容…", "Click to edit…").to_string(),
        }
    };

    let a_edit = on_action.clone();
    let a_commit = on_action.clone();
    let a_run_from = on_action.clone();
    let a_dup = on_action.clone();
    let a_lock = on_action.clone();
    let a_disc = on_action.clone();
    let a_del = on_action.clone();
    let nid_edit = node.id.clone();
    let nid_run = node.id.clone();
    let nid_disc = node.id.clone();
    let is_locked = node.locked == Some(true);

    let mut panel = div()
        .id("wfn-node-inspector")
        .absolute()
        .top(px(62.0))
        .right(px(14.0))
        .w(px(268.0))
        .p(px(12.0))
        .rounded(px(14.0))
        .bg(bg_surface)
        .border_1()
        .border_color(border_strong)
        .flex()
        .flex_col()
        .gap(px(10.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon_rgba(
                            node_kind_icon(node.kind),
                            14.0,
                            node_kind_accent(node.kind, palette),
                        ))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_pri)
                                .child(localized_kind_label(node.kind, lang)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(text_mut)
                        .child(node.id.clone()),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_mut)
                                .child(field_label),
                        )
                        .when(editing_this, |el| {
                            el.child(
                                div()
                                    .id("wfn-insp-save-field")
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(accent)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            a_commit(this, WorkflowUiAction::CommitTextInput, cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "保存 (Enter)", "Save")),
                            )
                        }),
                )
                .child(
                    div()
                        .id("wfn-insp-field-box")
                        .min_h(px(54.0))
                        .p(px(8.0))
                        .rounded(px(9.0))
                        .bg(bg_raised)
                        .border_1()
                        .border_color(if editing_this { accent } else { border_hair })
                        .when(!read_only, |el| {
                            el.cursor_text().on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_edit(
                                        this,
                                        WorkflowUiAction::StartEditNodeField {
                                            node_id: nid_edit.clone(),
                                            field_key: field_key.to_string(),
                                        },
                                        cx,
                                    );
                                }),
                            )
                        })
                        .text_size(px(12.0))
                        .text_color(text_sec)
                        .child(current_text),
                ),
        );

    // Quick option pills for Trigger / Output / Agent
    if !read_only && node.kind == WorkflowNodeKind::Trigger {
        let cur = match node.fields.get("trigger") {
            Some(WorkflowFieldValue::Text(s)) => s.as_str(),
            _ => "manual",
        };
        let mut pills = div().flex().flex_wrap().gap(px(5.0));
        for (val, lbl_z, lbl_e) in [
            ("manual", "手动", "Manual"),
            ("schedule", "定时", "Schedule"),
            ("webhook", "Webhook", "Webhook"),
            ("github", "GitHub", "GitHub"),
        ] {
            let active = cur == val;
            let a_set = on_action.clone();
            let nid = node.id.clone();
            pills = pills.child(
                div()
                    .id(SharedString::from(format!("wfn-trig-opt-{val}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .bg(if active { accent } else { bg_raised })
                    .text_size(px(11.0))
                    .text_color(if active { rgb(0xffffff) } else { text_sec })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_set(
                                this,
                                WorkflowUiAction::SetNodeFieldValue {
                                    node_id: nid.clone(),
                                    field_key: "trigger".to_string(),
                                    value: val.to_string(),
                                },
                                cx,
                            );
                        }),
                    )
                    .child(if is_zh { lbl_z } else { lbl_e }),
            );
        }
        panel = panel.child(pills);
    } else if !read_only && node.kind == WorkflowNodeKind::Output {
        let cur = match node.fields.get("target") {
            Some(WorkflowFieldValue::Text(s)) => s.as_str(),
            _ => "notify",
        };
        let mut pills = div().flex().flex_wrap().gap(px(5.0));
        for (val, lbl_z, lbl_e) in [
            ("notify", "通知", "Notify"),
            ("chat-channel", "频道", "Channel"),
            ("telegram", "Telegram", "Telegram"),
            ("github", "GitHub", "GitHub"),
        ] {
            let active = cur == val;
            let a_set = on_action.clone();
            let nid = node.id.clone();
            pills = pills.child(
                div()
                    .id(SharedString::from(format!("wfn-out-opt-{val}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .bg(if active { accent } else { bg_raised })
                    .text_size(px(11.0))
                    .text_color(if active { rgb(0xffffff) } else { text_sec })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_set(
                                this,
                                WorkflowUiAction::SetNodeFieldValue {
                                    node_id: nid.clone(),
                                    field_key: "target".to_string(),
                                    value: val.to_string(),
                                },
                                cx,
                            );
                        }),
                    )
                    .child(if is_zh { lbl_z } else { lbl_e }),
            );
        }
        panel = panel.child(pills);
    } else if !read_only
        && node.kind == WorkflowNodeKind::Agent
        && !state.available_agents.is_empty()
    {
        let cur = node.agent_id().unwrap_or("");
        let mut pills = div().flex().flex_wrap().gap(px(5.0));
        for (aid, aname) in state.available_agents.iter().take(6) {
            let active = cur == aid;
            let a_set = on_action.clone();
            let nid = node.id.clone();
            let aid_c = aid.clone();
            pills = pills.child(
                div()
                    .id(SharedString::from(format!("wfn-agent-opt-{aid}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .bg(if active { accent } else { bg_raised })
                    .text_size(px(11.0))
                    .text_color(if active { rgb(0xffffff) } else { text_sec })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_set(
                                this,
                                WorkflowUiAction::SetNodeFieldValue {
                                    node_id: nid.clone(),
                                    field_key: "agentId".to_string(),
                                    value: aid_c.clone(),
                                },
                                cx,
                            );
                        }),
                    )
                    .child(aname.clone()),
            );
        }
        panel = panel.child(pills);
    }

    if !read_only {
        panel = panel
            .child(div().h(px(1.0)).bg(border_hair))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .when(node.kind.is_step(), |el| {
                        el.child(
                            div()
                                .id("wfn-insp-run-step")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(7.0))
                                .bg(bg_raised)
                                .hover(move |s| s.bg(bg_hover))
                                .text_size(px(11.5))
                                .text_color(text_pri)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        a_run_from(
                                            this,
                                            WorkflowUiAction::RunWorkflow {
                                                from_node_id: Some(nid_run.clone()),
                                            },
                                            cx,
                                        );
                                    }),
                                )
                                .child(tr_str(lang, "▶ 从此运行", "▶ Run from here")),
                        )
                    })
                    .child(
                        div()
                            .id("wfn-insp-dup")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(7.0))
                            .bg(bg_raised)
                            .hover(move |s| s.bg(bg_hover))
                            .text_size(px(11.5))
                            .text_color(text_pri)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_dup(this, WorkflowUiAction::DuplicateSelected, cx);
                                }),
                            )
                            .child(tr_str(lang, "复制", "Duplicate")),
                    )
                    .child(
                        div()
                            .id("wfn-insp-lock")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(7.0))
                            .bg(bg_raised)
                            .hover(move |s| s.bg(bg_hover))
                            .text_size(px(11.5))
                            .text_color(text_pri)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_lock(this, WorkflowUiAction::ToggleLockSelected, cx);
                                }),
                            )
                            .child(if is_locked {
                                tr_str(lang, "解锁", "Unlock")
                            } else {
                                tr_str(lang, "锁定", "Lock")
                            }),
                    )
                    .child(
                        div()
                            .id("wfn-insp-disconnect")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(7.0))
                            .bg(bg_raised)
                            .hover(move |s| s.bg(bg_hover))
                            .text_size(px(11.5))
                            .text_color(text_mut)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_disc(
                                        this,
                                        WorkflowUiAction::DisconnectNodeEdges(nid_disc.clone()),
                                        cx,
                                    );
                                }),
                            )
                            .child(tr_str(lang, "断开连线", "Disconnect")),
                    )
                    .when(!is_locked, |el| {
                        el.child(
                            div()
                                .id("wfn-insp-delete")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(7.0))
                                .bg(bg_raised)
                                .hover(move |s| s.bg(bg_hover))
                                .text_size(px(11.5))
                                .text_color(danger)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        a_del(this, WorkflowUiAction::DeleteSelected, cx);
                                    }),
                                )
                                .child(tr_str(lang, "删除", "Delete")),
                        )
                    }),
            );
    }

    panel
}

#[allow(clippy::too_many_arguments)]
fn render_bottom_tool_dock<V: 'static>(
    state: &WorkflowCanvasState,
    palette: &OpenBotPalette,
    lang: &str,
    bg_surface: Rgba,
    bg_raised: Rgba,
    bg_hover: Rgba,
    bg_active: Rgba,
    border_hair: Rgba,
    border_strong: Rgba,
    text_pri: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    on_accent: Rgba,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Stateful<Div> {
    let mut container = div()
        .id("wfn-bottom-dock-wrap")
        .absolute()
        .bottom(px(16.0))
        .left(px(0.0))
        .right(px(0.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0));

    // Categorized `WorkflowAddMenu` popover directly above dock
    if state.add_menu_open {
        let groups: [(&str, &[(&str, &str, WorkflowNodeKind, Option<&'static str>)]); 3] = [
            (
                tr_str(lang, "输入", "Inputs"),
                &[
                    ("触发器", "Trigger", WorkflowNodeKind::Trigger, None),
                    ("提示词", "Prompt", WorkflowNodeKind::Prompt, None),
                    ("素材", "Asset", WorkflowNodeKind::Asset, None),
                ],
            ),
            (
                tr_str(lang, "步骤", "Steps"),
                &[
                    ("大模型", "LLM", WorkflowNodeKind::Llm, None),
                    ("智能体", "Agent", WorkflowNodeKind::Agent, None),
                    ("图片", "Image", WorkflowNodeKind::Image, None),
                    ("视频", "Video", WorkflowNodeKind::Image, Some("video")),
                ],
            ),
            (
                tr_str(lang, "流程", "Flow"),
                &[
                    ("条件判断", "Condition", WorkflowNodeKind::Condition, None),
                    ("人工审批", "Approval", WorkflowNodeKind::Approval, None),
                    ("输出交付", "Output", WorkflowNodeKind::Output, None),
                ],
            ),
        ];
        let is_zh = lang.starts_with("zh");
        let mut pop = div()
            .id("wfn-add-node-popover")
            .w(px(336.0))
            .p(px(10.0))
            .rounded(px(14.0))
            .bg(bg_surface)
            .border_1()
            .border_color(border_strong)
            .flex()
            .flex_col()
            .gap(px(10.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        for (group_lbl, items) in groups {
            let mut row = div().flex().flex_wrap().gap(px(6.0));
            for &(lbl_z, lbl_e, kind, preset) in items {
                let a_add = on_action.clone();
                let icon_col = node_kind_accent(kind, palette);
                row = row.child(
                    div()
                        .id(SharedString::from(format!(
                            "wfn-add-kind-{:?}-{}",
                            kind,
                            preset.unwrap_or("std")
                        )))
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(bg_raised)
                        .hover(move |s| s.bg(bg_hover))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(12.0))
                        .text_color(text_pri)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_add(this, WorkflowUiAction::AddNode { kind, preset }, cx);
                            }),
                        )
                        .child(tg_icon_rgba(node_kind_icon(kind), 13.0, icon_col))
                        .child(if is_zh { lbl_z } else { lbl_e }),
                );
            }
            pop = pop.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_mut)
                            .child(group_lbl),
                    )
                    .child(row),
            );
        }
        container = container.child(pop);
    }

    // Floating sub-toolbars (`wfn-hud-sub`) for Draw / Note / Shape
    match state.sub_tool_panel {
        WorkflowSubToolPanel::Draw => {
            let mut sub = div()
                .id("wfn-subbar-draw")
                .h(px(36.0))
                .px(px(6.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .items_center()
                .gap(px(4.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            for (tool, lbl_z, lbl_e) in [
                (WorkflowInkTool::Pen, "笔", "Pen"),
                (WorkflowInkTool::Highlighter, "记号笔", "Marker"),
                (WorkflowInkTool::Eraser, "橡皮擦", "Eraser"),
            ] {
                let active = state.active_tool == tool;
                let a_t = on_action.clone();
                sub = sub.child(
                    div()
                        .id(SharedString::from(format!("wfn-draw-{:?}", tool)))
                        .cursor_pointer()
                        .h(px(26.0))
                        .px(px(8.0))
                        .rounded(px(7.0))
                        .when(active, |el| el.bg(bg_active))
                        .when(!active, |el| el.hover(move |s| s.bg(bg_hover)))
                        .flex()
                        .items_center()
                        .text_size(px(11.5))
                        .text_color(if active { text_pri } else { text_mut })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_t(this, WorkflowUiAction::SetTool(tool), cx);
                            }),
                        )
                        .child(tr_str(lang, lbl_z, lbl_e)),
                );
            }
            sub = sub.child(div().w(px(1.0)).h(px(16.0)).mx(px(2.0)).bg(border_hair));
            for hex in INK_SWATCHES {
                let active = state.ink_color_hex.eq_ignore_ascii_case(hex);
                let a_col = on_action.clone();
                let hex_s = hex.to_string();
                sub = sub.child(
                    div()
                        .id(SharedString::from(format!("wfn-ink-{hex}")))
                        .cursor_pointer()
                        .w(px(20.0))
                        .h(px(20.0))
                        .rounded_full()
                        .border_1()
                        .border_color(if active { text_pri } else { border_hair })
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_col(this, WorkflowUiAction::SetInkColor(hex_s.clone()), cx);
                            }),
                        )
                        .child(
                            div()
                                .w(px(12.0))
                                .h(px(12.0))
                                .rounded_full()
                                .bg(parse_hex_rgb(hex, 0x42a0ff)),
                        ),
                );
            }
            container = container.child(sub);
        }
        WorkflowSubToolPanel::Note => {
            let mut sub = div()
                .id("wfn-subbar-note")
                .h(px(36.0))
                .px(px(8.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .items_center()
                .gap(px(6.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            for hex in NOTE_TONES {
                let a_pick = on_action.clone();
                let a_add = on_action.clone();
                let hex_s = hex.to_string();
                sub = sub.child(
                    div()
                        .id(SharedString::from(format!("wfn-note-tone-{hex}")))
                        .cursor_pointer()
                        .w(px(20.0))
                        .h(px(20.0))
                        .rounded(px(5.0))
                        .bg(parse_hex_rgb(hex, 0xe5a50a))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_pick(this, WorkflowUiAction::SetNoteTone(hex_s.clone()), cx);
                                a_add(
                                    this,
                                    WorkflowUiAction::AddNode {
                                        kind: WorkflowNodeKind::Note,
                                        preset: None,
                                    },
                                    cx,
                                );
                            }),
                        ),
                );
            }
            container = container.child(sub);
        }
        WorkflowSubToolPanel::Shape => {
            let mut sub = div()
                .id("wfn-subbar-shape")
                .h(px(36.0))
                .px(px(6.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .items_center()
                .gap(px(4.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            for (kind_key, lbl_z, lbl_e) in SHAPE_KINDS {
                let a_set = on_action.clone();
                let a_add = on_action.clone();
                sub = sub.child(
                    div()
                        .id(SharedString::from(format!("wfn-shape-{kind_key}")))
                        .cursor_pointer()
                        .h(px(26.0))
                        .px(px(8.0))
                        .rounded(px(7.0))
                        .hover(move |s| s.bg(bg_hover))
                        .flex()
                        .items_center()
                        .text_size(px(11.5))
                        .text_color(text_pri)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_set(
                                    this,
                                    WorkflowUiAction::SetShapeKind(kind_key.to_string()),
                                    cx,
                                );
                                a_add(
                                    this,
                                    WorkflowUiAction::AddNode {
                                        kind: WorkflowNodeKind::Shape,
                                        preset: None,
                                    },
                                    cx,
                                );
                            }),
                        )
                        .child(tr_str(lang, lbl_z, lbl_e)),
                );
            }
            container = container.child(sub);
        }
        WorkflowSubToolPanel::None => {}
    }

    // Main floating tool dock pill (`wfn-hud-tools`)
    let tool_btn = |id: &'static str,
                    label: &'static str,
                    active: bool,
                    action: WorkflowUiAction,
                    cx: &mut Context<V>| {
        let cb = on_action.clone();
        div()
            .id(id)
            .cursor_pointer()
            .h(px(30.0))
            .px(px(9.0))
            .rounded(px(8.0))
            .when(active, |el| el.bg(bg_active))
            .when(!active, |el| el.hover(move |s| s.bg(bg_hover)))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.0))
            .font_weight(if active {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            })
            .text_color(if active { text_pri } else { text_mut })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    cb(this, action.clone(), cx);
                }),
            )
            .child(label)
    };

    let draw_active = matches!(
        state.active_tool,
        WorkflowInkTool::Pen | WorkflowInkTool::Highlighter | WorkflowInkTool::Eraser
    ) || state.sub_tool_panel == WorkflowSubToolPanel::Draw;
    let a_add_menu = on_action.clone();
    let dock = div()
        .id("wfn-hud-tools")
        .h(px(40.0))
        .px(px(6.0))
        .rounded(px(14.0))
        .bg(bg_surface)
        .border_1()
        .border_color(border_strong)
        .flex()
        .items_center()
        .gap(px(3.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(tool_btn(
            "wfn-tool-select",
            tr_str(lang, "选择", "Select"),
            state.active_tool == WorkflowInkTool::Select
                && state.sub_tool_panel == WorkflowSubToolPanel::None,
            WorkflowUiAction::SetTool(WorkflowInkTool::Select),
            cx,
        ))
        .child(tool_btn(
            "wfn-tool-pan",
            tr_str(lang, "平移", "Pan"),
            state.active_tool == WorkflowInkTool::Pan,
            WorkflowUiAction::SetTool(WorkflowInkTool::Pan),
            cx,
        ))
        .child(div().w(px(1.0)).h(px(18.0)).mx(px(2.0)).bg(border_hair))
        .child(tool_btn(
            "wfn-tool-draw",
            tr_str(lang, "绘图", "Draw"),
            draw_active,
            WorkflowUiAction::ToggleSubToolPanel(WorkflowSubToolPanel::Draw),
            cx,
        ))
        .child(tool_btn(
            "wfn-tool-note",
            tr_str(lang, "便签", "Note"),
            state.sub_tool_panel == WorkflowSubToolPanel::Note,
            WorkflowUiAction::ToggleSubToolPanel(WorkflowSubToolPanel::Note),
            cx,
        ))
        .child(tool_btn(
            "wfn-tool-shape",
            tr_str(lang, "形状", "Shape"),
            state.sub_tool_panel == WorkflowSubToolPanel::Shape,
            WorkflowUiAction::ToggleSubToolPanel(WorkflowSubToolPanel::Shape),
            cx,
        ))
        .child(tool_btn(
            "wfn-tool-text",
            tr_str(lang, "文本", "Text"),
            false,
            WorkflowUiAction::AddNode {
                kind: WorkflowNodeKind::Text,
                preset: None,
            },
            cx,
        ))
        .child(div().w(px(1.0)).h(px(18.0)).mx(px(2.0)).bg(border_hair))
        .child(tool_btn(
            "wfn-tool-search",
            tr_str(lang, "搜索", "Search"),
            state.search_open,
            WorkflowUiAction::ToggleSearch,
            cx,
        ))
        .child(
            div()
                .id("wfn-tool-add-node")
                .cursor_pointer()
                .h(px(30.0))
                .px(px(11.0))
                .ml(px(2.0))
                .rounded(px(9.0))
                .bg(accent)
                .flex()
                .items_center()
                .gap(px(4.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(on_accent)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_add_menu(this, WorkflowUiAction::ToggleAddMenu, cx);
                    }),
                )
                .child("+")
                .child(tr_str(lang, "添加节点", "Add node")),
        );

    container.child(dock)
}

#[allow(clippy::too_many_arguments)]
fn render_zoom_menu_hud<V: 'static>(
    state: &WorkflowCanvasState,
    lang: &str,
    bg_surface: Rgba,
    bg_hover: Rgba,
    bg_active: Rgba,
    border_hair: Rgba,
    border_strong: Rgba,
    text_pri: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    on_accent: Rgba,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Stateful<Div> {
    let a_out = on_action.clone();
    let a_tog = on_action.clone();
    let a_in = on_action.clone();
    let a_fit = on_action.clone();
    let a_tidy = on_action.clone();
    let a_mm = on_action.clone();
    let can_tidy = state.graph.nodes.len() > 1 && state.preview_version_id.is_none();

    let mut wrap = div()
        .id("wfn-zoom-hud-wrap")
        .absolute()
        .left(px(16.0))
        .bottom(px(16.0))
        .flex()
        .flex_col()
        .items_start()
        .gap(px(6.0));

    if state.zoom_menu_open {
        let mut presets = div().flex().items_center().gap(px(4.0));
        for p in WORKFLOW_ZOOM_PRESETS {
            let cur = (p - state.zoom).abs() < 0.02;
            let a_z = on_action.clone();
            presets = presets.child(
                div()
                    .id(SharedString::from(format!("wfn-zoom-preset-{}", (p * 100.0) as u32)))
                    .cursor_pointer()
                    .px(px(7.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .bg(if cur { accent } else { bg_hover })
                    .text_size(px(11.0))
                    .text_color(if cur { on_accent } else { text_pri })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_z(this, WorkflowUiAction::ZoomTo(p), cx);
                        }),
                    )
                    .child(format!("{}%", (p * 100.0).round() as u32)),
            );
        }
        wrap = wrap.child(
            div()
                .id("wfn-zoom-presets-pop")
                .p(px(8.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(presets),
        );
    }

    let bar = div()
        .id("wfn-hud-zoom")
        .h(px(36.0))
        .px(px(4.0))
        .rounded(px(12.0))
        .bg(bg_surface)
        .border_1()
        .border_color(border_strong)
        .flex()
        .items_center()
        .gap(px(2.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .id("wfn-zoom-out")
                .w(px(26.0))
                .h(px(26.0))
                .rounded(px(7.0))
                .when(state.zoom > WORKFLOW_ZOOM_MIN + 0.01, |el| {
                    el.cursor_pointer()
                        .hover(move |s| s.bg(bg_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_out(this, WorkflowUiAction::ZoomOut, cx);
                            }),
                        )
                })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(14.0))
                .text_color(text_pri)
                .child("−"),
        )
        .child(
            div()
                .id("wfn-zoom-pct")
                .cursor_pointer()
                .h(px(26.0))
                .px(px(6.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(bg_hover))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(text_pri)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_tog(this, WorkflowUiAction::ToggleZoomMenu, cx);
                    }),
                )
                .child(format!("{}%", (state.zoom * 100.0).round() as u32)),
        )
        .child(
            div()
                .id("wfn-zoom-in")
                .w(px(26.0))
                .h(px(26.0))
                .rounded(px(7.0))
                .when(state.zoom < WORKFLOW_ZOOM_MAX - 0.01, |el| {
                    el.cursor_pointer()
                        .hover(move |s| s.bg(bg_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_in(this, WorkflowUiAction::ZoomIn, cx);
                            }),
                        )
                })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(14.0))
                .text_color(text_pri)
                .child("+"),
        )
        .child(div().w(px(1.0)).h(px(16.0)).mx(px(2.0)).bg(border_hair))
        .child(
            div()
                .id("wfn-zoom-fit")
                .cursor_pointer()
                .h(px(26.0))
                .px(px(7.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(bg_hover))
                .flex()
                .items_center()
                .text_size(px(11.5))
                .text_color(text_mut)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_fit(this, WorkflowUiAction::FitToView, cx);
                    }),
                )
                .child(tr_str(lang, "适应", "Fit")),
        )
        .when(can_tidy, |el| {
            el.child(
                div()
                    .id("wfn-zoom-tidy")
                    .cursor_pointer()
                    .h(px(26.0))
                    .px(px(7.0))
                    .rounded(px(7.0))
                    .hover(move |s| s.bg(bg_hover))
                    .flex()
                    .items_center()
                    .text_size(px(11.5))
                    .text_color(text_mut)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_tidy(this, WorkflowUiAction::TidyLayout, cx);
                        }),
                    )
                    .child(tr_str(lang, "整理", "Tidy")),
            )
        })
        .child(
            div()
                .id("wfn-zoom-minimap")
                .cursor_pointer()
                .h(px(26.0))
                .px(px(7.0))
                .rounded(px(7.0))
                .when(state.minimap_visible, |el| el.bg(bg_active))
                .when(!state.minimap_visible, |el| el.hover(move |s| s.bg(bg_hover)))
                .flex()
                .items_center()
                .text_size(px(11.5))
                .text_color(if state.minimap_visible { text_pri } else { text_mut })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_mm(this, WorkflowUiAction::ToggleMinimap, cx);
                    }),
                )
                .child(tr_str(lang, "小地图", "Map")),
        );

    wrap.child(bar)
}

#[allow(clippy::too_many_arguments)]
fn render_workflow_search_popover<V: 'static>(
    state: &WorkflowCanvasState,
    lang: &str,
    bg_surface: Rgba,
    bg_raised: Rgba,
    bg_hover: Rgba,
    border_strong: Rgba,
    text_pri: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Stateful<Div> {
    let q = state.search_query.trim().to_ascii_lowercase();
    let matches: Vec<&WorkflowNode> = state
        .display_graph()
        .nodes
        .iter()
        .filter(|n| {
            q.is_empty()
                || n.id.to_ascii_lowercase().contains(&q)
                || node_summary_text(n).to_ascii_lowercase().contains(&q)
                || localized_kind_label(n.kind, lang)
                    .to_ascii_lowercase()
                    .contains(&q)
        })
        .take(8)
        .collect();
    let a_close = on_action.clone();

    let mut box_el = div()
        .id("wfn-search-popover")
        .absolute()
        .top(px(62.0))
        .left(px(14.0))
        .w(px(292.0))
        .p(px(10.0))
        .rounded(px(14.0))
        .bg(bg_surface)
        .border_1()
        .border_color(border_strong)
        .flex()
        .flex_col()
        .gap(px(8.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(bg_raised)
                .border_1()
                .border_color(accent)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(if state.search_query.is_empty() {
                            text_mut
                        } else {
                            text_pri
                        })
                        .child(if state.search_query.is_empty() {
                            tr_str(lang, "搜索步骤、提示词和结果…", "Search steps & prompts…")
                                .to_string()
                        } else {
                            format!("{}|", state.search_query)
                        }),
                )
                .child(
                    div()
                        .id("wfn-search-close")
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(text_mut)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_close(this, WorkflowUiAction::ToggleSearch, cx);
                            }),
                        )
                        .child("×"),
                ),
        );

    if matches.is_empty() {
        box_el = box_el.child(
            div()
                .px(px(6.0))
                .py(px(4.0))
                .text_size(px(12.0))
                .text_color(text_mut)
                .child(tr_str(lang, "没有匹配的步骤", "No matching steps")),
        );
    } else {
        for node in matches {
            let nid = node.id.clone();
            let a_focus = on_action.clone();
            box_el = box_el.child(
                div()
                    .id(SharedString::from(format!("wfn-search-hit-{nid}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(7.0))
                    .hover(move |s| s.bg(bg_hover))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_focus(this, WorkflowUiAction::FocusNode(nid.clone()), cx);
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_pri)
                            .child(localized_kind_label(node.kind, lang))
                            .child(div().text_size(px(10.5)).text_color(text_mut).child(node.id.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.0))
                            .text_color(text_mut)
                            .child(node_summary_text(node)),
                    ),
            );
        }
    }
    box_el
}

#[allow(clippy::too_many_arguments)]
fn render_folder_name_dialog<V: 'static>(
    state: &WorkflowCanvasState,
    lang: &str,
    bg_surface: Rgba,
    bg_raised: Rgba,
    bg_hover: Rgba,
    border_strong: Rgba,
    text_pri: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    on_accent: Rgba,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Stateful<Div> {
    let is_rename = matches!(
        state.active_text_input,
        WorkflowTextInputTarget::RenameFolder { .. }
    );
    let a_ok = on_action.clone();
    let a_cancel = on_action.clone();
    div()
        .id("wfn-folder-dialog-backdrop")
        .absolute()
        .inset_0()
        .bg(Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.32,
        })
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .w(px(320.0))
                .p(px(16.0))
                .rounded(px(14.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child(if is_rename {
                            tr_str(lang, "重命名文件夹", "Rename folder")
                        } else {
                            tr_str(lang, "新建文件夹", "New folder")
                        }),
                )
                .child(
                    div()
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(9.0))
                        .bg(bg_raised)
                        .border_1()
                        .border_color(accent)
                        .text_size(px(13.0))
                        .text_color(text_pri)
                        .child(format!("{}|", state.input_buffer)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("wfn-folder-dlg-cancel")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .hover(move |s| s.bg(bg_hover))
                                .text_size(px(12.5))
                                .text_color(text_mut)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        a_cancel(this, WorkflowUiAction::CancelTextInput, cx);
                                    }),
                                )
                                .child(tr_str(lang, "取消", "Cancel")),
                        )
                        .child(
                            div()
                                .id("wfn-folder-dlg-save")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(accent)
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(on_accent)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        a_ok(this, WorkflowUiAction::CommitTextInput, cx);
                                    }),
                                )
                                .child(tr_str(lang, "确定", "Confirm")),
                        ),
                ),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_import_workflow_dialog<V: 'static>(
    state: &WorkflowCanvasState,
    lang: &str,
    bg_surface: Rgba,
    bg_raised: Rgba,
    bg_hover: Rgba,
    border_strong: Rgba,
    text_pri: Rgba,
    text_mut: Rgba,
    accent: Rgba,
    on_accent: Rgba,
    danger: Rgba,
    cx: &mut Context<V>,
    on_action: &Rc<dyn Fn(&mut V, WorkflowUiAction, &mut Context<V>)>,
) -> Stateful<Div> {
    let a_ok = on_action.clone();
    let a_cancel = on_action.clone();
    div()
        .id("wfn-import-dialog-backdrop")
        .absolute()
        .inset_0()
        .bg(Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.36,
        })
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .w(px(440.0))
                .p(px(16.0))
                .rounded(px(14.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_strong)
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .text_size(px(14.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child(tr_str(lang, "导入工作流 (.openbot-workflow.json)", "Import workflow (.openbot-workflow.json)")),
                )
                .child(
                    div()
                        .id("wfn-import-textarea")
                        .min_h(px(120.0))
                        .max_h(px(200.0))
                        .overflow_y_scroll()
                        .p(px(10.0))
                        .rounded(px(9.0))
                        .bg(bg_raised)
                        .border_1()
                        .border_color(accent)
                        .text_size(px(12.0))
                        .text_color(if state.input_buffer.is_empty() {
                            text_mut
                        } else {
                            text_pri
                        })
                        .child(if state.input_buffer.is_empty() {
                            tr_str(
                                lang,
                                "粘贴 .openbot-workflow.json 内容后点击导入…",
                                "Paste .openbot-workflow.json contents and click Import…",
                            )
                            .to_string()
                        } else {
                            format!("{}|", state.input_buffer)
                        }),
                )
                .when_some(state.import_error.clone(), |el, err| {
                    el.child(div().text_size(px(11.5)).text_color(danger).child(err))
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("wfn-import-cancel")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .hover(move |s| s.bg(bg_hover))
                                .text_size(px(12.5))
                                .text_color(text_mut)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        a_cancel(this, WorkflowUiAction::CloseImportDialog, cx);
                                    }),
                                )
                                .child(tr_str(lang, "取消", "Cancel")),
                        )
                        .child(
                            div()
                                .id("wfn-import-confirm")
                                .cursor_pointer()
                                .px(px(14.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(accent)
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(on_accent)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        a_ok(this, WorkflowUiAction::ConfirmImportJson, cx);
                                    }),
                                )
                                .child(tr_str(lang, "导入", "Import")),
                        ),
                ),
        )
}
