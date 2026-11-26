//! Headless-testable state & controller layer for `OrcaIdeWorkspace` (`F15`, `F17`, `F19`, `F22`).
//!
//! Binds directly to live pure-Rust `OrcaDesktopRuntime` (`OrcaIdeFacade` + `OrcaBotsFacade`):
//! - Per-workspace `WorkspaceLayoutModel` (synced with `runtime.ide().layouts()`)
//! - 103-action `KeybindingTable::default_table()`
//! - `ChatJournalStore` sessions & Chat<->Terminal flip / fork / resume
//! - 5-column Agent Kanban (`AgentStatusEngine::list_statuses()`), Worktrees + Ports,
//!   and `OrchestrationEngine` (Runs, DAG Tasks, generation-fenced Workers, 9-type Mailbox,
//!   Review/Merge/Decision Gates, Federated Hosts, Push Wakes)
//! - `PersistentDiffReviewStore`, local AI-vs-Human attribution, Commit/PR drafting, `TasksHubStore`
//! - 9-Provider Usage Roster + Multi-Account Hot-Swap + `UsageLimitContinue`
//! - `AiVaultSearchEngine` FTS5 search, fork, and cross-agent handoff
//! - 4-Tier `PluginHostV2` + `SecuritySurfaceModalState` approval + `McpConfigSync` + `TransactionalSkillInstaller`

use std::collections::BTreeMap;
use std::path::PathBuf;

use orca_core::bot_ecosystem::UsageLimitContinue;
use orca_core::orchestration_protocol::{
    DiffAnnotation, DiffSide, ExecutionHostRecord, LineAttribution, MailboxMessageType,
    MailboxPriority, MailboxRoute, MergeGate, OrchestrationTask, OrchestrationTaskStatus,
    PluginExecutionDecision, PluginHookErrorPolicy, PluginHookSpec, PluginManifestV2, PluginTier,
    ReviewGate, WorkerLivenessState,
};
use orca_core::workspace_layout::{
    ClosedTabTombstone, FolderWorkspace, KeybindingTable, SplitDirection, SplitPaneNode,
    TabContentType, TabLeafState, TabViewMode, WorkspaceLayoutModel, WorkspaceTab,
};
use orca_core::{LOCAL_EXECUTION_HOST_ID, OrcaIdeError, OrcaIdeErrorCode};
use orca_ide::{
    ACCOUNT_HOME_PROVIDERS_9, AccountHotSwapRequest, AgentSessionRecord, AgentSessionSnapshot,
    AgentSessionStatus, AgentRunState, AppendJournalItemRequest, AttributionExport,
    AutoContinueDue, CheckMailboxRequest, CodexResetCreditEntry, CommitPrDraft,
    CommitPrDraftRequest, CompleteWorkerRequest, CreateAgentSessionRequest, CreateGateRequest,
    CreateRunRequest, CreateTaskRequest, DecisionGateKind, DecisionGatePayload,
    DecisionGateRecord, DeliveryWithMessage, DiffFileChange, EnqueueSubmissionRequest,
    HibernatedTabRecord, HotSwapDecision, HubTaskRecord, InstalledPluginRecord,
    JournalItemRecord, LivenessReportOutcome, McpConfigSync, McpServerEntry, McpSyncReport,
    OrchestrationRunRecord, OrchestrationWakeEvent, PluginContributionPoint,
    PluginValidationSummary, RehydrationPlan, ResolveGateAction, SendMessageRequest,
    SkillInstallRecord, SkillRecoveryReport, StartWorkerRequest, StartWorkerResult, TaskForge,
    TasksHubFilter, UsageRosterEntry, VaultSearchHit, VaultSearchRequest, VaultSearchResponse,
    WorktreePortAttribution, WorktreeRecord, WorkerTerminalArchive, compose_commit_and_pr_draft,
    parse_mailbox_target,
};

use super::runtime::OrcaDesktopRuntime;
use super::security_surface::SecuritySurfaceModalState;

/// Actions emitted upward by `OrcaIdeWorkspace` to `AppShell`.
#[derive(Debug, Clone)]
pub enum OrcaIdeAction {
    OpenBotsWorkspace,
    OpenBrowserDesignMode,
    ShowToast(String),
    SendPromptToActiveChat(String),
    OpenJumpPalette,
    OpenPopoutKanban,
    OpenSecurityModal(SecuritySurfaceModalState),
    StatusBarSummaryChanged {
        warning_count: usize,
        active_agents: usize,
    },
}

/// Active right-hand slide-over drawer in `OrcaIdeWorkspace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OrcaIdeDrawer {
    #[default]
    None,
    Kanban,
    DiffTasks,
    UsagePopover,
    AiVault,
    Plugins,
}

/// Active sub-tab inside the Orchestration & Kanban drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OrchestrationBoardTab {
    #[default]
    Kanban,
    RunsDag,
    Workers,
    Mailbox,
    Gates,
    Hosts,
}

/// Active sub-tab inside the Diff / Attribution / Commit / Tasks Hub drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffTasksSubTab {
    #[default]
    AnnotateDiff,
    CommitPr,
    TasksHub,
}

/// Diff display layout mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffDisplayMode {
    #[default]
    SideBySide,
    Unified,
}

/// Focused text-input field across all Orca IDE panels (for GPUI IME & keydown routing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdeInputTarget {
    #[default]
    None,
    SessionPromptDraft,
    NewRunTitle,
    NewTaskTitle,
    NewTaskDeps,
    MailboxRecipient,
    MailboxBody,
    GatePrompt,
    DiffFilePath,
    DiffLineStart,
    DiffLineEnd,
    DiffAnnotationBody,
    CommitSummary,
    CommitScope,
    CommitParentPr,
    TaskHubSearch,
    TaskHubExternalKey,
    TaskHubTitle,
    TaskHubArtifactUrl,
    TaskHubCommandTemplate,
    ClaudeStatusLine,
    HotSwapTargetAccount,
    VaultQuery,
    VaultHandoffTargetAgent,
    PluginIdDraft,
    PluginPublisherDraft,
    PluginVersionDraft,
    McpServerName,
    McpServerCommand,
    McpTargetConfigPath,
    SkillNameDraft,
    SkillSourceDirDraft,
}

/// Detached Pop-out Kanban window state synchronized with `OrcaIdeSurfaceState`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PopoutKanbanWindowState {
    pub is_open: bool,
    pub selected_run_id: Option<String>,
    pub last_synced_ms: i64,
}

/// Complete headless-testable state model for `OrcaIdeWorkspace`.
#[derive(Debug, Clone)]
pub struct OrcaIdeSurfaceState {
    // Workspace, Layout & Keybindings
    pub active_workspace_id: String,
    pub layout: WorkspaceLayoutModel,
    pub keybindings: KeybindingTable,
    pub active_drawer: OrcaIdeDrawer,
    pub active_input: IdeInputTarget,
    pub popout_kanban: PopoutKanbanWindowState,
    pub status_banner: Option<String>,
    pub error_banner: Option<String>,
    pub active_cli_owner_banner: Option<String>,
    pub last_rehydration_plan: Option<RehydrationPlan>,
    pub hibernated_tabs: Vec<HibernatedTabRecord>,

    // Chat Journal & Chat<->Terminal Sessions
    pub tab_session_bindings: BTreeMap<String, String>,
    pub sessions: Vec<AgentSessionRecord>,
    pub active_session_snapshot: Option<AgentSessionSnapshot>,
    pub active_session_items: Vec<JournalItemRecord>,
    pub session_prompt_draft: String,

    // Kanban, Worktrees, Ports & Orchestration
    pub board_tab: OrchestrationBoardTab,
    pub agent_statuses: Vec<AgentSessionStatus>,
    pub worktrees: Vec<WorktreeRecord>,
    pub folder_workspaces: Vec<FolderWorkspace>,
    pub worktree_ports: Vec<WorktreePortAttribution>,
    pub runs: Vec<OrchestrationRunRecord>,
    pub selected_run_id: Option<String>,
    pub ready_tasks: Vec<OrchestrationTask>,
    pub tracked_tasks: Vec<OrchestrationTask>,
    pub tracked_workers: Vec<StartWorkerResult>,
    pub worker_liveness_outcomes: BTreeMap<String, LivenessReportOutcome>,
    pub worker_archives: BTreeMap<String, Vec<WorkerTerminalArchive>>,
    pub mailbox_deliveries: Vec<DeliveryWithMessage>,
    pub mailbox_recipient_id: String,
    pub tracked_gates: Vec<DecisionGateRecord>,
    pub hosts: Vec<ExecutionHostRecord>,
    pub recent_wakes: Vec<OrchestrationWakeEvent>,

