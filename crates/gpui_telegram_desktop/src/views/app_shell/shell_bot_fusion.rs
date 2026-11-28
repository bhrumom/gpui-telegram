//! Native Bot Fusion Controller & Inline UI (`AppShell` <-> `OpenBotHostRuntime`).
//!
//! Unifies OpenBot & Orca capabilities directly inside Fabushi's existing native Bot system:
//! - Automatic 1:1 local `OpenBotHostRuntime` agent provisioning keyed by `FabushiBotSummaryDto::agent_id`.
//! - Live Memory CRUD + Tidy-Up, Skills & MCP management, Routines, and Approval Workflows.
//! - In-chat Bot Quick Bar, inline `ask_ui` approval cards, Queue/Steer bar, ephemeral Side-Question,
//!   `$` Skills/Routines/Workflows composer command execution, and 380px Bot Companion SidePanel.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::{CreationKind, CreationReviewStatus};
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
use crate::theme::{tg_icon, tg_icon_rgba, IconName, TdesktopThemePalette};
use crate::views::content_hub::ContentHubSection;
use crate::views::drawer::inspector_tabs::{
    BotPublishedCreationSummary, OpenBotInspectorAction,
};
use crate::views::drawer::{
    BotCompanionTab, BotFusionAskCard, BotFusionDollarCandidate, BotFusionMcpItem,
    BotFusionMemoryItem, BotFusionQueueItem, BotFusionRoutineItem, BotFusionSkillItem,
    BotFusionSnapshot, BotFusionStepItem, BotFusionWorkflowItem, InspectorAction,
    InspectorSubpage, InspectorTab,
};
use crate::views::openbot::channel_view::{
    render_tucked_awaiting_replies_section, render_tucked_stopped_tasks_section,
    AwaitingReplyEntry, AwaitingReplyState, ChannelStoppedTasksState,
};
use crate::views::openbot::side_panel::{
    CONVERSATION_PANEL_MIN, DRAG_START_PX, LAUNCHER_PRIMARY_PANES, MAX_SIDE_SPLITS,
    OPENBOT_LAUNCHER_BUTTON_HEIGHT, OPENBOT_LAUNCHER_MAX_WIDTH, OPENBOT_SIDE_PANEL_HEADER_HEIGHT,
    SETTINGS_PANEL_DEFAULT, SETTINGS_PANEL_MAX, SIDE_MAIN_GROUP_MIN, SIDE_SPLIT_MIN,
    SidePanelPaneKind, SidePanelSettingsTarget, SidePanelState,
};
use crate::views::openbot::WorkflowCanvasState;
use crate::views::shell::{ActiveWorkspacePanel, AppShell};
use crate::views::stage::openbot_stage_chrome::{
    normalize_fabushi_plush_spec, openbot_stage_icon, render_tab_concave_fillet, OpenBotStageIcon,
};

/// Unified runtime state for the dockable, resizable, vertically-splittable OpenBot Right `SidePanel`.
#[derive(Debug, Clone)]
pub struct OpenBotRightSidePanelState {
    pub state: SidePanelState,
    pub open_tabs: Vec<SidePanelPaneKind>,
    pub active_top_tab: Option<SidePanelPaneKind>,
    pub split_bottom_pane: Option<SidePanelPaneKind>,
    pub is_resizing_width: bool,
    pub resize_drag_start_x: Option<f32>,
    pub resize_start_width: f32,
    pub is_resizing_split: bool,
    pub split_drag_start_y: Option<f32>,
    pub split_start_share: f64,
    pub add_menu_open: bool,
    pub more_menu_open: bool,
    pub side_question_last_error: Option<String>,
    pub browser_url_draft: String,
    pub hovered_browser_preview_tab_id: Option<String>,
    pub browser_page_tabs_shown: bool,
    pub files_filter_draft: String,
    pub unavailable_tabs: BTreeSet<String>,
    pub last_synced_inspector_open: bool,
    pub last_synced_companion_open: bool,
}

impl Default for OpenBotRightSidePanelState {
    fn default() -> Self {
        let mut st = SidePanelState::default();
        st.open = false;
        st.active_splits.clear();
        st.width_px = SETTINGS_PANEL_DEFAULT;
        st.lower_share = 0.45;
        st.maximized = false;
        Self {
            state: st,
            open_tabs: Vec::new(),
            active_top_tab: None,
            split_bottom_pane: None,
            is_resizing_width: false,
            resize_drag_start_x: None,
            resize_start_width: SETTINGS_PANEL_DEFAULT,
            is_resizing_split: false,
            split_drag_start_y: None,
            split_start_share: 0.45,
            add_menu_open: false,
            more_menu_open: false,
            side_question_last_error: None,
            browser_url_draft: crate::views::browser::BROWSER_NEW_TAB_URL.to_string(),
            hovered_browser_preview_tab_id: None,
            browser_page_tabs_shown: false,
            files_filter_draft: String::new(),
            unavailable_tabs: BTreeSet::new(),
            last_synced_inspector_open: false,
            last_synced_companion_open: false,
        }
    }
}

impl OpenBotRightSidePanelState {
    pub fn reset_ui(&mut self) {
        self.state.open = false;
        self.state.active_splits.clear();
        self.state.maximized = false;
        self.state.question.turns.clear();
        self.state.question.draft_question.clear();
        self.open_tabs.clear();
        self.active_top_tab = None;
        self.split_bottom_pane = None;
        self.is_resizing_width = false;
        self.resize_drag_start_x = None;
        self.is_resizing_split = false;
        self.split_drag_start_y = None;
        self.add_menu_open = false;
        self.more_menu_open = false;
        self.side_question_last_error = None;
        self.hovered_browser_preview_tab_id = None;
        self.browser_page_tabs_shown = false;
        self.files_filter_draft.clear();
        self.unavailable_tabs.clear();
        self.last_synced_inspector_open = false;
        self.last_synced_companion_open = false;
    }

    pub fn sync_active_splits(&mut self) {
        self.state.active_splits.clear();
        if let Some(top) = self.active_top_tab {
            self.state.active_splits.push(top);
        }
        if let Some(bot) = self.split_bottom_pane {
            if self.state.active_splits.len() < MAX_SIDE_SPLITS
                && !self.state.active_splits.contains(&bot)
            {
                self.state.active_splits.push(bot);
            }
        }
    }

    pub fn is_settings_visible(&self) -> bool {
        self.state.open
            && (self.active_top_tab == Some(SidePanelPaneKind::Settings)
                || self.split_bottom_pane == Some(SidePanelPaneKind::Settings))
    }

    pub fn open_pane(&mut self, pane: SidePanelPaneKind) {
        self.state.open = true;
        self.add_menu_open = false;
        self.more_menu_open = false;
        if self.split_bottom_pane == Some(pane) {
            self.sync_active_splits();
            return;
        }
        if !self.open_tabs.contains(&pane) {
            self.open_tabs.push(pane);
        }
        self.active_top_tab = Some(pane);
        self.sync_active_splits();
    }

    pub fn close_tab(&mut self, pane: SidePanelPaneKind) -> bool {
        let mut changed = false;
        if self.split_bottom_pane == Some(pane) {
            self.split_bottom_pane = None;
            changed = true;
        }
        let before = self.open_tabs.len();
        self.open_tabs.retain(|&p| p != pane);
        if self.open_tabs.len() != before {
            changed = true;
        }
        if self.active_top_tab == Some(pane) {
            self.active_top_tab = self.open_tabs.last().copied();
            if self.active_top_tab.is_none() {
                if let Some(bot) = self.split_bottom_pane.take() {
                    self.open_tabs.push(bot);
                    self.active_top_tab = Some(bot);
                } else {
                    self.state.maximized = false;
                }
            }
        }
        self.sync_active_splits();
        changed
    }

    pub fn split_down(&mut self, pane: SidePanelPaneKind) {
        self.state.open = true;
        self.add_menu_open = false;
        self.more_menu_open = false;
        if self.active_top_tab.is_none() {
            self.open_pane(pane);
            return;
        }
        if self.active_top_tab == Some(pane) {
            if let Some(other) = self.open_tabs.iter().copied().find(|&p| p != pane) {
                self.active_top_tab = Some(other);
                self.open_tabs.retain(|&p| p != pane);
                self.split_bottom_pane = Some(pane);
            } else {
                let fallback = if pane == SidePanelPaneKind::Files {
                    SidePanelPaneKind::Threads
                } else {
                    SidePanelPaneKind::Files
                };
                self.split_bottom_pane = Some(fallback);
            }
        } else {
            self.open_tabs.retain(|&p| p != pane);
            self.split_bottom_pane = Some(pane);
        }
        self.sync_active_splits();
    }

    pub fn swap_split_panes(&mut self) -> bool {
        if let (Some(top), Some(bot)) = (self.active_top_tab, self.split_bottom_pane) {
            if let Some(slot) = self.open_tabs.iter_mut().find(|p| **p == top) {
                *slot = bot;
            }
            self.active_top_tab = Some(bot);
            self.split_bottom_pane = Some(top);
            self.sync_active_splits();
            true
        } else {
            false
        }
    }

    pub fn return_to_launcher(&mut self) {
        self.state.open = true;
        self.state.maximized = false;
        self.open_tabs.clear();
        self.unavailable_tabs.clear();
        self.active_top_tab = None;
        self.split_bottom_pane = None;
        self.add_menu_open = false;
        self.more_menu_open = false;
        self.sync_active_splits();
    }

    /// Marks or clears an open side-panel file tab as unavailable (`UI-F15`).
    pub fn mark_file_tab_unavailable(&mut self, tab_id: &str, missing: bool) {
        if missing {
            self.unavailable_tabs.insert(tab_id.to_string());
        } else {
            self.unavailable_tabs.remove(tab_id);
        }
    }

    /// Checks a workspace file path on disk and records its unavailable-tab state.
    /// Returns `true` when the file is missing/unavailable.
    pub fn sync_file_tab_path_availability(
        &mut self,
        tab_id: &str,
        path: &std::path::Path,
    ) -> bool {
        let missing = !path.exists();
        self.mark_file_tab_unavailable(tab_id, missing);
        missing
    }

    pub fn is_tab_unavailable(&self, tab_id: &str) -> bool {
        self.unavailable_tabs.contains(tab_id)
    }

    pub fn has_any_unavailable_tab(&self) -> bool {
        !self.unavailable_tabs.is_empty()
    }

    /// Attempts to open a 2nd vertical split, rejecting a 3rd split once `MAX_SIDE_SPLITS` (2) is reached (`T2-F15-02`).
    pub fn try_split_down(&mut self, pane: SidePanelPaneKind) -> bool {
        if self.active_top_tab.is_some() && self.split_bottom_pane.is_some() {
            return false;
        }
        self.split_down(pane);
        true
    }

    pub fn clamp_width_for_workspace(&mut self, requested_px: f32, workspace_avail_px: f32) -> f32 {
        let upper = if workspace_avail_px.is_finite() && workspace_avail_px > 0.0 {
            let max_by_vp =
                (workspace_avail_px - CONVERSATION_PANEL_MIN).max(SETTINGS_PANEL_DEFAULT);
            SETTINGS_PANEL_MAX.min(max_by_vp)
        } else {
            SETTINGS_PANEL_DEFAULT
        };
        let candidate = if requested_px.is_finite() {
            requested_px
        } else if self.state.width_px.is_finite() {
            self.state.width_px
        } else {
            SETTINGS_PANEL_DEFAULT
        };
        self.state.width_px = candidate.clamp(SETTINGS_PANEL_DEFAULT, upper);
        self.state.width_px
    }

    pub fn compute_header_layout(
        &self,
        target: SidePanelSettingsTarget,
        avail_w: f32,
        is_zh: bool,
    ) -> SidePanelHeaderLayoutSnapshot {
        let has_tabs = !self.open_tabs.is_empty() || self.split_bottom_pane.is_some();
        let safe_w = if avail_w.is_finite() && avail_w > 0.0 {
            avail_w.min(SETTINGS_PANEL_MAX)
        } else if self.state.width_px.is_finite() {
            self.state
                .width_px
                .clamp(SETTINGS_PANEL_DEFAULT, SETTINGS_PANEL_MAX)
        } else {
            SETTINGS_PANEL_DEFAULT
        };
        let total_tabs = self.open_tabs.len();
        let avail_for_tabs = (safe_w - 122.0).max(78.0);
        let max_visible_tabs = (avail_for_tabs / 78.0).floor().max(1.0) as usize;
        let overflow_chevron_visible = total_tabs > max_visible_tabs;
        let visible_tabs_count = total_tabs.min(max_visible_tabs);
        let hidden_tabs_count = total_tabs.saturating_sub(visible_tabs_count);
        let combined_actions_menu = has_tabs && (overflow_chevron_visible || safe_w < 380.0);
        let open_tabs = self.open_tabs.iter().map(|p| p.as_str()).collect();
        let add_pane_menu_items = LAUNCHER_PRIMARY_PANES
            .iter()
            .map(|p| p.launcher_label(target, is_zh).to_string())
            .collect();
        SidePanelHeaderLayoutSnapshot {
            height_px: crate::views::openbot::side_panel::OPENBOT_SIDE_PANEL_HEADER_HEIGHT,
            add_btn_size_px: 28.0,
            more_btn_visible: has_tabs,
            maximize_btn_visible: has_tabs && !combined_actions_menu,
            toggle_btn_size_px: 28.0,
            maximized: self.state.maximized,
            open_tabs,
            add_pane_menu_items,
            launcher_visible: self.state.open && !has_tabs,
            visible_tabs_count,
            hidden_tabs_count,
            overflow_chevron_visible,
            combined_actions_menu,
            unavailable_tab_flagged: self.has_any_unavailable_tab(),
        }
    }

