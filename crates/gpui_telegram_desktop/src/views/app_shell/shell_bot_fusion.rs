//! Native Bot Fusion Controller & Inline UI (`AppShell` <-> `OpenBotHostRuntime`).
//!
//! Unifies OpenBot & Orca capabilities directly inside Fabushi's existing native Bot system:
//! - Automatic 1:1 local `OpenBotHostRuntime` agent provisioning keyed by `FabushiBotSummaryDto::agent_id`.
//! - Live Memory CRUD + Tidy-Up, Skills & MCP management, Routines, and Approval Workflows.
//! - In-chat Bot Quick Bar, inline `ask_ui` approval cards, Queue/Steer bar, ephemeral Side-Question,
//!   `$` Skills/Routines/Workflows composer command execution, and 380px Bot Companion SidePanel.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto;
use openbot_contracts::routines::{
    RoutineLimitPolicy, RoutineMissedPolicy, RoutineRunKind, RoutineRunPlace, RoutineSchedule,
};
use openbot_contracts::workflows::{
    WorkflowEdge, WorkflowFieldValue, WorkflowGraph, WorkflowNode, WorkflowNodeKind,
    WorkflowPoint, WorkflowPortType,
};
use openbot_runtime::agent_lifecycle::CreateAgentRequest;
use openbot_runtime::drain_scheduler::{
    BranchThreadInput, QueueAddInput, QueueSteerInput, SideQuestionInput,
};
use openbot_runtime::platform::routines::{RoutineRunRequest, RoutineSaveRequest};
use openbot_runtime::platform::workflows::{WorkflowRunRequest, WorkflowSaveRequest};
use openbot_runtime::skill_mcp_service::SaveSkillInput;
use openbot_runtime::OpenBotHostRuntime;
use openbot_storage::AgentMemoryStore;

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::drawer::{
    BotCompanionTab, BotFusionAskCard, BotFusionDollarCandidate, BotFusionMcpItem,
    BotFusionMemoryItem, BotFusionQueueItem, BotFusionRoutineItem, BotFusionSkillItem,
    BotFusionSnapshot, BotFusionStepItem, BotFusionWorkflowItem, InspectorAction,
};
use crate::views::openbot::WorkflowCanvasState;
use crate::views::shell::AppShell;

/// Grouped state for Native Bot Fusion stored on `AppShell`.
#[derive(Default)]
pub struct ShellBotFusionState {
    pub runtime: Option<Arc<OpenBotHostRuntime>>,
    pub agent_map: HashMap<String, String>,
    pub snapshot: BotFusionSnapshot,
    pub companion_open: bool,
    pub companion_tab: BotCompanionTab,
    pub selected_workflow_id: Option<String>,
    pub workflow_canvas: Option<WorkflowCanvasState>,
    pub pending_ask_cards: Vec<BotFusionAskCard>,
    pub recent_steps: Vec<BotFusionStepItem>,
    pub side_question_last: Option<(String, String)>,
}

impl ShellBotFusionState {
    pub fn reset_ui(&mut self) {
        self.snapshot = BotFusionSnapshot::default();
        self.companion_open = false;
        self.selected_workflow_id = None;
        self.pending_ask_cards.clear();
        self.recent_steps.clear();
        self.side_question_last = None;
    }
}

/// Converts any human-readable skill name into a valid OpenBot skill slug (`[a-z0-9-]`, max 64 chars).
pub fn sanitize_openbot_skill_slug(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            out.push(lower);
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 56 {
            break;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "custom-skill".to_string()
    } else {
        trimmed
    }
}

/// Formats an OpenBot `RoutineSchedule` into a concise human-readable label.
pub fn format_routine_schedule_label(schedule: &RoutineSchedule, is_zh: bool) -> String {
    match schedule {
        RoutineSchedule::Daily { time } => {
            if is_zh {
                format!("每天 {time}")
            } else {
                format!("Daily {time}")
            }
        }
        RoutineSchedule::Weekdays { time } => {
            if is_zh {
                format!("工作日 {time}")
            } else {
                format!("Weekdays {time}")
            }
        }
        RoutineSchedule::Hourly { minute } => {
            if is_zh {
                format!("每小时 :{minute:02}")
            } else {
                format!("Hourly :{minute:02}")
            }
        }
        RoutineSchedule::Weekly { weekday, time } => {
            if is_zh {
                format!("每周({weekday}) {time}")
            } else {
                format!("Weekly({weekday}) {time}")
            }
        }
        RoutineSchedule::Monthly { day, time } => {
            if is_zh {
                format!("每月{day}日 {time}")
            } else {
                format!("Monthly #{day} {time}")
            }
        }
        RoutineSchedule::Interval { amount, unit, .. } => {
            if is_zh {
                format!("每 {amount} {unit:?}")
            } else {
                format!("Every {amount} {unit:?}")
            }
        }
        RoutineSchedule::Once { date, time } => format!("{date} {time}"),
        RoutineSchedule::Custom { expression } => expression.clone(),
        RoutineSchedule::Advanced { .. } => {
            if is_zh {
                "高级计划".to_string()
            } else {
                "Advanced schedule".to_string()
            }
        }
    }
}

/// Builds a valid 4-node DAG (`Trigger -> Agent -> Approval -> Output`) for the active Bot.
pub fn build_starter_approval_workflow_graph(
    openbot_agent_id: &str,
    bot_name: &str,
) -> WorkflowGraph {
    let mut trigger_fields = BTreeMap::new();
    trigger_fields.insert(
        "label".to_string(),
        WorkflowFieldValue::Text("触发入口".to_string()),
    );

    let mut agent_fields = BTreeMap::new();
    agent_fields.insert(
        "agentId".to_string(),
        WorkflowFieldValue::Text(openbot_agent_id.to_string()),
    );
    agent_fields.insert(
        "prompt".to_string(),
        WorkflowFieldValue::Text(format!(
            "由 {bot_name} 分析当前任务输入，生成执行方案并准备交付物。"
        )),
    );

    let mut approval_fields = BTreeMap::new();
    approval_fields.insert(
        "title".to_string(),
        WorkflowFieldValue::Text(format!("{bot_name} 执行审批")),
    );
    approval_fields.insert(
        "prompt".to_string(),
        WorkflowFieldValue::Text("确认方案无误后点击批准，继续输出最终结果。".to_string()),
    );

    let mut output_fields = BTreeMap::new();
    output_fields.insert(
        "label".to_string(),
        WorkflowFieldValue::Text("汇总交付".to_string()),
    );

    WorkflowGraph {
        nodes: vec![
            WorkflowNode {
                id: "n-trigger".to_string(),
                kind: WorkflowNodeKind::Trigger,
                at: WorkflowPoint { x: 40.0, y: 80.0 },
                size: None,
                locked: None,
                fields: trigger_fields,
                schedule: None,
            },
            WorkflowNode {
                id: "n-agent".to_string(),
                kind: WorkflowNodeKind::Agent,
                at: WorkflowPoint { x: 340.0, y: 80.0 },
                size: None,
                locked: None,
                fields: agent_fields,
                schedule: None,
            },
            WorkflowNode {
                id: "n-approval".to_string(),
                kind: WorkflowNodeKind::Approval,
                at: WorkflowPoint { x: 640.0, y: 80.0 },
                size: None,
                locked: None,
                fields: approval_fields,
                schedule: None,
            },
            WorkflowNode {
                id: "n-output".to_string(),
                kind: WorkflowNodeKind::Output,
                at: WorkflowPoint { x: 940.0, y: 80.0 },
                size: None,
                locked: None,
                fields: output_fields,
                schedule: None,
            },
        ],
        edges: vec![
            WorkflowEdge {
                id: "e-1".to_string(),
                from: "n-trigger".to_string(),
                to: "n-agent".to_string(),
                port_type: WorkflowPortType::Text,
                branch: None,
            },
            WorkflowEdge {
                id: "e-2".to_string(),
                from: "n-agent".to_string(),
                to: "n-approval".to_string(),
                port_type: WorkflowPortType::Text,
                branch: None,
            },
            WorkflowEdge {
                id: "e-3".to_string(),
                from: "n-approval".to_string(),
                to: "n-output".to_string(),
                port_type: WorkflowPortType::Text,
                branch: None,
            },
        ],
        strokes: Vec::new(),
    }
}