    // Orchestration Drafts
    pub new_run_title: String,
    pub new_task_title: String,
    pub new_task_deps: String,
    pub selected_worker_agent: String,
    pub mailbox_target_input: String,
    pub mailbox_type_selected: MailboxMessageType,
    pub mailbox_priority_selected: MailboxPriority,
    pub mailbox_body_draft: String,
    pub gate_kind_selected: DecisionGateKind,
    pub gate_prompt_draft: String,

    // Diff Review, Local AI Attribution, Commit/PR & Tasks Hub
    pub diff_tab: DiffTasksSubTab,
    pub diff_display_mode: DiffDisplayMode,
    pub show_ai_attribution: bool,
    pub include_resolved_annotations: bool,
    pub diff_file_path: String,
    pub diff_side: DiffSide,
    pub diff_line_start_draft: String,
    pub diff_line_end_draft: String,
    pub diff_annotation_body_draft: String,
    pub diff_annotations: Vec<DiffAnnotation>,
    pub file_attributions: BTreeMap<u32, LineAttribution>,
    pub attribution_export: Option<AttributionExport>,
    pub last_batch_prompt: Option<String>,
    pub commit_type: String,
    pub commit_scope: String,
    pub commit_summary: String,
    pub commit_base_ref: String,
    pub commit_head_branch: String,
    pub commit_parent_pr: String,
    pub last_commit_pr_draft: Option<CommitPrDraft>,
    pub tasks_hub_forge_filter: Option<TaskForge>,
    pub tasks_hub_search: String,
    pub hub_tasks: Vec<HubTaskRecord>,
    pub new_hub_task_forge: TaskForge,
    pub new_hub_task_key: String,
    pub new_hub_task_title: String,
    pub new_hub_task_url: String,
    pub issue_command_template: String,
    pub last_expanded_launch_cmd: Option<String>,

    // 9-Provider Usage Popover & Multi-Account Hot-Swap
    pub usage_roster: Vec<UsageRosterEntry>,
    pub usage_show_remaining: bool,
    pub claude_status_line_draft: String,
    pub hotswap_provider: String,
    pub hotswap_target_account: String,
    pub hotswap_turn_in_progress: bool,
    pub last_hotswap_decision: Option<HotSwapDecision>,
    pub active_accounts_by_provider: BTreeMap<String, String>,
    pub codex_reset_credits: Vec<CodexResetCreditEntry>,
    pub usage_limit_pauses: Vec<(String, UsageLimitContinue)>,
    pub due_auto_continues: Vec<AutoContinueDue>,

    // AI Vault FTS5 Search
    pub vault_query: String,
    pub vault_workspace_filter: Option<String>,
    pub vault_agent_filter: Option<String>,
    pub vault_response: Option<VaultSearchResponse>,
    pub vault_accumulated_hits: Vec<VaultSearchHit>,
    pub vault_handoff_target_agent: String,
    pub last_vault_handoff_prompt: Option<String>,
    pub last_vault_fork_session_id: Option<String>,

    // Plugin System V2 + MCP Sync + Transactional Skills
    pub plugins_enabled: bool,
    pub installed_plugins: Vec<InstalledPluginRecord>,
    pub plugin_decisions: BTreeMap<String, Result<PluginExecutionDecision, String>>,
    pub plugin_id_draft: String,
    pub plugin_publisher_draft: String,
    pub plugin_version_draft: String,
    pub plugin_tier_draft: PluginTier,
    pub plugin_selected_contributions: Vec<PluginContributionPoint>,
    pub plugin_hook_error_policy: PluginHookErrorPolicy,
    pub last_plugin_validation: Option<PluginValidationSummary>,
    pub mcp_server_name_draft: String,
    pub mcp_server_cmd_draft: String,
    pub mcp_target_path_draft: String,
    pub mcp_strip_secrets: bool,
    pub last_mcp_sync_report: Option<McpSyncReport>,
    pub skill_name_draft: String,
    pub skill_source_dir_draft: String,
    pub skill_install_records: Vec<SkillInstallRecord>,
    pub last_skill_recovery: Option<SkillRecoveryReport>,

    next_counter: u64,
}

impl Default for OrcaIdeSurfaceState {
    fn default() -> Self {
        Self {
            active_workspace_id: "default".to_string(),
            layout: WorkspaceLayoutModel::new("default", LOCAL_EXECUTION_HOST_ID, "main"),
            keybindings: KeybindingTable::default_table(),
            active_drawer: OrcaIdeDrawer::None,
            active_input: IdeInputTarget::None,
            popout_kanban: PopoutKanbanWindowState::default(),
            status_banner: None,
            error_banner: None,
            active_cli_owner_banner: None,
            last_rehydration_plan: None,
            hibernated_tabs: Vec::new(),
            tab_session_bindings: BTreeMap::new(),
            sessions: Vec::new(),
            active_session_snapshot: None,
            active_session_items: Vec::new(),
            session_prompt_draft: String::new(),
            board_tab: OrchestrationBoardTab::Kanban,
            agent_statuses: Vec::new(),
            worktrees: Vec::new(),
            folder_workspaces: Vec::new(),
            worktree_ports: Vec::new(),
            runs: Vec::new(),
            selected_run_id: None,
            ready_tasks: Vec::new(),
            tracked_tasks: Vec::new(),
            tracked_workers: Vec::new(),
            worker_liveness_outcomes: BTreeMap::new(),
            worker_archives: BTreeMap::new(),
            mailbox_deliveries: Vec::new(),
            mailbox_recipient_id: "coordinator".to_string(),
            tracked_gates: Vec::new(),
            hosts: Vec::new(),
            recent_wakes: Vec::new(),
            new_run_title: String::new(),
            new_task_title: String::new(),
            new_task_deps: String::new(),
            selected_worker_agent: "claude".to_string(),
            mailbox_target_input: "@all".to_string(),
            mailbox_type_selected: MailboxMessageType::Status,
            mailbox_priority_selected: MailboxPriority::Normal,
            mailbox_body_draft: String::new(),
            gate_kind_selected: DecisionGateKind::Review,
            gate_prompt_draft: String::new(),
            diff_tab: DiffTasksSubTab::AnnotateDiff,
            diff_display_mode: DiffDisplayMode::SideBySide,
            show_ai_attribution: true,
            include_resolved_annotations: false,
            diff_file_path: "src/main.rs".to_string(),
            diff_side: DiffSide::Right,
            diff_line_start_draft: "1".to_string(),
            diff_line_end_draft: "1".to_string(),
            diff_annotation_body_draft: String::new(),
            diff_annotations: Vec::new(),
            file_attributions: BTreeMap::new(),
            attribution_export: None,
            last_batch_prompt: None,
            commit_type: "feat".to_string(),
            commit_scope: "orca".to_string(),
            commit_summary: String::new(),
            commit_base_ref: "main".to_string(),
            commit_head_branch: "HEAD".to_string(),
            commit_parent_pr: String::new(),
            last_commit_pr_draft: None,
            tasks_hub_forge_filter: None,
            tasks_hub_search: String::new(),
            hub_tasks: Vec::new(),
            new_hub_task_forge: TaskForge::GitHub,
            new_hub_task_key: String::new(),
            new_hub_task_title: String::new(),
            new_hub_task_url: String::new(),
            issue_command_template: "orca work --issue {{artifact_url}}".to_string(),
            last_expanded_launch_cmd: None,
            usage_roster: Vec::new(),
            usage_show_remaining: false,
            claude_status_line_draft: String::new(),
            hotswap_provider: "claude".to_string(),
            hotswap_target_account: String::new(),
            hotswap_turn_in_progress: false,
            last_hotswap_decision: None,
            active_accounts_by_provider: BTreeMap::new(),
            codex_reset_credits: Vec::new(),
            usage_limit_pauses: Vec::new(),
            due_auto_continues: Vec::new(),
            vault_query: String::new(),
            vault_workspace_filter: None,
            vault_agent_filter: None,
            vault_response: None,
            vault_accumulated_hits: Vec::new(),
            vault_handoff_target_agent: "codex".to_string(),
            last_vault_handoff_prompt: None,
            last_vault_fork_session_id: None,
            plugins_enabled: true,
            installed_plugins: Vec::new(),
            plugin_decisions: BTreeMap::new(),
            plugin_id_draft: String::new(),
            plugin_publisher_draft: String::new(),
            plugin_version_draft: "1.0.0".to_string(),
            plugin_tier_draft: PluginTier::Declarative,
            plugin_selected_contributions: vec![PluginContributionPoint::Commands],
            plugin_hook_error_policy: PluginHookErrorPolicy::Deny,
            last_plugin_validation: None,
            mcp_server_name_draft: String::new(),
            mcp_server_cmd_draft: String::new(),
            mcp_target_path_draft: String::new(),
            mcp_strip_secrets: true,
            last_mcp_sync_report: None,
            skill_name_draft: String::new(),
            skill_source_dir_draft: String::new(),
            skill_install_records: Vec::new(),
            last_skill_recovery: None,
            next_counter: 1,
        }
    }
}