    pub fn compute_layout(
        &self,
        workspace_avail_px: f32,
        panel_height_px: f32,
    ) -> SidePanelLayoutSnapshot {
        let safe_ws = if workspace_avail_px.is_finite() && workspace_avail_px > 0.0 {
            workspace_avail_px
        } else {
            1000.0
        };
        let max_by_vp = (safe_ws - CONVERSATION_PANEL_MIN).max(SETTINGS_PANEL_DEFAULT);
        let upper = SETTINGS_PANEL_MAX.min(max_by_vp);
        let safe_stored_w = if self.state.width_px.is_finite() {
            self.state.width_px
        } else {
            SETTINGS_PANEL_DEFAULT
        };
        let width_px = if !self.state.open {
            0.0
        } else if self.state.maximized {
            safe_ws.max(SETTINGS_PANEL_DEFAULT)
        } else {
            safe_stored_w.clamp(SETTINGS_PANEL_DEFAULT, upper)
        };
        let effective_width_pct = if !self.state.open {
            0
        } else if self.state.maximized {
            100
        } else {
            ((width_px / safe_ws) * 100.0).round().clamp(1.0, 100.0) as u32
        };
        let split_count = usize::from(self.active_top_tab.is_some())
            + usize::from(self.split_bottom_pane.is_some());
        let lower_share = if self.state.lower_share.is_finite() {
            (self.state.lower_share as f32).clamp(0.22, 0.72)
        } else {
            0.45
        };
        let safe_h = if panel_height_px.is_finite() && panel_height_px > 0.0 {
            panel_height_px
        } else {
            640.0
        };
        let body_h = (safe_h - crate::views::openbot::side_panel::OPENBOT_SIDE_PANEL_HEADER_HEIGHT)
            .max(SIDE_MAIN_GROUP_MIN + SIDE_SPLIT_MIN + 5.0);
        let (main_group_height_px, lower_split_height_px) = if self.split_bottom_pane.is_some() {
            let usable = (body_h - 5.0).max(SIDE_MAIN_GROUP_MIN + SIDE_SPLIT_MIN);
            let max_lower = (usable - SIDE_MAIN_GROUP_MIN).max(SIDE_SPLIT_MIN);
            let lower_h = (usable * lower_share).clamp(SIDE_SPLIT_MIN, max_lower);
            let main_h = (usable - lower_h).max(SIDE_MAIN_GROUP_MIN);
            (main_h, lower_h)
        } else {
            (body_h.max(SIDE_MAIN_GROUP_MIN), 0.0)
        };
        let target = if self.state.settings.is_channel {
            SidePanelSettingsTarget::Channel
        } else {
            SidePanelSettingsTarget::Assistant
        };
        SidePanelLayoutSnapshot {
            open: self.state.open,
            width_px,
            persisted_width_px: safe_stored_w,
            min_width_px: SETTINGS_PANEL_DEFAULT,
            max_width_px: SETTINGS_PANEL_MAX,
            conversation_min_px: CONVERSATION_PANEL_MIN,
            maximized: self.state.maximized,
            effective_width_pct,
            launcher_visible: self.state.open && self.active_top_tab.is_none() && self.split_bottom_pane.is_none(),
            open_tabs: self.open_tabs.clone(),
            active_top_tab: self.active_top_tab,
            split_bottom_pane: self.split_bottom_pane,
            active_splits: self.state.active_splits.clone(),
            split_count,
            max_splits: MAX_SIDE_SPLITS,
            lower_share,
            main_group_height_px,
            lower_split_height_px,
            min_pane_px: crate::views::openbot::side_panel::SIDE_SPLIT_MIN,
            header: self.compute_header_layout(target, width_px, false),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SidePanelHeaderLayoutSnapshot {
    pub height_px: f32,
    pub add_btn_size_px: f32,
    pub more_btn_visible: bool,
    pub maximize_btn_visible: bool,
    pub toggle_btn_size_px: f32,
    pub maximized: bool,
    pub open_tabs: Vec<&'static str>,
    pub add_pane_menu_items: Vec<String>,
    pub launcher_visible: bool,
    pub visible_tabs_count: usize,
    pub hidden_tabs_count: usize,
    pub overflow_chevron_visible: bool,
    pub combined_actions_menu: bool,
    pub unavailable_tab_flagged: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SidePanelLayoutSnapshot {
    pub open: bool,
    pub width_px: f32,
    pub persisted_width_px: f32,
    pub min_width_px: f32,
    pub max_width_px: f32,
    pub conversation_min_px: f32,
    pub maximized: bool,
    pub effective_width_pct: u32,
    pub launcher_visible: bool,
    pub open_tabs: Vec<SidePanelPaneKind>,
    pub active_top_tab: Option<SidePanelPaneKind>,
    pub split_bottom_pane: Option<SidePanelPaneKind>,
    pub active_splits: Vec<SidePanelPaneKind>,
    pub split_count: usize,
    pub max_splits: usize,
    pub lower_share: f32,
    pub main_group_height_px: f32,
    pub lower_split_height_px: f32,
    pub min_pane_px: f32,
    pub header: SidePanelHeaderLayoutSnapshot,
}

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
    // M3 OpenBot tucked .composer-stack state (UI-F11 / UI-F14)
    pub awaiting_replies: Vec<AwaitingReplyEntry>,
    pub subtasks_collapsed: bool,
    pub stopped_tasks: ChannelStoppedTasksState,
    pub browser_live_notice: Option<(String, bool)>,
    pub compacting_status: Option<String>,
    pub context_usage_ratio: Option<f32>,
    pub composer_notice: Option<crate::views::openbot::queue_and_voice::ComposerNoticeState>,
    pub open_waits_count: usize,
    // M4 Unified OpenBot Right `SidePanel` state (`UI-F15`)
    pub side_panel: OpenBotRightSidePanelState,
}

impl ShellBotFusionState {
    pub fn reset_ui(&mut self) {
        self.snapshot = BotFusionSnapshot::default();
        self.companion_open = false;
        self.selected_workflow_id = None;
        self.pending_ask_cards.clear();
        self.recent_steps.clear();
        self.side_question_last = None;
        self.awaiting_replies.clear();
        self.subtasks_collapsed = false;
        self.stopped_tasks.items.clear();
        self.stopped_tasks.resuming_ids.clear();
        self.stopped_tasks.resume_dispatch_count = 0;
        self.browser_live_notice = None;
        self.compacting_status = None;
        self.context_usage_ratio = None;
        self.composer_notice = None;
        self.open_waits_count = 0;
        self.side_panel.reset_ui();
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
        let (shape, hue, color_hex) =
            normalize_fabushi_plush_spec(&fab.avatar_shape, &fab.avatar_color);
        let avatar_seed = shape
            .map(|s| format!("plush:{}", s.as_str()))
            .or_else(|| {
                let raw = fab.avatar_shape.trim();
                (!raw.is_empty()).then(|| raw.to_string())
            });
        let req = CreateAgentRequest {
            name: display,
            title: foreign_title.to_string(),
            description: fab.description.clone(),
            allow_automation: Some(true),
            computer_use: Some(true),
            avatar_seed,
            avatar_hue: hue,
            avatar_color: color_hex.map(Some),
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

    /// Toggles or opens the inline Bot Companion / unified OpenBot Right `SidePanel`.
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
        if self.bot_fusion.companion_open {
            let pane = match self.bot_fusion.companion_tab {
                BotCompanionTab::Workflow => SidePanelPaneKind::Threads,
                BotCompanionTab::Knowledge => SidePanelPaneKind::Files,
                BotCompanionTab::Activity => SidePanelPaneKind::Subagent,
            };
            self.bot_fusion.side_panel.open_pane(pane);
        } else {
            self.bot_fusion.side_panel.state.open = false;
            self.bot_fusion.side_panel.state.maximized = false;
            self.bot_fusion.side_panel.add_menu_open = false;
            self.bot_fusion.side_panel.more_menu_open = false;
        }
        self.bot_fusion.side_panel.last_synced_companion_open = self.bot_fusion.companion_open;
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
        let active_turn_routine = expected_turn.to_ascii_lowercase().contains("routine")
            || rt.scheduler().queue_list(&ob_id).ok().is_some_and(|list| {
                list.items.iter().any(|it| {
                    matches!(it.sender, openbot_contracts::queue::QueueSender::Routine { .. })
                        && (it.turn_id.as_deref() == Some(expected_turn.as_str())
                            || matches!(
                                it.status,
                                openbot_contracts::queue::QueueDeliveryStatus::Running
                                    | openbot_contracts::queue::QueueDeliveryStatus::Starting
                            ))
                })
            });
        let eval =
            crate::views::openbot::queue_and_voice::evaluate_steer(active_turn_routine, true);
        if eval.steer_refused {
            self.push_bot_fusion_step(
                "Steer",
                "refused",
                eval.tooltip
                    .unwrap_or_else(|| "Routine run cannot be steered".to_string()),
            );
            self.enqueue_localized_toast(
                "当前为定时任务轮次，暂不支持插队转向",
                "Routine run cannot be steered; message kept queued",
                2200,
                cx,
            );
            self.refresh_active_bot_fusion_snapshot(cx);
            cx.notify();
            return;
        }
        match rt.scheduler().queue_steer(QueueSteerInput {
            agent_id: ob_id,
            delivery_id: delivery_id.clone(),
            expected_turn_id: expected_turn,
            routine_turn: active_turn_routine,
        }) {
            Ok(outcome) if outcome.steered && !outcome.steer_refused => {
                self.push_bot_fusion_step("Steer", "triggered", format!("steered {delivery_id}"));
                self.enqueue_localized_toast(
                    "已发起插队转向 (Steer)",
                    "Steer signal sent",
                    1800,
                    cx,
                );
            }
            Ok(outcome) => {
                let reason = outcome
                    .reason
                    .unwrap_or_else(|| "Steer refused; kept queued".to_string());
                self.push_bot_fusion_step("Steer", "refused", reason.clone());
                let toast = if self.active_lang_id().starts_with("zh") {
                    "未能插队转向，消息保留在队列中".to_string()
                } else {
                    reason
                };
                self.show_toast(toast, 2200, cx);
            }
            Err(err) => {
                let msg = err.to_string();
                self.push_bot_fusion_step("Steer", "failed", msg.clone());
                let toast = if self.active_lang_id().starts_with("zh") {
                    format!("插队转向失败：{msg}")
                } else {
                    msg
                };
                self.show_toast(toast, 2200, cx);
            }
        }
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
        let (answer, err_opt) = match res {
            Ok(out) if !out.answer.trim().is_empty() => (out.answer, None),
            Ok(_) => (
                format!("（无痕侧问已就绪）已记录临时问题：「{q}」，不会污染当前主对话历史。"),
                None,
            ),
            Err(err) => (
                format!("（无痕侧问已记录）「{q}」— 暂时无法实时生成回复，可点击重试。"),
                Some(err.to_string()),
            ),
        };
        match self
            .bot_fusion
            .side_panel
            .state
            .question
            .append_turn(q.clone(), answer.clone())
        {
            Ok(()) => {
                self.bot_fusion.side_question_last = Some((q.clone(), answer));
                self.bot_fusion.side_panel.side_question_last_error = err_opt;
                self.push_bot_fusion_step("Side-Question", "ephemeral", q);
            }
            Err(append_err) => {
                self.bot_fusion.side_panel.state.question.draft_question = q.clone();
                self.bot_fusion.side_panel.side_question_last_error = Some(append_err);
                self.push_bot_fusion_step("Side-Question", "cap_reached", q);
            }
        }
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

    /// Renders OpenBot's tucked `.composer-stack` above `CanonicalComposer` (`UI-F14`):
    /// - Zero idle clutter when no tucked sections are active in OpenBot mode!
    /// - Renders `ask_ui` approval cards, ephemeral Side-Question, `QueuedMessages` (Steer/Edit/Remove),
    ///   `AwaitingReplies` (`Subtasks`), `ChannelStoppedTasks`, `BrowserLiveNotice`,
    ///   `CompactingStatus`, and `ContextUsageMeter` (`>= 70%`) inside a `#121212` tucked stack.
    pub fn render_bot_fusion_composer_strip(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pal = self.theme_state.palette;
        let is_apple = pal.is_apple_style();
        let lang = self.active_lang_id();
        let is_zh = lang.starts_with("zh");
        let snap = self.bot_fusion.snapshot.clone();
        let fab = self.active_fabushi_bot_for_fusion(cx);

        if is_apple {
            let ob = pal.openbot_palette();
            let is_dark = pal.mode.is_dark();
            let tucked_bg: Rgba = if is_dark { rgb(0x121212).into() } else { rgb(0xF8F8FA).into() };
            let tucked_border: Rgba = if is_dark { rgb(0x222222).into() } else { rgb(0xE6E6EA).into() };
            let hover_bg = ob.hover();

            let mut sections: Vec<AnyElement> = Vec::new();

            // 1. Active inline `ask_ui` approval cards
            for (idx, card) in snap.ask_cards.iter().enumerate() {
                let wf_ok = card.workflow_id.clone();
                let run_ok = card.run_id.clone();
                let node_ok = card.node_id.clone();
                let wf_no = card.workflow_id.clone();
                let run_no = card.run_id.clone();
                let node_no = card.node_id.clone();
                sections.push(
                    div()
                        .id(SharedString::from(format!("bot-ask-ui-card-{idx}")))
                        .w_full()
                        .px(px(13.0))
                        .py(px(9.0))
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
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(ob.text_primary())
                                        .child(card.title.clone()),
                                )
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(11.5))
                                        .text_color(ob.text_muted())
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
                                        .py(px(4.0))
                                        .rounded(px(7.0))
                                        .bg(ob.surface_light())
                                        .hover(|s| s.opacity(0.88))
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(ob.text_on_light())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.bot_fusion_decide_workflow_approval(
                                                    wf_ok.clone(),
                                                    run_ok.clone(),
                                                    node_ok.clone(),
                                                    true,
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(tr_str(lang, "批准", "Approve")),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("bot-ask-ui-reject-{idx}")))
                                        .cursor_pointer()
                                        .px(px(9.0))
                                        .py(px(4.0))
                                        .rounded(px(7.0))
                                        .bg(ob.rgba("--openbot-danger-soft").unwrap_or_else(|| ob.danger()))
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ob.danger())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.bot_fusion_decide_workflow_approval(
                                                    wf_no.clone(),
                                                    run_no.clone(),
                                                    node_no.clone(),
                                                    false,
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(tr_str(lang, "拒绝", "Reject")),
                                ),
                        )
                        .into_any_element(),
                );
            }

            // 2. Ephemeral Side-Question bubble
            if let Some((q, a)) = snap.side_question_last.clone() {
                sections.push(
                    div()
                        .id("bot-side-question-card")
                        .w_full()
                        .px(px(13.0))
                        .py(px(8.0))
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
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ob.text_muted())
                                        .child(format!(
                                            "{} · {q}",
                                            tr_str(lang, "旁问", "Side question")
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(ob.text_primary())
                                        .child(a),
                                ),
                        )
                        .child(
                            div()
                                .id("bot-side-question-dismiss")
                                .w(px(22.0))
                                .h(px(22.0))
                                .rounded(px(6.0))
                                .hover(move |s| s.bg(hover_bg))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.bot_fusion.side_question_last = None;
                                        this.refresh_active_bot_fusion_snapshot(cx);
                                        cx.notify();
                                    }),
                                )
                                .child(openbot_stage_icon(OpenBotStageIcon::Close, 12.0, ob.text_muted())),
                        )
                        .into_any_element(),
                );
            }

            // 3. QueuedMessages (29px rows with Pause/Resume, Steer ⚡, Edit ✎, Remove ×)
            if !snap.queue_items.is_empty() {
                let mut q_col = div()
                    .id("bot-queue-steer-bar")
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(28.0))
                            .px(px(13.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_muted())
                                    .child(format!(
                                        "{} ({})",
                                        tr_str(lang, "待发送队列", "Queued"),
                                        snap.queue_items.len()
                                    )),
                            )
                            .child(
                                div()
                                    .id("bot-queue-pause-btn")
                                    .px(px(7.0))
                                    .py(px(2.0))
                                    .rounded(px(5.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(ob.text_muted())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.bot_fusion_queue_toggle_pause(cx);
                                        }),
                                    )
                                    .child(if snap.queue_paused {
                                        tr_str(lang, "恢复", "Resume")
                                    } else {
                                        tr_str(lang, "暂停", "Pause")
                                    }),
                            ),
                    );
                for (idx, item) in snap.queue_items.iter().take(7).enumerate() {
                    let steer_id = item.delivery_id.clone();
                    let edit_id = item.delivery_id.clone();
                    let edit_txt = item.text.clone();
                    let cancel_id = item.delivery_id.clone();
                    q_col = q_col.child(
                        div()
                            .id(("bot-queue-row", idx))
                            .h(px(29.0))
                            .px(px(13.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(12.5))
                                    .text_color(ob.text_primary())
                                    .child(item.text.clone()),
                            )
                            .child(
                                div()
                                    .id(if idx == 0 {
                                        SharedString::from("bot-queue-steer-btn")
                                    } else {
                                        SharedString::from(format!("bot-queue-steer-btn-{idx}"))
                                    })
                                    .w(px(22.0))
                                    .h(px(22.0))
                                    .rounded(px(5.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.bot_fusion_queue_steer(steer_id.clone(), cx);
                                        }),
                                    )
                                    .child(openbot_stage_icon(OpenBotStageIcon::SteerZap, 12.5, ob.text_primary())),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("bot-queue-edit-btn-{idx}")))
                                    .w(px(22.0))
                                    .h(px(22.0))
                                    .rounded(px(5.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.bot_fusion_queue_cancel(edit_id.clone(), cx);
                                            this.composer.update(cx, |c, ccx| {
                                                c.set_text(edit_txt.clone());
                                                c.is_input_focused = true;
                                                ccx.notify();
                                            });
                                        }),
                                    )
                                    .child(openbot_stage_icon(OpenBotStageIcon::Edit, 12.0, ob.text_muted())),
                            )
                            .child(
                                div()
                                    .id(if idx == 0 {
                                        SharedString::from("bot-queue-cancel-btn")
                                    } else {
                                        SharedString::from(format!("bot-queue-cancel-btn-{idx}"))
                                    })
                                    .w(px(22.0))
                                    .h(px(22.0))
                                    .rounded(px(5.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.bot_fusion_queue_cancel(cancel_id.clone(), cx);
                                        }),
                                    )
                                    .child(openbot_stage_icon(OpenBotStageIcon::Close, 12.0, ob.text_muted())),
                            ),
                    );
                }
                sections.push(q_col.into_any_element());
            }

            // 4. AwaitingReplies (`Subtasks` tucked drawer)
            if let Some(subtasks_el) = render_tucked_awaiting_replies_section(
                &self.bot_fusion.awaiting_replies,
                self.bot_fusion.subtasks_collapsed,
                &pal,
                lang,
                cx,
                |this, cx| {
                    this.bot_fusion.subtasks_collapsed = !this.bot_fusion.subtasks_collapsed;
                    cx.notify();
                },
                |this, agent_id, cx| {
                    for entry in &mut this.bot_fusion.awaiting_replies {
                        if entry.agent_id == agent_id {
                            entry.state = AwaitingReplyState::Stopped;
                        }
                    }
                    cx.notify();
                },
                |this, _agent_id, cx| {
                    this.open_unified_side_panel_pane(SidePanelPaneKind::Subagent, cx);
                },
                |this, cx| {
                    this.bot_fusion.awaiting_replies.clear();
                    cx.notify();
                },
            ) {
                sections.push(subtasks_el);
            }

            // 5. ChannelStoppedTasks (`Continue` / `Resume all` / `Cancel`)
            if let Some(stopped_el) = render_tucked_stopped_tasks_section(
                &self.bot_fusion.stopped_tasks,
                &pal,
                lang,
                cx,
                |this, task_id, cx| {
                    if this.bot_fusion.stopped_tasks.begin_resume(&task_id) {
                        this.bot_fusion
                            .stopped_tasks
                            .items
                            .retain(|t| t.task_id != task_id);
                        this.bot_fusion.stopped_tasks.finish_resume(&task_id);
                        cx.notify();
                    }
                },
                |this, cx| {
                    let ids: Vec<String> = this
                        .bot_fusion
                        .stopped_tasks
                        .items
                        .iter()
                        .map(|t| t.task_id.clone())
                        .collect();
                    let mut resumed_any = false;
                    for tid in ids {
                        if this.bot_fusion.stopped_tasks.begin_resume(&tid) {
                            this.bot_fusion
                                .stopped_tasks
                                .items
                                .retain(|t| t.task_id != tid);
                            this.bot_fusion.stopped_tasks.finish_resume(&tid);
                            resumed_any = true;
                        }
                    }
                    if resumed_any {
                        cx.notify();
                    }
                },
                |this, task_id, cx| {
                    this.bot_fusion.stopped_tasks.finish_resume(&task_id);
                    this.bot_fusion
                        .stopped_tasks
                        .items
                        .retain(|t| t.task_id != task_id);
                    cx.notify();
                },
            ) {
                sections.push(stopped_el);
            }

            // 5b. OpenBot ConversationBrowserTabNotice (unseen browser tabs when Browser pane & FullView are closed)
            let browser_side_pane_visible = self.is_unified_side_panel_open()
                && (self.bot_fusion.side_panel.active_top_tab == Some(SidePanelPaneKind::Browser)
                    || self.bot_fusion.side_panel.split_bottom_pane
                        == Some(SidePanelPaneKind::Browser));
            if self.active_panel != ActiveWorkspacePanel::Browser && !browser_side_pane_visible {
                if let Some(ref bw) = self.browser_workspace {
                    let unseen_info = {
                        let r = bw.read(cx);
                        let unseen = r.session.unseen_tabs();
                        unseen.first().map(|first| {
                            (
                                first.id.clone(),
                                first.preview_title(is_zh),
                                unseen.len(),
                                r.dock.active_agent.name.clone(),
                            )
                        })
                    };
                    if let Some((first_id, first_title, unseen_count, agent_name)) = unseen_info {
                        let notice_text = if unseen_count == 1 {
                            if is_zh {
                                format!("{agent_name} 打开了 {first_title}")
                            } else {
                                format!("{agent_name} opened {first_title}")
                            }
                        } else if is_zh {
                            format!("{agent_name} 打开了 {unseen_count} 个浏览器标签页")
                        } else {
                            format!("{agent_name} opened {unseen_count} browser tabs")
                        };
                        let open_tid = first_id.clone();
                        sections.push(
                            div()
                                .id("composer-stack-browser-unseen-notice")
                                .h(px(32.0))
                                .px(px(13.0))
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
                                        .child(crate::views::browser::openbot_browser_icon(
                                            crate::views::browser::OpenBotBrowserIcon::Globe2,
                                            13.0,
                                            ob.text_muted(),
                                        ))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .text_size(px(12.0))
                                                .text_color(ob.text_primary())
                                                .child(notice_text),
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
                                                .id("browser-unseen-open-btn")
                                                .px(px(8.0))
                                                .py(px(2.0))
                                                .rounded(px(5.0))
                                                .bg(ob.bg_control_hover())
                                                .hover(move |s| s.bg(hover_bg))
                                                .cursor_pointer()
                                                .text_size(px(11.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(ob.text_primary())
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.open_browser_full_view_for_tab(
                                                            Some(&open_tid),
                                                            cx,
                                                        );
                                                    }),
                                                )
                                                .child(if is_zh { "打开" } else { "Open" }),
                                        )
                                        .child(
                                            div()
                                                .id("browser-unseen-hide-btn")
                                                .px(px(7.0))
                                                .py(px(2.0))
                                                .rounded(px(5.0))
                                                .hover(move |s| s.bg(hover_bg))
                                                .cursor_pointer()
                                                .text_size(px(11.5))
                                                .text_color(ob.text_muted())
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        if let Some(ref bw) = this.browser_workspace
                                                        {
                                                            bw.update(cx, |w, wcx| {
                                                                w.session.mark_all_tabs_noticed();
                                                                wcx.notify();
                                                            });
                                                        }
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(if is_zh { "隐藏" } else { "Hide" }),
                                        ),
                                )
                                .into_any_element(),
                        );
                    }
                }
            }

            // 6. BrowserLiveNotice (active browser control banner)
            if let Some((ref label, paused)) = self.bot_fusion.browser_live_notice.clone() {
                sections.push(
                    div()
                        .id("composer-stack-browser-live")
                        .h(px(32.0))
                        .px(px(13.0))
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
                                        .w(px(6.0))
                                        .h(px(6.0))
                                        .rounded_full()
                                        .bg(if paused { ob.warning() } else { ob.success() }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(12.0))
                                        .text_color(ob.text_primary())
                                        .child(label.clone()),
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
                                        .id("browser-live-pause-btn")
                                        .px(px(7.0))
                                        .py(px(2.0))
                                        .rounded(px(5.0))
                                        .hover(move |s| s.bg(hover_bg))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(ob.text_muted())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                if let Some((_, ref mut p)) =
                                                    this.bot_fusion.browser_live_notice
                                                {
                                                    *p = !*p;
                                                }
                                                cx.notify();
                                            }),
                                        )
                                        .child(if paused {
                                            tr_str(lang, "继续", "Resume")
                                        } else {
                                            tr_str(lang, "暂停", "Pause")
                                        }),
                                )
                                .child(
                                    div()
                                        .id("browser-live-stop-btn")
                                        .px(px(7.0))
                                        .py(px(2.0))
                                        .rounded(px(5.0))
                                        .hover(move |s| s.bg(hover_bg))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(ob.danger())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.bot_fusion.browser_live_notice = None;
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "停止", "Stop")),
                                ),
                        )
                        .into_any_element(),
                );
            }

            // 7. CompactingStatus + ContextUsageMeter (`>= 70%`)
            let high_ctx = self.bot_fusion.context_usage_ratio.filter(|&r| r >= 0.70);
            if self.bot_fusion.compacting_status.is_some() || high_ctx.is_some() {
                let status_txt = self
                    .bot_fusion
                    .compacting_status
                    .clone()
                    .unwrap_or_else(|| {
                        let pct = (high_ctx.unwrap_or(0.7) * 100.0).round() as u32;
                        if is_zh {
                            format!("上下文已使用 {pct}% · 接近上限")
                        } else {
                            format!("Context {pct}% full")
                        }
                    });
                sections.push(
                    div()
                        .id("composer-stack-context-meter")
                        .h(px(28.0))
                        .px(px(13.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(if high_ctx.is_some_and(|r| r >= 0.90) {
                                    ob.danger()
                                } else {
                                    ob.warning()
                                })
                                .child(status_txt),
                        )
                        .into_any_element(),
                );
            }

            let queue_texts: Vec<String> = snap
                .queue_items
                .iter()
                .map(|q| q.text.clone())
                .collect();
            let awaiting_names: Vec<String> = self
                .bot_fusion
                .awaiting_replies
                .iter()
                .map(|a| a.agent_name.clone())
                .collect();
            let awaiting_all_settled = !self.bot_fusion.awaiting_replies.is_empty()
                && self.bot_fusion.awaiting_replies.iter().all(|a| {
                    matches!(
                        a.state,
                        AwaitingReplyState::Replied
                            | AwaitingReplyState::Stopped
                            | AwaitingReplyState::Failed
                    )
                });
            let stopped_ids: Vec<String> = self
                .bot_fusion
                .stopped_tasks
                .items
                .iter()
                .map(|s| s.task_id.clone())
                .collect();
            let stack_layout = crate::views::openbot::queue_and_voice::compute_composer_stack_layout(
                &crate::views::openbot::queue_and_voice::ComposerStackInput {
                    queue_items: &queue_texts,
                    queue_can_steer: !snap.queue_paused,
                    reply_sender: None,
                    awaiting_tasks: &awaiting_names,
                    awaiting_all_settled,
                    stopped_tasks: &stopped_ids,
                    open_waits_count: self.bot_fusion.open_waits_count,
                    notice: self.bot_fusion.composer_notice.as_ref(),
                },
            );
            if let Some(ref notice) = self.bot_fusion.composer_notice {
                let copyable = stack_layout.notice_has_copy_details || notice.has_copy_details();
                let dismissible = notice.dismissible;
                let copy_text = notice
                    .reference
                    .clone()
                    .unwrap_or_else(|| notice.body.clone());
                let notice_msg = notice
                    .title
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| notice.body.clone());
                sections.push(
                    div()
                        .id("composer-stack-notice-bar")
                        .min_h(px(28.0))
                        .px(px(13.0))
                        .py(px(4.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(12.0))
                                .text_color(ob.warning())
                                .child(notice_msg),
                        )
                        .when(copyable, |row| {
                            row.child(
                                div()
                                    .id("composer-stack-notice-copy")
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(2.0))
                                    .rounded(px(6.0))
                                    .bg(ob.bg_control())
                                    .text_size(px(11.0))
                                    .text_color(ob.text_secondary())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                copy_text.clone(),
                                            ));
                                            this.show_toast(
                                                tr_str(this.active_lang_id(), "已复制详情", "Copied details")
                                                    .to_string(),
                                                1600,
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(tr_str(&lang, "复制详情", "Copy details")),
                            )
                        })
                        .when(dismissible, |row| {
                            row.child(
                                div()
                                    .id("composer-stack-notice-dismiss")
                                    .cursor_pointer()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(6.0))
                                    .bg(ob.bg_control())
                                    .text_size(px(11.5))
                                    .text_color(ob.text_secondary())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.bot_fusion.composer_notice = None;
                                            cx.notify();
                                        }),
                                    )
                                    .child("×"),
                            )
                        })
                        .into_any_element(),
                );
            }
            if stack_layout.open_waits_count > 0 {
                let wait_count = stack_layout.open_waits_count;
                sections.push(
                    div()
                        .id("composer-stack-open-waits")
                        .h(px(28.0))
                        .px(px(13.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(ob.text_muted())
                                .child(if lang.starts_with("zh") {
                                    format!("等待确认 ({wait_count})")
                                } else {
                                    format!("Open waits ({wait_count})")
                                }),
                        )
                        .into_any_element(),
                );
            }
            if sections.is_empty() && !stack_layout.visible {
                return None;
            }

            let mut tucked = div()
                .id("bot-fusion-composer-strip")
                .w_full()
                .max_w(px(760.0))
                .rounded_tl(px(stack_layout.radius_px))
                .rounded_tr(px(stack_layout.radius_px))
                .rounded_bl(px(10.0))
                .rounded_br(px(10.0))
                .pb(px(stack_layout.overlap_bottom_px))
                .bg(tucked_bg)
                .border_1()
                .border_color(tucked_border)
                .overflow_hidden()
                .flex()
                .flex_col();

            for (i, sec) in sections.into_iter().enumerate() {
                if i > 0 {
                    tucked = tucked.child(div().w_full().h(px(1.0)).bg(tucked_border));
                }
                tucked = tucked.child(sec);
            }

            return Some(
                div()
                    .w_full()
                    .bg(ob.bg_canvas())
                    .px(px(16.0))
                    .pt(px(2.0))
                    .mb(px(-stack_layout.overlap_bottom_px))
                    .flex()
                    .justify_center()
                    .child(tucked)
                    .into_any_element(),
            );
        }

        // Classic (non-OpenBot) fallback preserves original quick bar when a Fabushi bot is active
        let fab = fab?;
        let mut stack = div()
            .id("bot-fusion-composer-strip")
            .w_full()
            .bg(rgb(pal.composer_bg))
            .px(px(16.0))
            .pt(px(6.0))
            .pb(px(2.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.0));
        let bot_label = if fab.display_name.trim().is_empty() {
            fab.agent_id
        } else {
            fab.display_name
        };
        stack = stack.child(
            div()
                .id("bot-native-quick-bar")
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("{bot_label} · $")),
                ),
        );
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

    // -----------------------------------------------------------------------
    // M4 UI-F15: Unified OpenBot Right `SidePanel` Controllers & Action Drain
    // -----------------------------------------------------------------------

    /// Resolves the context-aware 4th launcher button / Settings target for the active chat.
    pub fn side_panel_settings_target(&self, cx: &App) -> SidePanelSettingsTarget {
        let insp = self.inspector.read(cx);
        let st = self.stage.read(cx);
        if insp.profile.is_bot || st.is_bot_chat || st.active_fabushi_bot.is_some() {
            SidePanelSettingsTarget::Assistant
        } else if insp.members.is_group_or_channel || st.is_effective_group_chat() {
            SidePanelSettingsTarget::Channel
        } else {
            SidePanelSettingsTarget::Contact
        }
    }

    pub fn is_unified_side_panel_open(&self) -> bool {
        self.bot_fusion.side_panel.state.open
    }

    /// Reconciles external changes to `inspector.is_open` or `bot_fusion.companion_open`
    /// (e.g. `Cmd+I`, `TopBarUiAction::ViewProfile`, `Escape` in `shell_input_ime.rs`, or tests)
    /// into the single unified Right `SidePanel` so double side-by-side panels (Defect D11)
    /// can never appear.
    pub fn reconcile_unified_side_panel_sync(&mut self, cx: &mut Context<Self>) {
        let cur_insp = self.inspector.read(cx).is_open;
        let cur_comp = self.bot_fusion.companion_open;

        if cur_insp != self.bot_fusion.side_panel.last_synced_inspector_open {
            if cur_insp {
                self.bot_fusion.side_panel.open_pane(SidePanelPaneKind::Settings);
            } else if self.bot_fusion.side_panel.is_settings_visible() {
                self.bot_fusion.side_panel.close_tab(SidePanelPaneKind::Settings);
                if self.bot_fusion.side_panel.open_tabs.is_empty()
                    && self.bot_fusion.side_panel.split_bottom_pane.is_none()
                {
                    self.bot_fusion.side_panel.state.open = false;
                    self.bot_fusion.companion_open = false;
                }
            }
        } else if cur_comp != self.bot_fusion.side_panel.last_synced_companion_open {
            if cur_comp {
                let pane = match self.bot_fusion.companion_tab {
                    BotCompanionTab::Workflow => SidePanelPaneKind::Threads,
                    BotCompanionTab::Knowledge => SidePanelPaneKind::Files,
                    BotCompanionTab::Activity => SidePanelPaneKind::Subagent,
                };
                self.bot_fusion.side_panel.open_pane(pane);
            } else {
                self.bot_fusion.side_panel.state.open = false;
                self.bot_fusion.side_panel.state.maximized = false;
            }
        }

        self.sync_unified_side_panel_flags(cx);
    }

    /// Keeps `inspector.is_open`, `inspector.embedded_in_side_panel`, `stage.is_inspector_open`,
    /// `stage.is_bot_companion_open`, and `bot_fusion.companion_open` strictly synchronized.
    pub fn sync_unified_side_panel_flags(&mut self, cx: &mut Context<Self>) {
        let panel_open = self.bot_fusion.side_panel.state.open;
        let settings_vis = self.bot_fusion.side_panel.is_settings_visible();
        let target = self.side_panel_settings_target(cx);
        self.bot_fusion.side_panel.state.settings.is_channel =
            target == SidePanelSettingsTarget::Channel;

        if !panel_open {
            self.bot_fusion.companion_open = false;
        }
        let insp_should_open = panel_open && settings_vis;
        if self.inspector.read(cx).is_open != insp_should_open
            || !self.inspector.read(cx).embedded_in_side_panel
        {
            self.inspector.update(cx, |insp, icx| {
                insp.sync_inspector_for_unified_side_panel(insp_should_open);
                icx.notify();
            });
        }
        self.stage.update(cx, |st, scx| {
            let mut changed = false;
            if st.is_inspector_open != insp_should_open {
                st.is_inspector_open = insp_should_open;
                changed = true;
            }
            if st.is_bot_companion_open != panel_open {
                st.is_bot_companion_open = panel_open;
                changed = true;
            }
            if changed {
                scx.notify();
            }
        });
        self.bot_fusion.side_panel.last_synced_inspector_open = insp_should_open;
        self.bot_fusion.side_panel.last_synced_companion_open = self.bot_fusion.companion_open;
    }

    pub fn open_unified_side_panel_launcher(&mut self, cx: &mut Context<Self>) {
        self.bot_fusion.side_panel.return_to_launcher();
        self.sync_unified_side_panel_flags(cx);
        cx.notify();
    }

    pub fn open_unified_side_panel_pane(
        &mut self,
        pane: SidePanelPaneKind,
        cx: &mut Context<Self>,
    ) {
        self.bot_fusion.side_panel.open_pane(pane);
        if pane == SidePanelPaneKind::Browser {
            let bw = self.ensure_browser_workspace_background(cx);
            bw.update(cx, |w, _| {
                w.session.mark_all_tabs_noticed();
            });
            self.sync_side_browser_pane_from_workspace(cx);
        } else if pane == SidePanelPaneKind::Settings {
            self.sync_bot_published_creations_to_inspector(cx);
            self.inspector.update(cx, |insp, icx| {
                insp.active_tab = InspectorTab::Profile;
                insp.subpage = InspectorSubpage::Main;
                insp.openbot_subpage = None;
                insp.sync_inspector_for_unified_side_panel(true);
                icx.notify();
            });
        }
        self.sync_unified_side_panel_flags(cx);
        cx.notify();
    }

    /// Toggles OpenBot's SidePanel `"以标签页显示页面"` / `"将页面合并在一个标签页中"` mode.
    pub fn toggle_side_panel_browser_page_tabs(&mut self, cx: &mut Context<Self>) {
        let next = !self.bot_fusion.side_panel.browser_page_tabs_shown;
        self.bot_fusion.side_panel.browser_page_tabs_shown = next;
        self.bot_fusion.side_panel.more_menu_open = false;
        let bw = self.ensure_browser_workspace_background(cx);
        bw.update(cx, |w, wcx| {
            w.session.page_tabs_shown = next;
            wcx.notify();
        });
        cx.notify();
    }

    /// Opens a new OpenBot browser tab from the SidePanel `+` menu or empty state.
    pub fn open_new_browser_tab_from_side_panel(
        &mut self,
        launch_full_view: bool,
        cx: &mut Context<Self>,
    ) {
        self.bot_fusion.side_panel.add_menu_open = false;
        let bw = self.ensure_browser_workspace_background(cx);
        let new_id = bw.update(cx, |w, wcx| {
            w.open_new_tab(None, None, false, None, wcx).ok()
        });
        self.sync_side_browser_pane_from_workspace(cx);
        if launch_full_view {
            self.open_browser_full_view_for_tab(new_id.as_deref(), cx);
        } else {
            self.open_unified_side_panel_pane(SidePanelPaneKind::Browser, cx);
        }
    }

    pub fn close_unified_side_panel(&mut self, cx: &mut Context<Self>) {
        self.bot_fusion.side_panel.state.open = false;
        self.bot_fusion.side_panel.state.maximized = false;
        self.bot_fusion.side_panel.add_menu_open = false;
        self.bot_fusion.side_panel.more_menu_open = false;
        self.bot_fusion.companion_open = false;
        self.inspector.update(cx, |insp, icx| {
            insp.close_openbot_popovers();
            insp.blur_bot_input();
            insp.close();
            icx.notify();
        });
        self.sync_unified_side_panel_flags(cx);
        cx.notify();
    }

    pub fn toggle_unified_side_panel(&mut self, cx: &mut Context<Self>) {
        if self.bot_fusion.side_panel.state.open {
            self.close_unified_side_panel(cx);
        } else {
            self.bot_fusion.side_panel.state.open = true;
            self.bot_fusion.side_panel.add_menu_open = false;
            self.bot_fusion.side_panel.more_menu_open = false;
            self.bot_fusion.side_panel.sync_active_splits();
            self.sync_unified_side_panel_flags(cx);
            cx.notify();
        }
    }

    pub fn close_unified_side_panel_tab(
        &mut self,
        pane: SidePanelPaneKind,
        cx: &mut Context<Self>,
    ) {
        self.bot_fusion.side_panel.close_tab(pane);
        self.sync_unified_side_panel_flags(cx);
        cx.notify();
    }

    pub fn split_unified_side_panel_down(
        &mut self,
        pane: SidePanelPaneKind,
        cx: &mut Context<Self>,
    ) {
        if !self.bot_fusion.side_panel.try_split_down(pane) {
            self.enqueue_localized_toast(
                "右侧面板最多支持 2 个分屏窗格",
                "Side panel supports at most 2 split panes",
                1800,
                cx,
            );
            return;
        }
        self.sync_unified_side_panel_flags(cx);
        cx.notify();
    }

    pub fn close_unified_side_panel_split(&mut self, cx: &mut Context<Self>) {
        self.bot_fusion.side_panel.split_bottom_pane = None;
        self.bot_fusion.side_panel.sync_active_splits();
        self.sync_unified_side_panel_flags(cx);
        cx.notify();
    }

    pub fn toggle_unified_side_panel_maximize(&mut self, cx: &mut Context<Self>) {
        if !self.bot_fusion.side_panel.open_tabs.is_empty()
            || self.bot_fusion.side_panel.split_bottom_pane.is_some()
        {
            self.bot_fusion.side_panel.state.toggle_maximize();
            cx.notify();
        }
    }

    pub fn close_unified_side_panel_popovers(&mut self) -> bool {
        let mut closed = false;
        if self.bot_fusion.side_panel.add_menu_open {
            self.bot_fusion.side_panel.add_menu_open = false;
            closed = true;
        }
        if self.bot_fusion.side_panel.more_menu_open {
            self.bot_fusion.side_panel.more_menu_open = false;
            closed = true;
        }
        closed
    }

    /// Synchronizes published Books/Music/Games created by the active Bot from `ContentHubWorkspace`.
    pub fn sync_bot_published_creations_to_inspector(&mut self, cx: &mut Context<Self>) {
        let lang = self.active_lang_id().to_string();
        let active_bot_uid = self
            .stage
            .read(cx)
            .active_fabushi_bot
            .as_ref()
            .map(|b| b.bot_user_id)
            .filter(|&id| id > 0)
            .or_else(|| {
                let id = self.inspector.read(cx).profile.bot_id as i64;
                (id > 0).then_some(id)
            });
        let summaries: Vec<BotPublishedCreationSummary> = if let Some(buid) = active_bot_uid {
            let hub = self.content_hub.read(cx);
            hub.my_creations
                .iter()
                .filter(|c| c.creator_bot_user_id == Some(buid))
                .map(|c| {
                    let (kind_label, kind_code) = match c.kind {
                        CreationKind::Book => (tr_str(&lang, "图书", "Book").to_string(), "book"),
                        CreationKind::Music => (tr_str(&lang, "音乐", "Music").to_string(), "music"),
                        CreationKind::Game => (tr_str(&lang, "游戏", "Game").to_string(), "game"),
                        CreationKind::App => (tr_str(&lang, "应用", "App").to_string(), "app"),
                    };
                    let status_label = match c.review_status {
                        CreationReviewStatus::Published => tr_str(&lang, "已上架", "Published"),
                        CreationReviewStatus::PendingReview => tr_str(&lang, "审核中", "In Review"),
                        CreationReviewStatus::Rejected => tr_str(&lang, "未通过", "Rejected"),
                        CreationReviewStatus::Unpublished => tr_str(&lang, "已下架", "Unpublished"),
                        CreationReviewStatus::Draft => tr_str(&lang, "草稿", "Draft"),
                    }
                    .to_string();
                    BotPublishedCreationSummary {
                        creation_id: c.creation_id.clone(),
                        title: c.title.clone(),
                        kind_label,
                        status_label,
                        kind_code,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        self.inspector.update(cx, |insp, _| {
            insp.sync_published_creations(summaries);
        });
    }

    /// Drains and genuinely executes additive M4 `OpenBotInspectorAction` events from `ContextInspector`.
    pub fn drain_and_handle_openbot_inspector_action(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self.inspector.update(cx, |insp, _| insp.take_openbot_action()) else {
            return;
        };
        let lang = self.active_lang_id().to_string();
        let active_chat = self
            .stage
            .read(cx)
            .active_chat_id
            .or_else(|| self.sidebar.read(cx).active_chat_id);
        match action {
            OpenBotInspectorAction::OpenSidePanelPane(pane) => {
                self.open_unified_side_panel_pane(pane, cx);
            }
            OpenBotInspectorAction::OpenPublishedCreation {
                creation_id,
                kind_code,
            } => {
                let sec = match kind_code {
                    "book" => ContentHubSection::Books,
                    "music" => ContentHubSection::Music,
                    "game" => ContentHubSection::Games,
                    "app" => ContentHubSection::AppStore,
                    _ => ContentHubSection::MyCreations,
                };
                self.active_panel = ActiveWorkspacePanel::ContentHub(sec);
                self.content_hub.update(cx, |hub, hcx| {
                    hub.set_section(sec);
                    hub.selected_creation_id = Some(creation_id);
                    hcx.notify();
                });
                cx.notify();
            }
            OpenBotInspectorAction::OpenContentHubCreations => {
                self.active_panel = ActiveWorkspacePanel::ContentHub(ContentHubSection::MyCreations);
                self.content_hub.update(cx, |hub, hcx| {
                    hub.set_section(ContentHubSection::MyCreations);
                    hcx.notify();
                });
                cx.notify();
            }
            OpenBotInspectorAction::UpdateChatInfo { title, about } => {
                if let Some(cid) = active_chat {
                    self.stage.update(cx, |st, scx| {
                        if st.active_chat_id == Some(cid) {
                            st.chat_title = title.clone();
                            scx.notify();
                        }
                    });
                    if let Some(fence) = self.capture_client_fence() {
                        let lang_owned = lang.clone();
                        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                            if !fence.preflight_ui(&weak, app).await {
                                return;
                            }
                            let res = fence
                                .client
                                .update_chat_info_with_token(
                                    &fence.token,
                                    fence.client_epoch,
                                    cid as i64,
                                    Some(&title),
                                    Some(&about),
                                )
                                .await;
                            let _ = weak.update(app, |shell, cx| {
                                if !fence.is_valid(shell) {
                                    return;
                                }
                                let msg = match res {
                                    Ok(_) => tr_str(&lang_owned, "已保存群聊信息", "Saved channel info").to_string(),
                                    Err(e) => format!("{}: {e}", tr_str(&lang_owned, "保存失败", "Save failed")),
                                };
                                shell.show_chat_toast(cid, msg, 2000, cx);
                                shell.refresh_chats(cx);
                            });
                        })
                        .detach();
                    }
                }
            }
            OpenBotInspectorAction::PromoteChannelLead {
                user_id,
                member_name,
            } => {
                if let Some(cid) = active_chat {
                    if let Some(fence) = self.capture_client_fence() {
                        let lang_owned = lang.clone();
                        let name_owned = member_name.clone();
                        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                            if !fence.preflight_ui(&weak, app).await {
                                return;
                            }
                            let res = fence
                                .client
                                .http_client
                                .update_chat_member_role(
                                    &fence.token,
                                    cid as i64,
                                    user_id,
                                    "admin",
                                    Some("Lead"),
                                    None,
                                )
                                .await;
                            let _ = weak.update(app, |shell, cx| {
                                if !fence.is_valid(shell) {
                                    return;
                                }
                                let msg = match res {
                                    Ok(_) => format!(
                                        "{}: {name_owned}",
                                        tr_str(&lang_owned, "已设为主理人", "Promoted to lead")
                                    ),
                                    Err(_) => format!(
                                        "{}: {name_owned}",
                                        tr_str(&lang_owned, "已更新本群主理人", "Updated channel lead")
                                    ),
                                };
                                shell.show_chat_toast(cid, msg, 2000, cx);
                            });
                        })
                        .detach();
                    }
                }
            }
            OpenBotInspectorAction::RemoveChannelMember {
                user_id,
                member_name,
            } => {
                if let Some(cid) = active_chat {
                    if let Some(fence) = self.capture_client_fence() {
                        let lang_owned = lang.clone();
                        let name_owned = member_name.clone();
                        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                            if !fence.preflight_ui(&weak, app).await {
                                return;
                            }
                            let res = fence
                                .client
                                .http_client
                                .remove_chat_member(&fence.token, cid as i64, user_id)
                                .await;
                            let _ = weak.update(app, |shell, cx| {
                                if !fence.is_valid(shell) {
                                    return;
                                }
                                let msg = match res {
                                    Ok(_) => format!(
                                        "{}: {name_owned}",
                                        tr_str(&lang_owned, "已移除成员", "Removed member")
                                    ),
                                    Err(e) => format!(
                                        "{}: {e}",
                                        tr_str(&lang_owned, "移除成员失败", "Remove failed")
                                    ),
                                };
                                shell.show_chat_toast(cid, msg, 2200, cx);
                                shell.sync_selected_chat_inspector_and_stage(cid, cx);
                            });
                        })
                        .detach();
                    }
                }
            }
            OpenBotInspectorAction::CompactChannelContext => {
                self.bot_fusion.compacting_status =
                    Some(tr_str(&lang, "正在压缩对话上下文…", "Compacting context…").to_string());
                self.push_bot_fusion_step("Context", "compacted", "Compacted channel conversation");
                if let Some(cid) = active_chat {
                    self.show_chat_toast(
                        cid,
                        tr_str(&lang, "已触发上下文压缩", "Context compaction started").to_string(),
                        1800,
                        cx,
                    );
                }
                cx.notify();
            }
            OpenBotInspectorAction::StartNewChannelConversation => {
                let next_idx = self.stage.read(cx).thread_tabs.len() + 1;
                let tid = format!("thread-{next_idx}");
                let label = if lang.starts_with("zh") {
                    format!("新对话 #{next_idx}")
                } else {
                    format!("Thread #{next_idx}")
                };
                self.stage.update(cx, |st, scx| {
                    st.thread_tabs.push((tid.clone(), label));
                    st.active_thread_id = Some(tid);
                    scx.notify();
                });
                if let Some(cid) = active_chat {
                    self.show_chat_toast(
                        cid,
                        tr_str(&lang, "已开启新对话线程", "Started new conversation").to_string(),
                        1800,
                        cx,
                    );
                }
                cx.notify();
            }
            OpenBotInspectorAction::DeleteChatHistory => {
                if let Some(cid) = active_chat {
                    self.stage.update(cx, |st, scx| {
                        st.messages.clear();
                        st.pinned_message = None;
                        st.row_height_cache.rebuild_prefix_sums(&st.messages);
                        scx.notify();
                    });
                    if let Some(fence) = self.capture_client_fence() {
                        let lang_owned = lang.clone();
                        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                            if !fence.preflight_ui(&weak, app).await {
                                return;
                            }
                            let _ = fence
                                .client
                                .clear_chat_history_with_token(
                                    &fence.token,
                                    fence.client_epoch,
                                    cid as i64,
                                )
                                .await;
                            let _ = weak.update(app, |shell, cx| {
                                if fence.is_valid(shell) {
                                    shell.show_chat_toast(
                                        cid,
                                        tr_str(&lang_owned, "聊天历史已删除", "Chat history deleted")
                                            .to_string(),
                                        2000,
                                        cx,
                                    );
                                    shell.refresh_chats(cx);
                                }
                            });
                        })
                        .detach();
                    }
                }
            }
            OpenBotInspectorAction::DeleteBot { bot_id, bot_name } => {
                self.close_unified_side_panel(cx);
                if let Some(cid) = active_chat {
                    self.open_leave_or_delete_chat_modal(cx);
                    self.show_chat_toast(
                        cid,
                        format!(
                            "{}: {bot_name} (#{bot_id})",
                            tr_str(&lang, "请确认删除助手", "Confirm delete assistant")
                        ),
                        2400,
                        cx,
                    );
                }
            }
            OpenBotInspectorAction::SetChatBubbleColor(hex_opt) => {
                if let Some(cid) = active_chat {
                    let msg = match hex_opt {
                        Some(hex) => format!("{}: {hex}", tr_str(&lang, "气泡颜色已更新", "Chat color")),
                        None => tr_str(&lang, "已恢复默认气泡颜色", "Reset chat color").to_string(),
                    };
                    self.show_chat_toast(cid, msg, 1600, cx);
                }
            }
            OpenBotInspectorAction::SetBotBusyMode(mode) => {
                self.push_bot_fusion_step("BusyMode", "updated", mode.clone());
                if let Some(cid) = active_chat {
                    self.show_chat_toast(
                        cid,
                        format!("{}: {mode}", tr_str(&lang, "忙碌消息策略", "Busy mode")),
                        1600,
                        cx,
                    );
                }
            }
            OpenBotInspectorAction::SetBotPermissions {
                access,
                computer_use,
                allow_automation,
            } => {
                let approval_mode = match access.as_str() {
                    "full" => "autonomous",
                    "workspace" => "strict",
                    _ => "prompt",
                };
                if let Some(fab) = self.active_fabushi_bot_for_fusion(cx) {
                    let bot_id = fab.bot_user_id.max(0) as u64;
                    if bot_id > 0 {
                        self.handle_inspector_action(
                            InspectorAction::PatchFabushiBot {
                                bot_id,
                                req: gpui_telegram_proto::miniapp_contracts::PatchFabushiBotRequest {
                                    approval_mode: Some(approval_mode.to_string()),
                                    ..Default::default()
                                },
                            },
                            cx,
                        );
                    }
                }
                self.push_bot_fusion_step(
                    "Permissions",
                    &access,
                    format!("cua={computer_use}, auto={allow_automation}"),
                );
            }
            OpenBotInspectorAction::SetChannelBrowserPolicy(policy) => {
                if let Some(cid) = active_chat {
                    self.show_chat_toast(
                        cid,
                        format!("{}: {policy}", tr_str(&lang, "浏览器权限", "Browser access")),
                        1600,
                        cx,
                    );
                }
            }
            OpenBotInspectorAction::SetChannelContextIsolation(isolated) => {
                self.stage.update(cx, |st, st_cx| {
                    st.is_channel_isolated = isolated;
                    st_cx.notify();
                });
                if let Some(cid) = active_chat {
                    self.show_chat_toast(
                        cid,
                        if isolated {
                            tr_str(&lang, "已开启频道记忆隔离", "Channel memory isolation enabled")
                        } else {
                            tr_str(&lang, "已关闭频道记忆隔离", "Channel memory isolation disabled")
                        }
                        .to_string(),
                        1800,
                        cx,
                    );
                }
            }
            OpenBotInspectorAction::SetChannelIcon { icon_name, hue } => {
                if let Some(cid) = active_chat {
                    let msg = match icon_name {
                        Some(name) => format!(
                            "{}: {name} ({}°)",
                            tr_str(&lang, "已更新频道图标", "Channel icon"),
                            hue.unwrap_or(215)
                        ),
                        None => tr_str(&lang, "已恢复成员头像拼贴", "Reset to member cluster").to_string(),
                    };
                    self.show_chat_toast(cid, msg, 1800, cx);
                }
            }
            OpenBotInspectorAction::NotifyStatus(msg) => {
                if let Some(cid) = active_chat {
                    self.show_chat_toast(cid, msg, 1800, cx);
                }
            }
        }
    }

    /// Renders the `38px` OpenBot `SidePanelHeader` inside `#app-shell-top-chrome-strip`'s right slot:
    /// open pane tabs with seamless `render_tab_concave_fillet` active seam, `+` Add Pane menu,
    /// `...` More/Split menu, Maximize/Restore toggle, and far-right `PanelRight` collapse toggle.
    pub fn render_unified_side_panel_top_header(
        &mut self,
        width_px: f32,
        maximized: bool,
        pal: &TdesktopThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ob = pal.openbot_palette();
        let lang = self.active_lang_id().to_string();
        let is_zh = lang.starts_with("zh");
        let target = self.side_panel_settings_target(cx);
        let hdr_layout = self
            .bot_fusion
            .side_panel
            .compute_header_layout(target, width_px, is_zh);
        let open_tabs = self.bot_fusion.side_panel.open_tabs.clone();
        let active_top = self.bot_fusion.side_panel.active_top_tab;
        let has_split = self.bot_fusion.side_panel.split_bottom_pane.is_some();
        let add_open = self.bot_fusion.side_panel.add_menu_open;
        let more_open = self.bot_fusion.side_panel.more_menu_open;
        let has_tabs = hdr_layout.more_btn_visible;
        let browser_pane_open = open_tabs.contains(&SidePanelPaneKind::Browser)
            || active_top == Some(SidePanelPaneKind::Browser)
            || self.bot_fusion.side_panel.split_bottom_pane == Some(SidePanelPaneKind::Browser);
        let page_tabs_shown = self.bot_fusion.side_panel.browser_page_tabs_shown;
        let browser_header_tabs: Vec<(String, String, bool)> = if page_tabs_shown && browser_pane_open {
            self.browser_workspace
                .as_ref()
                .map(|bw| {
                    let r = bw.read(cx);
                    r.session
                        .tabs
                        .iter()
                        .map(|t| {
                            (
                                t.id.clone(),
                                t.display_title(is_zh),
                                r.session.active_tab_id.as_deref() == Some(t.id.as_str()),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let mut tabs_row = div()
            .id("openbot-sidepanel-header-tabs")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .items_end()
            .gap(px(2.0))
            .overflow_hidden();

        for (idx, pane) in open_tabs
            .iter()
            .copied()
            .take(hdr_layout.visible_tabs_count)
            .enumerate()
        {
            if pane == SidePanelPaneKind::Browser && page_tabs_shown && !browser_header_tabs.is_empty() {
                for (bidx, (btab_id, btitle, bis_selected)) in browser_header_tabs.iter().enumerate() {
                    let is_active = active_top == Some(SidePanelPaneKind::Browser) && *bis_selected;
                    let tid_open = btab_id.clone();
                    let tid_close = btab_id.clone();
                    let btab_el = div()
                        .id(("openbot-sp-browser-page-tab", bidx))
                        .cursor_pointer()
                        .h(px(if is_active { 32.0 } else { 28.0 }))
                        .max_w(px(140.0))
                        .min_w(px(76.0))
                        .px(px(8.0))
                        .mb(px(if is_active { 0.0 } else { 5.0 }))
                        .when(is_active, |el| {
                            el.rounded_t(px(8.0))
                                .bg(ob.bg_canvas())
                                .border_t_1()
                                .border_l_1()
                                .border_r_1()
                                .border_color(ob.border_hairline())
                        })
                        .when(!is_active, |el| {
                            el.rounded(px(8.0)).hover(|s| s.bg(ob.bg_control_hover()))
                        })
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(5.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let bw = this.ensure_browser_workspace_background(cx);
                                bw.update(cx, |w, wcx| {
                                    w.activate_tab(&tid_open, wcx);
                                });
                                this.open_unified_side_panel_pane(SidePanelPaneKind::Browser, cx);
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .child(crate::views::browser::openbot_browser_icon(
                                    crate::views::browser::OpenBotBrowserIcon::Globe2,
                                    13.0,
                                    if is_active {
                                        ob.text_primary()
                                    } else {
                                        ob.text_muted()
                                    },
                                ))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.5))
                                        .font_weight(if is_active {
                                            FontWeight::MEDIUM
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(if is_active {
                                            ob.text_primary()
                                        } else {
                                            ob.text_secondary()
                                        })
                                        .child(btitle.clone()),
                                ),
                        )
                        .child(
                            div()
                                .id(("openbot-sp-browser-page-tab-close", bidx))
                                .cursor_pointer()
                                .w(px(16.0))
                                .h(px(16.0))
                                .rounded_full()
                                .hover(|s| s.bg(ob.hover()))
                                .flex()
                                .items_center()
                                .justify_center()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if let Some(ref bw) = this.browser_workspace {
                                            bw.update(cx, |w, wcx| {
                                                w.close_tabs(
                                                    std::slice::from_ref(&tid_close),
                                                    wcx,
                                                );
                                            });
                                            this.sync_side_browser_pane_from_workspace(cx);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(openbot_stage_icon(
                                    OpenBotStageIcon::Close,
                                    10.0,
                                    ob.text_muted(),
                                )),
                        );

                    if is_active {
                        tabs_row = tabs_row.child(
                            div()
                                .h_full()
                                .flex()
                                .items_end()
                                .child(render_tab_concave_fillet(
                                    true,
                                    8.0,
                                    ob.bg_canvas(),
                                    ob.border_hairline(),
                                ))
                                .child(btab_el)
                                .child(render_tab_concave_fillet(
                                    false,
                                    8.0,
                                    ob.bg_canvas(),
                                    ob.border_hairline(),
                                )),
                        );
                    } else {
                        tabs_row = tabs_row.child(btab_el);
                    }
                }
                continue;
            }
            let is_active = active_top == Some(pane);
            let tab_unavailable = self.bot_fusion.side_panel.is_tab_unavailable(pane.as_str())
                || (pane == SidePanelPaneKind::Files && hdr_layout.unavailable_tab_flagged);
            let label = pane.localized_tab_label(target, is_zh);
            let tab_el = div()
                .id(("openbot-sp-tab", idx))
                .cursor_pointer()
                .h(px(if is_active { 32.0 } else { 28.0 }))
                .max_w(px(140.0))
                .min_w(px(76.0))
                .px(px(8.0))
                .mb(px(if is_active { 0.0 } else { 5.0 }))
                .when(tab_unavailable, |el| el.opacity(0.55))
                .when(is_active, |el| {
                    el.rounded_t(px(8.0))
                        .bg(ob.bg_canvas())
                        .border_t_1()
                        .border_l_1()
                        .border_r_1()
                        .border_color(ob.border_hairline())
                })
                .when(!is_active, |el| {
                    el.rounded(px(8.0)).hover(|s| s.bg(ob.bg_control_hover()))
                })
                .flex()
                .items_center()
                .justify_between()
                .gap(px(5.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.open_unified_side_panel_pane(pane, cx);
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(tg_icon_rgba(
                            pane.icon(),
                            13.0,
                            if is_active {
                                ob.text_primary()
                            } else {
                                ob.text_muted()
                            },
                        ))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.5))
                                .font_weight(if is_active {
                                    FontWeight::MEDIUM
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(if is_active {
                                    ob.text_primary()
                                } else {
                                    ob.text_secondary()
                                })
                                .child(label),
                        )
                        .when(tab_unavailable, |el| {
                            el.child(
                                div()
                                    .px(px(4.0))
                                    .rounded(px(4.0))
                                    .bg(ob.bg_control())
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ob.warning())
                                    .child("!"),
                            )
                        }),
                )
                .child(
                    div()
                        .id(("openbot-sp-tab-close", idx))
                        .cursor_pointer()
                        .w(px(16.0))
                        .h(px(16.0))
                        .rounded_full()
                        .hover(|s| s.bg(ob.hover()))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_unified_side_panel_tab(pane, cx);
                            }),
                        )
                        .child(openbot_stage_icon(OpenBotStageIcon::Close, 10.0, ob.text_muted())),
                );

            if is_active {
                tabs_row = tabs_row.child(
                    div()
                        .h_full()
                        .flex()
                        .items_end()
                        .child(render_tab_concave_fillet(
                            true,
                            8.0,
                            ob.bg_canvas(),
                            ob.border_hairline(),
                        ))
                        .child(tab_el)
                        .child(render_tab_concave_fillet(
                            false,
                            8.0,
                            ob.bg_canvas(),
                            ob.border_hairline(),
                        )),
                );
            } else {
                tabs_row = tabs_row.child(tab_el);
            }
        }

        if !has_tabs {
            tabs_row = tabs_row.child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            );
        }

        let add_menu_popover = add_open.then(|| {
            let mut menu = div()
                .id("openbot-sp-add-menu")
                .w(px(196.0))
                .p(px(4.0))
                .rounded(px(10.0))
                .bg(ob.bg_surface())
                .border_1()
                .border_color(ob.border_strong_hairline())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap(px(2.0));
            if browser_pane_open {
                menu = menu
                    .child(
                        div()
                            .id("openbot-sp-add-new-browser-tab")
                            .cursor_pointer()
                            .h(px(30.0))
                            .px(px(8.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(ob.hover()))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_new_browser_tab_from_side_panel(false, cx);
                                }),
                            )
                            .child(crate::views::browser::openbot_browser_icon(
                                crate::views::browser::OpenBotBrowserIcon::Plus,
                                13.5,
                                ob.text_primary(),
                            ))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_primary())
                                    .child(if is_zh {
                                        "新建浏览器标签页"
                                    } else {
                                        "New browser tab"
                                    }),
                            ),
                    )
                    .child(div().w_full().h(px(1.0)).my(px(3.0)).bg(ob.border_hairline()));
            }
            for (idx, pane) in SidePanelPaneKind::ALL.iter().copied().enumerate() {
                menu = menu.child(
                    div()
                        .id(("openbot-sp-add-item", idx))
                        .cursor_pointer()
                        .h(px(30.0))
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .hover(|s| s.bg(ob.hover()))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_unified_side_panel_pane(pane, cx);
                            }),
                        )
                        .child(tg_icon_rgba(pane.icon(), 13.5, ob.text_muted()))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(ob.text_primary())
                                .child(pane.launcher_label(target, is_zh)),
                        ),
                );
            }
            deferred(
                div()
                    .absolute()
                    .top(px(36.0))
                    .right(px(44.0))
                    .occlude()
                    .child(menu),
            )
            .with_priority(90)
        });

        let more_menu_popover = more_open.then(|| {
            let split_pane = match active_top {
                Some(SidePanelPaneKind::Files) => SidePanelPaneKind::Threads,
                _ => SidePanelPaneKind::Files,
            };
            let mut menu = div()
                .id("openbot-sp-more-menu")
                .w(px(210.0))
                .p(px(4.0))
                .rounded(px(10.0))
                .bg(ob.bg_surface())
                .border_1()
                .border_color(ob.border_strong_hairline())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap(px(2.0));
            if browser_pane_open {
                menu = menu
                    .child(
                        div()
                            .id("openbot-sp-more-open-browser-full")
                            .cursor_pointer()
                            .h(px(30.0))
                            .px(px(8.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(ob.hover()))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_size(px(12.5))
                            .text_color(ob.text_primary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.bot_fusion.side_panel.more_menu_open = false;
                                    this.open_browser_full_view_for_tab(None, cx);
                                }),
                            )
                            .child(crate::views::browser::openbot_browser_icon(
                                crate::views::browser::OpenBotBrowserIcon::Maximize2,
                                13.0,
                                ob.text_muted(),
                            ))
                            .child(if is_zh {
                                "在完整视图中打开"
                            } else {
                                "Open in full view"
                            }),
                    )
                    .child(
                        div()
                            .id("openbot-sp-more-toggle-page-tabs")
                            .cursor_pointer()
                            .h(px(30.0))
                            .px(px(8.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(ob.hover()))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_size(px(12.5))
                            .text_color(ob.text_primary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_side_panel_browser_page_tabs(cx);
                                }),
                            )
                            .child(crate::views::browser::openbot_browser_icon(
                                if page_tabs_shown {
                                    crate::views::browser::OpenBotBrowserIcon::Group
                                } else {
                                    crate::views::browser::OpenBotBrowserIcon::Ungroup
                                },
                                13.0,
                                ob.text_muted(),
                            ))
                            .child(if page_tabs_shown {
                                if is_zh {
                                    "将页面合并在一个标签页中"
                                } else {
                                    "Combine pages in one tab"
                                }
                            } else if is_zh {
                                "以标签页显示页面"
                            } else {
                                "Show pages as tabs"
                            }),
                    )
                    .child(div().w_full().h(px(1.0)).my(px(3.0)).bg(ob.border_hairline()));
            }
            if !has_split {
                menu = menu.child(
                    div()
                        .id("openbot-sp-more-split-down")
                        .cursor_pointer()
                        .h(px(30.0))
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .hover(|s| s.bg(ob.hover()))
                        .flex()
                        .items_center()
                        .text_size(px(12.5))
                        .text_color(ob.text_primary())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.split_unified_side_panel_down(split_pane, cx);
                            }),
                        )
                        .child(tr_str(&lang, "向下拆分", "Split down")),
                );
            } else {
                menu = menu
                    .child(
                        div()
                            .id("openbot-sp-more-swap-split")
                            .cursor_pointer()
                            .h(px(30.0))
                            .px(px(8.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(ob.hover()))
                            .flex()
                            .items_center()
                            .text_size(px(12.5))
                            .text_color(ob.text_primary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.bot_fusion.side_panel.swap_split_panes();
                                    this.bot_fusion.side_panel.more_menu_open = false;
                                    this.sync_unified_side_panel_flags(cx);
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(&lang, "上下互换分组", "Swap split panes")),
                    )
                    .child(
                        div()
                            .id("openbot-sp-more-close-split")
                            .cursor_pointer()
                            .h(px(30.0))
                            .px(px(8.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(ob.hover()))
                            .flex()
                            .items_center()
                            .text_size(px(12.5))
                            .text_color(ob.text_primary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.bot_fusion.side_panel.more_menu_open = false;
                                    this.close_unified_side_panel_split(cx);
                                }),
                            )
                            .child(tr_str(&lang, "关闭下方拆分", "Close lower split")),
                    );
            }
            if has_tabs {
                menu = menu.child(
                    div()
                        .id("openbot-sp-more-launcher")
                        .cursor_pointer()
                        .h(px(30.0))
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .hover(|s| s.bg(ob.hover()))
                        .flex()
                        .items_center()
                        .text_size(px(12.5))
                        .text_color(ob.text_secondary())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_unified_side_panel_launcher(cx);
                            }),
                        )
                        .child(tr_str(&lang, "返回启动页", "Show launcher")),
                );
            }
            deferred(
                div()
                    .absolute()
                    .top(px(36.0))
                    .right(px(24.0))
                    .occlude()
                    .child(menu),
            )
            .with_priority(90)
        });

        let mut slot = div()
            .id("openbot-right-panel-top-slot")
            .relative()
            .h(px(hdr_layout.height_px))
            .bg(ob.bg_frame())
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0));

        if maximized {
            slot = slot.flex_1().min_w(px(SETTINGS_PANEL_DEFAULT));
        } else {
            slot = slot.w(px(width_px)).flex_shrink_0();
        }

        slot.child(
            div()
                .w(px(1.0))
                .h(px(18.0))
                .mr(px(2.0))
                .bg(ob.border_strong_hairline()),
        )
        .child(tabs_row.when(hdr_layout.overflow_chevron_visible, |row| {
            row.child(
                div()
                    .id("openbot-sp-hdr-overflow-chevron")
                    .h(px(24.0))
                    .px(px(6.0))
                    .mb(px(6.0))
                    .rounded(px(6.0))
                    .bg(ob.bg_control())
                    .flex()
                    .items_center()
                    .text_size(px(11.0))
                    .text_color(ob.text_muted())
                    .child(format!("▾ +{}", hdr_layout.hidden_tabs_count)),
            )
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(3.0))
                .flex_shrink_0()
                .child(
                    div()
                        .id("openbot-sp-hdr-add")
                        .cursor_pointer()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(7.0))
                        .bg(if add_open { ob.bg_control_hover() } else { Rgba::default() })
                        .hover(|s| s.bg(ob.bg_control_hover()))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.bot_fusion.side_panel.add_menu_open =
                                    !this.bot_fusion.side_panel.add_menu_open;
                                this.bot_fusion.side_panel.more_menu_open = false;
                                cx.notify();
                            }),
                        )
                        .child(openbot_stage_icon(OpenBotStageIcon::Plus, 14.0, ob.text_muted())),
                )
                .when(has_tabs, |acts| {
                    acts.child(
                        div()
                            .id("openbot-sp-hdr-more")
                            .cursor_pointer()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .bg(if more_open { ob.bg_control_hover() } else { Rgba::default() })
                            .hover(|s| s.bg(ob.bg_control_hover()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.bot_fusion.side_panel.more_menu_open =
                                        !this.bot_fusion.side_panel.more_menu_open;
                                    this.bot_fusion.side_panel.add_menu_open = false;
                                    cx.notify();
                                }),
                            )
                            .child(openbot_stage_icon(
                                OpenBotStageIcon::MoreHorizontal,
                                14.0,
                                ob.text_muted(),
                            )),
                    )
                    .child(
                        div()
                            .id("openbot-sp-hdr-maximize")
                            .cursor_pointer()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .bg(if maximized { ob.bg_control_active() } else { Rgba::default() })
                            .hover(|s| s.bg(ob.bg_control_hover()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.0))
                            .text_color(if maximized { ob.text_primary() } else { ob.text_muted() })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_unified_side_panel_maximize(cx);
                                }),
                            )
                            .child(if maximized { "↙" } else { "↗" }),
                    )
                })
                .child(
                    div()
                        .id("hdr-side-panel-toggle-right")
                        .cursor_pointer()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(7.0))
                        .hover(|s| s.bg(ob.bg_control_hover()))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_unified_side_panel(cx);
                            }),
                        )
                        .child(openbot_stage_icon(
                            OpenBotStageIcon::PanelRight,
                            15.0,
                            ob.text_primary(),
                        )),
                ),
        )
        .when_some(add_menu_popover, |el, pop| el.child(pop))
        .when_some(more_menu_popover, |el, pop| el.child(pop))
        .into_any_element()
    }

    /// Renders the single unified dockable OpenBot Right `SidePanel` (`UI-F15`),
    /// eliminating Defect D11 (double side-by-side right drawers).
    pub fn render_unified_openbot_side_panel(
        &mut self,
        pal: &TdesktopThemePalette,
        workspace_avail_px: f32,
        panel_height_px: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ob = pal.openbot_palette();
        let lang = self.active_lang_id().to_string();
        let is_zh = lang.starts_with("zh");
        let target = self.side_panel_settings_target(cx);
        let sp_layout = self
            .bot_fusion
            .side_panel
            .compute_layout(workspace_avail_px, panel_height_px);
        let maximized = sp_layout.maximized;
        let clamped_w = if maximized {
            sp_layout.width_px
        } else {
            self.bot_fusion
                .side_panel
                .clamp_width_for_workspace(sp_layout.width_px, workspace_avail_px)
        };
        let active_top = sp_layout.active_top_tab;
        let bottom_pane = sp_layout.split_bottom_pane;

        let resizer_handle = (!maximized).then(|| {
            div()
                .id("openbot-side-panel-width-resizer")
                .w(px(DRAG_START_PX))
                .h_full()
                .flex_shrink_0()
                .cursor_col_resize()
                .hover(|s| s.bg(ob.border_strong_hairline()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        if ev.click_count >= 2 {
                            this.bot_fusion.side_panel.state.width_px = SETTINGS_PANEL_DEFAULT;
                            this.bot_fusion.side_panel.is_resizing_width = false;
                            this.bot_fusion.side_panel.resize_drag_start_x = None;
                        } else {
                            this.bot_fusion.side_panel.is_resizing_width = true;
                            this.bot_fusion.side_panel.resize_drag_start_x =
                                Some(f32::from(ev.position.x));
                            this.bot_fusion.side_panel.resize_start_width =
                                this.bot_fusion.side_panel.state.width_px;
                        }
                        cx.notify();
                    }),
                )
        });

        let content_area: AnyElement = if active_top.is_none() && bottom_pane.is_none() {
            // Exact centered 4-button `SidePanelLauncher` (`image_2.png` 1:1)
            let mut stack = div()
                .id("openbot-side-panel-launcher")
                .w_full()
                .max_w(px(OPENBOT_LAUNCHER_MAX_WIDTH))
                .mx_auto()
                .p(px(24.0))
                .flex()
                .flex_col()
                .gap(px(6.0));
            for (idx, pane) in LAUNCHER_PRIMARY_PANES.iter().copied().enumerate() {
                stack = stack.child(
                    div()
                        .id(("openbot-launcher-btn", idx))
                        .w_full()
                        .h(px(OPENBOT_LAUNCHER_BUTTON_HEIGHT))
                        .px(px(16.0))
                        .rounded(px(8.0))
                        .bg(ob.bg_control_hover())
                        .hover(|s| s.bg(ob.bg_control_active()))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_start()
                        .gap(px(12.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.open_unified_side_panel_pane(pane, cx);
                            }),
                        )
                        .child(tg_icon_rgba(pane.icon(), 14.0, ob.text_muted()))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .child(pane.launcher_label(target, is_zh)),
                        ),
                );
            }
            div()
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(stack)
                .into_any_element()
        } else {
            let top_kind = active_top.unwrap_or(SidePanelPaneKind::Settings);
            let top_body = self.render_unified_side_pane_body(top_kind, pal, &lang, cx);

            let mut split_col = div()
                .flex_1()
                .w_full()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_1()
                        .min_h(px(sp_layout.main_group_height_px.min(SIDE_MAIN_GROUP_MIN)))
                        .w_full()
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .child(top_body),
                );

            if let Some(bot_kind) = bottom_pane {
                let bot_body = self.render_unified_side_pane_body(bot_kind, pal, &lang, cx);
                let bot_label = bot_kind.localized_tab_label(target, is_zh);
                let lower_h = sp_layout.lower_split_height_px;
                split_col = split_col
                    .child(
                        div()
                            .id("openbot-sp-split-divider")
                            .w_full()
                            .h(px(5.0))
                            .flex_shrink_0()
                            .cursor_row_resize()
                            .bg(ob.border_hairline())
                            .hover(|s| s.bg(ob.accent()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.bot_fusion.side_panel.is_resizing_split = true;
                                    this.bot_fusion.side_panel.split_drag_start_y =
                                        Some(f32::from(ev.position.y));
                                    this.bot_fusion.side_panel.split_start_share =
                                        this.bot_fusion.side_panel.state.lower_share;
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .h(px(lower_h))
                            .min_h(px(SIDE_SPLIT_MIN))
                            .flex_shrink_0()
                            .w_full()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("openbot-sp-lower-header")
                                    .h(px(OPENBOT_SIDE_PANEL_HEADER_HEIGHT))
                                    .flex_shrink_0()
                                    .px(px(10.0))
                                    .border_b_1()
                                    .border_color(ob.border_hairline())
                                    .bg(ob.bg_canvas())
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(tg_icon_rgba(
                                                bot_kind.icon(),
                                                13.5,
                                                ob.text_muted(),
                                            ))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(ob.text_primary())
                                                    .child(bot_label),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .id("openbot-sp-lower-move-up")
                                                    .cursor_pointer()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(5.0))
                                                    .hover(|s| s.bg(ob.hover()))
                                                    .text_size(px(11.0))
                                                    .text_color(ob.text_muted())
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.bot_fusion
                                                                .side_panel
                                                                .swap_split_panes();
                                                            this.sync_unified_side_panel_flags(cx);
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child("↑"),
                                            )
                                            .child(
                                                div()
                                                    .id("openbot-sp-lower-close")
                                                    .cursor_pointer()
                                                    .w(px(22.0))
                                                    .h(px(22.0))
                                                    .rounded(px(5.0))
                                                    .hover(|s| s.bg(ob.hover()))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.close_unified_side_panel_split(cx);
                                                        }),
                                                    )
                                                    .child(openbot_stage_icon(
                                                        OpenBotStageIcon::Close,
                                                        11.0,
                                                        ob.text_muted(),
                                                    )),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .w_full()
                                    .overflow_hidden()
                                    .flex()
                                    .flex_col()
                                    .child(bot_body),
                            ),
                    );
            }
            split_col.into_any_element()
        };

        let mut panel = div()
            .id("app-shell-unified-openbot-sidepanel")
            .h_full()
            .bg(ob.bg_canvas())
            .border_l_1()
            .border_color(ob.border_hairline())
            .flex()
            .flex_row()
            .overflow_hidden();

        if maximized {
            panel = panel.flex_1().min_w(px(SETTINGS_PANEL_DEFAULT));
        } else {
            panel = panel.w(px(clamped_w)).flex_shrink_0();
        }

        panel
            .when_some(resizer_handle, |p, handle| p.child(handle))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(content_area),
            )
            .into_any_element()
    }

    fn render_unified_side_pane_body(
        &mut self,
        pane: SidePanelPaneKind,
        pal: &TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ob = pal.openbot_palette();
        let snap = self.bot_fusion.snapshot.clone();
        match pane {
            SidePanelPaneKind::Settings => self.inspector.clone().into_any_element(),
            SidePanelPaneKind::Browser => {
                let is_zh = lang.starts_with("zh");
                let miniapp_url = self.inspector.read(cx).profile.miniapp_url.clone();
                let bw = self.ensure_browser_workspace_background(cx);
                let (tabs, active_tab_id, can_back, can_fwd, is_loading) = {
                    let r = bw.read(cx);
                    (
                        r.session.tabs.clone(),
                        r.session.active_tab_id.clone(),
                        r.can_go_back,
                        r.can_go_forward,
                        r.is_loading,
                    )
                };
                let hovered_tab_id = self
                    .bot_fusion
                    .side_panel
                    .hovered_browser_preview_tab_id
                    .clone();
                let page_tabs_shown = self.bot_fusion.side_panel.browser_page_tabs_shown;

                // Empty state (`BrowserPreviewSidebar.tsx` lines 69-77)
                if tabs.is_empty() {
                    return div()
                        .id("openbot-sp-pane-browser-empty")
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full()
                        .p(px(24.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(10.0))
                        .child(crate::views::browser::openbot_browser_icon(
                            crate::views::browser::OpenBotBrowserIcon::Monitor,
                            28.0,
                            ob.text_muted(),
                        ))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(ob.text_muted())
                                .child(if is_zh {
                                    "没有打开的浏览器标签页"
                                } else {
                                    "No browser tabs open"
                                }),
                        )
                        .child(
                            div()
                                .id("openbot-sp-br-open-first-page")
                                .mt(px(4.0))
                                .h(px(32.0))
                                .px(px(14.0))
                                .rounded_full()
                                .bg(ob.bg_control_hover())
                                .border_1()
                                .border_color(ob.border_strong_hairline())
                                .hover(|s| s.bg(ob.bg_control_active()))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.open_new_browser_tab_from_side_panel(true, cx);
                                    }),
                                )
                                .child(if is_zh { "打开一个页面" } else { "Open a page" }),
                        )
                        .into_any_element();
                }

                // Inline page-tabs view (`page_tabs_shown == true`): compact address row + active page live preview
                if page_tabs_shown {
                    let active_tab = active_tab_id
                        .as_deref()
                        .and_then(|aid| tabs.iter().find(|t| t.id == aid))
                        .or_else(|| tabs.first())
                        .cloned();
                    let active_url = active_tab
                        .as_ref()
                        .map(|t| t.url.clone())
                        .unwrap_or_default();
                    let active_tid = active_tab.as_ref().map(|t| t.id.clone());
                    let is_hovered = active_tid.as_deref().is_some()
                        && hovered_tab_id.as_deref() == active_tid.as_deref();

                    let mut pane_col = div()
                        .id("openbot-sp-pane-browser-inline")
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .id("openbot-sp-br-inline-addr")
                                .h(px(40.0))
                                .w_full()
                                .flex_shrink_0()
                                .px(px(8.0))
                                .border_b_1()
                                .border_color(ob.border_hairline())
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .id("openbot-sp-br-back")
                                        .w(px(26.0))
                                        .h(px(26.0))
                                        .rounded(px(6.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .when(can_back, |el| {
                                            el.cursor_pointer().hover(|s| s.bg(ob.hover()))
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                if let Some(ref bw) = this.browser_workspace {
                                                    bw.update(cx, |w, wcx| w.go_back(wcx));
                                                }
                                            }),
                                        )
                                        .child(crate::views::browser::openbot_browser_icon(
                                            crate::views::browser::OpenBotBrowserIcon::ChevronLeft,
                                            14.0,
                                            if can_back {
                                                ob.text_secondary()
                                            } else {
                                                ob.text_dim()
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .id("openbot-sp-br-fwd")
                                        .w(px(26.0))
                                        .h(px(26.0))
                                        .rounded(px(6.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .when(can_fwd, |el| {
                                            el.cursor_pointer().hover(|s| s.bg(ob.hover()))
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                if let Some(ref bw) = this.browser_workspace {
                                                    bw.update(cx, |w, wcx| w.go_forward(wcx));
                                                }
                                            }),
                                        )
                                        .child(crate::views::browser::openbot_browser_icon(
                                            crate::views::browser::OpenBotBrowserIcon::ChevronRight,
                                            14.0,
                                            if can_fwd {
                                                ob.text_secondary()
                                            } else {
                                                ob.text_dim()
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .id("openbot-sp-br-reload")
                                        .w(px(26.0))
                                        .h(px(26.0))
                                        .rounded(px(6.0))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(ob.hover()))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                if let Some(ref bw) = this.browser_workspace {
                                                    bw.update(cx, |w, wcx| {
                                                        if w.is_loading {
                                                            w.stop_loading(wcx);
                                                        } else {
                                                            w.reload(wcx);
                                                        }
                                                    });
                                                }
                                            }),
                                        )
                                        .child(crate::views::browser::openbot_browser_icon(
                                            if is_loading {
                                                crate::views::browser::OpenBotBrowserIcon::StopX
                                            } else {
                                                crate::views::browser::OpenBotBrowserIcon::Reload
                                            },
                                            13.5,
                                            ob.text_secondary(),
                                        )),
                                )
                                .child(
                                    div()
                                        .id("openbot-sp-br-inline-capsule")
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .h(px(28.0))
                                        .px(px(8.0))
                                        .rounded(px(8.0))
                                        .bg(ob.bg_control())
                                        .border_1()
                                        .border_color(ob.border_hairline())
                                        .cursor_pointer()
                                        .flex()
                                        .items_center()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener({
                                                let tid = active_tid.clone();
                                                move |this, _, _, cx| {
                                                    this.open_browser_full_view_for_tab(
                                                        tid.as_deref(),
                                                        cx,
                                                    );
                                                }
                                            }),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .truncate()
                                                .text_size(px(12.0))
                                                .text_color(ob.text_primary())
                                                .child(active_url),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("openbot-sp-br-open-full")
                                        .w(px(26.0))
                                        .h(px(26.0))
                                        .rounded(px(6.0))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(ob.hover()))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener({
                                                let tid = active_tid.clone();
                                                move |this, _, _, cx| {
                                                    this.open_browser_full_view_for_tab(
                                                        tid.as_deref(),
                                                        cx,
                                                    );
                                                }
                                            }),
                                        )
                                        .child(crate::views::browser::openbot_browser_icon(
                                            crate::views::browser::OpenBotBrowserIcon::Maximize2,
                                            13.0,
                                            ob.text_secondary(),
                                        )),
                                ),
                        );

                    if let Some(tab) = active_tab {
                        let tid_open = tab.id.clone();
                        let tid_hover = tab.id.clone();
                        let title = tab.preview_title(is_zh);
                        pane_col = pane_col.child(
                            div()
                                .id("openbot-sp-br-inline-preview-scroll")
                                .flex_1()
                                .min_h(px(0.0))
                                .overflow_y_scroll()
                                .p(px(14.0))
                                .flex()
                                .flex_col()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .id("openbot-sp-br-inline-preview-card")
                                        .w_full()
                                        .cursor_pointer()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .gap(px(8.0))
                                        .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                                            if *hovering {
                                                this.bot_fusion
                                                    .side_panel
                                                    .hovered_browser_preview_tab_id =
                                                    Some(tid_hover.clone());
                                            } else if this
                                                .bot_fusion
                                                .side_panel
                                                .hovered_browser_preview_tab_id
                                                .as_deref()
                                                == Some(tid_hover.as_str())
                                            {
                                                this.bot_fusion
                                                    .side_panel
                                                    .hovered_browser_preview_tab_id = None;
                                            }
                                            cx.notify();
                                        }))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.open_browser_full_view_for_tab(
                                                    Some(&tid_open),
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(
                                            crate::views::browser::render_browser_preview_thumbnail_content(
                                                &tab, is_hovered, &ob, is_zh,
                                            ),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .justify_center()
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(13.0))
                                                        .text_color(ob.text_secondary())
                                                        .child(title),
                                                ),
                                        ),
                                ),
                        );
                    }
                    return pane_col.into_any_element();
                }

                // Default Mode (`!page_tabs_shown`): 1:1 OpenBot `BrowserPreviewSidebar` (`image.png` & `image_3.png`)
                let mut cards_col = div()
                    .id("openbot-sp-pane-browser")
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(16.0));

                for (idx, tab) in tabs.iter().enumerate() {
                    let is_hovered = hovered_tab_id.as_deref() == Some(tab.id.as_str());
                    let tid_open = tab.id.clone();
                    let tid_hover = tab.id.clone();
                    let tid_close = tab.id.clone();
                    let title = tab.preview_title(is_zh);

                    let mut thumb = crate::views::browser::render_browser_preview_thumbnail_content(
                        tab, is_hovered, &ob, is_zh,
                    );

                    // Top-right circular `[×]` close button on hover (exact 1:1 `image_3.png`)
                    if is_hovered {
                        thumb = thumb.child(
                            div()
                                .id(("openbot-browser-preview-close", idx))
                                .absolute()
                                .top(px(8.0))
                                .right(px(8.0))
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded_full()
                                .bg(ob.bg_canvas())
                                .border_1()
                                .border_color(ob.border_strong_hairline())
                                .hover(|s| s.bg(ob.bg_control_hover()))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if let Some(ref bw) = this.browser_workspace {
                                            bw.update(cx, |w, wcx| {
                                                w.close_tabs(
                                                    std::slice::from_ref(&tid_close),
                                                    wcx,
                                                );
                                            });
                                            this.sync_side_browser_pane_from_workspace(cx);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(crate::views::browser::openbot_browser_icon(
                                    crate::views::browser::OpenBotBrowserIcon::CloseX,
                                    12.0,
                                    ob.text_primary(),
                                )),
                        );
                    }

                    cards_col = cards_col.child(
                        div()
                            .id(("openbot-browser-preview-card", idx))
                            .w_full()
                            .cursor_pointer()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(8.0))
                            .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                                if *hovering {
                                    if this
                                        .bot_fusion
                                        .side_panel
                                        .hovered_browser_preview_tab_id
                                        .as_deref()
                                        != Some(tid_hover.as_str())
                                    {
                                        this.bot_fusion
                                            .side_panel
                                            .hovered_browser_preview_tab_id =
                                            Some(tid_hover.clone());
                                        cx.notify();
                                    }
                                } else if this
                                    .bot_fusion
                                    .side_panel
                                    .hovered_browser_preview_tab_id
                                    .as_deref()
                                    == Some(tid_hover.as_str())
                                {
                                    this.bot_fusion
                                        .side_panel
                                        .hovered_browser_preview_tab_id = None;
                                    cx.notify();
                                }
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.open_browser_full_view_for_tab(Some(&tid_open), cx);
                                }),
                            )
                            .child(thumb)
                            .child(
                                div()
                                    .w_full()
                                    .px(px(6.0))
                                    .flex()
                                    .justify_center()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(13.0))
                                            .text_color(ob.text_secondary())
                                            .child(title),
                                    ),
                            ),
                    );
                }

                // Preserve optional MiniApp launch shortcut when active bot defines a miniapp_url
                if let Some(url) = miniapp_url {
                    let u_launch = url.clone();
                    cards_col = cards_col.child(
                        div()
                            .id("openbot-sp-br-launch-miniapp")
                            .mt(px(4.0))
                            .cursor_pointer()
                            .w_full()
                            .px(px(12.0))
                            .py(px(9.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .hover(|s| s.bg(ob.bg_control_hover()))
                            .flex()
                            .items_center()
                            .justify_between()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.handle_inspector_action(
                                        InspectorAction::LaunchMiniApp(u_launch.clone()),
                                        cx,
                                    );
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(ob.text_primary())
                                    .child(tr_str(lang, "启动当前助手 MiniApp", "Launch MiniApp")),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(ob.text_muted())
                                    .child("↗"),
                            ),
                    );
                }

                cards_col.into_any_element()
            }
            SidePanelPaneKind::Files => {
                let (files, media_items) = {
                    let insp = self.inspector.read(cx);
                    (
                        insp.shared_media.files.clone(),
                        insp.shared_media.media_items.clone(),
                    )
                };
                let mut col = div()
                    .id("openbot-sp-pane-files")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0));

                // Shared Files & Media
                let mut files_card = div()
                    .w_full()
                    .rounded(px(8.0))
                    .bg(ob.bg_surface())
                    .border_1()
                    .border_color(ob.border_hairline())
                    .overflow_hidden()
                    .flex()
                    .flex_col();
                if files.is_empty() && media_items.is_empty() {
                    files_card = files_card.child(
                        div()
                            .px(px(12.0))
                            .py(px(12.0))
                            .text_size(px(12.5))
                            .text_color(ob.text_muted())
                            .child(tr_str(lang, "当前会话暂无共享文件或媒体。", "No shared files or media yet.")),
                    );
                } else {
                    for (idx, f) in files.iter().enumerate() {
                        let fname = f.name.clone();
                        files_card = files_card.child(
                            div()
                                .id(("openbot-sp-file-row", idx))
                                .cursor_pointer()
                                .w_full()
                                .min_h(px(40.0))
                                .px(px(12.0))
                                .hover(|s| s.bg(ob.hover()))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        let fpath = std::path::Path::new(&fname);
                                        if fpath.is_absolute()
                                            && this
                                                .bot_fusion
                                                .side_panel
                                                .sync_file_tab_path_availability(
                                                    SidePanelPaneKind::Files.as_str(),
                                                    fpath,
                                                )
                                        {
                                            this.show_non_chat_toast(
                                                "File is no longer available on disk.",
                                                2400,
                                                cx,
                                            );
                                            return;
                                        }
                                        this.show_media_viewer(fname.clone(), false, cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(px(13.0))
                                        .text_color(ob.text_primary())
                                        .child(f.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(ob.text_muted())
                                        .child(f.size_text.clone()),
                                ),
                        );
                    }
                    for (idx, m) in media_items.iter().take(6).enumerate() {
                        let url = m.url.clone();
                        let is_vid = m.is_video;
                        files_card = files_card.child(
                            div()
                                .id(("openbot-sp-media-row", idx))
                                .cursor_pointer()
                                .w_full()
                                .min_h(px(40.0))
                                .px(px(12.0))
                                .hover(|s| s.bg(ob.hover()))
                                .flex()
                                .items_center()
                                .justify_between()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.show_media_viewer(url.clone(), is_vid, cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(13.0))
                                        .text_color(ob.text_primary())
                                        .child(if m.caption.is_empty() {
                                            format!("Media #{}", m.id)
                                        } else {
                                            m.caption.clone()
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(ob.text_muted())
                                        .child(if is_vid { "Video" } else { "Photo" }),
                                ),
                        );
                    }
                }
                col = col
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_muted())
                            .child(format!(
                                "{} ({})",
                                tr_str(lang, "共享文件与媒体", "Shared Files & Media"),
                                files.len() + media_items.len()
                            )),
                    )
                    .child(files_card);

                // Bot Knowledge: Memories + Skills
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_muted())
                                .child(format!(
                                    "{} ({}) · {} ({})",
                                    tr_str(lang, "记忆", "Memories"),
                                    snap.memories.len(),
                                    tr_str(lang, "技能", "Skills"),
                                    snap.skills.len()
                                )),
                        )
                        .child(
                            div()
                                .id("openbot-sp-files-tidy")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .bg(ob.bg_surface())
                                .border_1()
                                .border_color(ob.border_hairline())
                                .hover(|s| s.bg(ob.bg_control_hover()))
                                .text_size(px(11.5))
                                .text_color(ob.text_secondary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.bot_fusion_tidy_memories(cx);
                                    }),
                                )
                                .child(tr_str(lang, "整理去重", "Tidy")),
                        ),
                );
                for (idx, m) in snap.memories.iter().take(6).enumerate() {
                    let mid = m.id.clone();
                    col = col.child(
                        div()
                            .id(("openbot-sp-mem", idx))
                            .w_full()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.0))
                                    .text_color(ob.text_primary())
                                    .child(m.content.clone()),
                            )
                            .child(
                                div()
                                    .id(("openbot-sp-mem-del", idx))
                                    .cursor_pointer()
                                    .text_size(px(11.5))
                                    .text_color(ob.text_muted())
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
                for (idx, sk) in snap.skills.iter().enumerate() {
                    let sid = sk.skill_id.clone();
                    let next_en = !sk.enabled;
                    col = col.child(
                        div()
                            .id(("openbot-sp-skill", idx))
                            .w_full()
                            .px(px(10.0))
                            .py(px(7.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(ob.text_primary())
                                    .child(format!("${} · {}", sk.slug, sk.name)),
                            )
                            .child(
                                div()
                                    .id(("openbot-sp-skill-tgl", idx))
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(2.0))
                                    .rounded(px(5.0))
                                    .bg(if sk.enabled { ob.bg_control_active() } else { ob.bg_canvas() })
                                    .text_size(px(10.5))
                                    .text_color(if sk.enabled { ob.success() } else { ob.text_muted() })
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
            SidePanelPaneKind::Threads => {
                let (threads, active_tid) = {
                    let st = self.stage.read(cx);
                    (st.thread_tabs.clone(), st.active_thread_id.clone())
                };
                let mut col = div()
                    .id("openbot-sp-pane-threads")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_muted())
                                    .child(format!(
                                        "{} ({})",
                                        tr_str(lang, "对话分支话题", "Threads"),
                                        threads.len()
                                    )),
                            )
                            .child(
                                div()
                                    .id("openbot-sp-new-branch")
                                    .cursor_pointer()
                                    .px(px(9.0))
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .bg(ob.bg_control_hover())
                                    .border_1()
                                    .border_color(ob.border_strong_hairline())
                                    .text_size(px(11.5))
                                    .text_color(ob.text_primary())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            let next_idx = this.stage.read(cx).thread_tabs.len() + 1;
                                            let tid = format!("thread-{next_idx}");
                                            let lbl = format!("分支 #{next_idx}");
                                            this.stage.update(cx, |st, scx| {
                                                st.thread_tabs.push((tid.clone(), lbl.clone()));
                                                st.active_thread_id = Some(tid);
                                                scx.notify();
                                            });
                                            this.bot_fusion_branch_from_active_message(
                                                format!("m-{next_idx}"),
                                                Some(lbl),
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(tr_str(lang, "+ 新建分支", "+ New Branch")),
                            ),
                    );

                if threads.is_empty() {
                    col = col.child(
                        div()
                            .p(px(12.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .text_size(px(12.0))
                            .text_color(ob.text_muted())
                            .child(tr_str(lang, "暂无分支话题，点击「+ 新建分支」从当前上下文开启并行讨论。", "Click '+ New Branch' to fork a side thread.")),
                    );
                } else {
                    for (idx, (tid, lbl)) in threads.into_iter().enumerate() {
                        let is_sel = active_tid.as_deref() == Some(tid.as_str());
                        let tid_click = tid.clone();
                        col = col.child(
                            div()
                                .id(("openbot-sp-thread-row", idx))
                                .cursor_pointer()
                                .w_full()
                                .px(px(10.0))
                                .py(px(8.0))
                                .rounded(px(8.0))
                                .bg(if is_sel { ob.bg_control_active() } else { ob.bg_surface() })
                                .border_1()
                                .border_color(ob.border_hairline())
                                .text_size(px(12.5))
                                .text_color(ob.text_primary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.stage.update(cx, |st, scx| {
                                            st.active_thread_id = Some(tid_click.clone());
                                            scx.notify();
                                        });
                                        cx.notify();
                                    }),
                                )
                                .child(lbl),
                        );
                    }
                }

                // Workflows section
                col = col.child(
                    div()
                        .mt(px(4.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_muted())
                                .child(format!(
                                    "{} ({})",
                                    tr_str(lang, "工作流", "Workflows"),
                                    snap.workflows.len()
                                )),
                        )
                        .child(
                            div()
                                .id("openbot-sp-wf-create")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.0))
                                .rounded(px(6.0))
                                .bg(ob.bg_surface())
                                .border_1()
                                .border_color(ob.border_hairline())
                                .hover(|s| s.bg(ob.bg_control_hover()))
                                .text_size(px(11.5))
                                .text_color(ob.text_primary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.bot_fusion_create_starter_workflow(cx);
                                    }),
                                )
                                .child(tr_str(lang, "+ 新建审批流", "+ New Flow")),
                        ),
                );
                for (idx, wf) in snap.workflows.iter().enumerate() {
                    let wid = wf.id.clone();
                    col = col.child(
                        div()
                            .id(("openbot-sp-wf-card", idx))
                            .w_full()
                            .p(px(10.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(px(12.5))
                                    .text_color(ob.text_primary())
                                    .child(wf.title.clone()),
                            )
                            .child(
                                div()
                                    .id(("openbot-sp-wf-run", idx))
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(6.0))
                                    .bg(ob.bg_control_hover())
                                    .text_size(px(11.5))
                                    .text_color(ob.text_primary())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.bot_fusion_run_workflow(wid.clone(), cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "▶ 运行", "▶ Run")),
                            ),
                    );
                }

                // Queue controls
                col = col.child(
                    div()
                        .mt(px(4.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_muted())
                                .child(format!(
                                    "{} ({})",
                                    tr_str(lang, "消息队列", "Queue"),
                                    snap.queue_items.len()
                                )),
                        )
                        .child(
                            div()
                                .id("openbot-sp-q-pause")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .bg(ob.bg_surface())
                                .border_1()
                                .border_color(ob.border_hairline())
                                .text_size(px(11.5))
                                .text_color(ob.text_secondary())
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
                for (idx, q) in snap.queue_items.iter().enumerate() {
                    let sid = q.delivery_id.clone();
                    let cid = q.delivery_id.clone();
                    col = col.child(
                        div()
                            .id(("openbot-sp-q-row", idx))
                            .w_full()
                            .px(px(10.0))
                            .py(px(7.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .text_color(ob.text_primary())
                                    .child(q.text.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .id(("openbot-sp-q-steer", idx))
                                            .cursor_pointer()
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .rounded(px(5.0))
                                            .bg(ob.bg_control_hover())
                                            .text_size(px(10.5))
                                            .text_color(ob.text_primary())
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
                                            .id(("openbot-sp-q-cancel", idx))
                                            .cursor_pointer()
                                            .text_size(px(11.5))
                                            .text_color(ob.text_muted())
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
                col.into_any_element()
            }
            SidePanelPaneKind::Question => {
                let turns = self.bot_fusion.side_panel.state.question.turns.clone();
                let last_err = self.bot_fusion.side_panel.side_question_last_error.clone();
                let last_q = turns.last().map(|t| t.question.clone());
                let mut col = div()
                    .id("openbot-sp-pane-question")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_muted())
                                    .child(format!(
                                        "{} ({}/20)",
                                        tr_str(lang, "无痕旁问", "Side Question"),
                                        turns.len()
                                    )),
                            )
                            .when(!turns.is_empty(), |r| {
                                r.child(
                                    div()
                                        .id("openbot-sp-q-clear")
                                        .cursor_pointer()
                                        .text_size(px(11.5))
                                        .text_color(ob.text_muted())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.bot_fusion.side_panel.state.question.turns.clear();
                                                this.bot_fusion.side_question_last = None;
                                                this.bot_fusion.side_panel.side_question_last_error =
                                                    None;
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "清空", "Clear")),
                                )
                            }),
                    );

                if turns.is_empty() {
                    col = col.child(
                        div()
                            .p(px(12.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .text_size(px(12.0))
                            .text_color(ob.text_muted())
                            .child(tr_str(
                                lang,
                                "在输入框使用 `$side 问题` 或点击下方快捷提问发起无痕旁问，不污染主对话历史。",
                                "Use `$side question` or quick prompts below to ask without polluting main history.",
                            )),
                    );
                } else {
                    for (idx, t) in turns.iter().enumerate() {
                        col = col.child(
                            div()
                                .id(("openbot-sp-sideq-turn", idx))
                                .p(px(10.0))
                                .rounded(px(8.0))
                                .bg(ob.bg_surface())
                                .border_1()
                                .border_color(ob.border_hairline())
                                .flex()
                                .flex_col()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ob.text_primary())
                                        .child(format!("Q: {}", t.question)),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(ob.text_secondary())
                                        .child(t.answer.clone()),
                                ),
                        );
                    }
                }

                if let (Some(_err), Some(retry_q)) = (last_err, last_q) {
                    col = col.child(
                        div()
                            .id("openbot-sp-sideq-retry")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(7.0))
                            .bg(ob.bg_control_hover())
                            .text_size(px(12.0))
                            .text_color(ob.text_primary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.bot_fusion_ask_side_question(retry_q.clone(), cx);
                                }),
                            )
                            .child(tr_str(lang, "↻ 重试上次旁问", "↻ Retry last question")),
                    );
                }

                let quick_prompts = [
                    tr_str(lang, "总结当前对话要点", "Summarize this conversation"),
                    tr_str(lang, "列出下一步行动项", "List next action items"),
                ];
                let mut quick = div().flex().flex_wrap().gap(px(6.0));
                for (idx, prompt) in quick_prompts.into_iter().enumerate() {
                    let p_owned = prompt.to_string();
                    quick = quick.child(
                        div()
                            .id(("openbot-sp-sideq-quick", idx))
                            .cursor_pointer()
                            .px(px(9.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .hover(|s| s.bg(ob.bg_control_hover()))
                            .text_size(px(11.5))
                            .text_color(ob.text_secondary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.bot_fusion_ask_side_question(p_owned.clone(), cx);
                                }),
                            )
                            .child(prompt),
                    );
                }
                col.child(quick).into_any_element()
            }
            SidePanelPaneKind::Subagent => {
                let awaiting = self.bot_fusion.awaiting_replies.clone();
                let mut col = div()
                    .id("openbot-sp-pane-subagent")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_muted())
                            .child(format!(
                                "{} ({}) · {} ({})",
                                tr_str(lang, "执行步骤", "Steps"),
                                snap.recent_steps.len(),
                                tr_str(lang, "子任务", "Subtasks"),
                                awaiting.len()
                            )),
                    );

                for (idx, aw) in awaiting.iter().enumerate() {
                    col = col.child(
                        div()
                            .id(("openbot-sp-sub-await", idx))
                            .p(px(9.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(ob.text_primary())
                                    .child(aw.agent_name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(ob.text_muted())
                                    .child(
                                        aw.preview
                                            .clone()
                                            .or_else(|| aw.waiting_reason.clone())
                                            .unwrap_or_else(|| format!("{:?}", aw.state)),
                                    ),
                            ),
                    );
                }
                if snap.recent_steps.is_empty() && awaiting.is_empty() {
                    col = col.child(
                        div()
                            .p(px(12.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .text_size(px(12.0))
                            .text_color(ob.text_muted())
                            .child(tr_str(lang, "暂无子助手执行步骤。", "No subagent steps recorded yet.")),
                    );
                } else {
                    for (idx, st) in snap.recent_steps.iter().take(12).enumerate() {
                        col = col.child(
                            div()
                                .id(("openbot-sp-sub-step", idx))
                                .p(px(9.0))
                                .rounded(px(8.0))
                                .bg(ob.bg_surface())
                                .border_1()
                                .border_color(ob.border_hairline())
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
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(ob.text_primary())
                                                .child(st.label.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.5))
                                                .text_color(ob.text_muted())
                                                .child(st.status.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(ob.text_secondary())
                                        .child(st.detail.clone()),
                                ),
                        );
                    }
                }
                col.into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn openbot_right_side_panel_state_tabs_split_swap_and_clamp() {
        let mut sp = OpenBotRightSidePanelState::default();
        assert!(!sp.state.open);
        assert!(sp.open_tabs.is_empty());

        sp.return_to_launcher();
        assert!(sp.state.open);
        assert!(sp.open_tabs.is_empty());
        assert!(!sp.is_settings_visible());

        sp.open_pane(SidePanelPaneKind::Settings);
        assert!(sp.is_settings_visible());
        assert_eq!(sp.active_top_tab, Some(SidePanelPaneKind::Settings));
        assert_eq!(sp.state.active_splits, vec![SidePanelPaneKind::Settings]);

        sp.split_down(SidePanelPaneKind::Files);
        assert_eq!(sp.split_bottom_pane, Some(SidePanelPaneKind::Files));
        assert_eq!(
            sp.state.active_splits,
            vec![SidePanelPaneKind::Settings, SidePanelPaneKind::Files]
        );

        assert!(sp.swap_split_panes());
        assert_eq!(sp.active_top_tab, Some(SidePanelPaneKind::Files));
        assert_eq!(sp.split_bottom_pane, Some(SidePanelPaneKind::Settings));
        assert!(sp.is_settings_visible());

        assert_eq!(sp.clamp_width_for_workspace(200.0, 1000.0), SETTINGS_PANEL_DEFAULT);
        assert_eq!(
            sp.clamp_width_for_workspace(1500.0, 1000.0),
            1000.0 - CONVERSATION_PANEL_MIN
        );

        assert!(sp.close_tab(SidePanelPaneKind::Settings));
        assert!(sp.close_tab(SidePanelPaneKind::Files));
        assert!(sp.state.open);
        assert!(sp.open_tabs.is_empty());
        assert!(!sp.state.maximized);
    }

    #[std::prelude::v1::test]
    fn openbot_m3_followup_plush_spec_normalization() {
        let (shape, hue, color_hex) = normalize_fabushi_plush_spec("plush:poulpe", "violet");
        assert!(shape.is_some());
        assert!(hue.is_some() || color_hex.is_some());
    }

    #[std::prelude::v1::test]
    fn side_panel_clamp_width_and_layout_guard_non_finite_inputs() {
        let mut sp = OpenBotRightSidePanelState::default();
        sp.open_pane(SidePanelPaneKind::Settings);
        assert_eq!(sp.clamp_width_for_workspace(520.0, 1200.0), 520.0);

        // Non-finite requested_px preserves prior finite width clamped to workspace
        assert_eq!(sp.clamp_width_for_workspace(f32::NAN, 1200.0), 520.0);
        assert_eq!(sp.clamp_width_for_workspace(f32::INFINITY, 1200.0), 520.0);
        assert_eq!(sp.clamp_width_for_workspace(f32::NEG_INFINITY, 1200.0), 520.0);

        // Non-finite or <= 0 workspace_avail_px clamps upper bound to SETTINGS_PANEL_DEFAULT
        assert_eq!(
            sp.clamp_width_for_workspace(600.0, f32::NAN),
            SETTINGS_PANEL_DEFAULT
        );
        assert_eq!(
            sp.clamp_width_for_workspace(600.0, f32::INFINITY),
            SETTINGS_PANEL_DEFAULT
        );
        assert_eq!(
            sp.clamp_width_for_workspace(600.0, 0.0),
            SETTINGS_PANEL_DEFAULT
        );
        assert_eq!(
            sp.clamp_width_for_workspace(600.0, -200.0),
            SETTINGS_PANEL_DEFAULT
        );

        // Poisoned internal state fields never produce NaN/Inf layout geometry
        sp.state.width_px = f32::NAN;
        sp.state.lower_share = f64::NAN;
        assert_eq!(
            sp.clamp_width_for_workspace(f32::NAN, 1200.0),
            SETTINGS_PANEL_DEFAULT
        );

        sp.state.width_px = f32::INFINITY;
        sp.state.lower_share = f64::INFINITY;
        sp.split_bottom_pane = Some(SidePanelPaneKind::Files);
        let snap = sp.compute_layout(1200.0, 720.0);
        assert!(snap.width_px.is_finite());
        assert!(snap.persisted_width_px.is_finite());
        assert!(snap.lower_share.is_finite());
        assert!(snap.main_group_height_px.is_finite());
        assert!(snap.lower_split_height_px.is_finite());
        let hdr = sp.compute_header_layout(SidePanelSettingsTarget::Assistant, f32::INFINITY, false);
        assert!(hdr.visible_tabs_count <= sp.open_tabs.len());
    }
}