fn workflow_node_display_label(node: &WorkflowNode) -> String {
    let field_txt = node
        .fields
        .get("title")
        .or_else(|| node.fields.get("label"))
        .and_then(|v| match v {
            WorkflowFieldValue::Text(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        });
    if let Some(txt) = field_txt {
        return txt;
    }
    match node.kind {
        WorkflowNodeKind::Trigger => "Trigger".to_string(),
        WorkflowNodeKind::Prompt => "Prompt".to_string(),
        WorkflowNodeKind::Asset => "Asset".to_string(),
        WorkflowNodeKind::Llm => "LLM".to_string(),
        WorkflowNodeKind::Agent => "Agent".to_string(),
        WorkflowNodeKind::Image => "Image".to_string(),
        WorkflowNodeKind::Condition => "Condition".to_string(),
        WorkflowNodeKind::Approval => "Approval Gate".to_string(),
        WorkflowNodeKind::Output => "Output".to_string(),
        WorkflowNodeKind::Text => "Text".to_string(),
        WorkflowNodeKind::Note => "Note".to_string(),
        WorkflowNodeKind::Shape => "Shape".to_string(),
    }
}

impl AppShell {
    /// Lazily opens the local `OpenBotHostRuntime` under `<preferences_dir>/openbot`.
    pub fn ensure_native_openbot_runtime(&mut self) -> Option<Arc<OpenBotHostRuntime>> {
        if let Some(ref rt) = self.bot_fusion.runtime {
            return Some(Arc::clone(rt));
        }
        let dir = self.preferences_dir.join("openbot");
        match OpenBotHostRuntime::open(&dir) {
            Ok(rt) => {
                self.bot_fusion.runtime = Some(Arc::clone(&rt));
                Some(rt)
            }
            Err(_) => None,
        }
    }

    /// Idempotently maps a native `FabushiBotSummaryDto` 1:1 to a local OpenBot agent
    /// and optionally installs bundled template skills.
    pub fn provision_openbot_agent_for_fabushi(
        &mut self,
        fab: &FabushiBotSummaryDto,
        bundled_skills: &[String],
    ) -> Option<String> {
        let rt = self.ensure_native_openbot_runtime()?;
        let fab_key = fab.agent_id.trim().to_string();
        if fab_key.is_empty() {
            return None;
        }
        let foreign_title = format!("fabushi:{fab_key}");

        let openbot_id = if let Some(cached) = self.bot_fusion.agent_map.get(&fab_key) {
            if rt.agents().get_agent(cached).is_some() {
                cached.clone()
            } else {
                self.bot_fusion.agent_map.remove(&fab_key);
                Self::find_or_create_openbot_agent(&rt, fab, &foreign_title)?
            }
        } else {
            Self::find_or_create_openbot_agent(&rt, fab, &foreign_title)?
        };

        self.bot_fusion
            .agent_map
            .insert(fab_key, openbot_id.clone());

        if !bundled_skills.is_empty() {
            let existing: HashSet<String> = rt
                .skills()
                .list_skills(&openbot_id)
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.slug.to_ascii_lowercase())
                .collect();
            for raw_skill in bundled_skills {
                let name = raw_skill.trim();
                if name.is_empty() {
                    continue;
                }
                let slug = sanitize_openbot_skill_slug(name);
                if existing.contains(&slug) {
                    continue;
                }
                let _ = rt.skills().save_skill(SaveSkillInput {
                    agent_id: openbot_id.clone(),
                    slug: slug.clone(),
                    name: name.to_string(),
                    description: format!("Bundled template skill: {name}"),
                    instructions: format!(
                        "# {name}\nUse the `{slug}` skill workflow when assisting as {}.",
                        fab.display_name
                    ),
                    triggers: vec![format!("${slug}")],
                    tools: Vec::new(),
                    origin: Some("template".to_string()),
                    enabled: Some(true),
                });
            }
        }

        Some(openbot_id)
    }

    fn find_or_create_openbot_agent(
        rt: &Arc<OpenBotHostRuntime>,
        fab: &FabushiBotSummaryDto,
        foreign_title: &str,
    ) -> Option<String> {
        let agents = rt.agents().list_agents();
        if let Some(found) = agents.iter().find(|a| a.title == foreign_title) {
            return Some(found.id.clone());
        }
        let display = if fab.display_name.trim().is_empty() {
            fab.agent_id.clone()
        } else {
            fab.display_name.trim().to_string()
        };
        let req = CreateAgentRequest {
            name: display,
            title: foreign_title.to_string(),
            description: fab.description.clone(),
            allow_automation: Some(true),
            computer_use: Some(true),
            ..Default::default()
        };
        rt.agents().create_agent_full(req).ok().map(|a| a.id)
    }

    fn active_fabushi_bot_for_fusion(&self, cx: &App) -> Option<FabushiBotSummaryDto> {
        self.stage
            .read(cx)
            .active_fabushi_bot
            .clone()
            .or_else(|| self.inspector.read(cx).active_fabushi_bot.clone())
    }

    fn push_bot_fusion_step(&mut self, label: impl Into<String>, status: &str, detail: impl Into<String>) {
        self.bot_fusion.recent_steps.insert(
            0,
            BotFusionStepItem {
                label: label.into(),
                status: status.to_string(),
                detail: detail.into(),
            },
        );
        self.bot_fusion.recent_steps.truncate(16);
    }

    /// Rebuilds `BotFusionSnapshot` from live `OpenBotHostRuntime` state for the active Bot
    /// and pushes it into `ContextInspector`, `CanonicalComposer` (`$` picker), and `ConversationStage`.
    pub fn refresh_active_bot_fusion_snapshot(&mut self, cx: &mut Context<Self>) {
        let is_zh = self.active_lang_id().starts_with("zh");
        let Some(fab) = self.active_fabushi_bot_for_fusion(cx) else {
            self.bot_fusion.snapshot = BotFusionSnapshot::default();
            self.inspector.update(cx, |insp, icx| {
                insp.set_bot_fusion_snapshot(BotFusionSnapshot::default());
                icx.notify();
            });
            self.composer.update(cx, |comp, ccx| {
                comp.clear_bot_dollar_candidates();
                ccx.notify();
            });
            self.stage.update(cx, |st, scx| {
                if st.is_bot_companion_open {
                    st.is_bot_companion_open = false;
                    scx.notify();
                }
            });
            return;
        };

        let Some(openbot_id) = self.provision_openbot_agent_for_fabushi(&fab, &[]) else {
            return;
        };
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };

        let mem_store = AgentMemoryStore::new(rt.database().clone());
        let memories: Vec<BotFusionMemoryItem> = mem_store
            .list(&openbot_id)
            .unwrap_or_default()
            .into_iter()
            .map(|m| BotFusionMemoryItem {
                id: m.entry.id,
                content: m.entry.text,
                origin: format!("{:?}", m.entry.origin),
                updated_at: m.entry.updated_at,
            })
            .collect();

        let skills: Vec<BotFusionSkillItem> = rt
            .skills()
            .list_skills(&openbot_id)
            .unwrap_or_default()
            .into_iter()
            .map(|s| BotFusionSkillItem {
                skill_id: s.skill_id,
                slug: s.slug,
                name: s.name,
                description: s.description,
                enabled: s.enabled,
            })
            .collect();

        let mcp_servers: Vec<BotFusionMcpItem> = rt
            .mcp()
            .list_servers()
            .unwrap_or_default()
            .into_iter()
            .map(|s| BotFusionMcpItem {
                id: s.id,
                name: s.name,
                transport: format!("{:?}", s.transport),
                enabled: s.enabled,
            })
            .collect();

        let routines: Vec<BotFusionRoutineItem> = rt
            .routines()
            .list_routines(&openbot_id, false)
            .unwrap_or_default()
            .into_iter()
            .map(|r| BotFusionRoutineItem {
                id: r.id,
                name: r.name,
                schedule_label: format_routine_schedule_label(&r.trigger.schedule, is_zh),
                active: r.active,
                instruction: r.instruction,
            })
            .collect();

        let mut workflows: Vec<BotFusionWorkflowItem> = Vec::new();
        for wf_sum in rt.workflows().list_workflows().unwrap_or_default() {
            let detail = rt.workflows().get_workflow(&wf_sum.id).ok().flatten();
            let node_labels = detail
                .as_ref()
                .map(|d| {
                    d.graph
                        .nodes
                        .iter()
                        .map(workflow_node_display_label)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let pending_card = self
                .bot_fusion
                .pending_ask_cards
                .iter()
                .find(|c| c.workflow_id == wf_sum.id);
            workflows.push(BotFusionWorkflowItem {
                id: wf_sum.id.clone(),
                title: wf_sum.title,
                step_count: wf_sum.steps as usize,
                last_run_status: pending_card
                    .map(|_| if is_zh { "等待审批".to_string() } else { "awaiting approval".to_string() })
                    .or_else(|| wf_sum.last_run.map(|lr| format!("{:?}", lr.status))),
                last_run_id: pending_card.map(|c| c.run_id.clone()),
                pending_approval_node: pending_card.map(|c| c.node_id.clone()),
                node_labels,
            });
        }

        if self.bot_fusion.selected_workflow_id.is_none() {
            self.bot_fusion.selected_workflow_id = workflows.first().map(|w| w.id.clone());
        }

        let queue_list = rt.scheduler().queue_list(&openbot_id).ok();
        let queue_paused = queue_list.as_ref().is_some_and(|q| q.paused);
        let active_turn_id = queue_list.as_ref().and_then(|q| q.active_turn_id.clone());
        let queue_items: Vec<BotFusionQueueItem> = queue_list
            .map(|q| {
                q.items
                    .into_iter()
                    .filter(|it| {
                        matches!(
                            it.status,
                            openbot_contracts::queue::QueueDeliveryStatus::Queued
                                | openbot_contracts::queue::QueueDeliveryStatus::Starting
                                | openbot_contracts::queue::QueueDeliveryStatus::Running
                        )
                    })
                    .map(|it| BotFusionQueueItem {
                        delivery_id: it.delivery_id,
                        text: it.text,
                        status: format!("{:?}", it.status),
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Build `$` composer candidates for this Bot
        let mut dollar_candidates: Vec<BotFusionDollarCandidate> = Vec::new();
        for sk in &skills {
            if sk.enabled {
                dollar_candidates.push(BotFusionDollarCandidate {
                    token: format!("skill:{}", sk.slug),
                    title: sk.name.clone(),
                    subtitle: sk.description.clone(),
                    badge: if is_zh { "技能" } else { "Skill" }.to_string(),
                });
            }
        }
        for rt_item in &routines {
            dollar_candidates.push(BotFusionDollarCandidate {
                token: format!("routine:{}", sanitize_openbot_skill_slug(&rt_item.name)),
                title: rt_item.name.clone(),
                subtitle: rt_item.schedule_label.clone(),
                badge: if is_zh { "定时" } else { "Routine" }.to_string(),
            });
        }
        for wf in &workflows {
            dollar_candidates.push(BotFusionDollarCandidate {
                token: format!("workflow:{}", sanitize_openbot_skill_slug(&wf.title)),
                title: wf.title.clone(),
                subtitle: format!("{} steps", wf.step_count),
                badge: if is_zh { "工作流" } else { "Flow" }.to_string(),
            });
        }
        dollar_candidates.push(BotFusionDollarCandidate {
            token: "side".to_string(),
            title: if is_zh { "无痕侧问" } else { "Side Question" }.to_string(),
            subtitle: if is_zh {
                "临时追问，不写入主对话历史"
            } else {
                "Ephemeral question without polluting history"
            }
            .to_string(),
            badge: if is_zh { "快捷" } else { "Quick" }.to_string(),
        });
        dollar_candidates.push(BotFusionDollarCandidate {
            token: "queue".to_string(),
            title: if is_zh { "加入消息队列" } else { "Enqueue Message" }.to_string(),
            subtitle: if is_zh {
                "排队发送，不打断 Bot 当前任务"
            } else {
                "Queue follow-up without interrupting active turn"
            }
            .to_string(),
            badge: if is_zh { "队列" } else { "Queue" }.to_string(),
        });
        dollar_candidates.push(BotFusionDollarCandidate {
            token: "workflow".to_string(),
            title: if is_zh { "打开工作流画布" } else { "Open Workflow Canvas" }.to_string(),
            subtitle: if is_zh {
                "在右侧伴生工作台查看与运行工作流"
            } else {
                "Inspect and run DAG workflows in Companion Workbench"
            }
            .to_string(),
            badge: if is_zh { "工作台" } else { "Canvas" }.to_string(),
        });

        let bot_display_name = if fab.display_name.trim().is_empty() {
            fab.agent_id.clone()
        } else {
            fab.display_name.clone()
        };

        let snapshot = BotFusionSnapshot {
            fabushi_agent_id: fab.agent_id.clone(),
            openbot_agent_id: openbot_id,
            bot_display_name,
            memories,
            skills,
            mcp_servers,
            routines,
            workflows,
            queue_items,
            queue_paused,
            active_turn_id,
            ask_cards: self.bot_fusion.pending_ask_cards.clone(),
            recent_steps: self.bot_fusion.recent_steps.clone(),
            side_question_last: self.bot_fusion.side_question_last.clone(),
            dollar_candidates: dollar_candidates.clone(),
            companion_open: self.bot_fusion.companion_open,
            companion_tab: self.bot_fusion.companion_tab,
            selected_workflow_id: self.bot_fusion.selected_workflow_id.clone(),
        };

        self.bot_fusion.snapshot = snapshot.clone();
        self.inspector.update(cx, |insp, icx| {
            insp.set_bot_fusion_snapshot(snapshot);
            icx.notify();
        });
        self.composer.update(cx, |comp, ccx| {
            comp.set_bot_dollar_candidates(dollar_candidates);
            ccx.notify();
        });
        let comp_open = self.bot_fusion.companion_open;
        self.stage.update(cx, |st, scx| {
            if st.is_bot_companion_open != comp_open {
                st.is_bot_companion_open = comp_open;
                scx.notify();
            }
        });
        self.sync_selected_workflow_to_canvas();
    }

    fn sync_selected_workflow_to_canvas(&mut self) {
        if !self.bot_fusion.companion_open {
            return;
        }
        let Some(ref rt) = self.bot_fusion.runtime else {
            return;
        };
        let Some(ref wf_id) = self.bot_fusion.selected_workflow_id else {
            return;
        };
        let Ok(Some(detail)) = rt.workflows().get_workflow(wf_id) else {
            return;
        };
        let wc = self
            .bot_fusion
            .workflow_canvas
            .get_or_insert_with(|| WorkflowCanvasState::new(detail.summary.title.clone()));
        if let Ok(json_val) = serde_json::to_value(&detail.graph) {
            wc.workflow_id = detail.summary.id.clone();
            wc.title = detail.summary.title.clone();
            let _ = wc.load_graph_json(&json_val);
        }
    }

    /// Toggles or opens the inline 380px Bot Companion SidePanel beside `ConversationStage`.
    pub fn toggle_bot_companion_panel(
        &mut self,
        target_tab: Option<BotCompanionTab>,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = target_tab {
            self.bot_fusion.companion_open = true;
            self.bot_fusion.companion_tab = tab;
        } else {
            self.bot_fusion.companion_open = !self.bot_fusion.companion_open;
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    /// Handles Native Bot Fusion `InspectorAction` variants emitted from `ContextInspector`.
    pub fn handle_bot_fusion_inspector_action(
        &mut self,
        action: InspectorAction,
        cx: &mut Context<Self>,
    ) -> bool {
        match action {
            InspectorAction::AddBotMemory(text) => {
                self.bot_fusion_add_memory(text, cx);
                true
            }
            InspectorAction::DeleteBotMemory(mem_id) => {
                self.bot_fusion_delete_memory(mem_id, cx);
                true
            }
            InspectorAction::TidyBotMemories => {
                self.bot_fusion_tidy_memories(cx);
                true
            }
            InspectorAction::InstallBotSkill {
                slug,
                name,
                description,
            } => {
                self.bot_fusion_install_skill(slug, name, description, cx);
                true
            }
            InspectorAction::ToggleBotSkill { skill_id, enabled } => {
                self.bot_fusion_toggle_skill(skill_id, enabled, cx);
                true
            }
            InspectorAction::CreatePresetRoutine { preset } => {
                self.bot_fusion_create_preset_routine(&preset, cx);
                true
            }
            InspectorAction::ToggleBotRoutine { routine_id, active } => {
                self.bot_fusion_toggle_routine(routine_id, active, cx);
                true
            }
            InspectorAction::RunBotRoutineNow { routine_id } => {
                self.bot_fusion_run_routine_now(routine_id, cx);
                true
            }
            InspectorAction::CreateStarterWorkflow => {
                self.bot_fusion_create_starter_workflow(cx);
                true
            }
            InspectorAction::RunBotWorkflow { workflow_id } => {
                self.bot_fusion_run_workflow(workflow_id, cx);
                true
            }
            InspectorAction::DecideBotWorkflowApproval {
                workflow_id,
                run_id,
                node_id,
                approve,
            } => {
                self.bot_fusion_decide_workflow_approval(workflow_id, run_id, node_id, approve, cx);
                true
            }
            InspectorAction::OpenBotCompanion(tab) => {
                self.toggle_bot_companion_panel(Some(tab), cx);
                true
            }
            _ => false,
        }
    }

    fn ensure_active_openbot_agent(&mut self, cx: &App) -> Option<(Arc<OpenBotHostRuntime>, String, String)> {
        let fab = self.active_fabushi_bot_for_fusion(cx)?;
        let ob_id = self.provision_openbot_agent_for_fabushi(&fab, &[])?;
        let rt = self.ensure_native_openbot_runtime()?;
        let name = if fab.display_name.trim().is_empty() {
            fab.agent_id
        } else {
            fab.display_name
        };
        Some((rt, ob_id, name))
    }

    pub fn bot_fusion_add_memory(&mut self, text: String, cx: &mut Context<Self>) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let store = AgentMemoryStore::new(rt.database().clone());
        if store.create_manual(&ob_id, &trimmed).is_ok() {
            self.push_bot_fusion_step("Memory", "saved", trimmed.clone());
            self.enqueue_localized_toast("已写入 Bot 专属记忆", "Saved to Bot Memory", 1800, cx);
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_delete_memory(&mut self, mem_id: String, cx: &mut Context<Self>) {
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let store = AgentMemoryStore::new(rt.database().clone());
        if store.delete(&ob_id, &mem_id).unwrap_or(false) {
            self.enqueue_localized_toast("已删除记忆条目", "Deleted memory entry", 1600, cx);
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_tidy_memories(&mut self, cx: &mut Context<Self>) {
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let store = AgentMemoryStore::new(rt.database().clone());
        let list = store.list(&ob_id).unwrap_or_default();
        let mut seen: HashSet<String> = HashSet::new();
        let mut removed = 0usize;
        for mem in list {
            let norm = mem
                .entry
                .text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            if !seen.insert(norm) && store.delete(&ob_id, &mem.entry.id).unwrap_or(false) {
                removed += 1;
            }
        }
        self.push_bot_fusion_step(
            "Memory Tidy",
            "done",
            format!("deduplicated {removed} duplicate entries"),
        );
        let lang = self.active_lang_id();
        let msg = if lang.starts_with("zh") {
            format!("记忆整理完成：合并清理 {removed} 条重复记忆")
        } else {
            format!("Memory tidy complete: removed {removed} duplicates")
        };
        self.show_non_chat_toast(msg, 2000, cx);
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_install_skill(
        &mut self,
        slug: String,
        name: String,
        description: String,
        cx: &mut Context<Self>,
    ) {
        let Some((rt, ob_id, bot_name)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let clean_slug = sanitize_openbot_skill_slug(&slug);
        let res = rt.skills().save_skill(SaveSkillInput {
            agent_id: ob_id,
            slug: clean_slug.clone(),
            name: name.clone(),
            description: description.clone(),
            instructions: format!("# {name}\n{description}\nBound to {bot_name}."),
            triggers: vec![format!("${clean_slug}")],
            tools: Vec::new(),
            origin: Some("manual".to_string()),
            enabled: Some(true),
        });
        if res.is_ok() {
            self.push_bot_fusion_step("Skill", "installed", format!("${clean_slug} · {name}"));
            self.enqueue_localized_toast("已为该 Bot 安装技能", "Installed Bot skill", 1800, cx);
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_toggle_skill(&mut self, skill_id: String, enabled: bool, cx: &mut Context<Self>) {
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let _ = rt.skills().set_skill_enabled(&ob_id, &skill_id, enabled);
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_create_preset_routine(&mut self, preset: &str, cx: &mut Context<Self>) {
        let Some((rt, ob_id, bot_name)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let is_zh = self.active_lang_id().starts_with("zh");
        let (name, instruction, schedule) = match preset {
            "hourly" => (
                if is_zh {
                    "每小时状态巡检".to_string()
                } else {
                    "Hourly Health Check".to_string()
                },
                format!("由 {bot_name} 每小时检查待办事项、异常告警并输出简短状态摘要。"),
                RoutineSchedule::Hourly { minute: 0 },
            ),
            _ => (
                if is_zh {
                    "每日早报摘要".to_string()
                } else {
                    "Daily Morning Digest".to_string()
                },
                format!("由 {bot_name} 在每天 09:00 汇总重点记忆、待审批项与当日行动清单。"),
                RoutineSchedule::Daily {
                    time: "09:00".to_string(),
                },
            ),
        };
        let res = rt.routines().save_routine(RoutineSaveRequest {
            owner_id: ob_id,
            routine_id: None,
            is_channel: false,
            name: name.clone(),
            instruction,
            timezone: "UTC".to_string(),
            active: true,
            schedule: Some(schedule),
            webhook_event_type: None,
            webhook_filters: Vec::new(),
            webhook_secret: None,
            limit_policy: RoutineLimitPolicy::Skip,
            missed_policy: RoutineMissedPolicy::Skip,
            run_in: RoutineRunPlace::Chat,
        });
        if res.is_ok() {
            self.push_bot_fusion_step("Routine", "created", name);
            self.enqueue_localized_toast("已创建定时任务", "Created Bot Routine", 1800, cx);
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_toggle_routine(&mut self, routine_id: String, active: bool, cx: &mut Context<Self>) {
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        if let Some(existing) = rt
            .routines()
            .list_routines(&ob_id, false)
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.id == routine_id)
        {
            let _ = rt.routines().save_routine(RoutineSaveRequest {
                owner_id: ob_id,
                routine_id: Some(existing.id),
                is_channel: false,
                name: existing.name,
                instruction: existing.instruction,
                timezone: existing.timezone,
                active,
                schedule: Some(existing.trigger.schedule),
                webhook_event_type: None,
                webhook_filters: Vec::new(),
                webhook_secret: None,
                limit_policy: existing.limit_policy.unwrap_or(RoutineLimitPolicy::Skip),
                missed_policy: existing.missed_policy.unwrap_or(RoutineMissedPolicy::Skip),
                run_in: existing.run_in.unwrap_or(RoutineRunPlace::Chat),
            });
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_run_routine_now(&mut self, routine_id: String, cx: &mut Context<Self>) {
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let res = rt.routines().run_routine(RoutineRunRequest {
            owner_id: ob_id,
            routine_id: routine_id.clone(),
            is_channel: false,
            kind: RoutineRunKind::Manual,
            source: None,
            payload: None,
            wait_id: None,
            quiet_no_update: false,
            report_text: Some("Manual routine trigger from Fabushi Bot Inspector".to_string()),
            await_events: Vec::new(),
        });
        if let Ok(outcome) = res {
            self.push_bot_fusion_step(
                "Routine Run",
                &outcome.status,
                format!("run {}", outcome.run_id),
            );
            self.enqueue_localized_toast("已立即执行定时任务", "Routine executed", 1800, cx);
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_create_starter_workflow(&mut self, cx: &mut Context<Self>) {
        let Some((rt, ob_id, bot_name)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let is_zh = self.active_lang_id().starts_with("zh");
        let title = if is_zh {
            format!("{bot_name} · 审批执行流")
        } else {
            format!("{bot_name} · Approval Flow")
        };
        let graph = build_starter_approval_workflow_graph(&ob_id, &bot_name);
        if let Ok(saved) = rt.workflows().save_workflow(WorkflowSaveRequest {
            workflow_id: None,
            title: title.clone(),
            graph,
            raw_json_bytes: None,
            allow_loops: false,
            base_revision: None,
        }) {
            self.bot_fusion.selected_workflow_id = Some(saved.workflow_id);
            self.bot_fusion.companion_open = true;
            self.bot_fusion.companion_tab = BotCompanionTab::Workflow;
            self.push_bot_fusion_step(
                "Workflow",
                "created",
                format!("{title} ({} nodes)", saved.node_count),
            );
            self.enqueue_localized_toast(
                "已创建审批工作流并在工作台打开",
                "Created approval workflow in Workbench",
                2200,
                cx,
            );
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_run_workflow(&mut self, workflow_id: String, cx: &mut Context<Self>) {
        let Some((rt, _ob_id, bot_name)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        self.bot_fusion.selected_workflow_id = Some(workflow_id.clone());
        let res = rt.workflows().run_workflow(WorkflowRunRequest {
            workflow_id: workflow_id.clone(),
            run_id: None,
            payload: Some(format!("Triggered from {bot_name} chat")),
            approve_node: None,
            reject_node: None,
            condition_mocks: BTreeMap::new(),
            idempotency_key: None,
        });
        if let Ok(outcome) = res {
            for step in &outcome.steps {
                self.push_bot_fusion_step(
                    format!("Step · {}", step.node_id),
                    &step.status,
                    step.output
                        .clone()
                        .unwrap_or_else(|| format!("{:?}", step.kind)),
                );
            }
            if let Some(approval_step) = outcome
                .steps
                .iter()
                .find(|s| s.status.eq_ignore_ascii_case("approval"))
            {
                let wf_title = rt
                    .workflows()
                    .get_workflow(&workflow_id)
                    .ok()
                    .flatten()
                    .map(|d| d.summary.title)
                    .unwrap_or_else(|| workflow_id.clone());
                self.bot_fusion
                    .pending_ask_cards
                    .retain(|c| c.workflow_id != workflow_id);
                self.bot_fusion.pending_ask_cards.push(BotFusionAskCard {
                    workflow_id: workflow_id.clone(),
                    run_id: outcome.run_id.clone(),
                    node_id: approval_step.node_id.clone(),
                    title: format!("工作流审批 · {wf_title}"),
                    prompt: format!(
                        "节点 `{}` 暂停等待你确认，批准后将自动继续下游交付。",
                        approval_step.node_id
                    ),
                });
                self.enqueue_localized_toast(
                    "工作流已暂停在审批节点，请在输入框上方审批",
                    "Workflow paused at approval gate",
                    2600,
                    cx,
                );
            } else {
                self.enqueue_localized_toast("工作流执行完成", "Workflow completed", 2000, cx);
            }
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_decide_workflow_approval(
        &mut self,
        workflow_id: String,
        run_id: String,
        node_id: String,
        approve: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((rt, _, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let res = rt
            .workflows()
            .decide_approval(&run_id, &node_id, approve, &BTreeMap::new());
        self.bot_fusion
            .pending_ask_cards
            .retain(|c| !(c.workflow_id == workflow_id && c.run_id == run_id));
        if let Ok(outcome) = res {
            self.push_bot_fusion_step(
                format!("Approval · {node_id}"),
                if approve { "approved" } else { "rejected" },
                format!("workflow status: {}", outcome.status),
            );
            if approve {
                self.enqueue_localized_toast("已批准，工作流继续执行完成", "Approved · workflow resumed", 2200, cx);
            } else {
                self.enqueue_localized_toast("已拒绝该审批节点", "Rejected approval step", 2000, cx);
            }
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_queue_add(&mut self, text: String, cx: &mut Context<Self>) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let res = rt.scheduler().queue_add(QueueAddInput {
            agent_id: ob_id,
            text: trimmed.clone(),
            task_id: None,
            attachments: Vec::new(),
            welcome: false,
            sender: None,
            sender_member: None,
            idempotency_key: None,
        });
        if res.is_ok() {
            self.push_bot_fusion_step("Queue", "queued", trimmed);
            self.enqueue_localized_toast("已加入 Bot 发送队列", "Added to Bot Queue", 1800, cx);
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_queue_toggle_pause(&mut self, cx: &mut Context<Self>) {
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        if self.bot_fusion.snapshot.queue_paused {
            let _ = rt.scheduler().queue_resume(&ob_id);
            self.push_bot_fusion_step("Queue", "resumed", "Drain scheduler resumed");
        } else {
            let _ = rt.scheduler().queue_pause(&ob_id);
            self.push_bot_fusion_step("Queue", "paused", "Drain scheduler paused");
        }
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_queue_steer(&mut self, delivery_id: String, cx: &mut Context<Self>) {
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let expected_turn = self
            .bot_fusion
            .snapshot
            .active_turn_id
            .clone()
            .unwrap_or_default();
        let _ = rt.scheduler().queue_steer(QueueSteerInput {
            agent_id: ob_id,
            delivery_id: delivery_id.clone(),
            expected_turn_id: expected_turn,
            routine_turn: false,
        });
        self.push_bot_fusion_step("Steer", "triggered", format!("steered {delivery_id}"));
        self.enqueue_localized_toast("已发起插队转向 (Steer)", "Steer signal sent", 1800, cx);
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_queue_cancel(&mut self, delivery_id: String, cx: &mut Context<Self>) {
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        if rt.scheduler().queue_cancel(&ob_id, &delivery_id).is_err() {
            if let Some(turn_id) = self.bot_fusion.snapshot.active_turn_id.clone() {
                let _ = rt.scheduler().complete_active_turn(&ob_id, &turn_id);
            }
            let _ = rt.scheduler().queue_cancel(&ob_id, &delivery_id);
        }
        self.push_bot_fusion_step("Queue", "canceled", delivery_id);
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_ask_side_question(&mut self, question: String, cx: &mut Context<Self>) {
        let q = question.trim().to_string();
        if q.is_empty() {
            return;
        }
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let res = rt.scheduler().ask_side_question(SideQuestionInput {
            agent_id: ob_id,
            question: q.clone(),
            task_id: None,
            earlier: Vec::new(),
        });
        let answer = match res {
            Ok(out) if !out.answer.trim().is_empty() => out.answer,
            _ => format!("（无痕侧问已就绪）已记录临时问题：「{q}」，不会污染当前主对话历史。"),
        };
        self.bot_fusion.side_question_last = Some((q.clone(), answer));
        self.push_bot_fusion_step("Side-Question", "ephemeral", q);
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn bot_fusion_branch_from_active_message(
        &mut self,
        message_id: String,
        title: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some((rt, ob_id, _)) = self.ensure_active_openbot_agent(cx) else {
            return;
        };
        let req_id = format!(
            "branch-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let outcome = rt.scheduler().branch_thread(BranchThreadInput {
            agent_id: ob_id,
            message_id: message_id.clone(),
            request_id: req_id,
            source_task_id: None,
            title: title.clone(),
            simulate_source_busy: false,
        });
        let detail = match outcome {
            Ok(b) => format!("branched thread {} (cloned {} msgs)", b.thread_id, b.cloned_messages),
            Err(_) => format!("created side branch from #{message_id}"),
        };
        self.push_bot_fusion_step("Branch", "created", detail);
        self.enqueue_localized_toast("已从当前上下文创建新分支话题", "Created branched topic", 2000, cx);
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    /// Intercepts native `$` commands submitted from `CanonicalComposer` in a Bot chat.
    /// Returns `true` when handled locally (no remote chat message needed).
    pub fn try_handle_bot_fusion_dollar_command(
        &mut self,
        text: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_fabushi_bot_for_fusion(cx).is_none() {
            return false;
        }
        let trimmed = text.trim();
        if !trimmed.starts_with('$') {
            return false;
        }
        let rest = trimmed.trim_start_matches('$').trim();
        let (head, arg) = match rest.split_once(char::is_whitespace) {
            Some((h, a)) => (h.trim(), a.trim()),
            None => (rest, ""),
        };
        let lower = head.to_ascii_lowercase();
        if lower == "side" {
            if !arg.is_empty() {
                self.bot_fusion_ask_side_question(arg.to_string(), cx);
            } else {
                self.toggle_bot_companion_panel(Some(BotCompanionTab::Activity), cx);
            }
            return true;
        }
        if lower == "queue" {
            if !arg.is_empty() {
                self.bot_fusion_queue_add(arg.to_string(), cx);
            } else {
                self.toggle_bot_companion_panel(Some(BotCompanionTab::Activity), cx);
            }
            return true;
        }
        if lower == "workflow" {
            self.toggle_bot_companion_panel(Some(BotCompanionTab::Workflow), cx);
            return true;
        }
        if let Some(wf_slug) = lower.strip_prefix("workflow:") {
            if let Some(wf) = self
                .bot_fusion
                .snapshot
                .workflows
                .iter()
                .find(|w| sanitize_openbot_skill_slug(&w.title) == wf_slug || w.id == wf_slug)
                .cloned()
            {
                self.bot_fusion_run_workflow(wf.id, cx);
                return true;
            }
        }
        if let Some(rt_slug) = lower.strip_prefix("routine:") {
            if let Some(rt_item) = self
                .bot_fusion
                .snapshot
                .routines
                .iter()
                .find(|r| sanitize_openbot_skill_slug(&r.name) == rt_slug || r.id == rt_slug)
                .cloned()
            {
                self.bot_fusion_run_routine_now(rt_item.id, cx);
                return true;
            }
        }
        false
    }

    /// Renders the compact Native Bot Fusion strip right above `CanonicalComposer` for Bot chats:
    /// - Inline `ask_ui` approval cards (with Approve / Reject buttons)
    /// - Ephemeral Side-Question answer bubble
    /// - Compact Queue / Steer strip when queued messages exist
    /// - Clean 1-line Muse Quick Bar (`工作台 · 记忆 · $技能 · 队列 · 侧问 · 工作流`)
    pub fn render_bot_fusion_composer_strip(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let fab = self.active_fabushi_bot_for_fusion(cx)?;
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();
        let snap = self.bot_fusion.snapshot.clone();
        let is_apple = pal.is_apple_style();

        let mut stack = div()
            .id("bot-fusion-composer-strip")
            .w_full()
            .bg(rgb(pal.composer_bg))
            .px(px(if is_apple { 20.0 } else { 16.0 }))
            .pt(px(6.0))
            .pb(px(2.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.0));

        let mut inner = div()
            .w_full()
            .when(is_apple, |d| d.max_w(px(740.0)))
            .flex()
            .flex_col()
            .gap(px(6.0));

        // 1. Active inline `ask_ui` cards (e.g. Workflow Approval Gates)
        for (idx, card) in snap.ask_cards.iter().enumerate() {
            let wf_ok = card.workflow_id.clone();
            let run_ok = card.run_id.clone();
            let node_ok = card.node_id.clone();
            let wf_no = card.workflow_id.clone();
            let run_no = card.run_id.clone();
            let node_no = card.node_id.clone();
            inner = inner.child(
                div()
                    .id(SharedString::from(format!("bot-ask-ui-card-{idx}")))
                    .w_full()
                    .px(px(12.0))
                    .py(px(9.0))
                    .rounded(px(12.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.10))
                    .border_1()
                    .border_color(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.35))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(card.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(card.prompt.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("bot-ask-ui-approve-{idx}")))
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(5.0))
                                    .rounded_full()
                                    .bg(rgb(pal.accent))
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xFFFFFF))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.bot_fusion_decide_workflow_approval(
                                                wf_ok.clone(),
                                                run_ok.clone(),
                                                node_ok.clone(),
                                                true,
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(tr_str(lang, "✓ 批准继续", "✓ Approve")),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("bot-ask-ui-reject-{idx}")))
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(5.0))
                                    .rounded_full()
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.danger))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.bot_fusion_decide_workflow_approval(
                                                wf_no.clone(),
                                                run_no.clone(),
                                                node_no.clone(),
                                                false,
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(tr_str(lang, "✕ 拒绝", "✕ Reject")),
                            ),
                    ),
            );
        }

        // 2. Ephemeral Side-Question bubble (never touches chat history)
        if let Some((q, a)) = snap.side_question_last.clone() {
            inner = inner.child(
                div()
                    .id("bot-side-question-card")
                    .w_full()
                    .px(px(11.0))
                    .py(px(8.0))
                    .rounded(px(10.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .child(format!(
                                        "{} · {q}",
                                        tr_str(lang, "无痕侧问 (不写入历史)", "Side Question (Ephemeral)")
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal.text_primary))
                                    .child(a),
                            ),
                    )
                    .child(
                        div()
                            .id("bot-side-question-dismiss")
                            .cursor_pointer()
                            .px(px(4.0))
                            .text_size(px(12.0))
                            .text_color(rgb(pal.text_secondary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.bot_fusion.side_question_last = None;
                                    this.refresh_active_bot_fusion_snapshot(cx);
                                    cx.notify();
                                }),
                            )
                            .child("×"),
                    ),
            );
        }

        // 3. Compact Queue / Steer strip when queued messages exist
        if !snap.queue_items.is_empty() {
            let first = snap.queue_items[0].clone();
            let first_steer_id = first.delivery_id.clone();
            let first_cancel_id = first.delivery_id.clone();
            inner = inner.child(
                div()
                    .id("bot-queue-steer-bar")
                    .w_full()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(10.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.5))
                                    .rounded_full()
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .child(format!(
                                        "{} {}",
                                        tr_str(lang, "队列", "Queue"),
                                        snap.queue_items.len()
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal.text_primary))
                                    .child(first.text),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id("bot-queue-pause-btn")
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(2.5))
                                    .rounded_full()
                                    .bg(rgb(pal.card_bg))
                                    .border_1()
                                    .border_color(pal.separator_rgba())
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.bot_fusion_queue_toggle_pause(cx);
                                        }),
                                    )
                                    .child(if snap.queue_paused {
                                        tr_str(lang, "恢复", "Resume")
                                    } else {
                                        tr_str(lang, "暂停", "Pause")
                                    }),
                            )
                            .child(
                                div()
                                    .id("bot-queue-steer-btn")
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(2.5))
                                    .rounded_full()
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.bot_fusion_queue_steer(first_steer_id.clone(), cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "⚡ 转向", "⚡ Steer")),
                            )
                            .child(
                                div()
                                    .id("bot-queue-cancel-btn")
                                    .cursor_pointer()
                                    .px(px(5.0))
                                    .py(px(2.5))
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.bot_fusion_queue_cancel(first_cancel_id.clone(), cx);
                                        }),
                                    )
                                    .child("×"),
                            ),
                    ),
            );
        }

        // 4. Compact 1-line Muse Native Bot Quick Bar
        let comp_open = self.bot_fusion.companion_open;
        let mem_count = snap.memories.len();
        let skill_count = snap.skills.iter().filter(|s| s.enabled).count();
        let wf_count = snap.workflows.len();
        let q_count = snap.queue_items.len();
        let bot_label = if fab.display_name.trim().is_empty() {
            fab.agent_id
        } else {
            fab.display_name
        };

        let quick_pill = |id: &'static str,
                          label: String,
                          active: bool|
         -> Stateful<Div> {
            div()
                .id(id)
                .cursor_pointer()
                .px(px(8.0))
                .py(px(3.0))
                .rounded_full()
                .bg(if active {
                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                } else {
                    rgb(pal.input_bg).into()
                })
                .border_1()
                .border_color(if active {
                    rgb(pal.accent).into()
                } else {
                    pal.separator_rgba()
                })
                .text_size(px(10.5))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(rgb(if active {
                    pal.accent
                } else {
                    pal.text_secondary
                }))
                .child(label)
        };

        let quick_bar = div()
            .id("bot-native-quick-bar")
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        quick_pill(
                            "bot-qb-workbench",
                            format!(
                                "✦ {}",
                                tr_str(lang, "伴生工作台", "Workbench")
                            ),
                            comp_open,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_bot_companion_panel(None, cx);
                            }),
                        ),
                    )
                    .child(
                        quick_pill(
                            "bot-qb-dollar",
                            format!("$ {} ({skill_count})", tr_str(lang, "技能", "Skills")),
                            false,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.composer.update(cx, |c, ccx| {
                                    c.open_dollar_picker();
                                    ccx.notify();
                                });
                            }),
                        ),
                    )
                    .child(
                        quick_pill(
                            "bot-qb-memory",
                            format!("{} ({mem_count})", tr_str(lang, "记忆", "Memory")),
                            comp_open && self.bot_fusion.companion_tab == BotCompanionTab::Knowledge,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_bot_companion_panel(
                                    Some(BotCompanionTab::Knowledge),
                                    cx,
                                );
                            }),
                        ),
                    )
                    .child(
                        quick_pill(
                            "bot-qb-workflow",
                            format!("{} ({wf_count})", tr_str(lang, "工作流", "Flow")),
                            comp_open && self.bot_fusion.companion_tab == BotCompanionTab::Workflow,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_bot_companion_panel(
                                    Some(BotCompanionTab::Workflow),
                                    cx,
                                );
                            }),
                        ),
                    )
                    .child(
                        quick_pill(
                            "bot-qb-queue",
                            format!("{} ({q_count})", tr_str(lang, "队列", "Queue")),
                            q_count > 0,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                let draft = this.composer.update(cx, |c, ccx| {
                                    let t = c.text().trim().to_string();
                                    if !t.is_empty() {
                                        c.set_text(String::new());
                                        ccx.notify();
                                    }
                                    t
                                });
                                if !draft.is_empty() {
                                    this.bot_fusion_queue_add(draft, cx);
                                } else {
                                    this.toggle_bot_companion_panel(
                                        Some(BotCompanionTab::Activity),
                                        cx,
                                    );
                                }
                            }),
                        ),
                    )
                    .child(
                        quick_pill(
                            "bot-qb-side",
                            tr_str(lang, "无痕侧问", "Side Q&A").to_string(),
                            false,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                let draft = this.composer.update(cx, |c, ccx| {
                                    let t = c.text().trim().to_string();
                                    if !t.is_empty() {
                                        c.set_text(String::new());
                                        ccx.notify();
                                    }
                                    t
                                });
                                if !draft.is_empty() {
                                    this.bot_fusion_ask_side_question(draft, cx);
                                } else {
                                    this.composer.update(cx, |c, ccx| {
                                        c.set_text("$side ".to_string());
                                        c.is_input_focused = true;
                                        ccx.notify();
                                    });
                                }
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(pal.text_secondary))
                    .truncate()
                    .child(format!("{bot_label} · $")),
            );

        inner = inner.child(quick_bar);
        stack = stack.child(inner);
        Some(stack.into_any_element())
    }

    /// Renders the 380px inline right-side Bot Companion SidePanel beside `ConversationStage`.
    pub fn render_bot_companion_sidepanel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();
        let snap = self.bot_fusion.snapshot.clone();
        let active_tab = self.bot_fusion.companion_tab;

        let tab_btn = |id: &'static str, label: &'static str, tab: BotCompanionTab, cx: &mut Context<Self>| {
            let selected = active_tab == tab;
            div()
                .id(id)
                .flex_1()
                .cursor_pointer()
                .py(px(5.0))
                .rounded(px(7.0))
                .bg(if selected {
                    rgb(pal.card_bg).into()
                } else {
                    Rgba::default()
                })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.5))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(rgb(if selected {
                    pal.text_primary
                } else {
                    pal.text_secondary
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.bot_fusion.companion_tab = tab;
                        this.refresh_active_bot_fusion_snapshot(cx);
                        cx.notify();
                    }),
                )
                .child(label)
        };

        let header = div()
            .w_full()
            .px(px(14.0))
            .py(px(12.0))
            .border_b_1()
            .border_color(pal.separator_rgba())
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(tg_icon(IconName::BotRobot, 15.0, pal.accent))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_primary))
                            .truncate()
                            .child(format!(
                                "{} · {}",
                                snap.bot_display_name,
                                tr_str(lang, "伴生工作台", "Workbench")
                            )),
                    ),
            )
            .child(
                div()
                    .id("bot-companion-close-btn")
                    .cursor_pointer()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .hover(|s| s.bg(rgb(pal.menu_hover_bg)))
                    .text_size(px(13.0))
                    .text_color(rgb(pal.text_secondary))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.bot_fusion.companion_open = false;
                            this.refresh_active_bot_fusion_snapshot(cx);
                            cx.notify();
                        }),
                    )
                    .child("×"),
            );

        let tabs_bar = div()
            .mx(px(12.0))
            .mt(px(10.0))
            .p(px(3.0))
            .rounded(px(9.0))
            .bg(rgb(pal.input_bg))
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(tab_btn(
                "bot-comp-tab-workflow",
                tr_str(lang, "工作流画布", "Workflow"),
                BotCompanionTab::Workflow,
                cx,
            ))
            .child(tab_btn(
                "bot-comp-tab-knowledge",
                tr_str(lang, "记忆与技能", "Knowledge"),
                BotCompanionTab::Knowledge,
                cx,
            ))
            .child(tab_btn(
                "bot-comp-tab-activity",
                tr_str(lang, "队列与活动", "Activity"),
                BotCompanionTab::Activity,
                cx,
            ));

        let body: AnyElement = match active_tab {
            BotCompanionTab::Workflow => {
                let mut col = div().flex().flex_col().gap(px(10.0)).child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "{} ({})",
                                    tr_str(lang, "DAG 工作流", "DAG Workflows"),
                                    snap.workflows.len()
                                )),
                        )
                        .child(
                            div()
                                .id("bot-comp-wf-create")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.0))
                                .rounded_full()
                                .bg(rgb(pal.accent))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xFFFFFF))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.bot_fusion_create_starter_workflow(cx);
                                    }),
                                )
                                .child(tr_str(lang, "+ 新建审批流", "+ New Flow")),
                        ),
                );

                if snap.workflows.is_empty() {
                    col = col.child(
                        div()
                            .p(px(12.0))
                            .rounded(px(10.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(pal.separator_rgba())
                            .text_size(px(11.5))
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "点击「+ 新建审批流」创建一个 4 节点工作流（触发器 → Bot 智能体 → 人工审批 → 交付），运行后会在聊天下方直接弹出可交互审批卡片。",
                                "Click '+ New Flow' to create a 4-node DAG (Trigger -> Agent -> Approval -> Output) with inline chat approval cards.",
                            )),
                    );
                } else {
                    for (idx, wf) in snap.workflows.iter().enumerate() {
                        let wid_run = wf.id.clone();
                        let wid_sel = wf.id.clone();
                        let is_sel = snap.selected_workflow_id.as_deref() == Some(wf.id.as_str());
                        let mut node_chain = div().flex().flex_wrap().items_center().gap(px(4.0));
                        for (ni, lbl) in wf.node_labels.iter().enumerate() {
                            if ni > 0 {
                                node_chain = node_chain.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child("→"),
                                );
                            }
                            node_chain = node_chain.child(
                                div()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(pal.card_bg))
                                    .border_1()
                                    .border_color(pal.separator_rgba())
                                    .text_size(px(10.0))
                                    .text_color(rgb(pal.text_primary))
                                    .child(lbl.clone()),
                            );
                        }

                        col = col.child(
                            div()
                                .id(SharedString::from(format!("bot-comp-wf-card-{idx}")))
                                .p(px(10.0))
                                .rounded(px(10.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(if is_sel {
                                    rgb(pal.accent).into()
                                } else {
                                    pal.separator_rgba()
                                })
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.bot_fusion.selected_workflow_id =
                                            Some(wid_sel.clone());
                                        this.refresh_active_bot_fusion_snapshot(cx);
                                        cx.notify();
                                    }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .truncate()
                                                .child(wf.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "bot-comp-wf-run-{idx}"
                                                )))
                                                .cursor_pointer()
                                                .px(px(8.0))
                                                .py(px(3.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(pal.accent))
                                                .text_size(px(10.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(0xFFFFFF))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.bot_fusion_run_workflow(
                                                            wid_run.clone(),
                                                            cx,
                                                        );
                                                    }),
                                                )
                                                .child(tr_str(lang, "▶ 运行", "▶ Run")),
                                        ),
                                )
                                .child(node_chain)
                                .when_some(wf.last_run_status.clone(), |c, st| {
                                    c.child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(pal.accent))
                                            .child(format!(
                                                "{}: {st}",
                                                tr_str(lang, "最近状态", "Last Status")
                                            )),
                                    )
                                }),
                        );
                    }
                }
                col.into_any_element()
            }
            BotCompanionTab::Knowledge => {
                let mut col = div().flex().flex_col().gap(px(12.0));
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "{} ({})",
                                    tr_str(lang, "专属记忆", "Memories"),
                                    snap.memories.len()
                                )),
                        )
                        .child(
                            div()
                                .id("bot-comp-mem-tidy")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded_full()
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                .text_size(px(10.5))
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.bot_fusion_tidy_memories(cx);
                                    }),
                                )
                                .child(tr_str(lang, "整理去重", "Tidy")),
                        ),
                );
                if snap.memories.is_empty() {
                    col = col.child(
                        div()
                            .p(px(10.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .text_size(px(11.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "在右侧「Bot 资料 → 智能体配置」可直接添加长期记忆。",
                                "Add long-term memories in Bot Inspector → Agent Config.",
                            )),
                    );
                } else {
                    for (idx, m) in snap.memories.iter().enumerate() {
                        let mid = m.id.clone();
                        col = col.child(
                            div()
                                .id(SharedString::from(format!("bot-comp-mem-{idx}")))
                                .px(px(9.0))
                                .py(px(7.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(pal.separator_rgba())
                                .flex()
                                .items_start()
                                .justify_between()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_primary))
                                        .child(m.content.clone()),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("bot-comp-mem-del-{idx}")))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.bot_fusion_delete_memory(mid.clone(), cx);
                                            }),
                                        )
                                        .child("×"),
                                ),
                        );
                    }
                }

                col = col.child(
                    div()
                        .mt(px(4.0))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "{} ({}) · {} ({})",
                            tr_str(lang, "已装技能", "Skills"),
                            snap.skills.len(),
                            tr_str(lang, "定时任务", "Routines"),
                            snap.routines.len()
                        )),
                );
                for (idx, sk) in snap.skills.iter().enumerate() {
                    let sid = sk.skill_id.clone();
                    let next_en = !sk.enabled;
                    col = col.child(
                        div()
                            .id(SharedString::from(format!("bot-comp-skill-{idx}")))
                            .px(px(9.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal.text_primary))
                                    .child(format!("${} · {}", sk.slug, sk.name)),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("bot-comp-skill-tgl-{idx}")))
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(2.0))
                                    .rounded_full()
                                    .bg(if sk.enabled {
                                        TdesktopThemePalette::rgba_with_alpha(pal.success, 0.16)
                                    } else {
                                        rgb(pal.card_bg).into()
                                    })
                                    .text_size(px(10.0))
                                    .text_color(rgb(if sk.enabled {
                                        pal.success
                                    } else {
                                        pal.text_secondary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.bot_fusion_toggle_skill(sid.clone(), next_en, cx);
                                        }),
                                    )
                                    .child(if sk.enabled { "ON" } else { "OFF" }),
                            ),
                    );
                }
                col.into_any_element()
            }
            BotCompanionTab::Activity => {
                let mut col = div().flex().flex_col().gap(px(10.0));
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "{} ({})",
                                    tr_str(lang, "消息队列 & 转向", "Queue & Steer"),
                                    snap.queue_items.len()
                                )),
                        )
                        .child(
                            div()
                                .id("bot-comp-queue-pause")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded_full()
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                .text_size(px(10.5))
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.bot_fusion_queue_toggle_pause(cx);
                                    }),
                                )
                                .child(if snap.queue_paused {
                                    tr_str(lang, "恢复队列", "Resume")
                                } else {
                                    tr_str(lang, "暂停队列", "Pause")
                                }),
                        ),
                );
                if snap.queue_items.is_empty() {
                    col = col.child(
                        div()
                            .p(px(10.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .text_size(px(11.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "输入 `$queue 内容` 或在输入框输入后点「队列」可将消息排队。",
                                "Type `$queue text` or click Queue with draft text to enqueue.",
                            )),
                    );
                } else {
                    for (idx, q) in snap.queue_items.iter().enumerate() {
                        let sid = q.delivery_id.clone();
                        let cid = q.delivery_id.clone();
                        col = col.child(
                            div()
                                .id(SharedString::from(format!("bot-comp-q-{idx}")))
                                .px(px(9.0))
                                .py(px(7.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_primary))
                                        .child(q.text.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .id(SharedString::from(format!("bot-comp-q-steer-{idx}")))
                                                .cursor_pointer()
                                                .px(px(6.0))
                                                .py(px(2.0))
                                                .rounded(px(5.0))
                                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                                .text_size(px(10.0))
                                                .text_color(rgb(pal.accent))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.bot_fusion_queue_steer(sid.clone(), cx);
                                                    }),
                                                )
                                                .child("⚡"),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(format!("bot-comp-q-cancel-{idx}")))
                                                .cursor_pointer()
                                                .text_size(px(11.0))
                                                .text_color(rgb(pal.text_secondary))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.bot_fusion_queue_cancel(cid.clone(), cx);
                                                    }),
                                                )
                                                .child("×"),
                                        ),
                                ),
                        );
                    }
                }

                col = col.child(
                    div()
                        .mt(px(4.0))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "{} ({})",
                            tr_str(lang, "执行步骤 (AgentSteps)", "Agent Steps"),
                            snap.recent_steps.len()
                        )),
                );
                for (idx, st) in snap.recent_steps.iter().take(10).enumerate() {
                    col = col.child(
                        div()
                            .id(SharedString::from(format!("bot-comp-step-{idx}")))
                            .px(px(9.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(st.label.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(pal.accent))
                                            .child(st.status.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(st.detail.clone()),
                            ),
                    );
                }
                col.into_any_element()
            }
        };

        let panel_bg = if pal.is_apple_style() {
            pal.stage_bg
        } else {
            pal.dialogs_bg
        };

        div()
            .id("app-shell-bot-companion-slot")
            .w(px(380.0))
            .flex_shrink_0()
            .h_full()
            .bg(rgb(panel_bg))
            .border_l_1()
            .border_color(pal.separator_rgba())
            .flex()
            .flex_col()
            .child(header)
            .child(tabs_bar)
            .child(
                div()
                    .id("bot-companion-scroll-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(12.0))
                    .child(body),
            )
            .into_any_element()
    }
}