impl OrcaIdeSurfaceState {
    fn next_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}-{}", self.next_counter);
        self.next_counter = self.next_counter.saturating_add(1);
        id
    }

    /// Synchronizes local `self.layout` into `runtime.ide().layouts()`.
    pub fn persist_layout_to_runtime(&self, runtime: &OrcaDesktopRuntime) {
        let mut layouts = runtime.ide().layouts().write();
        layouts.insert(self.active_workspace_id.clone(), self.layout.clone());
    }

    /// Loads or initializes the active workspace layout from `runtime.ide().layouts()`.
    pub fn load_layout_from_runtime(&mut self, runtime: &OrcaDesktopRuntime) {
        let mut layouts = runtime.ide().layouts().write();
        let entry = layouts
            .entry(self.active_workspace_id.clone())
            .or_insert_with(|| {
                WorkspaceLayoutModel::new(
                    self.active_workspace_id.clone(),
                    runtime.ide().config().host_id.clone(),
                    "main",
                )
            });
        self.layout = entry.clone();
    }

    /// Full refresh of all IDE subsystems directly from live `OrcaDesktopRuntime` stores.
    pub fn refresh_all(&mut self, runtime: &OrcaDesktopRuntime, now_ms: i64) -> Result<(), String> {
        self.load_layout_from_runtime(runtime);
        self.refresh_sessions(runtime)?;
        self.refresh_kanban_and_orchestration(runtime)?;
        self.refresh_diff_and_tasks(runtime)?;
        let _ = self.refresh_usage_roster(runtime, now_ms);
        self.refresh_plugins(runtime);
        self.hibernated_tabs = runtime.ide().hibernation().list_hibernated();
        Ok(())
    }

    /// Returns the currently focused tab in `self.layout`, if any.
    pub fn active_tab(&self) -> Option<&WorkspaceTab> {
        fn find_focused_tab<'a>(
            node: &'a SplitPaneNode,
            focused_pane_id: &str,
        ) -> Option<&'a WorkspaceTab> {
            match node {
                SplitPaneNode::Leaf {
                    pane_id,
                    tabs,
                    active_tab_id,
                } if pane_id == focused_pane_id => active_tab_id
                    .as_deref()
                    .and_then(|id| tabs.iter().find(|t| t.tab_id == id))
                    .or_else(|| tabs.first()),
                SplitPaneNode::Leaf { .. } => None,
                SplitPaneNode::Split { first, second, .. } => {
                    find_focused_tab(first, focused_pane_id)
                        .or_else(|| find_focused_tab(second, focused_pane_id))
                }
            }
        }
        find_focused_tab(&self.layout.root, &self.layout.focused_pane_id)
    }

    /// Collects all open tabs across the entire split tree with `(pane_id, tab, is_active_in_pane)`.
    pub fn all_open_tabs(&self) -> Vec<(String, WorkspaceTab, bool)> {
        fn walk(node: &SplitPaneNode, out: &mut Vec<(String, WorkspaceTab, bool)>) {
            match node {
                SplitPaneNode::Leaf {
                    pane_id,
                    tabs,
                    active_tab_id,
                } => {
                    for tab in tabs {
                        let is_active = active_tab_id.as_deref() == Some(tab.tab_id.as_str());
                        out.push((pane_id.clone(), tab.clone(), is_active));
                    }
                }
                SplitPaneNode::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.layout.root, &mut out);
        out
    }

    /// Builds a fresh `WorkspaceTab` of the given `TabContentType` (supports all 10 variants).
    pub fn make_tab(&mut self, content_type: TabContentType, now_ms: i64) -> WorkspaceTab {
        let tab_id = self.next_id(content_type.as_str());
        let traits = content_type.traits();
        let view_mode = match content_type {
            TabContentType::AgentSession => TabViewMode::Chat,
            _ => TabViewMode::Terminal,
        };
        let pty_incarnation_id = if traits.requires_pty_tombstone {
            Some(self.next_id("pty-inc"))
        } else {
            None
        };
        WorkspaceTab {
            tab_id: tab_id.clone(),
            content_type,
            title: format!("{} · {}", traits.label, tab_id),
            pinned: false,
            preview: false,
            view_mode,
            leaf_state: TabLeafState::Live,
            pty_incarnation_id,
            dirty: false,
            created_at_ms: now_ms,
            last_active_at_ms: now_ms,
        }
    }

    // -----------------------------------------------------------------------
    // Explorer 3 §7 Required Headless Methods: Split Anything & Chat<->Terminal
    // -----------------------------------------------------------------------

    pub fn open_tab(
        &mut self,
        pane_id: &str,
        tab: WorkspaceTab,
        now_ms: i64,
    ) -> Result<(), OrcaIdeError> {
        self.layout.open_tab(pane_id, tab, now_ms)
    }

    pub fn open_tab_and_sync(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        pane_id: &str,
        tab: WorkspaceTab,
        now_ms: i64,
    ) -> Result<(), OrcaIdeError> {
        let tab_id = tab.tab_id.clone();
        let content_type = tab.content_type;
        let view_mode = tab.view_mode;
        self.open_tab(pane_id, tab, now_ms)?;
        if content_type.traits().supports_dual_view {
            if let Ok(snap) = runtime.ide().chat_journal().create_session(
                CreateAgentSessionRequest {
                    session_id: Some(format!("sess-{tab_id}")),
                    workspace_id: self.active_workspace_id.clone(),
                    host_id: Some(self.layout.host_id.clone()),
                    agent_id: self.selected_worker_agent.clone(),
                    view_mode,
                    active_cli_owner: false,
                    provider_handle_chain: Vec::new(),
                    adopted_native_id: None,
                    now_ms: Some(now_ms),
                },
            ) {
                self.tab_session_bindings
                    .insert(tab_id, snap.session.session_id.clone());
                self.active_session_snapshot = Some(snap);
            }
            let _ = self.refresh_sessions(runtime);
        }
        self.persist_layout_to_runtime(runtime);
        Ok(())
    }

    pub fn split_pane(
        &mut self,
        pane_id: &str,
        new_pane_id: &str,
        dir: SplitDirection,
        ratio_bp: u16,
        initial_tab: Option<WorkspaceTab>,
    ) -> Result<(), OrcaIdeError> {
        self.layout
            .split_pane(pane_id, new_pane_id, dir, ratio_bp, initial_tab)
    }

    pub fn split_pane_and_sync(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        pane_id: &str,
        dir: SplitDirection,
        content_type: TabContentType,
        now_ms: i64,
    ) -> Result<String, OrcaIdeError> {
        let new_pane_id = self.next_id("pane");
        let tab = self.make_tab(content_type, now_ms);
        self.split_pane(pane_id, &new_pane_id, dir, 5_000, Some(tab))?;
        self.persist_layout_to_runtime(runtime);
        Ok(new_pane_id)
    }

    pub fn toggle_pin_boundary(
        &mut self,
        branch_path: &[bool],
        ratio_bp: u16,
        pinned: bool,
    ) -> Result<(), OrcaIdeError> {
        self.layout
            .set_split_ratio(branch_path, ratio_bp, Some(pinned))
    }

    pub fn rebalance_splits_and_sync(&mut self, runtime: &OrcaDesktopRuntime) {
        self.layout.rebalance_splits();
        self.persist_layout_to_runtime(runtime);
    }

    pub fn close_and_tombstone_tab(
        &mut self,
        tab_id: &str,
        now_ms: i64,
    ) -> Result<ClosedTabTombstone, OrcaIdeError> {
        self.layout.close_tab(tab_id, now_ms)
    }

    pub fn reopen_last_closed_tab(&mut self, now_ms: i64) -> Result<WorkspaceTab, OrcaIdeError> {
        self.layout.reopen_closed_tab(None, now_ms)
    }

    /// Flips `TabViewMode::Terminal <-> TabViewMode::Chat` in-place on `tab_id`
    /// (`Cmd+Shift+J`) and mirrors the flip into `ChatJournalStore` preserving
    /// `provider_handle_chain`.
    pub fn flip_chat_terminal(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        tab_id: &str,
        session_id: Option<&str>,
        now_ms: i64,
    ) -> Result<TabViewMode, String> {
        let next_mode = self
            .layout
            .toggle_tab_view_mode(tab_id)
            .map_err(|e| e.to_string())?;
        self.persist_layout_to_runtime(runtime);

        let bound_session = session_id
            .map(ToOwned::to_owned)
            .or_else(|| self.tab_session_bindings.get(tab_id).cloned());

        if let Some(sid) = bound_session {
            let snap = runtime
                .ide()
                .chat_journal()
                .toggle_view_mode(&sid, None, Some(now_ms))
                .map_err(|e| e.to_string())?;
            self.active_session_snapshot = Some(snap);
            self.active_cli_owner_banner = None;
            let _ = self.refresh_sessions(runtime);
        }
        Ok(next_mode)
    }

    pub fn refresh_sessions(&mut self, runtime: &OrcaDesktopRuntime) -> Result<(), String> {
        let sessions = runtime
            .ide()
            .chat_journal()
            .list_sessions(Some(&self.active_workspace_id))
            .map_err(|e| e.to_string())?;
        self.sessions = sessions;
        let active_sid = self
            .active_tab()
            .and_then(|t| self.tab_session_bindings.get(&t.tab_id).cloned())
            .or_else(|| {
                self.active_session_snapshot
                    .as_ref()
                    .map(|s| s.session.session_id.clone())
            })
            .or_else(|| self.sessions.first().map(|s| s.session_id.clone()));

        if let Some(sid) = active_sid {
            if let Ok(snap) = runtime.ide().chat_journal().get_session(&sid) {
                self.active_session_items = runtime
                    .ide()
                    .chat_journal()
                    .list_items(&sid, false)
                    .unwrap_or_default();
                self.active_session_snapshot = Some(snap);
            }
        }
        Ok(())
    }

    /// Ensures the active dual-view tab has a bound `ChatJournalStore` session and returns its ID.
    pub fn ensure_session_for_tab(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        tab_id: &str,
        now_ms: i64,
    ) -> Result<String, String> {
        if let Some(sid) = self.tab_session_bindings.get(tab_id) {
            return Ok(sid.clone());
        }
        let view_mode = self
            .layout
            .find_tab(tab_id)
            .map(|t| t.view_mode)
            .unwrap_or(TabViewMode::Chat);
        let snap = runtime
            .ide()
            .chat_journal()
            .create_session(CreateAgentSessionRequest {
                session_id: Some(format!("sess-{tab_id}")),
                workspace_id: self.active_workspace_id.clone(),
                host_id: Some(self.layout.host_id.clone()),
                agent_id: self.selected_worker_agent.clone(),
                view_mode,
                active_cli_owner: false,
                provider_handle_chain: Vec::new(),
                adopted_native_id: None,
                now_ms: Some(now_ms),
            })
            .map_err(|e| e.to_string())?;
        let sid = snap.session.session_id.clone();
        self.tab_session_bindings
            .insert(tab_id.to_string(), sid.clone());
        self.active_session_snapshot = Some(snap);
        let _ = self.refresh_sessions(runtime);
        Ok(sid)
    }

    pub fn fork_session_into_tab(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        source_session_id: &str,
        now_ms: i64,
    ) -> Result<AgentSessionSnapshot, String> {
        let forked = runtime
            .ide()
            .chat_journal()
            .fork_session(source_session_id, None, None, Some(now_ms))
            .map_err(|e| e.to_string())?;
        let mut tab = self.make_tab(TabContentType::AgentSession, now_ms);
        tab.view_mode = forked.session.view_mode;
        tab.title = format!("Fork · {}", forked.session.session_id);
        let pane_id = self.layout.focused_pane_id.clone();
        let tab_id = tab.tab_id.clone();
        self.layout
            .open_tab(&pane_id, tab, now_ms)
            .map_err(|e| e.to_string())?;
        self.tab_session_bindings
            .insert(tab_id, forked.session.session_id.clone());
        self.active_session_snapshot = Some(forked.clone());
        self.persist_layout_to_runtime(runtime);
        let _ = self.refresh_sessions(runtime);
        Ok(forked)
    }

    pub fn resume_session_in_new_tab(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        source_session_id: &str,
        target_mode: TabViewMode,
        now_ms: i64,
    ) -> Result<AgentSessionSnapshot, String> {
        let resumed = match runtime.ide().chat_journal().resume_in_new_session(
            source_session_id,
            target_mode,
            None,
            Some(now_ms),
        ) {
            Ok(snap) => {
                self.active_cli_owner_banner = None;
                snap
            }
            Err(err) => {
                let msg = format!("{}: {err}", err.code_str());
                if err.code_str() == "active_cli_owner" {
                    self.active_cli_owner_banner = Some(msg.clone());
                }
                return Err(msg);
            }
        };
        let content_type = match target_mode {
            TabViewMode::Chat => TabContentType::AgentSession,
            TabViewMode::Terminal => TabContentType::Terminal,
        };
        let mut tab = self.make_tab(content_type, now_ms);
        tab.view_mode = target_mode;
        tab.title = format!("Resume · {}", resumed.session.session_id);
        let pane_id = self.layout.focused_pane_id.clone();
        let tab_id = tab.tab_id.clone();
        self.layout
            .open_tab(&pane_id, tab, now_ms)
            .map_err(|e| e.to_string())?;
        self.tab_session_bindings
            .insert(tab_id, resumed.session.session_id.clone());
        self.active_session_snapshot = Some(resumed.clone());
        self.persist_layout_to_runtime(runtime);
        let _ = self.refresh_sessions(runtime);
        Ok(resumed)
    }

    pub fn set_session_cli_owner(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        session_id: &str,
        active: bool,
        now_ms: i64,
    ) -> Result<AgentSessionRecord, String> {
        let rec = runtime
            .ide()
            .chat_journal()
            .set_active_cli_owner(session_id, active, Some(now_ms))
            .map_err(|e| e.to_string())?;
        if !active {
            self.active_cli_owner_banner = None;
        }
        let _ = self.refresh_sessions(runtime);
        Ok(rec)
    }

    pub fn submit_session_prompt(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        session_id: &str,
        prompt: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return Err("prompt cannot be empty".to_string());
        }
        let op_id = self.next_id("op");
        let _ = runtime
            .ide()
            .chat_journal()
            .save_draft(session_id, trimmed, Some(now_ms));
        let (attempt, _) = runtime
            .ide()
            .chat_journal()
            .enqueue_submission(EnqueueSubmissionRequest {
                session_id: session_id.to_string(),
                operation_id: op_id,
                caller_key: "desktop-ide".to_string(),
                prompt_text: trimmed.to_string(),
                attachments: Vec::new(),
                now_ms: Some(now_ms),
            })
            .map_err(|e| e.to_string())?;
        let _ = runtime.ide().chat_journal().append_item(AppendJournalItemRequest {
            session_id: session_id.to_string(),
            item_id: None,
            item_kind: "user_prompt".to_string(),
            provider_item_id: None,
            payload_json: serde_json::json!({ "text": trimmed }),
            now_ms: Some(now_ms),
        });
        let _ = runtime
            .ide()
            .chat_journal()
            .complete_submission(session_id, &attempt.attempt_id, Some(now_ms));
        self.session_prompt_draft.clear();
        let _ = self.refresh_sessions(runtime);
        Ok(())
    }

    /// Wakes a sleeping/hibernated tab via `HibernationController::build_rehydration_plan`
    /// (`Sleeping -> Resume -> Live`) and persists the updated layout.
    pub fn wake_hibernated_tab(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        tab_id: &str,
        now_ms: i64,
    ) -> Result<RehydrationPlan, String> {
        fn find_tab_mut<'a>(
            node: &'a mut SplitPaneNode,
            tab_id: &str,
        ) -> Option<&'a mut WorkspaceTab> {
            match node {
                SplitPaneNode::Leaf { tabs, .. } => tabs.iter_mut().find(|t| t.tab_id == tab_id),
                SplitPaneNode::Split { first, second, .. } => {
                    find_tab_mut(first, tab_id).or_else(|| find_tab_mut(second, tab_id))
                }
            }
        }
        let tab = find_tab_mut(&mut self.layout.root, tab_id).ok_or_else(|| {
            OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                format!("tab '{tab_id}' not found"),
            )
            .to_string()
        })?;
        let plan = runtime
            .ide()
            .hibernation()
            .build_rehydration_plan(tab, now_ms)
            .map_err(|e| e.to_string())?;
        self.last_rehydration_plan = Some(plan.clone());
        self.hibernated_tabs = runtime.ide().hibernation().list_hibernated();
        self.persist_layout_to_runtime(runtime);
        Ok(plan)
    }

    // -----------------------------------------------------------------------
    // Explorer 3 §7 Required Headless Methods: Kanban, Worktrees & Orchestration
    // -----------------------------------------------------------------------

    pub fn refresh_kanban_and_orchestration(
        &mut self,
        runtime: &OrcaDesktopRuntime,
    ) -> Result<(), String> {
        let ide = runtime.ide();
        self.agent_statuses = ide.agent_status().list_statuses();
        self.worktrees = ide
            .worktrees()
            .list_worktrees()
            .map_err(|e| e.to_string())?;
        self.folder_workspaces = ide
            .worktrees()
            .list_folder_workspaces()
            .map_err(|e| e.to_string())?;

        let wt_pairs: Vec<(String, PathBuf)> = self
            .worktrees
            .iter()
            .map(|w| (w.branch_name.clone(), PathBuf::from(&w.worktree_path)))
            .collect();
        self.worktree_ports = ide
            .port_scanner()
            .attribute_ports_to_worktrees(&wt_pairs);

        self.runs = ide
            .orchestration()
            .list_runs(None)
            .map_err(|e| e.to_string())?;
        if self.selected_run_id.is_none() {
            self.selected_run_id = self.runs.first().map(|r| r.run_id.clone());
        }
        if let Some(ref run_id) = self.selected_run_id {
            self.ready_tasks = ide
                .orchestration()
                .ready_tasks(run_id)
                .unwrap_or_default();
        } else {
            self.ready_tasks.clear();
        }
        self.hosts = ide
            .orchestration()
            .list_hosts()
            .map_err(|e| e.to_string())?;
        self.mailbox_deliveries = ide
            .orchestration()
            .check_mailbox(CheckMailboxRequest {
                recipient_id: self.mailbox_recipient_id.clone(),
                dispatch_id: None,
                expected_generation: None,
                now_ms: None,
            })
            .unwrap_or_default();
        self.popout_kanban.selected_run_id = self.selected_run_id.clone();
        Ok(())
    }

    /// Groups live `AgentStatusEngine::list_statuses()` into the exact 5 Kanban columns:
    /// `Working | Blocked | Waiting | Done | Idle`.
    pub fn kanban_columns(&self) -> [(AgentRunState, Vec<AgentSessionStatus>); 5] {
        let mut working = Vec::new();
        let mut blocked = Vec::new();
        let mut waiting = Vec::new();
        let mut done = Vec::new();
        let mut idle = Vec::new();
        for status in &self.agent_statuses {
            match status.state {
                AgentRunState::Working => working.push(status.clone()),
                AgentRunState::Blocked => blocked.push(status.clone()),
                AgentRunState::Waiting => waiting.push(status.clone()),
                AgentRunState::Done => done.push(status.clone()),
                AgentRunState::Idle => idle.push(status.clone()),
            }
        }
        [
            (AgentRunState::Working, working),
            (AgentRunState::Blocked, blocked),
            (AgentRunState::Waiting, waiting),
            (AgentRunState::Done, done),
            (AgentRunState::Idle, idle),
        ]
    }

    /// Toggles Pop-out Kanban window state and returns the new `is_open` flag.
    pub fn toggle_popout_kanban(&mut self) -> bool {
        self.popout_kanban.is_open = !self.popout_kanban.is_open;
        self.popout_kanban.is_open
    }

    /// Subscribes to push-driven `OrchestrationEngine::subscribe_wakes()` broadcast events (zero polling).
    pub fn subscribe_orchestration_wakes(
        &self,
        runtime: &OrcaDesktopRuntime,
    ) -> tokio::sync::broadcast::Receiver<OrchestrationWakeEvent> {
        runtime.ide().orchestration().subscribe_wakes()
    }

    /// Records a push-wake notification into `self.recent_wakes` (capped to newest 32 events).
    pub fn record_wake_event(&mut self, event: OrchestrationWakeEvent) {
        self.recent_wakes.push(event);
        if self.recent_wakes.len() > 32 {
            let overflow = self.recent_wakes.len() - 32;
            self.recent_wakes.drain(0..overflow);
        }
    }

    pub fn create_orchestration_run(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        title: &str,
        now_ms: i64,
    ) -> Result<OrchestrationRunRecord, String> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err("run title cannot be empty".to_string());
        }
        let run = runtime
            .ide()
            .orchestration()
            .create_run(CreateRunRequest {
                run_id: None,
                title: trimmed.to_string(),
                coordinator_orca_session_id: self
                    .active_session_snapshot
                    .as_ref()
                    .map(|s| s.session.session_id.clone()),
                mailbox_is_local_only: true,
                now_ms: Some(now_ms),
            })
            .map_err(|e| e.to_string())?;
        self.selected_run_id = Some(run.run_id.clone());
        self.new_run_title.clear();
        let _ = self.refresh_kanban_and_orchestration(runtime);
        Ok(run)
    }

    pub fn create_orchestration_task(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        run_id: &str,
        title: &str,
        depends_on: Vec<String>,
        now_ms: i64,
    ) -> Result<OrchestrationTask, String> {
        let task = runtime
            .ide()
            .orchestration()
            .create_task(CreateTaskRequest {
                run_id: run_id.to_string(),
                task_id: None,
                title: title.trim().to_string(),
                depends_on,
                parent_id: None,
                nesting_depth: Some(1),
                now_ms: Some(now_ms),
            })
            .map_err(|e| e.to_string())?;
        self.tracked_tasks.push(task.clone());
        self.new_task_title.clear();
        self.new_task_deps.clear();
        let _ = self.refresh_kanban_and_orchestration(runtime);
        Ok(task)
    }

    pub fn transition_orchestration_task(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        run_id: &str,
        task_id: &str,
        next: OrchestrationTaskStatus,
        now_ms: i64,
    ) -> Result<OrchestrationTask, String> {
        let (updated, _) = runtime
            .ide()
            .orchestration()
            .transition_task(run_id, task_id, next, Some(now_ms))
            .map_err(|e| e.to_string())?;
        if let Some(existing) = self
            .tracked_tasks
            .iter_mut()
            .find(|t| t.task_id == updated.task_id)
        {
            *existing = updated.clone();
        } else {
            self.tracked_tasks.push(updated.clone());
        }
        let _ = self.refresh_kanban_and_orchestration(runtime);
        Ok(updated)
    }

    pub fn start_orchestration_worker(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        run_id: &str,
        task_id: &str,
        agent_id: &str,
        now_ms: i64,
    ) -> Result<StartWorkerResult, String> {
        let started = runtime
            .ide()
            .orchestration()
            .start_worker(StartWorkerRequest {
                run_id: run_id.to_string(),
                task_id: task_id.to_string(),
                dispatch_id: None,
                host_id: Some(self.layout.host_id.clone()),
                agent_id: agent_id.to_string(),
                worktree_id: self.worktrees.first().map(|w| w.id.clone()),
                depth: Some(1),
                capability_manifest: None,
                launch_token: None,
                follow_up_of_dispatch_id: None,
                keep_open: false,
                tab_id: self.active_tab().map(|t| t.tab_id.clone()),
                pty_incarnation_id: None,
                now_ms: Some(now_ms),
            })
            .map_err(|e| e.to_string())?;
        self.tracked_workers.push(started.clone());
        let _ = self.refresh_kanban_and_orchestration(runtime);
        Ok(started)
    }

    pub fn report_worker_liveness(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        dispatch_id: &str,
        expected_generation: u64,
        liveness: WorkerLivenessState,
        now_ms: i64,
    ) -> Result<LivenessReportOutcome, String> {
        let out = runtime
            .ide()
            .orchestration()
            .report_liveness(dispatch_id, expected_generation, liveness, Some(now_ms))
            .map_err(|e| e.to_string())?;
        if let Some(worker) = self
            .tracked_workers
            .iter_mut()
            .find(|w| w.dispatch.dispatch_id == dispatch_id)
        {
            worker.dispatch.liveness = out.liveness;
            worker.dispatch.status = out.status;
        }
        self.worker_liveness_outcomes
            .insert(dispatch_id.to_string(), out.clone());
        Ok(out)
    }

    pub fn complete_orchestration_worker(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        dispatch_id: &str,
        expected_generation: u64,
        succeeded: bool,
        transcript: Option<String>,
        now_ms: i64,
    ) -> Result<(), String> {
        let out = runtime
            .ide()
            .orchestration()
            .complete_worker(CompleteWorkerRequest {
                dispatch_id: dispatch_id.to_string(),
                expected_generation,
                succeeded,
                exit_code: Some(if succeeded { 0 } else { 1 }),
                transcript_text: transcript,
                keep_open: false,
                setup_failed: false,
                now_ms: Some(now_ms),
            })
            .map_err(|e| e.to_string())?;
        if let Some(worker) = self
            .tracked_workers
            .iter_mut()
            .find(|w| w.dispatch.dispatch_id == dispatch_id)
        {
            worker.dispatch.generation = out.new_generation;
            worker.dispatch.status = out.status;
            worker.dispatch.liveness = WorkerLivenessState::Exited {
                exit_code: Some(if succeeded { 0 } else { 1 }),
            };
        }
        let archives = runtime
            .ide()
            .orchestration()
            .read_worker_archive(dispatch_id)
            .unwrap_or_default();
        self.worker_archives
            .insert(dispatch_id.to_string(), archives);
        let _ = self.refresh_kanban_and_orchestration(runtime);
        Ok(())
    }

    pub fn load_worker_archive(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        dispatch_id: &str,
    ) -> Result<Vec<WorkerTerminalArchive>, String> {
        let archives = runtime
            .ide()
            .orchestration()
            .read_worker_archive(dispatch_id)
            .map_err(|e| e.to_string())?;
        self.worker_archives
            .insert(dispatch_id.to_string(), archives.clone());
        Ok(archives)
    }

    pub fn send_orchestration_mailbox(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        run_id: &str,
        raw_target: &str,
        message_type: MailboxMessageType,
        priority: MailboxPriority,
        body: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let target = parse_mailbox_target(raw_target);
        let outcome = runtime
            .ide()
            .orchestration()
            .send_message(SendMessageRequest {
                run_id: run_id.to_string(),
                from_id: "coordinator".to_string(),
                target,
                message_type,
                priority,
                route: MailboxRoute::LocalOnly,
                body: body.trim().to_string(),
                dispatch_generation: None,
                now_ms: Some(now_ms),
            })
            .map_err(|e| e.to_string())?;
        self.mailbox_body_draft.clear();
        self.status_banner = Some(format!(
            "Mailbox sent ({}, {} recipient(s))",
            message_type.as_str(),
            outcome.recipients.len()
        ));
        let _ = self.refresh_kanban_and_orchestration(runtime);
        Ok(())
    }

    pub fn ack_mailbox_delivery(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        delivery_id: &str,
        expected_generation: Option<u64>,
        now_ms: i64,
    ) -> Result<(), String> {
        runtime
            .ide()
            .orchestration()
            .ack_delivery(delivery_id, expected_generation, Some(now_ms))
            .map_err(|e| e.to_string())?;
        let _ = self.refresh_kanban_and_orchestration(runtime);
        Ok(())
    }

    pub fn create_orchestration_gate(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        run_id: &str,
        task_id: &str,
        kind: DecisionGateKind,
        prompt: &str,
        now_ms: i64,
    ) -> Result<DecisionGateRecord, String> {
        let gate_id = self.next_id("gate");
        let payload = match kind {
            DecisionGateKind::Review => DecisionGatePayload::Review(ReviewGate {
                gate_id: gate_id.clone(),
                task_id: task_id.to_string(),
                required_approvals: 1,
                approvals: Vec::new(),
                rejections: Vec::new(),
                status: orca_core::orchestration_protocol::GateStatus::Pending,
                deadline_ms: None,
            }),
            DecisionGateKind::Merge => DecisionGatePayload::Merge(MergeGate {
                gate_id: gate_id.clone(),
                task_id: task_id.to_string(),
                ci_passed: true,
                conflicts_resolved: true,
                review_gates: Vec::new(),
                require_human_approval: true,
                human_approved: false,
                status: orca_core::orchestration_protocol::GateStatus::Pending,
                deadline_ms: None,
            }),
            DecisionGateKind::Decision => DecisionGatePayload::Decision {
                prompt: if prompt.trim().is_empty() {
                    format!("Confirm decision gate for task {task_id}")
                } else {
                    prompt.trim().to_string()
                },
                resolution_note: None,
            },
        };
        let gate = runtime
            .ide()
            .orchestration()
            .create_gate(CreateGateRequest {
                run_id: run_id.to_string(),
                task_id: task_id.to_string(),
                gate_id: Some(gate_id),
                payload,
                deadline_ms: None,
                now_ms: Some(now_ms),
            })
            .map_err(|e| e.to_string())?;
        self.tracked_gates.push(gate.clone());
        self.gate_prompt_draft.clear();
        Ok(gate)
    }

    pub fn resolve_orchestration_gate(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        gate_id: &str,
        approve: bool,
        now_ms: i64,
    ) -> Result<DecisionGateRecord, String> {
        let action = if approve {
            ResolveGateAction::Approve {
                actor: "desktop-user".to_string(),
                note: Some("Approved from Orca Orchestration Board".to_string()),
            }
        } else {
            ResolveGateAction::Reject {
                actor: "desktop-user".to_string(),
                reason: Some("Rejected from Orca Orchestration Board".to_string()),
            }
        };
        let updated = runtime
            .ide()
            .orchestration()
            .resolve_gate(gate_id, action, Some(now_ms))
            .map_err(|e| e.to_string())?;
        if let Some(existing) = self
            .tracked_gates
            .iter_mut()
            .find(|g| g.gate_id == updated.gate_id)
        {
            *existing = updated.clone();
        }
        Ok(updated)
    }

    // -----------------------------------------------------------------------
    // Explorer 3 §7 Required Headless Methods: Diff, Attribution & Tasks Hub
    // -----------------------------------------------------------------------

    pub fn refresh_diff_and_tasks(&mut self, runtime: &OrcaDesktopRuntime) -> Result<(), String> {
        let fp = self.diff_file_path.trim();
        let file_opt = if fp.is_empty() { None } else { Some(fp) };
        self.diff_annotations = runtime
            .ide()
            .diff_review()
            .list_annotations(file_opt, self.include_resolved_annotations);
        self.file_attributions = if let Some(path) = file_opt {
            runtime.ide().diff_review().file_attribution(path)
        } else {
            BTreeMap::new()
        };
        self.attribution_export = Some(
            runtime
                .ide()
                .diff_review()
                .export_attribution_metadata(file_opt),
        );
        self.hub_tasks = runtime
            .ide()
            .tasks_hub()
            .list_tasks(&TasksHubFilter {
                forge: self.tasks_hub_forge_filter,
                status: None,
                search: if self.tasks_hub_search.trim().is_empty() {
                    None
                } else {
                    Some(self.tasks_hub_search.trim().to_string())
                },
                limit: 100,
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Persists a line-anchored `DiffAnnotation` and returns the single-batch `"Send to agent"`
    /// prompt built via `PersistentDiffReviewStore::build_agent_batch_prompt`.
    pub fn add_diff_annotation_and_build_batch(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        ann: DiffAnnotation,
    ) -> Result<String, String> {
        let file_path = ann.file_path.clone();
        runtime
            .ide()
            .diff_review()
            .add_annotation(ann)
            .map_err(|e| e.to_string())?;
        let batch = runtime
            .ide()
            .diff_review()
            .build_agent_batch_prompt(Some(&file_path));
        self.diff_file_path = file_path;
        self.last_batch_prompt = Some(batch.clone());
        let _ = self.refresh_diff_and_tasks(runtime);
        Ok(batch)
    }

    pub fn build_diff_batch_prompt_for_active_file(
        &mut self,
        runtime: &OrcaDesktopRuntime,
    ) -> String {
        let fp = self.diff_file_path.trim();
        let file_opt = if fp.is_empty() { None } else { Some(fp) };
        let batch = runtime
            .ide()
            .diff_review()
            .build_agent_batch_prompt(file_opt);
        self.last_batch_prompt = Some(batch.clone());
        batch
    }

    pub fn resolve_or_reopen_annotation(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        annotation_id: &str,
        resolve: bool,
        now_ms: i64,
    ) -> Result<(), String> {
        if resolve {
            runtime
                .ide()
                .diff_review()
                .resolve_annotation(annotation_id, now_ms)
                .map_err(|e| e.to_string())?;
        } else {
            runtime
                .ide()
                .diff_review()
                .reopen_annotation(annotation_id, now_ms)
                .map_err(|e| e.to_string())?;
        }
        self.refresh_diff_and_tasks(runtime)
    }

    pub fn apply_diff_line_shift(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        at_line: u32,
        delta: i32,
        now_ms: i64,
    ) -> Result<(), String> {
        let fp = self.diff_file_path.clone();
        runtime
            .ide()
            .diff_review()
            .apply_line_shift(&fp, self.diff_side, at_line, delta, now_ms)
            .map_err(|e| e.to_string())?;
        self.refresh_diff_and_tasks(runtime)
    }

    pub fn record_local_attribution(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        line_start: u32,
        line_end: u32,
        attribution: LineAttribution,
        now_ms: i64,
    ) -> Result<(), String> {
        let fp = self.diff_file_path.clone();
        match attribution {
            LineAttribution::Human => runtime
                .ide()
                .diff_review()
                .record_human_edit(&fp, line_start, line_end, now_ms)
                .map_err(|e| e.to_string())?,
            other => runtime
                .ide()
                .diff_review()
                .record_ai_range(&fp, line_start, line_end, other, now_ms)
                .map_err(|e| e.to_string())?,
        }
        self.refresh_diff_and_tasks(runtime)
    }

    pub fn generate_commit_and_pr_draft(&mut self) -> Result<CommitPrDraft, String> {
        let files = if self.diff_file_path.trim().is_empty() {
            Vec::new()
        } else {
            vec![DiffFileChange {
                file_path: self.diff_file_path.trim().to_string(),
                added_lines: self.file_attributions.len().max(1),
                deleted_lines: 0,
                summary: Some(format!(
                    "{} review annotation(s)",
                    self.diff_annotations.len()
                )),
            }]
        };
        let draft = compose_commit_and_pr_draft(&CommitPrDraftRequest {
            commit_type: self.commit_type.clone(),
            scope: if self.commit_scope.trim().is_empty() {
                None
            } else {
                Some(self.commit_scope.trim().to_string())
            },
            summary: self.commit_summary.clone(),
            base_ref: self.commit_base_ref.clone(),
            head_branch: self.commit_head_branch.clone(),
            files,
            stacked_parent_pr: if self.commit_parent_pr.trim().is_empty() {
                None
            } else {
                Some(self.commit_parent_pr.trim().to_string())
            },
            breaking_change: None,
        })
        .map_err(|e| e.to_string())?;
        self.last_commit_pr_draft = Some(draft.clone());
        Ok(draft)
    }

    pub fn upsert_hub_task(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        now_ms: i64,
    ) -> Result<HubTaskRecord, String> {
        let task_id = self.next_id("hub-task");
        let rec = runtime
            .ide()
            .tasks_hub()
            .upsert_task(HubTaskRecord {
                task_id,
                forge: self.new_hub_task_forge,
                external_key: self.new_hub_task_key.trim().to_string(),
                title: self.new_hub_task_title.trim().to_string(),
                description: String::new(),
                status: "open".to_string(),
                assignee: None,
                labels: vec![self.new_hub_task_forge.as_str().to_string()],
                artifact_url: self.new_hub_task_url.trim().to_string(),
                repo_or_project: self.active_workspace_id.clone(),
                updated_at_ms: now_ms,
            })
            .map_err(|e| e.to_string())?;
        self.new_hub_task_key.clear();
        self.new_hub_task_title.clear();
        self.new_hub_task_url.clear();
        let _ = self.refresh_diff_and_tasks(runtime);
        Ok(rec)
    }

    pub fn delete_hub_task(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        task_id: &str,
    ) -> Result<(), String> {
        runtime
            .ide()
            .tasks_hub()
            .delete_task(task_id)
            .map_err(|e| e.to_string())?;
        self.refresh_diff_and_tasks(runtime)
    }

    pub fn build_hub_task_launch_command(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        task_id: &str,
    ) -> Result<String, String> {
        let cmd = runtime
            .ide()
            .tasks_hub()
            .build_launch_command(task_id, &self.issue_command_template)
            .map_err(|e| e.to_string())?;
        self.last_expanded_launch_cmd = Some(cmd.clone());
        Ok(cmd)
    }

    // -----------------------------------------------------------------------
    // Explorer 3 §7 Required Headless Methods: Usage, Hot-Swap, Vault, Plugins
    // -----------------------------------------------------------------------

    /// Refreshes the 9-provider usage roster sorted tightest-first plus active accounts,
    /// Codex reset credits, and `UsageLimitContinue` auto-continue evaluations.
    pub fn refresh_usage_roster(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        now_ms: i64,
    ) -> Vec<UsageRosterEntry> {
        let ide = runtime.ide();
        let roster = ide.usage_roster().sorted_roster(now_ms);
        self.usage_roster = roster.clone();

        self.active_accounts_by_provider.clear();
        for provider in ACCOUNT_HOME_PROVIDERS_9 {
            if let Some(acct) = ide.accounts().active_account(provider) {
                self.active_accounts_by_provider
                    .insert(provider.to_string(), acct);
            }
        }
        self.codex_reset_credits = ide.accounts().list_codex_reset_credits();
        self.due_auto_continues = ide.usage_roster().evaluate_auto_continues(now_ms);

        let mut pauses = Vec::new();
        for due in &self.due_auto_continues {
            pauses.push((due.session_key.clone(), due.state.clone()));
        }
        if let Some(ref snap) = self.active_session_snapshot
            && let Some(p) = ide
                .usage_roster()
                .get_usage_limit_pause(&snap.session.session_id)
            && !pauses.iter().any(|(k, _)| k == &snap.session.session_id)
        {
            pauses.push((snap.session.session_id.clone(), p));
        }
        self.usage_limit_pauses = pauses;
        roster
    }

    pub fn ingest_claude_status_line(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        account_id: &str,
        status_line: &str,
        now_ms: i64,
    ) -> Result<UsageRosterEntry, String> {
        let acct = if account_id.trim().is_empty() {
            "default"
        } else {
            account_id.trim()
        };
        let entry = runtime
            .ide()
            .usage_roster()
            .ingest_claude_status_line(acct, status_line, now_ms)
            .map_err(|e| e.to_string())?;
        self.claude_status_line_draft.clear();
        let _ = self.refresh_usage_roster(runtime, now_ms);
        Ok(entry)
    }

    /// Triggers bounded Multi-Account Hot-Swap via `AccountHomeManager::request_account_hotswap`.
    pub fn trigger_account_hotswap(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        req: &AccountHotSwapRequest,
        now_ms: i64,
    ) -> Result<HotSwapDecision, String> {
        let decision = runtime
            .ide()
            .accounts()
            .request_account_hotswap(req, now_ms)
            .map_err(|e| e.to_string())?;
        self.last_hotswap_decision = Some(decision.clone());
        let _ = self.refresh_usage_roster(runtime, now_ms);
        Ok(decision)
    }

    /// Completes an in-flight hot-swap restart, releasing its concurrency slot and updating
    /// the active account for the provider.
    pub fn complete_account_hotswap(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        restart_id: &str,
        succeeded: bool,
        now_ms: i64,
    ) -> Result<(), String> {
        runtime
            .ide()
            .accounts()
            .complete_hotswap_restart(restart_id, succeeded)
            .map_err(|e| e.to_string())?;
        let _ = self.refresh_usage_roster(runtime, now_ms);
        Ok(())
    }

    pub fn mark_due_auto_continue_resumed(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        session_key: &str,
        now_ms: i64,
    ) -> Result<UsageLimitContinue, String> {
        let updated = runtime
            .ide()
            .usage_roster()
            .mark_auto_continued(session_key, now_ms)
            .map_err(|e| e.to_string())?;
        let _ = self.refresh_usage_roster(runtime, now_ms);
        Ok(updated)
    }

    /// Executes ranked FTS5 search against `AiVaultSearchEngine::search`.
    pub fn search_ai_vault(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        req: VaultSearchRequest,
    ) -> Result<VaultSearchResponse, String> {
        let is_paginated = req.cursor.is_some();
        let resp = runtime
            .ide()
            .vault_search()
            .search(&req)
            .map_err(|e| e.to_string())?;
        if is_paginated {
            self.vault_accumulated_hits.extend(resp.hits.clone());
        } else {
            self.vault_accumulated_hits = resp.hits.clone();
        }
        self.vault_response = Some(resp.clone());
        Ok(resp)
    }

    pub fn fork_vault_session(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        source_session_id: &str,
        now_ms: i64,
    ) -> Result<String, String> {
        let new_sid = self.next_id("vault-fork");
        let _msgs = runtime
            .ide()
            .vault_search()
            .fork_conversation(source_session_id, &new_sid, None, now_ms)
            .map_err(|e| e.to_string())?;
        self.last_vault_fork_session_id = Some(new_sid.clone());
        Ok(new_sid)
    }

    pub fn build_vault_handoff_prompt(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        session_id: &str,
    ) -> Result<String, String> {
        let target = if self.vault_handoff_target_agent.trim().is_empty() {
            "codex"
        } else {
            self.vault_handoff_target_agent.trim()
        };
        let prompt = runtime
            .ide()
            .vault_search()
            .build_cross_agent_handoff_prompt(session_id, target, 40)
            .map_err(|e| e.to_string())?;
        self.last_vault_handoff_prompt = Some(prompt.clone());
        Ok(prompt)
    }

    pub fn refresh_plugins(&mut self, runtime: &OrcaDesktopRuntime) {
        let host = runtime.ide().plugin_host();
        self.plugins_enabled = host.is_enabled();
        self.installed_plugins = host.list_plugins();
        self.plugin_decisions.clear();
        for rec in &self.installed_plugins {
            let decision = host
                .evaluate_plugin(&rec.manifest.id)
                .map_err(|e| format!("{}: {e}", e.code_str()));
            self.plugin_decisions
                .insert(rec.manifest.id.clone(), decision);
        }
    }

    /// Builds a non-themable `SecuritySurfaceModalState` displaying the exact SHA-256
    /// `capability_fingerprint()` for `plugin_id` (matching CLI exit `3` approval parity).
    pub fn request_plugin_approval_modal(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        plugin_id: &str,
    ) -> Result<SecuritySurfaceModalState, String> {
        let rec = runtime
            .ide()
            .plugin_host()
            .get_plugin(plugin_id)
            .ok_or_else(|| format!("plugin '{plugin_id}' not found"))?;
        Ok(SecuritySurfaceModalState::open_plugin_approval(
            &rec.manifest,
            &rec.contributions,
        ))
    }

    pub fn approve_plugin_with_fingerprint(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        plugin_id: &str,
        fingerprint: &str,
        now_ms: i64,
    ) -> Result<PluginExecutionDecision, String> {
        let dec = runtime
            .ide()
            .plugin_host()
            .approve_plugin(plugin_id, fingerprint, now_ms)
            .map_err(|e| e.to_string())?;
        self.refresh_plugins(runtime);
        Ok(dec)
    }

    pub fn build_draft_plugin_manifest(&self) -> PluginManifestV2 {
        use sha2::{Digest, Sha256};
        let id = self.plugin_id_draft.trim().to_string();
        let version = if self.plugin_version_draft.trim().is_empty() {
            "1.0.0".to_string()
        } else {
            self.plugin_version_draft.trim().to_string()
        };
        let publisher = if self.plugin_publisher_draft.trim().is_empty() {
            None
        } else {
            Some(self.plugin_publisher_draft.trim().to_string())
        };
        let mut h = Sha256::new();
        h.update(id.as_bytes());
        h.update(b"@");
        h.update(version.as_bytes());
        h.update(self.plugin_tier_draft.as_str().as_bytes());
        let sha256 = format!("{:x}", h.finalize());

        let hooks = if self.plugin_tier_draft == PluginTier::TrustedWorker
            && self
                .plugin_selected_contributions
                .contains(&PluginContributionPoint::Hooks)
        {
            vec![PluginHookSpec {
                event: "pre_command".to_string(),
                on_error: self.plugin_hook_error_policy,
                timeout_ms: 2_000,
            }]
        } else {
            Vec::new()
        };
        PluginManifestV2 {
            id,
            publisher,
            version,
            schema_version: 2,
            tier: self.plugin_tier_draft,
            permissions: if self.plugin_tier_draft.requires_approval() {
                vec!["workspace:read".to_string()]
            } else {
                Vec::new()
            },
            tools: Vec::new(),
            hooks,
            sha256,
        }
    }

    pub fn validate_draft_plugin(
        &mut self,
        runtime: &OrcaDesktopRuntime,
    ) -> Result<PluginValidationSummary, String> {
        let manifest = self.build_draft_plugin_manifest();
        let summary = runtime
            .ide()
            .plugin_host()
            .validate_manifest(&manifest, &self.plugin_selected_contributions)
            .map_err(|e| e.to_string())?;
        self.last_plugin_validation = Some(summary.clone());
        Ok(summary)
    }

    pub fn install_draft_plugin(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        now_ms: i64,
    ) -> Result<InstalledPluginRecord, String> {
        let manifest = self.build_draft_plugin_manifest();
        let rec = runtime
            .ide()
            .plugin_host()
            .install_plugin(manifest, self.plugin_selected_contributions.clone(), None, now_ms)
            .map_err(|e| e.to_string())?;
        self.plugin_id_draft.clear();
        self.refresh_plugins(runtime);
        Ok(rec)
    }

    pub fn run_mcp_sync(&mut self, runtime: &OrcaDesktopRuntime) -> Result<McpSyncReport, String> {
        let target = if self.mcp_target_path_draft.trim().is_empty() {
            runtime.data_dir().join("agent_mcp_config.json")
        } else {
            PathBuf::from(self.mcp_target_path_draft.trim())
        };
        let mut servers = Vec::new();
        if !self.mcp_server_name_draft.trim().is_empty()
            && !self.mcp_server_cmd_draft.trim().is_empty()
        {
            servers.push(McpServerEntry {
                name: self.mcp_server_name_draft.trim().to_string(),
                command: self.mcp_server_cmd_draft.trim().to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
                source: "desktop-ide".to_string(),
            });
        }
        let report =
            McpConfigSync::sync_to_agent_config(&target, &servers, &[], self.mcp_strip_secrets)
                .map_err(|e| e.to_string())?;
        self.last_mcp_sync_report = Some(report.clone());
        Ok(report)
    }

    pub fn stage_or_install_skill(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        commit_immediately: bool,
        now_ms: i64,
    ) -> Result<SkillInstallRecord, String> {
        let src = PathBuf::from(self.skill_source_dir_draft.trim());
        let target_root = runtime.data_dir().join("skills");
        let rec = if commit_immediately {
            runtime
                .ide()
                .skill_installer()
                .install_skill(self.skill_name_draft.trim(), &src, &target_root, now_ms)
                .map_err(|e| e.to_string())?
        } else {
            runtime
                .ide()
                .skill_installer()
                .begin_staged_install(self.skill_name_draft.trim(), &src, &target_root, now_ms)
                .map_err(|e| e.to_string())?
        };
        self.skill_install_records.push(rec.clone());
        Ok(rec)
    }

    pub fn commit_staged_skill(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        install_id: &str,
        now_ms: i64,
    ) -> Result<SkillInstallRecord, String> {
        let rec = runtime
            .ide()
            .skill_installer()
            .commit_staged_install(install_id, now_ms)
            .map_err(|e| e.to_string())?;
        if let Some(existing) = self
            .skill_install_records
            .iter_mut()
            .find(|r| r.install_id == rec.install_id)
        {
            *existing = rec.clone();
        }
        Ok(rec)
    }

    pub fn recover_skills(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        now_ms: i64,
    ) -> Result<SkillRecoveryReport, String> {
        let rep = runtime
            .ide()
            .skill_installer()
            .recover_interrupted_installs(now_ms)
            .map_err(|e| e.to_string())?;
        for healed_id in &rep.healed_install_ids {
            if let Ok(Some(updated)) = runtime.ide().skill_installer().get_record(healed_id) {
                if let Some(existing) = self
                    .skill_install_records
                    .iter_mut()
                    .find(|r| r.install_id == updated.install_id)
                {
                    *existing = updated;
                } else {
                    self.skill_install_records.push(updated);
                }
            }
        }
        self.last_skill_recovery = Some(rep.clone());
        Ok(rep)
    }
}
